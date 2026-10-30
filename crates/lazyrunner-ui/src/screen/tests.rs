//! Snapshot tests of every screen (insta: `cargo insta review` after a deliberate change), and the keys that
//! change what the screen shows. The sample project, without starting anything: the apps' states are set by hand.

#![allow(
    clippy::missing_const_for_thread_local,
    reason = "clippy 1.99 flags `thread_local!` blocks whose statics are already `const { .. }` or can't be"
)]

use std::cell::RefCell;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use lazyrunner_core::changelog::Line;
use lazyrunner_core::config::Toggle;
use lazyrunner_core::output::Hue;
use lazyrunner_core::paths::Paths;
use lazyrunner_core::profile::{ProfileDirs, import};
use lazyrunner_core::run::app::Status as AppStatus;
use lazyrunner_core::run::checks::Status;
use lazyrunner_core::run::controller::Setup;
use lazyrunner_core::run::history::Kind;
use lazyrunner_core::run::{Controller, Event};
use lazyrunner_core::testing::{TempDir, sample_profile, sample_repo};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use super::checklist::{Checklist, Item};
use super::mouse::{self, Target};
use super::{Exit, Overlay, Picker, Ui, draw, keys, text};

const SIZE: (u16, u16) = (110, 32);

struct Screen {
    temp: TempDir,
    events: mpsc::UnboundedReceiver<Event>,
    controller: Controller,
    ui: Ui,
}

thread_local! {
    /// What `y` copied in this test (each test runs on its own thread).
    static COPIED: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    /// What the tests' opener and launcher were asked to open.
    static OPENED: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// The tests' clipboard: records instead of touching the real one.
fn record(text: &str) -> Result<(), String> {
    COPIED.with(|copied| copied.borrow_mut().push(text.to_string()));
    Ok(())
}

fn copied() -> Vec<String> {
    COPIED.with(|copied| copied.borrow().clone())
}

const NEWLINE: &str = if cfg!(windows) { "\r\n" } else { "\n" };

fn opened(target: &str) -> bool {
    OPENED.with(|opened| opened.borrow_mut().push(target.to_string()));
    true
}

fn revealed(file: &std::path::Path) -> bool {
    OPENED.with(|opened| opened.borrow_mut().push(format!("reveal {}", file.display())));
    true
}

fn launched(program: &std::path::Path, args: &[String]) -> bool {
    let program = program.file_stem().unwrap_or_default().to_string_lossy().into_owned();
    OPENED.with(|opened| opened.borrow_mut().push(format!("{program} {}", args.join(" "))));
    true
}

/// The sample project's controller on its own (other modules' tests).
pub(crate) fn controller(label: &str) -> (TempDir, Controller) {
    let screen = screen(label, json!({}));
    (screen.temp, screen.controller)
}

fn screen(label: &str, config: Value) -> Screen {
    let temp = TempDir::new(label);
    let repo = sample_repo(&temp.join("repo"));
    let paths = Paths {
        config_file: temp.join("config.json"),
        state_dir: temp.join("state"),
        profiles: ProfileDirs {
            user: temp.join("profiles"),
            bundled: temp.join("bundled"),
            only: Some(sample_profile()),
        },
    };
    // Side by side unless a test says otherwise: SIZE is narrow enough that auto would stack the apps. Plain icons
    // too: the snapshots shouldn't need a Nerd Font.
    let mut config = config;
    if config.get("layout").is_none() {
        config["layout"] = json!("columns");
    }
    if config.get("nerdIcons").is_none() {
        config["nerdIcons"] = json!(false);
    }
    lazyrunner_core::config::write(&paths.config_file, &config).unwrap();
    let project = import(&sample_profile(), &paths.profiles).unwrap();
    let (events, rx) = mpsc::unbounded_channel();
    let mut controller = Controller::new(Setup { project, root: repo, paths, events, guard: None }).unwrap();
    // What would differ between machines: the folder, the branch, the database, the log's times.
    controller.root = PathBuf::from("/work/sample");
    controller.branch = "main".into();
    controller.db_label = Some("ShopLocal @ localhost".into());
    controller.log.clear();
    let started = Instant::now();
    let (mut ui, _) = Ui::new(&controller, None, started);
    ui.opener = opened;
    ui.launcher = launched;
    ui.revealer = revealed;
    ui.clipboard = record;
    ui.now = started + Duration::from_secs(10);
    // The frames are taken at one frozen instant: a box would never have opened. The motion tests turn it on.
    ui.motion.enabled = false;
    Screen { temp, events: rx, controller, ui }
}

impl Screen {
    fn render(&mut self, width: u16, height: u16) -> String {
        for entry in &mut self.controller.log {
            entry.time = "12:00:00".into();
        }
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| draw(frame, &mut self.ui, &self.controller)).unwrap();
        // The version in titles, and the day it was released, change with every release; the snapshots shouldn't.
        use lazyrunner_core::changelog::{self, VERSION};
        let released = changelog::entries(changelog::CHANGELOG, None, None)
            .into_iter()
            .find(|entry| entry.version == VERSION)
            .map_or(String::new(), |entry| format!("released {}", entry.date));
        let frame = text(terminal.backend().buffer()).replace(&format!("v{VERSION}"), "v<version>");
        if released.is_empty() { frame } else { frame.replace(&released, "released <date>   ") }
    }

    fn frame(&mut self) -> String {
        self.render(SIZE.0, SIZE.1)
    }

    fn press(&mut self, spec: &str) {
        for key in crate::keys::parse(spec).unwrap() {
            keys::press(&mut self.ui, &mut self.controller, key);
        }
    }

    /// A key event as the terminal sends it; false: the loop drops it.
    fn event(&mut self, c: char, kind: KeyEventKind) -> bool {
        let key = KeyEvent::new_with_kind(KeyCode::Char(c), KeyModifiers::NONE, kind);
        let dropped = super::held::dropped(&mut self.ui, &key);
        if !dropped {
            keys::press(&mut self.ui, &mut self.controller, key);
        }
        !dropped
    }

    fn last_log(&self) -> String {
        self.controller.log.back().map(|entry| entry.text.clone()).unwrap_or_default()
    }

    /// Orders up with some output, Billing still starting, the rest stopped.
    /// Where a target is on the screen (a frame is drawn first, as the mouse sees the last one).
    fn spot(&mut self, target: Target) -> (u16, u16) {
        self.frame();
        let hit =
            self.ui.hits.iter().find(|hit| hit.target == target).unwrap_or_else(|| panic!("no {target:?} on screen"));
        (hit.x, hit.y)
    }

    fn click(&mut self, target: Target) {
        let (x, y) = self.spot(target);
        self.click_at(x, y);
    }

    fn click_at(&mut self, x: u16, y: u16) {
        let now = self.ui.now;
        mouse::click_at(&mut self.ui, &mut self.controller, x, y, now);
    }

    /// A click with Alt held: what opens a link by default (Ctrl+click too).
    fn alt_click_at(&mut self, x: u16, y: u16) {
        self.modified_click_at(x, y, crossterm::event::KeyModifiers::ALT);
    }

    fn modified_click_at(&mut self, x: u16, y: u16, modifiers: crossterm::event::KeyModifiers) {
        use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
        let kind = MouseEventKind::Down(MouseButton::Left);
        mouse::handle(&mut self.ui, &mut self.controller, MouseEvent { kind, column: x, row: y, modifiers });
    }

    fn running(&mut self) {
        let at = self.ui.now - Duration::from_secs(5);
        let orders: &[&str] = &[
            "info: Microsoft.Hosting.Lifetime[14]",
            "      Now listening on: http://localhost:47201",
            "warn: an error happened once",
            "info: Application started.",
        ];
        let billing: &[&str] = &["Building...", "error: Billing.cs(3,1): something is off"];
        for (index, lines) in [(0, orders), (1, billing)] {
            let state = &mut self.controller.states[index];
            state.status = AppStatus::Running;
            state.generation = 1;
            state.started_at = Some(at);
            for line in lines {
                state.output.push(line, at);
            }
        }
        self.controller.states[0].up_generation = Some(1);
        self.controller.states[0].up_after = Some(3.2);
        self.controller.add_log("Starting Orders + Billing", Hue::Cyan);
        self.controller.add_log("✓ Orders is up (3.2 s)", Hue::Green);
    }
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

// ---------------------------------------------------------------- snapshots

#[tokio::test]
async fn columns_stopped() {
    let mut screen = screen("snap-stopped", json!({}));
    screen.controller.add_log(
        "Ready - press U to start everything (auto-start when lazyrunner opens is off: , then u turns it on).",
        Hue::Cyan,
    );
    insta::assert_snapshot!(screen.frame());
}

#[tokio::test]
async fn columns_running() {
    let mut screen = screen("snap-running", json!({}));
    screen.running();
    insta::assert_snapshot!(screen.frame());
}

#[tokio::test]
async fn focus_moves_and_zoom_fills_the_body() {
    let mut screen = screen("snap-zoom", json!({}));
    screen.running();
    screen.press("l z");
    assert_eq!(screen.ui.focus, 1);
    insta::assert_snapshot!(screen.frame());
}

#[tokio::test]
async fn wider_columns_are_saved() {
    let mut screen = screen("snap-widths", json!({}));
    screen.press("+ + +");
    assert_eq!(screen.controller.config["columnWidths"]["orders"], json!(48.0));
    insta::assert_snapshot!(screen.frame());
    screen.press("=");
    assert_eq!(screen.last_log(), "Column widths reset to equal (saved)");
    assert_eq!(screen.controller.config["columnWidths"], json!({}));
}

#[tokio::test]
async fn key_reference() {
    let mut screen = screen("snap-help", json!({}));
    screen.press("?");
    insta::assert_snapshot!(screen.frame());
    screen.press("j");
    assert!(screen.ui.overlay.is_none(), "any key closes it");
}

#[tokio::test]
async fn command_palette_filters_and_runs() {
    let mut screen = screen("snap-palette", json!({}));
    screen.press(": r e s t");
    insta::assert_snapshot!(screen.frame());
    screen.press("Esc : z o o m Enter");
    assert!(screen.ui.zoom);
    assert!(screen.ui.overlay.is_none());
}

#[tokio::test]
async fn focused_app_menu() {
    let mut screen = screen("snap-menu", json!({}));
    screen.press("x");
    insta::assert_snapshot!(screen.frame());
}

#[tokio::test]
async fn settings_toggle_and_say_so() {
    let mut screen = screen("snap-settings", json!({}));
    screen.press(",");
    insta::assert_snapshot!(screen.frame());
    screen.press("a");
    assert_eq!(screen.last_log(), "Auto-restart on branch change: on (saved)");
    assert!(screen.controller.setting(Toggle::AutoRestartOnBranchChange));
    assert!(matches!(screen.ui.overlay, Some(Overlay::Settings { .. })), "settings stay open");
    screen.press("f");
    assert_eq!(screen.last_log(), "Reduced motion: no blinking, spinners or fades: on (saved)");
    assert!(screen.controller.setting(Toggle::ReducedMotion));
}

#[tokio::test]
async fn reduced_motion_keeps_a_search_cursor_and_starting_status_static() {
    let mut screen = screen("reduced-motion", json!({ "reducedMotion": true }));
    screen.running();
    screen.press("/ e");
    let first = screen.frame();
    screen.ui.now += Duration::from_millis(600);
    let second = screen.frame();
    assert!(first.contains("/e▏"), "{}", first);
    assert!(second.contains("/e▏"), "{}", second);
    assert!(first.contains(" :47202  ●"), "{}", first);
    assert!(second.contains(" :47202  ●"), "{}", second);
    assert!(!first.contains(" :47202  ○"), "{}", first);
    assert!(!second.contains(" :47202  ○"), "{}", second);
}

#[tokio::test]
async fn search_finds_and_steps_through_matches() {
    let mut screen = screen("snap-search", json!({}));
    screen.running();
    screen.press("/ e r r o r");
    insta::assert_snapshot!("search_typing", screen.frame());
    screen.press("Enter");
    let search = screen.ui.search.clone().unwrap();
    assert_eq!((search.position, search.total), (1, 1));
    insta::assert_snapshot!("search_match", screen.frame());
    screen.press("Esc");
    assert!(screen.ui.search.is_none());
}

#[tokio::test]
async fn search_filters_to_its_matches_and_takes_regex_and_case() {
    let mut screen = screen("search-filter", json!({}));
    let at = screen.ui.now - Duration::from_secs(5);
    for line in [
        "start",
        "Timeout calling payments",
        "after one",
        "filler two",
        "filler three",
        "before two",
        "timeout again",
        "end",
    ] {
        screen.controller.states[0].add_line(line, Hue::Gray, at);
    }
    screen.press("z / t i m e o u t");
    assert!(screen.frame().contains("2 matches"), "{}", screen.frame());
    screen.press("Alt+c");
    assert!(screen.frame().contains("1 match "), "case counts: {}", screen.frame());
    screen.press("Alt+c Tab Enter");
    let frame = screen.frame();
    assert!(frame.contains("after one") && frame.contains("before two") && !frame.contains("filler two"), "{frame}");
    assert!(frame.contains("⌕ /timeout and a line either side · 2 lines hidden - Esc shows all"), "{frame}");
    let search = screen.ui.search.clone().unwrap();
    assert_eq!((search.position, search.total), (1, 2));
    screen.press("n");
    assert_eq!(screen.ui.search.as_ref().map(|search| search.position), Some(2), "n steps to the older one");

    screen.press("/");
    let again = screen.ui.search.clone().unwrap();
    assert!(again.typing && again.filter && again.query == "timeout", "/ brings the search back");
    screen.press("Esc / t i m e . * a g Alt+r");
    assert!(screen.frame().contains("1 match "), "a regex: {}", screen.frame());
    screen.press("(");
    assert!(screen.frame().contains("not a regex"), "{}", screen.frame());
    screen.press("Esc");
    assert!(screen.ui.search.is_none());
}

#[tokio::test]
async fn database_picker() {
    let mut screen = screen("snap-picker", json!({}));
    let path = screen.temp.join("secrets.json");
    let secrets = "{\n  //\"DefaultConnection\": \"Server=dev-sql;Database=Shop;User Id=x;Password=p\",\n  \"DefaultConnection\": \"Server=localhost;Database=ShopLocal;Trusted_Connection=True\"\n}\n";
    std::fs::write(&path, secrets).unwrap();
    let groups = lazyrunner_core::run::secrets::groups(secrets);
    let mut picker =
        Picker { path, groups, group: 0, index: 0, top: 0, filter: String::new(), filtering: false, new_name: None };
    picker.index = picker.active_index();
    screen.ui.picker = Some(picker);
    insta::assert_snapshot!(screen.frame());
    screen.press("Enter");
    assert_eq!(screen.last_log(), "Already using ShopLocal @ localhost.");
    assert!(screen.ui.picker.is_none());
}

#[tokio::test]
async fn startup_checklist() {
    let mut screen = screen("snap-checklist", json!({}));
    let item = |name: &str, status, detail: &str| Item { name: name.into(), status, detail: detail.into() };
    let items = vec![
        item("lazyrunner", Status::Ok, &format!("v{}", lazyrunner_core::changelog::VERSION)),
        item("git", Status::Ok, "2.50"),
        item(".NET SDK", Status::Warn, "9.0 - the apps want 10.0"),
        item("Ports", Status::Running, ""),
        item("Project", Status::Pending, ""),
    ];
    let news = vec![
        Line { text: "0.2.0".into(), hue: Hue::Cyan },
        Line { text: "  • A faster lazyrunner".into(), hue: Hue::Gray },
    ];
    screen.ui.overlay = Some(Overlay::Checklist(Box::new(Checklist::fixed(items, news, "checking…"))));
    insta::assert_snapshot!(screen.frame());
    screen.press("Space");
    let Some(Overlay::Checklist(list)) = &screen.ui.overlay else { panic!("the checklist is open") };
    assert!(list.skip, "any key continues");
}

#[tokio::test]
async fn cut_off_labels_scroll_sideways_several_at_once() {
    let mut screen = screen("labels-scroll", json!({}));
    let long = "v1.40.0 is out, but not installed: the release has no Windows build yet, try again in a minute";
    let item = |name: &str, detail: String| Item { name: name.into(), status: Status::Warn, detail };
    let items = vec![item("Update", format!("1 {long}")), item("Ports", format!("2 {long}"))];
    screen.ui.overlay = Some(Overlay::Checklist(Box::new(Checklist::fixed(items, Vec::new(), "done"))));
    let first = screen.render(80, 30);
    assert!(first.contains("1 v1.40.0 is out") && first.contains("2 v1.40.0 is out"), "{first}");
    assert!(!first.contains("in a minute"), "cut off at first: {first}");
    screen.ui.now += Duration::from_secs(6);
    let later = screen.render(80, 30);
    assert_eq!(later.matches("try again in a minute").count(), 2, "both scroll to their ends: {later}");
}

#[tokio::test]
async fn changelog_view() {
    let mut screen = screen("snap-changelog", json!({}));
    let lines = (1..=40).map(|n| Line { text: format!("  • change {n}"), hue: Hue::Gray }).collect();
    screen.ui.overlay = Some(Overlay::Changelog { lines, top: 0 });
    screen.press("j j");
    insta::assert_snapshot!(screen.frame());
    screen.press("q");
    assert!(screen.ui.overlay.is_none());
}

// ---------------------------------------------------------------- D (durations)

#[tokio::test]
async fn durations_view() {
    let mut screen = screen("snap-durations", json!({}));
    screen.press("D");
    assert!(screen.ui.overlay.is_none(), "nothing recorded yet");
    assert!(screen.controller.log.back().is_some_and(|entry| entry.text.starts_with("No durations yet")));
    let durations = &mut screen.controller.durations;
    for (seconds, ok) in [(40.0, true), (38.0, true), (36.0, true), (52.0, false), (41.0, true), (55.0, true)] {
        durations.add(Kind::Build, "orders", "Orders", seconds, ok);
    }
    for (seconds, ok) in [(10.0, true), (10.0, true), (10.0, true), (125.0, true)] {
        durations.add(Kind::Build, "api", "Api", seconds, ok); // over 10x slower: the whole percentage shows
    }
    for seconds in [9.0, 8.0, 10.0, 9.0] {
        durations.add(Kind::Start, "orders", "Orders", seconds, true);
    }
    for (seconds, ok) in [(10.0, true), (10.0, true), (10.0, true), (2.0, false)] {
        durations.add(Kind::Start, "shop", "Shop", seconds, ok); // failed fast: not "faster"
    }
    durations.add(Kind::NpmCi, "shop", "A shop frontend with a very long name indeed", 61.0, true);
    screen.press("D");
    insta::assert_snapshot!(screen.frame());
    insta::assert_snapshot!("durations_view_80_columns", screen.render(80, 24));
    screen.press("D");
    assert!(screen.ui.overlay.is_none());
}

#[tokio::test]
async fn a_small_terminal_asks_for_room() {
    let mut screen = screen("snap-small", json!({}));
    insta::assert_snapshot!(screen.render(50, 10));
}

#[tokio::test]
async fn the_smallest_screen_still_fits() {
    let mut screen = screen("snap-minimum", json!({}));
    screen.running();
    insta::assert_snapshot!(screen.render(60, 15));
}

// ---------------------------------------------------------------- keys

#[tokio::test]
async fn gg_jumps_to_the_oldest_line_and_g_alone_does_nothing() {
    let mut screen = screen("keys-gg", json!({}));
    screen.running();
    screen.press("g");
    assert_eq!(screen.controller.states[0].scroll, 0);
    screen.press("g");
    assert_eq!(screen.controller.states[0].scroll, 3, "scrolled to the top");
    screen.press("G");
    assert_eq!(screen.controller.states[0].scroll, 0);
}

// ---------------------------------------------------------------- t (the theme picker)

fn saved_theme(screen: &Screen) -> Option<String> {
    screen.controller.config["theme"].as_str().map(str::to_string).filter(|id| id != "default")
}

#[tokio::test]
async fn the_theme_picker_previews_without_saving_and_enter_keeps_one() {
    let mut screen = screen("themes-keep", json!({}));
    screen.press("t");
    assert!(matches!(screen.ui.overlay, Some(Overlay::Themes { .. })));
    assert_eq!(screen.ui.theme.id, "default", "it opens on the theme in use");
    screen.press("Down");
    let previewed = screen.ui.theme.id.clone();
    assert_ne!(previewed, "default", "moving applies the next theme at once");
    assert_eq!(saved_theme(&screen), None, "a preview isn't saved");
    insta::assert_snapshot!(screen.frame());
    screen.press("Enter");
    assert!(screen.ui.overlay.is_none());
    assert_eq!(saved_theme(&screen).as_deref(), Some(previewed.as_str()));
    assert!(screen.last_log().starts_with(&format!("Theme: {}", screen.ui.theme.name)), "{}", screen.last_log());
    assert!(screen.last_log().ends_with("(saved)"));
}

#[tokio::test]
async fn tab_keeps_a_theme_for_this_project_alone_and_enter_gives_it_back() {
    let mut screen = screen("themes-own", json!({ "theme": "nord" }));
    let settings = lazyrunner_core::config::ConfigFile::new(screen.temp.join("config.json"));
    screen.press("t d r a c Tab");
    assert!(screen.ui.overlay.is_none());
    assert_eq!(screen.ui.theme.id, "dracula");
    assert_eq!(saved_theme(&screen).as_deref(), Some("dracula"), "this project uses its own");
    assert_eq!(screen.controller.own_setting("theme"), Some(json!("dracula")));
    assert_eq!(settings.load(Some("another-project"))["theme"], "nord", "every other project keeps theirs");
    let name = screen.controller.project.name.clone();
    assert!(screen.last_log().contains(&format!("(saved for {name} in config.json")), "{}", screen.last_log());

    screen.press("t");
    let Some(Overlay::Themes { own, .. }) = &screen.ui.overlay else { panic!("the picker is open") };
    assert_eq!(own.as_deref(), Some("dracula"));
    assert!(screen.frame().contains("◆ project's own · Tab undoes"), "{}", screen.frame());
    screen.press("g r u v Enter");
    assert_eq!(screen.ui.theme.id, "gruvbox");
    assert_eq!(screen.controller.own_setting("theme"), None, "Enter keeps it for every project, this one too");
    assert_eq!(settings.load(Some("another-project"))["theme"], "gruvbox");
}

#[tokio::test]
async fn tab_on_the_projects_own_theme_undoes_it() {
    let mut screen = screen("themes-own-undo", json!({ "theme": "nord" }));
    screen.press("t d r a c Tab");
    assert!(screen.last_log().contains("in config.json, Tab on it again undoes it"), "{}", screen.last_log());
    screen.press("t Tab");
    assert!(screen.ui.overlay.is_none());
    assert_eq!(screen.controller.own_setting("theme"), None, "Tab on its own theme takes it out");
    assert_eq!(screen.ui.theme.id, "nord", "back to every project's");
    assert!(screen.last_log().contains("own theme undone in config.json"), "{}", screen.last_log());
}

#[tokio::test]
async fn themes_with_the_same_name_show_their_ids() {
    let mut screen = screen("themes-twins", json!({}));
    let mine = screen.temp.join("my-themes");
    std::fs::create_dir_all(&mine).unwrap();
    std::fs::write(mine.join("onedark.json"), r#"{ "name": "One Dark", "extends": "one-dark" }"#).unwrap();
    screen.ui.user_themes = Some(mine);
    screen.press("t o n e d");
    let frame = screen.frame();
    assert!(frame.contains("One Dark · one-dark") && frame.contains("One Dark · onedark"), "{frame}");
    screen.press("Esc t n o r d");
    let frame = screen.frame();
    assert!(frame.contains("Nord ") && !frame.contains("Nord ·"), "a name of its own stays as it is: {frame}");
}

#[tokio::test]
async fn the_picker_groups_dark_and_light_themes_and_a_light_one_waits_to_light_the_terminal() {
    let mut screen = screen("themes-groups", json!({}));
    screen.press("t");
    let found = super::theme_picker::matches(&screen.ui.themes, "");
    assert_eq!(screen.ui.themes[found[0]].id, "default");
    let light: Vec<bool> = found.iter().map(|&at| screen.ui.themes[at].light).collect();
    let first_light = light.iter().position(|&light| light).expect("there are light themes");
    assert!(first_light > 1 && light[first_light..].iter().all(|&light| light), "dark, then light: {light:?}");
    assert!(screen.frame().contains(" Dark ─"), "{}", screen.frame());

    screen.press("Up");
    assert!(screen.ui.theme.light, "up from Default wraps to the light end");
    assert!(screen.frame().contains(" Light ─"), "{}", screen.frame());
    assert!(super::theme_picker::holds_terminal(&screen.ui, false), "the terminal keeps its dark background a moment");
    assert!(!super::theme_picker::holds_terminal(&screen.ui, true), "light already: nothing to wait for");
    screen.ui.now += Duration::from_millis(400);
    assert!(!super::theme_picker::holds_terminal(&screen.ui, false), "then it follows");
}

#[tokio::test]
async fn your_own_themes_are_listed_apart_dark_then_light() {
    let mut screen = screen("themes-custom", json!({}));
    let mine = screen.temp.join("my-themes");
    std::fs::create_dir_all(&mine).unwrap();
    std::fs::write(mine.join("paper.json"), r#"{ "name": "Paper", "extends": "github-light" }"#).unwrap();
    std::fs::write(mine.join("midnight.json"), r#"{ "name": "Midnight", "extends": "nord" }"#).unwrap();
    screen.ui.user_themes = Some(mine);
    screen.press("t");
    let themes = &screen.ui.themes;
    let ids: Vec<&str> = super::theme_picker::matches(themes, "").iter().map(|&at| themes[at].id.as_str()).collect();
    assert_eq!(ids[ids.len() - 2..], ["midnight", "paper"], "{ids:?}");
    assert!(themes.iter().filter(|theme| theme.custom).count() == 2);

    screen.press("Up");
    assert_eq!(screen.ui.theme.id, "paper");
    let frame = screen.frame();
    assert!(frame.contains(" Custom dark ─") && frame.contains(" Custom light ─"), "{frame}");
}

#[tokio::test]
async fn keeping_a_theme_ripples_out_from_where_it_was_picked() {
    use super::theme_picker::{RIPPLE, Ripple, paint_ripple};
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::style::{Color, Modifier};
    let mut screen = screen("themes-ripple", json!({}));
    screen.ui.motion.enabled = true;
    screen.press("t Down");
    assert!(screen.ui.theme_ripple.is_none(), "a preview doesn't ripple");
    screen.frame();
    let picked =
        screen.ui.hits.iter().find(|hit| hit.target == Target::Item(1)).map(|hit| (hit.x + hit.width / 2, hit.y));
    screen.press("Enter");
    let ripple = screen.ui.theme_ripple.expect("keeping one does");
    assert!(picked.is_some() && ripple.from == picked, "from the picked theme's row: {:?}", ripple.from);

    screen.ui.theme_ripple = Some(Ripple { started: ripple.started, from: Some((0, 0)) });
    let area = Rect::new(0, 0, 60, 10);
    let mut buf = Buffer::empty(area);
    screen.ui.now = ripple.started + RIPPLE / 4;
    paint_ripple(&mut buf, area, &mut screen.ui);
    assert!(buf[(59, 9)].modifier.contains(Modifier::DIM), "the wave isn't there yet");
    assert!(!buf[(0, 0)].modifier.contains(Modifier::DIM), "where it started, the new theme");
    assert!(buf.content.iter().any(|cell| cell.bg != Color::Reset), "its ring in the theme's colours");
    assert!(buf.content.iter().any(|cell| ["✦", "✧", "⋆", "·"].contains(&cell.symbol())), "sparkles on it");
    screen.ui.now = ripple.started + RIPPLE;
    paint_ripple(&mut buf, area, &mut screen.ui);
    assert!(screen.ui.theme_ripple.is_none(), "it ends by itself");

    let mut still = self::screen("themes-ripple-still", json!({ "reducedMotion": true }));
    still.ui.motion.enabled = true;
    still.frame();
    still.press("t Down Enter");
    assert!(still.ui.theme_ripple.is_none(), "none with reduced motion");
}

#[tokio::test]
async fn esc_puts_back_the_theme_the_picker_opened_with() {
    let mut screen = screen("themes-undo", json!({ "theme": "nord" }));
    screen.press("t Down Down Esc");
    assert_eq!(screen.ui.theme.id, "nord");
    assert_eq!(saved_theme(&screen).as_deref(), Some("nord"));
    assert!(screen.ui.overlay.is_none());
}

#[tokio::test]
async fn typing_filters_the_themes_and_previews_the_first_match() {
    let mut screen = screen("themes-filter", json!({}));
    screen.press("t d r a c");
    assert_eq!(screen.ui.theme.id, "dracula");
    screen.press("Enter");
    assert_eq!(saved_theme(&screen).as_deref(), Some("dracula"));
}

#[tokio::test]
async fn with_nothing_matching_enter_keeps_the_theme_it_opened_with() {
    let mut screen = screen("themes-none", json!({}));
    screen.press("t Down z z z z z");
    assert!(screen.frame().contains("no matching theme"));
    screen.press("Enter");
    assert_eq!(screen.ui.theme.id, "default");
    assert_eq!(saved_theme(&screen), None);
}

#[tokio::test]
async fn ctrl_c_quits_from_anywhere() {
    let mut screen = screen("keys-quit", json!({}));
    screen.press(",");
    keys::press(&mut screen.ui, &mut screen.controller, KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
    assert_eq!(screen.ui.exit, Some(Exit::Quit));
}

#[tokio::test]
async fn opening_the_focused_api_opens_its_swagger() {
    let mut screen = screen("keys-open", json!({}));
    screen.press("w");
    assert_eq!(screen.last_log(), "Opened http://localhost:47201/swagger");
}

#[test]
fn key_names_match_the_action_list() {
    assert_eq!(keys::key_name(&key(KeyCode::Char(' '))), "Space");
    assert_eq!(keys::key_name(&KeyEvent::new(KeyCode::Char('l'), KeyModifiers::CONTROL)), "Ctrl+L");
    assert_eq!(keys::key_name(&KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT)), "Shift+Tab");
    assert_eq!(keys::key_name(&key(KeyCode::Up)), "UpArrow");
}

#[tokio::test]
async fn a_start_that_builds_says_it_checks_the_sources_first() {
    let mut screen = screen("keys-busy", json!({}));
    screen.press("U");
    assert_eq!(screen.ui.busy, Some("Checking for source changes…"));
    assert!(screen.ui.deferred.is_some(), "the start waits for the frame that says so");
    assert!(screen.frame().contains("Checking for source changes…"));
    screen.ui.deferred = None; // not building the sample here
}

#[tokio::test]
async fn the_palette_selection_wraps_around_its_commands() {
    let mut screen = screen("keys-palette-end", json!({}));
    screen.press(": z o o m Up");
    let index = |screen: &Screen| match screen.ui.overlay {
        Some(Overlay::Palette { index, .. }) => index,
        _ => panic!("the palette is open"),
    };
    let count = super::actions::palette(&screen.ui, &screen.controller, "zoom").len();
    assert!(count > 1, "zoom finds more than one command");
    assert_eq!(index(&screen), count - 1, "up from the first: the last");
    screen.press("Down");
    assert_eq!(index(&screen), 0, "down from the last: the first");
}

#[tokio::test]
async fn altgr_characters_can_be_typed() {
    let mut screen = screen("keys-altgr", json!({}));
    screen.press("/");
    let altgr = KeyEvent::new(KeyCode::Char('@'), KeyModifiers::CONTROL | KeyModifiers::ALT);
    keys::press(&mut screen.ui, &mut screen.controller, altgr);
    assert_eq!(screen.ui.search.as_ref().unwrap().query, "@");
}

#[tokio::test]
async fn a_match_lands_in_the_middle_even_when_lines_wrap() {
    let mut screen = screen("keys-search-wrap", json!({}));
    let at = screen.ui.now - Duration::from_secs(5);
    let state = &mut screen.controller.states[0];
    state.output.push("needle", at);
    for n in 0..30 {
        state.output.push(&format!("{n:02} {}", "long line ".repeat(8)), at);
    }
    screen.frame(); // the columns' widths and the body's height
    screen.press("/ n e e d l e Enter");
    let frame = screen.frame();
    assert!(frame.lines().any(|line| line.starts_with("needle")), "the match is on screen:\n{frame}");
}

// ---------------------------------------------------------------- y (copy)

impl Screen {
    /// Lines in the first column (Orders), as an app printed them.
    fn output(&mut self, lines: &[&str]) {
        let at = self.ui.now - Duration::from_secs(5);
        for line in lines {
            self.controller.states[0].output.push(line, at);
        }
    }
}

const ERROR_ENTRY: [&str; 6] = [
    "info: Microsoft.Hosting.Lifetime[0]",
    "fail: Shop.Orders.OrdersController[0]",
    "      An unhandled exception has occurred while executing the request.",
    "      System.NullReferenceException: Object reference not set to an instance of an object.",
    "         at Shop.Orders.OrdersController.Get() in C:\\src\\Orders\\OrdersController.cs:line 12",
    "info: Microsoft.AspNetCore.Hosting.Diagnostics[2]",
];

#[tokio::test]
async fn y_copies_the_last_error_with_its_stack_trace() {
    let mut screen = screen("copy-error", json!({}));
    screen.output(&ERROR_ENTRY);
    screen.press("y y");
    assert_eq!(copied(), [ERROR_ENTRY[1..5].join(NEWLINE)]);
    assert_eq!(screen.last_log(), "Copied the last error from Orders (4 lines) to the clipboard");
}

#[tokio::test]
async fn y_asks_what_and_copies_every_error_and_warning_once() {
    let mut screen = screen("copy-all", json!({}));
    screen.output(&[
        "warn: Shop.Cache[0]",
        "      the cache is cold",
        "fail: Shop.Orders[0]",
        "      boom",
        "info: Shop.Orders[0]",
        "fail: Shop.Orders[0]",
        "      boom",
        "src/app.ts(3,1): error TS2322: Type 'string' is not assignable to type 'number'.",
    ]);
    screen.press("y");
    insta::assert_snapshot!(screen.frame());
    screen.press("e");
    let errors = [
        "fail: Shop.Orders[0]",
        "      boom",
        "",
        "src/app.ts(3,1): error TS2322: Type 'string' is not assignable to type 'number'.",
    ];
    assert_eq!(copied(), [errors.join(NEWLINE)], "the repeated one once, at its last place");
    assert_eq!(screen.last_log(), "Copied every error from Orders (2 entries, 3 lines) to the clipboard");
    screen.press("y w");
    assert_eq!(copied()[1], ["warn: Shop.Cache[0]", "      the cache is cold"].join(NEWLINE));
    screen.press("y a");
    assert!(copied()[2].starts_with("warn: Shop.Cache[0]") && copied()[2].ends_with("type 'number'."), "in order");
    screen.press("y o");
    assert_eq!(screen.last_log(), "Copied all of its output from Orders (8 lines) to the clipboard");
}

#[tokio::test]
async fn y_copies_the_search_match_with_its_indented_lines() {
    let mut screen = screen("copy-search", json!({}));
    screen.output(&ERROR_ENTRY);
    screen.press("/ N u l l R e f Enter y s");
    assert_eq!(copied(), [ERROR_ENTRY[3..5].join(NEWLINE)], "the match and the stack frame below it");
    assert_eq!(screen.last_log(), "Copied the search match from Orders (2 lines) to the clipboard");
}

#[tokio::test]
async fn y_copies_a_failed_builds_errors_with_their_full_paths() {
    let mut screen = screen("copy-build", json!({}));
    screen.output(&[
        "──── ⚙ 10:00:00 · dotnet build Orders.csproj ────",
        "OrdersController.cs(3,1): error CS1002: ; expected",
    ]);
    let at = screen.ui.now;
    screen.controller.states[0].output.push_hued("✗ build failed after 3s", Some(Hue::Red), at);
    let full = "C:\\src\\Orders\\OrdersController.cs(3,1): error CS1002: ; expected [C:\\src\\Orders\\Orders.csproj]";
    screen.controller.failed_build_errors.insert("orders".into(), vec![full.into()]);
    screen.press("y y");
    assert_eq!(copied(), [full]);
    assert_eq!(screen.last_log(), "Copied the last error from Orders (1 line) to the clipboard");
}

#[tokio::test]
async fn y_says_when_there_is_nothing_to_copy_or_the_clipboard_refuses() {
    let mut screen = screen("copy-nothing", json!({}));
    screen.output(&["info: Now listening on: http://localhost:47201"]);
    screen.press("y y");
    assert!(copied().is_empty());
    assert_eq!(screen.last_log(), "Nothing to copy: no error in Orders's output (/ finds a line to copy).");
    screen.press("y s");
    assert_eq!(screen.last_log(), "Nothing to copy: no search match in Orders (/ finds one).");
    screen.output(&ERROR_ENTRY);
    screen.ui.clipboard = |_| Err("the clipboard is busy".into());
    screen.press("y y");
    assert_eq!(screen.last_log(), "Couldn't copy to the clipboard: the clipboard is busy");
}

#[tokio::test]
async fn y_in_the_database_picker_copies_the_line_as_it_is_in_secrets_json() {
    let mut screen = screen("copy-picker", json!({}));
    let path = screen.temp.join("secrets.json");
    let secrets = "{\n  //\"DefaultConnection\": \"Server=dev-sql;Database=Shop;User Id=x;Password=p\",\n  \"DefaultConnection\": \"Server=localhost;Database=ShopLocal;Trusted_Connection=True\"\n}\n";
    std::fs::write(&path, secrets).unwrap();
    let groups = lazyrunner_core::run::secrets::groups(secrets);
    let picker = Picker {
        path: path.clone(),
        groups,
        group: 0,
        index: 0,
        top: 0,
        filter: String::new(),
        filtering: false,
        new_name: None,
    };
    screen.ui.picker = Some(picker);
    screen.press("y");
    assert_eq!(copied(), ["//\"DefaultConnection\": \"Server=dev-sql;Database=Shop;User Id=x;Password=p\","]);
    assert_eq!(
        screen.last_log(),
        "Copied the Shop @ dev-sql row from secrets.json (password included) to the clipboard"
    );
    assert!(screen.ui.picker.is_some(), "the picker stays open");
    std::fs::write(&path, "{\n}\n").unwrap();
    screen.press("y");
    assert_eq!(copied().len(), 1, "nothing copied from a file that changed");
    assert_eq!(screen.last_log(), "secrets.json changed since the picker opened - press d again to copy that row.");
}

// ---------------------------------------------------------------- installing packages

#[tokio::test]
async fn i_asks_npm_ci_or_npm_install_and_the_menu_and_palette_have_both() {
    let mut screen = screen("snap-install", json!({}));
    screen.press("l l i");
    insta::assert_snapshot!(screen.frame());
    screen.press("j");
    assert!(matches!(screen.ui.overlay, Some(Overlay::Install { index: 1 })));
    screen.press("Esc x");
    let menu = screen.frame();
    assert!(menu.contains("npm ci") && menu.contains("npm install"), "{menu}");
    screen.press("Esc :");
    for key in "install".chars() {
        screen.press(&key.to_string());
    }
    let palette = screen.frame();
    assert!(palette.contains(" i   n ") && palette.contains(" i   c "), "the keys as keycaps: {palette}");
}

// ---------------------------------------------------------------- Nerd Font icons

#[tokio::test]
async fn nerd_icons_are_on_by_default_and_swap_the_symbols_and_the_apps_icons() {
    assert!(Toggle::NerdIcons.default_on());
    let mut screen = screen("nerd", json!({}));
    let plain = screen.frame();
    assert!(plain.contains("⎇ main") && plain.contains("■ Billing") && plain.contains("◆ Orders"), "{plain}");
    screen.press(", i Esc");
    assert!(screen.controller.setting(Toggle::NerdIcons));
    let nerd = screen.frame();
    assert!(nerd.contains("\u{e725} main"), "the branch: {nerd}");
    assert!(nerd.contains("\u{e77f} Billing"), ".NET");
    assert!(nerd.contains("\u{e77f} Orders"), "its own icon ◆ is the plain one");
    assert!(nerd.contains("\u{e718} Shop"), "Node.js: no framework in its package.json");
}

#[tokio::test]
async fn nerd_icons_mark_the_states_the_chips_and_the_boxes() {
    let mut screen = screen("nerd-chrome", json!({ "nerdIcons": true }));
    let frame = screen.frame();
    assert!(frame.contains("\u{f04d} stopped"), "an app stopped, and all of them: {frame}");
    screen.press(":");
    assert!(screen.frame().contains("\u{f120} Commands"), "{}", screen.frame());
    screen.press("Esc ,");
    assert!(screen.frame().contains("\u{f013} Settings (saved)"), "{}", screen.frame());
    screen.press("Esc t");
    let themes = screen.frame();
    assert!(themes.contains("\u{f1fc} Themes") && themes.contains("\u{f186} Dark"), "{themes}");
    screen.press("Esc , i Esc");
    let plain = screen.frame();
    assert!(plain.contains("○ stopped") && !plain.contains('\u{f04d}'), "{plain}");
}

#[tokio::test]
async fn an_apps_nerd_icon_from_its_profile_wins_over_its_kinds() {
    let mut screen = screen("nerd-own", json!({ "nerdIcons": true }));
    let mut apps = screen.controller.apps.clone();
    apps[0].nerd_icon = Some("\u{f0ac}".into());
    screen.controller.apps = apps;
    let name = screen.controller.apps[0].name.clone();
    assert!(screen.frame().contains(&format!("\u{f0ac} {name}")));
}

// ---------------------------------------------------------------- reload window

#[tokio::test]
async fn ctrl_r_reads_the_settings_again_and_plays_its_show() {
    let mut screen = screen("reload", json!({}));
    let file = screen.temp.join("config.json");
    std::fs::write(&file, r#"{ "layout": "columns", "theme": "catppuccin-mocha" }"#).unwrap(); // a hand edit
    screen.press("Ctrl+R");
    assert_eq!(screen.ui.theme.id, "catppuccin-mocha", "the edited theme");
    assert!(screen.ui.reset_terminal, "the loop sets the terminal up again");
    assert!(screen.ui.is_animating(&screen.controller));
    screen.ui.now += Duration::from_millis(400); // the stage: the logo, the bar
    let stage = screen.frame();
    assert!(
        stage.contains("▀▀▀ ▀ ▀ ▀▀▀") && stage.contains("\u{f021} reloading"),
        "Nerd Font icons by default: {stage}"
    );
    screen.ui.now += Duration::from_secs(2);
    let after = screen.frame();
    assert!(screen.ui.reload.is_none() && after.contains("Orders"), "the show is over: {after}");
    assert!(screen.last_log().starts_with("↻ Window reloaded"));
}

#[tokio::test]
async fn with_reduced_motion_ctrl_r_reloads_without_a_show() {
    let mut screen = screen("reload-still", json!({ "reducedMotion": true }));
    screen.press("Ctrl+R");
    assert!(screen.ui.reload.is_none());
    assert!(screen.last_log().starts_with("↻ Window reloaded"));
}

// ---------------------------------------------------------------- the migrations view

#[tokio::test]
async fn the_migrations_view_shows_what_is_applied_and_asks_before_it_moves() {
    use lazyrunner_core::run::migrations::{Context, Migration, Status as Ef};
    let mut screen = screen("snap-migrations", json!({}));
    let migration = |id: &str, applied| Migration { id: id.into(), name: id[15..].into(), applied: Some(applied) };
    let mut shop =
        Context::new("orders", "ShopDbContext", "/work/sample/Data.csproj".into(), "/nowhere/Orders.csproj".into());
    shop.items = vec![
        migration("20260101000000_Init", true),
        migration("20260201000000_Orders", true),
        migration("20260301000000_Invoices", false),
    ];
    shop.status = Ef::Ok;
    // Checked today: an earlier day's check would show the day too.
    let today = chrono::Local::now().date_naive().and_hms_opt(9, 41, 7).unwrap();
    (shop.checked_at, shop.db) = (today.and_local_timezone(chrono::Local).single(), "ShopLocal".into());
    screen.controller.contexts = Some(vec![shop]);
    let setup = lazyrunner_core::profile::Migrations { project: None, contexts: vec!["ShopDbContext".into()] };
    screen.controller.apps[0].migrations = Some(setup);
    screen.ui.overlay = Some(Overlay::Migrations { index: 0, picking: None });
    insta::assert_snapshot!(screen.frame());
    screen.press("l j j m");
    let frame = screen.frame();
    assert!(frame.contains("Revert 1 migration of ShopDbContext on ShopLocal @ localhost, back to Init?"), "{frame}");
    assert!(frame.contains("Their Down() runs"));
    screen.press("n");
    screen.frame();
    assert!(matches!(screen.ui.overlay, Some(Overlay::Migrations { picking: Some(2), .. })), "back to the view");
    screen.press("u y");
    screen.frame();
    assert!(matches!(screen.ui.overlay, Some(Overlay::Migrations { .. })));
    assert_eq!(screen.last_log(), "✗ Migrations: Orders has no build yet - b builds it, then r here");
}

// ---------------------------------------------------------------- the requests view

#[tokio::test]
async fn y_copies_a_request_as_curl_powershell_and_more_and_p_sends_a_get_again() {
    let mut screen = screen("requests-copy-as", json!({}));
    with_requests(&mut screen);
    screen.press("H / o r d e r s Enter j y");
    assert!(screen.frame().contains("Copy POST /api/orders as"), "{}", screen.frame());
    screen.press("c");
    assert_eq!(copied().last().map(String::as_str), Some("curl -X POST 'http://localhost:47201/api/orders'"));
    assert!(!screen.frame().contains("Copy POST"), "the box closes");
    screen.press("y p");
    assert_eq!(
        copied().last().map(String::as_str),
        Some("Invoke-WebRequest -Uri 'http://localhost:47201/api/orders' -Method POST")
    );
    screen.press("p");
    assert!(screen.last_log().contains("a POST changes data"), "{}", screen.last_log());
    screen.press("k p");
    assert!(screen.last_log().contains("Orders isn't running"), "{}", screen.last_log());
    screen.press("y Esc");
    assert!(matches!(&screen.ui.overlay, Some(Overlay::Requests(view)) if view.copying.is_none()));
}

#[test]
fn a_request_becomes_a_command_for_each_shell() {
    use super::requests::{CopyAs, command};
    let url = "http://localhost:47201/api/orders?q=it's";
    assert_eq!(command(CopyAs::Curl, "GET", url, ""), "curl 'http://localhost:47201/api/orders?q=it'\\''s'");
    assert_eq!(command(CopyAs::CurlExe, "HEAD", url, ""), "curl.exe -I \"http://localhost:47201/api/orders?q=it's\"");
    assert_eq!(
        command(CopyAs::PowerShell, "GET", url, ""),
        "Invoke-WebRequest -Uri 'http://localhost:47201/api/orders?q=it''s'"
    );
    assert_eq!(
        command(CopyAs::Curl, "POST", "http://h/x", "application/json"),
        "curl -X POST 'http://h/x' -H 'Content-Type: application/json' --data-raw ''"
    );
    assert_eq!(
        command(CopyAs::Http, "PUT", "http://h/x", "text/plain"),
        "PUT http://h/x\nContent-Type: text/plain\n\n"
    );
    assert_eq!(command(CopyAs::Fetch, "GET", "http://h/x", ""), "await fetch('http://h/x');");
    assert_eq!(
        command(CopyAs::Fetch, "POST", "http://h/x", "application/json"),
        "await fetch('http://h/x', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: '' });"
    );
    assert_eq!(command(CopyAs::Url, "GET", "http://h/x", ""), "http://h/x");
}

/// Requests the sample's .NET apps served, by hand.
fn with_requests(screen: &mut Screen) {
    use chrono::TimeZone;
    use lazyrunner_core::run::requests::Finished;
    let at = chrono::Local.with_ymd_and_hms(2026, 10, 22, 9, 41, 7).unwrap();
    for (app, method, path, status, ms) in [
        ("orders", "GET", "/api/orders/42", 200, 12.4),
        ("orders", "POST", "/api/orders", 201, 48.0),
        ("billing", "GET", "/api/invoices/7", 500, 1530.0),
        ("orders", "GET", "/api/orders/43", 200, 9.6),
    ] {
        let (method, path) = (method.to_string(), path.to_string());
        let (length, content) = ("512".into(), "application/json".into());
        let request = Finished { method, path, status, ms, length, content, ..Finished::default() };
        screen.controller.requests.add(app, request, at);
    }
}

/// The failed request with what it logged meanwhile: a query, the exception; another request ran beside it.
fn with_logged_request(screen: &mut Screen) {
    use chrono::TimeZone;
    use lazyrunner_core::run::requests::Finished;
    let at = chrono::Local.with_ymd_and_hms(2026, 10, 22, 9, 41, 7).unwrap();
    let now = screen.ui.now;
    let app = screen.controller.states.iter_mut().find(|state| state.key == "billing").expect("the Billing app");
    let from = app.output.total() + 1;
    for (line, hue) in [
        ("info: Microsoft.EntityFrameworkCore.Database.Command[20101]", Hue::Gray),
        ("      Executed DbCommand (1,204ms) [Parameters=[@p0='7'], CommandType='Text']", Hue::Gray),
        ("fail: Microsoft.AspNetCore.Diagnostics.DeveloperExceptionPageMiddleware[1]", Hue::Red),
        ("      System.InvalidOperationException: Invoice 7 has no currency.", Hue::Red),
    ] {
        app.output.push_hued(line, Some(hue), now);
    }
    let db = lazyrunner_core::run::requests::db_summary(
        app.output.numbered(from, app.output.total()).map(|line| line.text.as_str()),
    );
    let request = Finished {
        method: "POST".into(),
        path: "/api/invoices/7/send".into(),
        status: 500,
        ms: 1530.0,
        proto: "HTTP/2".into(),
        sent_content: "application/json".into(),
        sent_length: "86".into(),
        logs: Some((from, app.output.total())),
        mixed: true,
        db,
        ..Finished::default()
    };
    screen.controller.requests.add("billing", request, at);
}

#[tokio::test]
async fn a_request_that_runs_one_query_over_and_over_is_marked_n_plus_one() {
    use lazyrunner_core::run::requests::{Finished, db_summary};
    let mut screen = screen("requests-n-plus-one", json!({}));
    with_requests(&mut screen);
    let now = screen.ui.now;
    let app = screen.controller.states.iter_mut().find(|state| state.key == "orders").expect("Orders");
    let from = app.output.total() + 1;
    for id in 1..=12 {
        app.output.push("info: Microsoft.EntityFrameworkCore.Database.Command[20101]", now);
        app.output
            .push(&format!("      Executed DbCommand (2ms) [Parameters=[@__id_0='{id}'], CommandType='Text']"), now);
        app.output.push("      SELECT [l].[Id] FROM [Lines] AS [l] WHERE [l].[OrderId] = @__id_0", now);
    }
    let db = db_summary(app.output.numbered(from, app.output.total()).map(|line| line.text.as_str()));
    let request = Finished {
        method: "GET".into(),
        path: "/api/orders/7/lines".into(),
        status: 200,
        ms: 90.0,
        logs: Some((from, app.output.total())),
        db,
        ..Finished::default()
    };
    screen.controller.requests.add("orders", request, chrono::Local::now());
    screen.press("H");
    assert!(screen.render(160, 30).contains("/api/orders/7/lines  N+1"), "{}", screen.render(160, 30));
    screen.press("Enter");
    let frame = screen.render(160, 30);
    assert!(frame.contains("N+1? The same query ran 12 times (24 ms)"), "{frame}");
    let rows = agent(&mut screen, "request", json!({ "id": 5 }), None);
    assert_eq!(rows["result"]["database"]["repeated"]["count"], 12, "{rows}");
    let listed = agent(&mut screen, "requests", json!({ "limit": 1 }), None);
    assert_eq!(listed["result"]["requests"][0]["n_plus_one"], 12, "{listed}");
}

#[tokio::test]
async fn the_requests_view_lists_the_newest_first_and_sums_up_endpoints() {
    let mut screen = screen("snap-requests", json!({}));
    with_requests(&mut screen);
    screen.press("H");
    insta::assert_snapshot!(screen.frame());
    screen.press("Tab");
    let frame = screen.frame();
    let line = frame.lines().find(|line| line.contains("/api/orders/{id}")).expect("the endpoint");
    assert!(line.contains("      2      0"), "two requests, no errors: {line}");
}

#[tokio::test]
async fn a_requests_details_show_what_it_logged_its_queries_and_what_it_sent() {
    let mut screen = screen("requests-details", json!({}));
    with_requests(&mut screen);
    with_logged_request(&mut screen);
    screen.press("H");
    assert!(!screen.render(160, 30).contains("Log  4 lines"), "the details stay closed until asked for");
    screen.press("Enter");
    let frame = screen.render(160, 30);
    assert!(frame.contains("500 Internal Server Error · 1530 ms"), "{frame}");
    assert!(frame.contains("Billing · 09:41:07 · HTTP/2"), "{frame}");
    assert!(frame.contains("sent      application/json · 86 bytes"), "{frame}");
    assert!(frame.contains("Log  4 lines · 2 errors"), "{frame}");
    assert!(frame.contains("DB 1 commands · 1204 ms · 79% of its time"), "{frame}");
    assert!(frame.contains("Possibly mixed"), "{frame}");
    assert!(frame.contains("  System.InvalidOperationException: Invoice 7"), "{frame}");
    screen.press("j");
    assert!(screen.render(160, 30).contains("Its log lines aren't known"), "a request without a start line");
    screen.press("Esc");
    assert!(!screen.render(160, 30).contains("Its log lines aren't known"), "Esc closes the details");
    assert!(matches!(screen.ui.overlay, Some(Overlay::Requests(_))), "and only the details");
}

#[tokio::test]
async fn an_endpoints_details_show_its_times_statuses_and_slowest_and_failing_requests() {
    let mut screen = screen("endpoint-details", json!({}));
    with_requests(&mut screen);
    screen.press("H Tab j j Space");
    let frame = screen.render(160, 30);
    assert!(frame.contains("Orders · 2 requests · 0 errors (0%)"), "{frame}");
    assert!(frame.contains("statuses  200 ×2"), "{frame}");
    assert!(frame.contains("     12 ms  09:41:07  /api/orders/42"), "the slowest first: {frame}");
    screen.press("g");
    assert!(screen.render(160, 30).contains("Latest failing"), "{}", screen.render(160, 30));
}

#[tokio::test]
async fn l_on_an_endpoint_lists_its_requests_and_h_goes_back_to_its_row() {
    let mut screen = screen("endpoint-requests", json!({}));
    with_requests(&mut screen);
    screen.press("H Tab j j");
    assert!(screen.frame().contains("l its requests"), "{}", screen.frame());
    screen.press("l");
    let frame = screen.frame();
    assert!(frame.contains("[GET /api/orders/{id} · Orders]"), "{frame}");
    assert!(frame.contains("2 shown") && frame.contains("h endpoints"), "{frame}");
    assert!(frame.contains("/api/orders/42") && !frame.contains("/api/invoices/7"), "{frame}");
    screen.press("h");
    let Some(Overlay::Requests(view)) = &screen.ui.overlay else { panic!("still the requests view") };
    assert_eq!((view.tab, view.index, view.endpoint.is_none()), (super::requests::Tab::Endpoints, 2, true));
    screen.press("l Enter Esc");
    assert!(
        matches!(&screen.ui.overlay, Some(Overlay::Requests(view)) if view.endpoint.is_some()),
        "Esc closes the details first"
    );
    screen.press("Esc");
    assert!(matches!(&screen.ui.overlay, Some(Overlay::Requests(view)) if view.endpoint.is_none()), "then goes back");
    let orders = agent(&mut screen, "requests", json!({ "endpoint": "/API/orders/{id}", "method": "get" }), None);
    assert_eq!(orders["result"]["matching"], 2, "{orders}");
}

#[tokio::test]
async fn in_the_requests_view_filters_copy_and_open_narrow_and_act_on_the_selection() {
    let mut screen = screen("requests-keys", json!({}));
    with_requests(&mut screen);
    screen.press("H e");
    assert!(screen.frame().contains("1 shown"), "errors only");
    screen.press("e / 4 2 Enter");
    assert!(screen.frame().contains("[/42]"), "{}", screen.frame());
    screen.press("y y");
    assert_eq!(copied(), ["GET http://localhost:47201/api/orders/42 -> 200 in 12 ms (Orders, 2026-10-22 09:41:07)"]);
    screen.press("w");
    assert_eq!(OPENED.with(|opened| opened.borrow().clone()), ["http://localhost:47201/api/orders/42"]);
    screen.press("/ Esc s");
    assert!(screen.frame().contains("sort (slowest)"));
    screen.click(Target::Item(1));
    assert!(matches!(&screen.ui.overlay, Some(Overlay::Requests(view)) if view.index == 1 && !view.follow));
    screen.press("c");
    assert!(screen.frame().contains("No requests yet"));
}

// ---------------------------------------------------------------- the tests view

/// The sample's suites by hand: one failed, one passed, one not run yet.
fn with_suites(screen: &mut Screen) {
    use chrono::TimeZone;
    use lazyrunner_core::run::suites::{Outcome, Runner, Status as SuiteStatus, Suite, TestResult};
    let test = |name: &str, outcome, ms, message: &str, stack: &str| TestResult {
        name: name.into(),
        outcome,
        ms,
        message: message.into(),
        stack: stack.into(),
    };
    let at = chrono::Local.with_ymd_and_hms(2026, 10, 22, 9, 41, 7).unwrap();
    let dotnet = |name: &str| Runner::Dotnet { project: format!("/work/sample/tests/{name}/{name}.csproj").into() };
    let mut orders = Suite::new("dotnet:orders", "Orders.Tests", dotnet("Orders.Tests"));
    orders.set_tests(vec![
        test("Orders.Tests.CartTests.Adds", Outcome::Passed, 12.0, "", ""),
        test(
            "Orders.Tests.CartTests.Totals(2, 3)",
            Outcome::Failed,
            41.0,
            "Expected 5 but was 6",
            "at Xunit.Assert.Equal()\nat Orders.Tests.CartTests.Totals() in C:\\work\\Orders.Tests\\CartTests.cs:line 42",
        ),
        test("Orders.Tests.CartTests.Later", Outcome::Skipped, 0.0, "", ""),
    ]);
    (orders.status, orders.last_run, orders.seconds) = (SuiteStatus::Failed, Some(at), 6.4);
    let mut billing = Suite::new("dotnet:billing", "Billing.Tests", dotnet("Billing.Tests"));
    billing.set_tests(vec![test("Billing.Tests.Invoices.Sums", Outcome::Passed, 230.0, "", "")]);
    (billing.status, billing.last_run, billing.seconds) = (SuiteStatus::Passed, Some(at), 2.1);
    let mut shop = Suite::new("npm:shop", "Shop", Runner::Npm { dir: "/work/sample/web/shop".into(), vitest: true });
    (orders.app, billing.app, shop.app) = (Some("orders".into()), Some("billing".into()), Some("shop".into()));
    let workspace = "/work/sample/web/shop".into();
    let mut ui = Suite::new("nx:ui", "shared-ui", Runner::Nx { workspace, project: "ui".into() });
    (ui.app, ui.untested) = (Some("shop".into()), true);
    // The list: 0 Backend, 1 Orders, 2 Orders.Tests, 3 Billing, 4 Billing.Tests, 5 Frontend, 6 Shop, 7 Shop's tests,
    // 8 the one project without test files.
    screen.controller.suites = Some(vec![orders, billing, shop, ui]);
}

#[tokio::test]
async fn chips_say_whats_new_since_a_view_was_open_and_open_it_on_the_first_new_thing() {
    use lazyrunner_core::run::suites::Status as SuiteStatus;
    let mut screen = screen("seen", json!({}));
    with_suites(&mut screen);
    screen.frame();
    with_requests(&mut screen);
    let frame = screen.frame();
    assert!(frame.contains(" H ") && frame.contains("1 new error"), "{frame}");
    let suites = screen.controller.suites.as_mut().expect("the sample's suites");
    let billing = suites.iter_mut().find(|suite| suite.key == "dotnet:billing").expect("Billing.Tests");
    (billing.status, billing.last_run) = (SuiteStatus::Failed, Some(chrono::Local::now()));
    assert!(screen.frame().contains("1 suite done · 1 red"), "{}", screen.frame());

    screen.press("H");
    let failed =
        screen.controller.requests.items.iter().find(|request| request.status == 500).map(|request| request.seq);
    assert!(
        matches!(&screen.ui.overlay, Some(Overlay::Requests(view)) if Some(view.selected) == failed && !view.follow)
    );
    screen.frame();
    screen.press("Esc");
    assert!(!screen.frame().contains("new error"), "having it open cleared the chip");

    screen.click(Target::News(super::jumps::View::Tests));
    let all = screen.controller.suites.as_deref().unwrap_or_default();
    let at = all.iter().position(|suite| suite.key == "dotnet:billing");
    let row = super::suites::list_rows(&screen.controller)
        .iter()
        .position(|row| matches!(row, super::suites::Row::Suite(suite) if Some(*suite) == at));
    assert!(matches!(screen.ui.overlay, Some(Overlay::Tests { index, .. }) if Some(index) == row), "on Billing.Tests");
    screen.frame();
    assert!(!screen.frame().contains("suite done"), "{}", screen.frame());
}

#[tokio::test]
async fn the_tests_view_lists_the_suites_and_shows_the_failures() {
    let mut screen = screen("snap-tests", json!({}));
    with_suites(&mut screen);
    screen.press("T");
    insta::assert_snapshot!(screen.frame());
}

#[tokio::test]
async fn in_the_tests_view_keys_choose_copy_and_say_what_they_cant_do() {
    let mut screen = screen("tests-keys", json!({}));
    with_suites(&mut screen);
    screen.press("T y");
    assert!(copied()[0].starts_with("Failed: Orders.Tests.CartTests.Totals(2, 3) (41 ms)"), "{:?}", copied());
    assert_eq!(screen.last_log(), "Copied 1 failed test from Orders.Tests to the clipboard");
    screen.press("j f");
    assert_eq!(screen.last_log(), "No failed tests to run again.", "Billing (the app) passed");
    screen.press("j o");
    assert_eq!(screen.last_log(), "No test log yet - r runs the suite.");
    screen.press("j j j");
    assert!(screen.frame().contains("Not run yet - r runs this suite"), "Shop's tests are selected");
    screen.press("< y");
    assert_eq!(screen.last_log(), "Copied 1 failed test from Backend · dotnet test to the clipboard");
    assert!(screen.frame().contains("2 suites · r runs them all"), "the backend is selected");
    screen.press("o");
    assert_eq!(screen.last_log(), "o opens a suite's log - choose one.");
    screen.press("s");
    assert_eq!(screen.last_log(), "No tests are running.");
    screen.press("Esc");
    assert!(screen.ui.overlay.is_none());
    screen.press("T ,");
    assert!(matches!(screen.ui.overlay, Some(Overlay::Settings { .. })), "keys it doesn't use do what they always do");
    screen.press(",");
    screen.frame();
    assert!(matches!(screen.ui.overlay, Some(Overlay::Tests { .. })), "closing the settings goes back to the tests");
    screen.press("> 2");
    assert!(matches!(screen.ui.overlay, Some(Overlay::Tests { index: 3, .. })), "2: the second app, Billing");
    assert!(screen.frame().contains(" 2 Billing"), "numbered");
    screen.press("Esc H ?");
    assert!(matches!(screen.ui.overlay, Some(Overlay::Help)), "from the requests view too");
    screen.press("x");
    screen.frame();
    assert!(matches!(screen.ui.overlay, Some(Overlay::Requests(_))), "and back to it");
}

#[tokio::test]
async fn l_picks_a_test_of_the_suite_and_shows_its_message_and_stack() {
    let mut screen = screen("snap-tests-pick", json!({}));
    with_suites(&mut screen);
    screen.press("T l j");
    insta::assert_snapshot!(screen.frame());
    screen.press("y");
    assert!(copied()[0].starts_with("Skipped: Orders.Tests.CartTests.Later"), "{:?}", copied());
    screen.press("h j l");
    assert_eq!(screen.last_log(), "Choose a suite to pick one of its tests.", "Billing, the app");
    screen.press("j l");
    assert!(matches!(screen.ui.overlay, Some(Overlay::Tests { index: 4, picking: Some(0), .. })));
    screen.press("Esc");
    assert!(matches!(screen.ui.overlay, Some(Overlay::Tests { picking: None, .. })), "back to the suites");
    screen.press("j j j l");
    assert_eq!(screen.last_log(), "No results for this suite yet - r runs it.", "Shop hasn't run");
    screen.press("k k k k k l");
    screen.click(Target::Test(2));
    assert!(matches!(screen.ui.overlay, Some(Overlay::Tests { index: 2, picking: Some(2), .. })));
    screen.ui.now += Duration::from_secs(1);
    screen.click(Target::Item(4));
    assert!(matches!(screen.ui.overlay, Some(Overlay::Tests { index: 4, picking: None, .. })), "a suite: leaves it");
}

#[tokio::test]
async fn in_the_tests_view_a_click_picks_a_suite_and_a_link_in_a_stack_opens() {
    let mut screen = screen("tests-mouse", json!({ "editor": "myeditor {file}:{line}" }));
    with_suites(&mut screen);
    screen.press("T");
    screen.click(Target::Item(4));
    assert!(matches!(screen.ui.overlay, Some(Overlay::Tests { index: 4, .. })));
    screen.ui.now += Duration::from_secs(1);
    screen.click(Target::Item(2));
    screen.frame();
    let link = screen.ui.hits.iter().find(|hit| matches!(hit.target, Target::Link { .. })).map(|hit| (hit.x, hit.y));
    let (x, y) = link.expect("the stack frame's place in the file is a link");
    screen.ui.now += Duration::from_secs(1);
    screen.alt_click_at(x, y);
    assert!(screen.ui.overlay.is_some(), "the view stays open");
    assert!(screen.last_log().contains("CartTests.cs isn't in"), "{}", screen.last_log());
}

#[tokio::test]
async fn the_pointer_on_an_app_or_a_side_says_a_click_runs_it_and_a_click_does() {
    let mut screen = screen("tests-groups", json!({}));
    with_suites(&mut screen);
    screen.press("T");
    let (x, y) = screen.spot(Target::Item(6));
    let moved = |screen: &mut Screen| {
        let event = crossterm::event::MouseEvent {
            kind: crossterm::event::MouseEventKind::Moved,
            column: x,
            row: y,
            modifiers: KeyModifiers::NONE,
        };
        mouse::handle(&mut screen.ui, &mut screen.controller, event)
    };
    assert!(moved(&mut screen), "onto the Shop app: a redraw");
    assert!(!moved(&mut screen), "still there: none");
    let frame = screen.frame();
    let row = frame.lines().find(|line| line.contains("▶ click to run")).expect("the hint");
    assert!(row.contains("Shop"), "{row}");
    screen.click(Target::Item(6));
    assert!(matches!(screen.ui.overlay, Some(Overlay::Tests { index: 6, .. })));
    assert!(screen.last_log().starts_with("1 suite can't run: "), "no node_modules here: {}", screen.last_log());
    assert!(screen.frame().contains("needs packages"), "the Shop app and its suite say so");
    screen.press("j j");
    assert!(screen.frame().contains("1 project without test files"));
    screen.press("r");
    assert_eq!(screen.last_log(), "Nothing to run here: these projects have no test files.");
}

#[tokio::test]
async fn the_status_line_sums_up_the_tests_and_a_click_on_it_opens_them() {
    let mut screen = screen("tests-badge", json!({}));
    assert!(!screen.frame().contains("⚗"), "no results, no summary");
    with_suites(&mut screen);
    let frame = screen.frame();
    let status = frame.lines().find(|line| line.contains(" lazyrunner ")).expect("the status line");
    assert!(status.contains("⚗ ✓2 ✗1"), "{status}");
    screen.click(Target::Tests);
    assert!(matches!(screen.ui.overlay, Some(Overlay::Tests { .. })));
}

#[tokio::test]
async fn the_tests_view_looks_for_suites_in_the_background_and_says_when_there_are_none() {
    let mut screen = screen("tests-none", json!({}));
    screen.press("T");
    assert!(screen.frame().contains("Looking for tests…"));
    let event = tokio::time::timeout(Duration::from_secs(30), screen.events.recv()).await.unwrap().unwrap();
    screen.controller.handle(event);
    assert!(screen.frame().contains("No test suites in this project."));
    assert!(screen.last_log().starts_with("No tests found: .NET projects referencing Microsoft.NET.Test.Sdk"));
}

// ---------------------------------------------------------------- the layout

#[tokio::test]
async fn a_narrow_terminal_stacks_the_apps_top_to_bottom() {
    let mut screen = screen("snap-stacked", json!({ "layout": "auto" }));
    screen.running();
    insta::assert_snapshot!(screen.frame());
    assert!(screen.render(130, 32).contains("┬"), "wide enough: side by side");
}

#[tokio::test]
async fn stacked_apps_take_the_focused_ones_share_and_keep_it_on_screen() {
    let mut screen = screen("stacked-focus", json!({ "layout": "stacked", "columnWidths": { "orders": 66 } }));
    screen.running();
    screen.frame();
    let rows = |screen: &Screen, index: usize| {
        let mut ys: Vec<u16> =
            screen.ui.hits.iter().filter(|hit| hit.target == Target::Output(index)).map(|hit| hit.y).collect();
        ys.dedup();
        ys
    };
    assert!(rows(&screen, 0).len() > rows(&screen, 1).len() * 2, "Orders has twice the share");
    screen.render(110, 18);
    let last = screen.controller.apps.len() - 1;
    screen.press("Tab ".repeat(last).trim());
    let frame = screen.render(110, 18);
    let title = |screen: &Screen, index: usize| screen.ui.hits.iter().any(|hit| hit.target == Target::Title(index));
    assert!(title(&screen, last), "the focused app is on screen\n{frame}");
    assert!(!title(&screen, 0), "the first gave way to it");
}

#[tokio::test]
async fn in_stacked_apps_a_click_focuses_the_app_and_the_wheel_scrolls_it() {
    let mut screen = screen("stacked-mouse", json!({ "layout": "stacked" }));
    for n in 0..40 {
        screen.controller.states[2].output.push(&format!("line {n}"), screen.ui.now);
    }
    screen.click(Target::Output(2));
    assert_eq!(screen.ui.focus, 2);
    let (x, y) = screen.spot(Target::Output(2));
    mouse::wheel_at(&mut screen.ui, &mut screen.controller, x, y, true);
    assert_eq!(screen.controller.states[2].scroll, 3);
}

#[tokio::test]
async fn the_layout_key_goes_auto_columns_stacked_and_saves_it() {
    let mut screen = screen("layout-key", json!({ "layout": "auto" }));
    screen.press("|");
    assert_eq!(screen.controller.config["layout"], "columns");
    assert_eq!(screen.last_log(), "Layout: columns side by side (saved)");
    screen.press("|");
    assert_eq!(screen.controller.config["layout"], "stacked");
    assert!(!screen.frame().contains("┬"), "top to bottom at any width");
    screen.press("|");
    assert_eq!(screen.controller.config["layout"], "auto");
    assert_eq!(screen.last_log(), "Layout: auto - stacked when narrower than 120 cells (saved)");
    let saved = std::fs::read_to_string(screen.temp.join("config.json")).unwrap();
    assert!(saved.contains("\"layout\": \"auto\""), "{saved}");
}

// ---------------------------------------------------------------- the mouse

#[tokio::test]
async fn a_click_on_a_title_focuses_its_column_and_a_double_click_zooms_it() {
    let mut screen = screen("mouse-title", json!({}));
    screen.running();
    screen.click(Target::Title(2));
    assert_eq!((screen.ui.focus, screen.ui.zoom), (2, false));
    screen.click(Target::Title(2));
    assert!(screen.ui.zoom, "a double click zooms");
    screen.click(Target::Output(2));
    assert_eq!(screen.ui.focus, 2, "a third click is a single one again");
    assert!(screen.ui.zoom);
}

#[tokio::test]
async fn the_wheel_scrolls_the_column_under_the_pointer() {
    let mut screen = screen("mouse-wheel", json!({}));
    for n in 0..40 {
        screen.controller.states[1].output.push(&format!("billing line {n}"), screen.ui.now);
    }
    let (x, y) = screen.spot(Target::Output(1));
    mouse::wheel_at(&mut screen.ui, &mut screen.controller, x, y, true);
    assert_eq!(screen.controller.states[1].scroll, 3, "the column under the pointer");
    assert_eq!(screen.controller.states[0].scroll, 0);
    assert_eq!(screen.ui.focus, 0, "the focus stays");
    mouse::wheel_at(&mut screen.ui, &mut screen.controller, x, y, false);
    assert_eq!(screen.controller.states[1].scroll, 0);
}

#[tokio::test]
async fn in_a_menu_a_click_picks_an_item_a_double_click_runs_it_and_a_click_outside_closes_it() {
    let mut screen = screen("mouse-menu", json!({}));
    screen.press(",");
    screen.click(Target::Item(1));
    assert!(matches!(screen.ui.overlay, Some(Overlay::Settings { index: 1 })), "picked, not run");
    assert!(screen.controller.setting(Toggle::AutoStartOnOpen), "still on");
    screen.click(Target::Item(1));
    assert!(!screen.controller.setting(Toggle::AutoStartOnOpen), "a double click runs it: auto-start toggled");
    screen.press("Esc ,");
    let (_, y) = screen.spot(Target::Item(0));
    screen.click_at(0, y); // the margin beside the box
    assert!(screen.ui.overlay.is_none(), "closed");
}

#[tokio::test]
async fn a_long_column_has_a_scrollbar_bright_while_scrolled_back() {
    let mut screen = screen("snap-scrollbar", json!({}));
    screen.running();
    for n in 0..120 {
        screen.controller.states[0].output.push(&format!("orders line {n}"), screen.ui.now - Duration::from_secs(5));
    }
    screen.controller.states[0].scroll = 60;
    insta::assert_snapshot!(screen.frame());
}

#[tokio::test]
async fn clicks_while_a_new_database_name_is_typed_add_nothing() {
    let mut screen = screen("mouse-picker-name", json!({}));
    let path = screen.temp.join("secrets.json");
    let secrets = "{
  \"DefaultConnection\": \"Server=localhost;Database=ShopLocal;Trusted_Connection=True\"
}
";
    std::fs::write(&path, secrets).unwrap();
    let groups = lazyrunner_core::run::secrets::groups(secrets);
    let picker = Picker {
        path: path.clone(),
        groups,
        group: 0,
        index: 0,
        top: 0,
        filter: String::new(),
        filtering: false,
        new_name: Some("ShopCopy".into()),
    };
    screen.ui.picker = Some(picker);
    screen.click(Target::Item(0));
    screen.click(Target::Item(0)); // a double click: not an Enter that adds the name
    assert_eq!(std::fs::read_to_string(&path).unwrap(), secrets, "secrets.json untouched");
    assert!(screen.ui.picker.as_ref().is_some_and(|picker| picker.new_name.is_some()), "still typing");
    screen.click_at(0, 0); // outside
    assert!(screen.ui.picker.is_none(), "closed");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), secrets);
}

#[tokio::test]
async fn the_click_that_closes_a_box_and_the_next_one_are_not_a_double_click() {
    let mut screen = screen("mouse-close-then-click", json!({}));
    let (x, y) = screen.spot(Target::Title(1));
    screen.press(",");
    screen.click_at(x, y); // outside the box: closes it
    assert!(screen.ui.overlay.is_none());
    screen.click_at(x, y);
    assert_eq!(screen.ui.focus, 1, "a single click focuses");
    assert!(!screen.ui.zoom, "not a double click on the title");
}

#[tokio::test]
async fn with_reduced_motion_nothing_animates_and_new_lines_dont_flash() {
    for (reduced, animating) in [(false, true), (true, false)] {
        let mut screen = screen("reduced-motion-frames", json!({ "reducedMotion": reduced }));
        screen.running(); // Billing is starting: its dot pulses
        let now = screen.ui.now;
        screen.controller.states[0].output.push("a brand new line", now);
        screen.frame();
        assert_eq!(screen.ui.is_animating(&screen.controller), animating, "reduced motion: {reduced}");
    }
}

// ---------------------------------------------------------------- links

#[tokio::test]
async fn a_click_on_a_link_opens_the_url_or_the_file_at_its_line() {
    let mut screen = screen("links", json!({ "editor": "myeditor --goto {file}:{line}" }));
    let repo = screen.temp.join("repo");
    std::fs::create_dir_all(repo.join("services/Orders")).unwrap();
    std::fs::write(repo.join("services/Orders/Checkout.cs"), "").unwrap();
    screen.controller.root = repo;
    let at = screen.ui.now - Duration::from_secs(5);
    screen.controller.states[0].output.push("Checkout.cs(12,5): error CS1002: ; expected", at);
    screen.controller.states[1].output.push("  Local:   http://localhost:47202/", at);
    screen.frame();
    let spot = |screen: &Screen, column: usize| {
        let hit = screen.ui.hits.iter().find(|hit| matches!(hit.target, Target::Link { column: c, .. } if c == column));
        hit.map(|hit| (hit.x, hit.y)).expect("a link on screen")
    };
    let (x, y) = spot(&screen, 1);
    screen.click_at(x, y);
    assert!(OPENED.with(|opened| opened.borrow().is_empty()), "a plain click opens nothing by default");
    assert_eq!(screen.last_log(), "Alt+click opens a link (, then l: a click does)");
    let said = screen.controller.log.len();
    screen.ui.now += Duration::from_secs(1);
    screen.click_at(x, y);
    assert_eq!(screen.controller.log.len(), said, "it says so once");
    screen.ui.now += Duration::from_secs(1);
    screen.alt_click_at(x, y);
    let (x, y) = spot(&screen, 0);
    screen.ui.now += Duration::from_secs(1); // not a double click
    screen.modified_click_at(x, y, crossterm::event::KeyModifiers::CONTROL); // Ctrl+click too
    let opened = OPENED.with(|opened| opened.borrow().clone());
    assert_eq!(opened[0], "http://localhost:47202/");
    assert!(opened[1].starts_with("myeditor --goto ") && opened[1].ends_with("Checkout.cs:12"), "{opened:?}");
    assert!(screen.last_log().starts_with("Opening Checkout.cs:12 in myeditor"), "{}", screen.last_log());
}

#[tokio::test]
async fn a_file_link_is_found_in_its_apps_own_folder_first() {
    let mut screen = screen("links-app-folder", json!({ "editor": "myeditor --goto {file}:{line}" }));
    let repo = screen.temp.join("repo");
    let wanted = repo.join("web/shop/libs/shared/sections/attachments.component.ts");
    std::fs::create_dir_all(wanted.parent().unwrap()).unwrap();
    std::fs::write(&wanted, "").unwrap();
    screen.controller.root = repo;
    let shop = screen.controller.apps.iter().position(|app| app.key == "shop").expect("the sample's Shop");
    let at = screen.ui.now - Duration::from_secs(5);
    screen.controller.states[shop].output.push("  libs/shared/sections/attachments.component.ts:30:16", at);
    screen.frame();
    let hit = screen.ui.hits.iter().find(|hit| matches!(hit.target, Target::Link { column, .. } if column == shop));
    let (x, y) = hit.map(|hit| (hit.x, hit.y)).expect("the path is a link");
    screen.alt_click_at(x, y);
    let opened = OPENED.with(|opened| opened.borrow().clone());
    assert!(opened.last().is_some_and(|it| it.ends_with("attachments.component.ts:30")), "{opened:?}");
    assert!(screen.last_log().starts_with("Opening attachments.component.ts:30 in myeditor"), "{}", screen.last_log());
}

#[tokio::test]
async fn with_links_on_alt_click_off_a_plain_click_opens_a_link() {
    let mut screen = screen("links-plain", json!({ "linksOnAltClick": false }));
    let at = screen.ui.now - Duration::from_secs(5);
    screen.controller.states[1].output.push("  Local:   http://localhost:47202/", at);
    screen.frame();
    let hit = screen.ui.hits.iter().find(|hit| matches!(hit.target, Target::Link { .. })).map(|hit| (hit.x, hit.y));
    let (x, y) = hit.expect("a link on screen");
    screen.click_at(x, y);
    assert_eq!(OPENED.with(|opened| opened.borrow().clone()), ["http://localhost:47202/"]);
}

// ---------------------------------------------------------------- about

/// A fixed list of contributors in the state folder, newer than the one built in - which the repo's workflow keeps
/// changing.
fn with_contributors(screen: &Screen) {
    let dir = &screen.controller.paths().state_dir;
    std::fs::create_dir_all(dir).unwrap();
    let list = json!({ "updated": "2999-01-01", "people": [
        { "login": "momcilovicluka", "name": "Luka Momčilović", "role": "creator · maintainer" },
        { "login": "ognjen004028", "name": "Ognjen Maričić", "prs": 1, "bugs": 2, "ideas": 7, "issues": 9 },
    ] });
    std::fs::write(dir.join("contributors.json"), list.to_string()).unwrap();
}

fn overlay(screen: &Screen) -> Option<&'static str> {
    screen.ui.overlay.as_ref().map(Overlay::kind)
}

#[tokio::test]
async fn the_about_box_shows_the_version_and_who_makes_it_and_opens_its_links() {
    let mut screen = screen("about", json!({}));
    with_contributors(&screen);
    screen.press(", A");
    assert_eq!(overlay(&screen), Some("about"));
    let frame = screen.frame();
    for text in [
        "v<version> · released",
        "Luka Momčilović",
        "Luka Momčilović    creator · maintainer · @momcilovicluka",
        "Ognjen Maričić     1 PR · 2 bug reports · 7 ideas · @ognjen004028",
        "Contributors",
        "Report a bug",
        "made with ♥",
    ] {
        assert!(frame.contains(text), "{text}: {frame}");
    }
    screen.press("b");
    let opened = OPENED.with(|opened| opened.borrow().clone());
    let report = "https://github.com/momcilovicluka/lazyrunner/issues/new?template=bug_report.yml&version=";
    assert!(opened.last().is_some_and(|url| url.starts_with(report)), "{opened:?}");
    assert_eq!(overlay(&screen), Some("about"), "a page leaves the box open");
    assert!(matches!(screen.ui.overlay, Some(Overlay::About { index: 4 })), "the letter's link is highlighted");
    screen.press("Home Down Enter");
    assert_eq!(overlay(&screen), Some("changelog"), "What's new");
}

#[tokio::test]
async fn shift_p_in_the_settings_opens_the_project_profile_in_the_editor() {
    let mut screen = screen("open-profile", json!({ "editor": "auto" }));
    let profile = screen.controller.project.path.clone();
    screen.press(",");
    assert!(screen.frame().contains("Open the project profile in your editor"), "{}", screen.frame());
    screen.press("P");
    let opened = OPENED.with(|opened| opened.borrow().clone());
    let wanted = profile.display().to_string();
    assert!(opened.last().is_some_and(|it| it.contains(&wanted)), "{opened:?} should open {wanted}");
    assert!(screen.controller.log.back().unwrap().text.starts_with("Opening "), "{:?}", screen.controller.log.back());
}

#[tokio::test]
async fn the_palette_finds_the_about_box_and_a_double_click_opens_a_link() {
    let mut screen = screen("about-palette", json!({}));
    screen.press(": a b o u t Enter");
    assert_eq!(overlay(&screen), Some("about"));
    screen.click(Target::Item(3));
    screen.click(Target::Item(3));
    let opened = OPENED.with(|opened| opened.borrow().clone());
    assert!(opened.last().is_some_and(|url| url.ends_with("/blob/main/LICENSE")), "{opened:?}");
    screen.press("Esc");
    assert_eq!(overlay(&screen), None);
}

#[tokio::test]
async fn the_about_box_on_a_tall_screen_has_its_logo() {
    let mut screen = screen("about-tall", json!({ "reducedMotion": true }));
    with_contributors(&screen);
    screen.press(", A");
    insta::assert_snapshot!(screen.render(SIZE.0, 44));
}

// ---------------------------------------------------------------- the agent channel

fn agent(screen: &mut Screen, op: &str, args: Value, repo: Option<&std::path::Path>) -> Value {
    let repo = repo.map(|repo| repo.display().to_string());
    let request = json!({ "v": crate::agent::PROTOCOL, "op": op, "args": args, "client": "claude-code", "repo": repo });
    crate::agent::handle(&mut screen.ui, &mut screen.controller, &request)
}

#[tokio::test]
async fn follow_mode_shows_what_the_agent_reads_while_your_keys_rest() {
    let mut screen = screen("follow", json!({}));
    let at = screen.ui.now - Duration::from_secs(5);
    let shop = &mut screen.controller.states[2];
    (1..=60).for_each(|n| shop.add_line(&format!("line {n}"), Hue::Gray, at));
    shop.add_line("payment timed out", Hue::Red, at);
    (61..=90).for_each(|n| shop.add_line(&format!("line {n}"), Hue::Gray, at));
    screen.frame();
    screen.press("F");
    assert!(screen.ui.follow.on);
    agent(&mut screen, "logs", json!({ "app": "Shop", "grep": "timed out" }), None);
    assert_eq!((screen.ui.focus, screen.controller.states[2].scroll), (2, 30), "Shop, the line read at the bottom");
    assert!(screen.ui.motion.looked(2, screen.ui.now).is_some_and(|lines| lines.contains("payment timed out")));
    let frame = screen.frame();
    assert!(frame.contains("following claude-code"), "{frame}");
    assert!(frame.contains("read Shop's output matching \"timed out\""), "{frame}");

    screen.press("1 q");
    assert!(screen.ui.focus == 2 && screen.ui.exit.is_none(), "your keys rest");
    agent(&mut screen, "requests", json!({ "errors_only": true }), None);
    assert!(matches!(&screen.ui.overlay, Some(Overlay::Requests(view)) if view.errors_only), "its filters");

    screen.press("F");
    assert!(!screen.ui.follow.on && screen.ui.overlay.is_none());
    assert_eq!((screen.ui.focus, screen.controller.states[2].scroll), (0, 0), "the screen back as it was");
    screen.press("Ctrl+o");
    assert_eq!(screen.ui.focus, 2, "the jump list has where the agent took you");
}

#[tokio::test]
async fn follow_agent_follows_an_agents_call_until_you_take_the_keys_back_while_it_works() {
    let mut screen = screen("follow-auto", json!({ "followAgent": true }));
    agent(&mut screen, "status", json!({}), None);
    assert!(screen.ui.follow.on);
    screen.press("F");
    agent(&mut screen, "status", json!({}), None);
    assert!(!screen.ui.follow.on, "not again while it works");
    screen.ui.now += Duration::from_secs(180);
    agent(&mut screen, "status", json!({}), None);
    assert!(screen.ui.follow.on, "its next task, a quiet while later");
}

#[tokio::test]
async fn an_agent_lists_every_action_runs_one_by_its_key_and_presses_keys() {
    let mut screen = screen("agent-drive", json!({}));
    let listed = agent(&mut screen, "actions", json!({}), None);
    let actions = listed["result"]["actions"].as_array().cloned().unwrap_or_default();
    let action = |key: &str| actions.iter().find(|action| action["key"] == key).cloned().unwrap_or_default();
    assert_eq!(action("D")["label"], "Durations", "{listed}");
    assert_eq!(action("q")["needs_confirm"], true);
    let settings = listed["result"]["settings"].as_array().cloned().unwrap_or_default();
    assert!(settings.iter().any(|setting| setting["setting"] == "followAgent" && setting["on"] == false));

    let zoom = agent(&mut screen, "run_action", json!({ "key": "z", "app": "Billing" }), None);
    assert_eq!(zoom["ok"], true, "{zoom}");
    assert!(screen.ui.zoom && screen.ui.focus == 1);
    assert!(screen.last_log().contains("Zoom Billing"), "{}", screen.last_log());
    let quit = agent(&mut screen, "run_action", json!({ "key": "q" }), None);
    assert!(quit["code"] == "needs_confirm" && screen.ui.exit.is_none(), "{quit}");

    let pressed = agent(&mut screen, "press_keys", json!({ "keys": "z :" }), None);
    assert_eq!(pressed["ok"], true, "{pressed}");
    assert!(!screen.ui.zoom && matches!(screen.ui.overlay, Some(Overlay::Palette { .. })));
    assert_eq!(agent(&mut screen, "press_keys", json!({ "keys": "Esc" }), None)["ok"], true, "its own palette");
    screen.press(":");
    let typing = agent(&mut screen, "press_keys", json!({ "keys": "Esc" }), None);
    assert_eq!(typing["code"], "user_typing", "the user's palette: {typing}");
    screen.press("Esc");
    assert_eq!(agent(&mut screen, "press_keys", json!({ "keys": "q" }), None)["code"], "needs_confirm");

    let set = agent(&mut screen, "set_setting", json!({ "setting": "followAgent", "on": true }), None);
    assert_eq!(set["ok"], true, "{set}");
    assert!(screen.controller.setting(Toggle::FollowAgent));
    let install = agent(&mut screen, "install_packages", json!({ "app": "Orders" }), None);
    assert_eq!(install["ok"], false, "Orders is .NET: {install}");
    let kill = agent(&mut screen, "kill_port_holder", json!({}), None);
    assert_eq!(kill["code"], "nothing_to_do");
}

#[tokio::test]
async fn an_agent_reads_the_requests_their_endpoints_and_one_request_in_full() {
    let mut screen = screen("agent-requests", json!({}));
    let empty = agent(&mut screen, "requests", json!({}), None);
    assert!(empty["result"]["hint"].as_str().unwrap().contains("No requests yet"), "{empty}");
    with_requests(&mut screen);
    with_logged_request(&mut screen);
    let ids = |answer: &Value, field: &str| -> Vec<u64> {
        answer["result"]["requests"].as_array().unwrap().iter().map(|row| row[field].as_u64().unwrap()).collect()
    };

    let newest = agent(&mut screen, "requests", json!({ "limit": 2 }), None);
    assert_eq!(ids(&newest, "id"), [5, 4]);
    assert_eq!((newest["result"]["matching"].as_u64(), newest["result"]["next"].as_u64()), (Some(5), Some(5)));
    let failed = agent(&mut screen, "requests", json!({ "errors_only": true, "sort": "slowest" }), None);
    assert_eq!(ids(&failed, "status"), [500, 500]);
    let orders = agent(&mut screen, "requests", json!({ "app": "Orders", "path": "/API/ORDERS/4", "since": 1 }), None);
    assert_eq!(ids(&orders, "id"), [4], "{orders}");
    let summed = agent(&mut screen, "requests", json!({ "endpoints": true }), None);
    let endpoints = summed["result"]["endpoints"].as_array().unwrap();
    let first = &endpoints[0];
    assert_eq!((first["total_ms"].as_f64(), first["errors"].as_u64()), (Some(1530.0), Some(1)), "the most time first");
    let orders = endpoints.iter().find(|row| row["endpoint"] == "/api/orders/{id}").unwrap();
    assert_eq!((orders["count"].as_u64(), orders["p50_ms"].as_f64()), (Some(2), Some(12.4)), "{summed}");
    assert_eq!(agent(&mut screen, "requests", json!({ "sort": "oldest" }), None)["code"], "bad_args");

    let answer = agent(&mut screen, "request", json!({ "id": 5 }), None);
    let one = &answer["result"];
    assert_eq!((one["status"].as_u64(), one["protocol"].as_str()), (Some(500), Some("HTTP/2")), "{answer}");
    assert_eq!(one["sent"]["content_type"], "application/json");
    assert_eq!((one["log"]["known"].as_bool(), one["log"]["count"].as_u64()), (Some(true), Some(4)));
    assert_eq!(one["log"]["possibly_mixed"], true);
    assert_eq!(one["log"]["lines"][3]["text"], "      System.InvalidOperationException: Invoice 7 has no currency.");
    assert_eq!((one["database"]["commands"].as_u64(), one["database"]["total_ms"].as_f64()), (Some(1), Some(1204.0)));
    let unlogged = agent(&mut screen, "request", json!({ "id": 1 }), None);
    assert_eq!(unlogged["result"]["log"]["known"], false);
    assert_eq!(unlogged["result"]["endpoint_stats"]["count"], 2, "GET /api/orders/{{id}} twice");
    assert_eq!(agent(&mut screen, "request", json!({ "id": 99 }), None)["code"], "not_found");
}

#[tokio::test]
async fn an_agent_reads_the_status_and_numbered_logs_and_the_last_error() {
    let mut screen = screen("agent", json!({}));
    let (at, base) = (screen.ui.now, screen.controller.states[0].output.total());
    let lines = [
        "info: Now listening on: http://localhost:47201",
        "fail: Orders.Api.Checkout[0]",
        "      System.InvalidOperationException: the basket is empty",
        r"         at Orders.Api.Checkout.Pay() in C:\src\Checkout.cs:line 42",
        "info: Request finished",
    ];
    for line in lines {
        screen.controller.states[0].output.push(line, at);
    }
    let status = agent(&mut screen, "status", json!({}), None);
    assert_eq!(status["ok"], true, "{status}");
    assert_eq!(status["result"]["apps"][0]["key"], "orders");
    assert_eq!(status["result"]["ui"]["view"], "columns");
    assert_eq!(status["result"]["busy"]["build"], Value::Null);

    let found = agent(&mut screen, "logs", json!({ "app": "Orders", "since": base + 1, "grep": "EXCEPTION" }), None);
    assert_eq!(found["result"]["lines"], json!([{ "seq": base + 3, "text": lines[2] }]));
    assert_eq!(found["result"]["next"], base + 5);
    let newest = agent(&mut screen, "logs", json!({ "app": "orders", "limit": 2 }), None);
    let seqs: Vec<u64> =
        newest["result"]["lines"].as_array().unwrap().iter().map(|line| line["seq"].as_u64().unwrap()).collect();
    assert_eq!(seqs, [base + 4, base + 5], "the newest, oldest first");
    assert_eq!(newest["result"]["truncated"], true, "older lines after since didn't fit");
    let after_a_restart = agent(&mut screen, "logs", json!({ "app": "orders", "since": base + 500 }), None);
    assert_eq!(after_a_restart["result"]["reset"], true);
    assert_eq!(after_a_restart["result"]["lines"].as_array().map(Vec::len), Some(base as usize + 5));
    let error = agent(&mut screen, "errors", json!({ "app": "orders" }), None);
    let text = error["result"]["text"].as_str().unwrap();
    assert!(text.starts_with("fail: Orders.Api.Checkout[0]") && text.contains("Checkout.cs:line 42"), "{text}");

    let refused = |answer: &Value| answer["code"].as_str().unwrap_or_default().to_string();
    assert_eq!(refused(&agent(&mut screen, "logs", json!({ "app": "nope" }), None)), "bad_args");
    assert_eq!(refused(&agent(&mut screen, "dance", json!({}), None)), "unknown_op");
    let old = crate::agent::handle(&mut screen.ui, &mut screen.controller, &json!({ "op": "status" }));
    assert_eq!(refused(&old), "protocol_mismatch");
}

#[tokio::test]
async fn an_agents_actions_say_so_in_the_log_and_another_folders_are_refused() {
    let mut screen = screen("agent-act", json!({}));
    let repo = screen.temp.join("repo");
    screen.controller.root = repo.clone();
    let stopped = agent(&mut screen, "stop", json!({ "app": "all" }), Some(&repo.join("services")));
    assert_eq!(stopped["result"]["accepted"], true, "{stopped}");
    assert_eq!(stopped["result"]["ui"]["view"], "columns");
    assert!(screen.controller.log.iter().any(|entry| entry.text == "▸ claude-code: stop every app"));

    let outside = screen.temp.path().to_path_buf();
    let typo = agent(&mut screen, "stop", json!({ "app": ["orders"] }), Some(&repo));
    assert_eq!(typo["code"], "bad_args", "not every app: {typo}");
    let elsewhere = agent(&mut screen, "stop", json!({}), Some(&outside));
    assert_eq!(elsewhere["code"], "wrong_repo", "{elsewhere}");
    let reading_elsewhere = agent(&mut screen, "status", json!({}), Some(&outside));
    assert_eq!(reading_elsewhere["ok"], true, "questions are answered from anywhere");
}

// ---------------------------------------------------------------- motion

#[tokio::test]
async fn a_bar_glides_half_the_way_every_100_ms_and_the_clock_flows_between_ticks() {
    let mut screen = screen("glide", json!({}));
    assert_eq!(screen.ui.glide("bar", 0.0), 0.0);
    screen.ui.now += Duration::from_millis(100);
    assert!((screen.ui.glide("bar", 1.0) - 0.5).abs() < 1e-9);
    screen.ui.now += Duration::from_millis(100);
    assert!((screen.ui.glide("bar", 1.0) - 0.75).abs() < 1e-9);
    let (tick, seconds) = (screen.ui.tick(), screen.ui.seconds());
    screen.ui.now += Duration::from_millis(30);
    assert_eq!(screen.ui.tick(), tick, "the same 100 ms step");
    assert!(screen.ui.seconds() > seconds, "but what flows has moved on");
}

// ---------------------------------------------------------------- answering input

#[tokio::test]
async fn a_box_opens_from_its_middle_without_holding_up_its_keys() {
    let mut screen = screen("reveal", json!({}));
    screen.ui.motion.enabled = true;
    screen.frame();
    screen.press(",");
    let box_rows = |frame: &str| frame.lines().filter(|line| line.contains('│')).count();
    let opening = screen.frame();
    screen.press("Down");
    assert_eq!(screen.ui.overlay.as_ref().map(Overlay::kind), Some("settings"), "keys act while it opens");
    screen.ui.now += Duration::from_millis(40);
    let partly = screen.frame();
    screen.ui.now += Duration::from_millis(200);
    let open = screen.frame();
    assert!(box_rows(&opening) < box_rows(&partly) && box_rows(&partly) < box_rows(&open), "{partly}");
}

#[tokio::test]
async fn an_action_key_lights_its_hint_and_the_apps_title_and_a_setting_flashes_where_it_shows() {
    let mut screen = screen("flash", json!({}));
    screen.ui.motion.enabled = true;
    screen.frame();
    screen.press("s");
    use super::motion::Spot;
    let now = screen.ui.now;
    assert!(screen.ui.motion.lit(&Spot::Hint("s".into()), now).is_some());
    assert!(screen.ui.motion.title_lit(screen.ui.focus, now));
    screen.press("S");
    assert!(screen.ui.motion.title_lit(3, now), "every title");
    screen.press(", f");
    assert!(screen.ui.motion.lit(&Spot::Setting(Toggle::ReducedMotion), now).is_some());
    screen.press("Esc l");
    screen.frame();
    assert!(screen.ui.motion.title_lit(1, screen.ui.now), "the column that got the focus pulses");
}

#[tokio::test]
async fn a_key_ends_the_checklist_and_is_run_and_a_key_ends_the_reload_show() {
    let mut screen = screen("no-waiting", json!({}));
    screen.press(", r");
    assert_eq!(screen.ui.overlay.as_ref().map(Overlay::kind), Some("checklist"));
    screen.press("T");
    assert!(screen.ui.replay.is_some(), "the loop runs T once the checklist is gone");
    screen.press("Esc");
    assert!(screen.ui.replay.take().is_some(), "T still waits; Esc only skips");
    screen.ui.overlay = None; // what the loop does once the skipped checklist is done
    screen.press("Ctrl+R");
    assert!(screen.ui.reload.is_some() || screen.controller.setting(Toggle::ReducedMotion));
    screen.press("j");
    assert!(screen.ui.reload.is_none(), "the show ends at the first key, which still scrolls");
}

#[tokio::test]
async fn r_in_the_checklist_retries_what_failed_and_another_key_still_closes_it() {
    let mut screen = screen("checklist-retry-key", json!({}));
    let failed = Item { name: "git".into(), status: Status::Fail, detail: "not found on PATH".into() };
    let mut list = Checklist::fixed(vec![failed], Vec::new(), "checking…");
    list.manual = true;
    list.update(screen.ui.now, &mut screen.controller);
    screen.ui.overlay = Some(Overlay::Checklist(Box::new(list)));
    assert!(screen.frame().contains("any key closes · r retries git"), "{}", screen.frame());
    screen.press("r");
    let Some(Overlay::Checklist(list)) = &screen.ui.overlay else { panic!("r keeps the checklist open") };
    assert_eq!(list.items[0].status, Status::Pending, "git is checked again");
    assert!(screen.ui.replay.is_none(), "and r isn't run as a key afterwards");
    screen.press("T");
    assert!(screen.ui.replay.is_some(), "any other key still closes it and is run");
}

#[tokio::test]
async fn s_cancels_a_build_from_inside_the_settings_menu_which_stays_open() {
    use lazyrunner_core::run::Tag;
    use lazyrunner_core::run::child::{Child, Spec};
    use lazyrunner_core::run::controller::ToolJob;
    let mut screen = screen("menu-cancel-build", json!({}));
    let (program, flag, wait) =
        if cfg!(windows) { ("cmd", "/C", "ping -n 30 127.0.0.1 > nul") } else { ("sh", "-c", "sleep 30") };
    let spec = Spec {
        program: program.into(),
        args: vec![flag.into(), wait.into()],
        dir: std::env::temp_dir(),
        env: Vec::new(),
    };
    let key = screen.controller.apps[0].key.clone();
    let (events, _keep) = tokio::sync::mpsc::unbounded_channel();
    let child = Child::spawn(&spec, Tag::Build { key: key.clone(), job: 1 }, events, None).unwrap();
    screen.controller.building = Some(ToolJob {
        key,
        id: 1,
        child,
        started: screen.ui.now,
        lines: Vec::new(),
        done: 0,
        total: 0,
        warnings: 0,
        start_after: false,
        what: "dotnet build",
    });
    screen.press(", r");
    assert!(screen.last_log().starts_with("Building - press S to cancel it first"), "{}", screen.last_log());
    screen.press("S");
    assert!(screen.controller.building.is_none(), "S cancels the build");
    assert!(matches!(screen.ui.overlay, Some(Overlay::Settings { .. })), "and the menu stays open");
    screen.press("r");
    assert!(matches!(screen.ui.overlay, Some(Overlay::Checklist(_))), "so r runs the checklist right away");
}

#[tokio::test]
async fn u_in_the_about_box_checks_for_an_update_right_there() {
    let mut screen = screen("about-update", json!({ "autoUpdate": false }));
    screen.press(", A u");
    assert!(matches!(screen.ui.overlay, Some(Overlay::About { .. })), "the box stays open");
    let deadline = Instant::now() + Duration::from_secs(10);
    while screen.ui.update_check.as_ref().is_some_and(|check| check.status.is_none()) && Instant::now() < deadline {
        assert_eq!(super::about::poll_update(&mut screen.ui, &mut screen.controller), None, "nothing installed");
        std::thread::sleep(Duration::from_millis(20));
    }
    let frame = screen.frame();
    assert!(frame.contains("↳ - v<version> · auto-update off"), "{frame}");
    assert!(screen.last_log().starts_with("lazyrunner: v"), "{}", screen.last_log());

    screen.press("Esc , A");
    assert!(screen.ui.update_check.is_none(), "reopened, the box starts without the old outcome");
}

#[tokio::test]
async fn the_menus_wrap_around_at_their_ends() {
    let mut screen = screen("menus-wrap", json!({}));
    let settings = |screen: &Screen| match &screen.ui.overlay {
        Some(Overlay::Settings { index }) => *index,
        _ => panic!("the settings menu is open"),
    };
    screen.press(", k");
    let count = super::overlays::menu_items(&screen.ui, &screen.controller).len();
    assert_eq!(settings(&screen), count - 1, "up from the first item: the last");
    screen.press("j");
    assert_eq!(settings(&screen), 0, "down from the last: the first");

    screen.press("Esc t Up");
    let Some(Overlay::Themes { index, .. }) = &screen.ui.overlay else { panic!("the theme picker is open") };
    assert_eq!(*index, screen.ui.themes.len() - 1, "the theme picker too");
    screen.press("Down");
    let Some(Overlay::Themes { index, .. }) = &screen.ui.overlay else { panic!("the theme picker is open") };
    assert_eq!(*index, 0);

    screen.press("Esc , A k");
    let Some(Overlay::About { index }) = screen.ui.overlay else { panic!("the About box is open") };
    assert!(index > 0, "and the About box's links");
    screen.press("j");
    assert!(matches!(screen.ui.overlay, Some(Overlay::About { index: 0 })));
}

#[tokio::test]
async fn a_selected_row_keeps_its_key_and_label_and_scrolls_only_its_hint() {
    let mut screen = screen("rows-tail", json!({}));
    let parts = [" f  💡  ", "Suggest a feature  ", "the feature form, for your idea"];
    let row = screen.ui.scrolling_parts(&parts, 44);
    assert!(row.starts_with(" f  💡  Suggest a feature  the feature"), "{row:?}");
    screen.ui.now += Duration::from_secs(3);
    let later = screen.ui.scrolling_parts(&parts, 44);
    assert!(later.starts_with(" f  💡  Suggest a feature  ") && later.trim_end().ends_with("your idea"), "{later:?}");
    let tight = screen.ui.scrolling_parts(&parts, 24);
    assert!(tight.starts_with(" f  💡  Sugg"), "too narrow for the label: all after the key scrolls: {tight:?}");
    assert_eq!(screen.ui.scrolling_parts(&[" f  ", "Short  ", "fits"], 40).trim_end(), " f  Short  fits");
}

#[tokio::test]
async fn a_long_row_widens_its_box_and_a_selected_one_still_too_long_scrolls() {
    let mut screen = screen("menus-long", json!({}));
    let label = "L opens the git tool beside lazyrunner (Windows Terminal)";
    screen.press(",");
    assert!(screen.frame().contains(label), "the box makes room for it: {}", screen.frame());
    let at =
        super::overlays::menu_items(&screen.ui, &screen.controller).iter().position(|item| item.key == "g").unwrap();
    if let Some(Overlay::Settings { index }) = &mut screen.ui.overlay {
        *index = at;
    }
    let narrow = screen.render(64, 32);
    assert!(narrow.contains("L opens the git tool") && !narrow.contains("(Windows Terminal)"), "{narrow}");
    assert!(screen.ui.is_animating(&screen.controller), "it scrolls, so frames keep coming");
    screen.ui.now += Duration::from_secs(3);
    let later = screen.render(64, 32);
    assert!(later.contains("(Windows Terminal)"), "then it shows its end: {later}");
    screen.press("k");
    screen.frame();
    assert!(screen.ui.marquees.borrow().is_empty(), "a row that fits doesn't scroll");
}

#[tokio::test]
async fn f_filters_the_focused_column_by_level_and_says_what_it_hides() {
    use lazyrunner_core::output::LevelFilter;
    let mut screen = screen("level-filter", json!({}));
    let at = screen.ui.now - Duration::from_secs(5);
    let key = screen.controller.states[0].key.clone();
    for (line, hue) in [
        ("info: Orders.Api[0]", Hue::Cyan),
        ("      Listing orders", Hue::Gray),
        ("fail: Orders.Api[1]", Hue::Red),
        ("      System.TimeoutException: payment provider", Hue::Red),
        ("warn: Orders.Api[2]", Hue::Yellow),
        ("dbug: Orders.Cache[3]", Hue::DarkGray),
    ] {
        screen.controller.states[0].add_line(line, hue, at);
    }
    screen.press("z f");
    assert_eq!(screen.controller.states[0].filter, LevelFilter::Errors);
    let frame = screen.frame();
    assert!(frame.contains("System.TimeoutException") && !frame.contains("Listing orders"), "{frame}");
    assert!(frame.contains("errors only · 4 lines hidden - f shows all"), "{frame}");
    assert_eq!(screen.controller.config["apps"][&key]["filter"], "errors", "kept for next time");
    assert!(screen.last_log().contains("errors only (f: warnings and errors)"), "{}", screen.last_log());

    screen.press("f");
    let frame = screen.frame();
    assert!(frame.contains("warn: Orders.Api[2]") && !frame.contains("Listing orders"), "{frame}");
    screen.press("f");
    let frame = screen.frame();
    assert!(frame.contains("Listing orders") && !frame.contains("Orders.Cache"), "info and up: {frame}");

    screen.controller.states[0].scroll = 1;
    screen.controller.states[0].add_line("      Listing more orders", Hue::Gray, at);
    assert_eq!(screen.controller.states[0].scroll, 2, "a line it shows: the scrolled-back view stays put");
    screen.controller.set_level_filter(&key, LevelFilter::Errors);
    screen.controller.states[0].scroll = 1;
    screen.controller.states[0].add_line("      Listing yet more", Hue::Gray, at);
    assert_eq!(screen.controller.states[0].scroll, 1, "a line it hides doesn't move it");

    screen.press("f f f");
    assert_eq!(screen.controller.states[0].filter, LevelFilter::All);
    assert!(screen.frame().contains("Orders.Cache") && !screen.frame().contains("hidden - f shows all"));
    assert!(screen.controller.config["apps"][&key].get("filter").is_none(), "all lines: nothing saved");
}

#[tokio::test]
async fn m_marks_lines_brackets_jump_between_them_and_the_marks_box_lists_them() {
    let mut screen = screen("marks", json!({}));
    let at = screen.ui.now - Duration::from_secs(5);
    for n in 1..=60 {
        screen.controller.states[0].add_line(&format!("line {n}"), Hue::Gray, at);
    }
    screen.press("z m");
    assert_eq!(screen.controller.states[0].marks, [60], "following: the newest line");
    assert!(screen.last_log().starts_with("Marked Orders: line 60"), "{}", screen.last_log());
    screen.controller.states[0].scroll = 30;
    screen.press("m");
    assert_eq!(screen.controller.states[0].marks, [30, 60], "scrolled back: the line at the bottom");
    assert!(screen.frame().contains("▌line 30"), "{}", screen.frame());
    screen.press("]");
    assert!(screen.frame().contains("▌line 60"), "] to the newer mark: {}", screen.frame());
    screen.press("[");
    assert!(screen.frame().contains("▌line 30"), "[ back to the older one: {}", screen.frame());

    screen.press("'");
    let frame = screen.frame();
    assert!(frame.contains("Marks") && frame.contains("line 30") && frame.contains("line 60"), "{frame}");
    screen.press("d");
    assert_eq!(screen.controller.states[0].marks, [60], "d unmarks");
    screen.press("Esc");
    screen.controller.states[0].scroll = 30;
    screen.press("m m");
    assert_eq!(screen.controller.states[0].marks, [60], "m again unmarks");
    screen.controller.states[0].reset_for_start();
    assert!(screen.controller.states[0].marks.is_empty(), "a new run, its output over again");
    screen.press("'");
    let empty = screen.frame();
    assert!(empty.contains("No marks yet.") && empty.contains("m marks the line"), "{empty}");
    screen.press("Esc");
    assert!(screen.ui.overlay.is_none());
}

#[tokio::test]
async fn a_mark_takes_a_note_and_y_copies_the_marks_as_a_timeline() {
    let mut screen = screen("mark-notes", json!({}));
    let at = screen.ui.now - Duration::from_secs(5);
    screen.controller.states[1].add_line("Retrying the payment", Hue::Gray, at + Duration::from_secs(1));
    screen.controller.states[0].add_line("Request timed out", Hue::Gray, at);
    screen.press("2 m 1 m '");
    screen.press("a q u i t Backspace Backspace Backspace Backspace t i m e o u t Space f i r s t");
    assert!(matches!(screen.ui.overlay, Some(Overlay::Marks { .. })), "q types while a note is written");
    assert!(screen.frame().contains("note: timeout first"), "{}", screen.frame());
    screen.press("Enter");
    let notes: Vec<&String> = screen.controller.states[0].notes.values().collect();
    assert_eq!(notes, ["timeout first"], "the oldest mark first: Orders'");
    assert!(screen.frame().contains("timeout first · Request timed out"), "{}", screen.frame());

    screen.press("y");
    let timeline = copied().pop().unwrap_or_default();
    let lines: Vec<&str> = timeline.lines().collect();
    assert_eq!(lines.len(), 2, "{timeline}");
    assert!(lines[0].ends_with("  Orders  timeout first - Request timed out"), "{timeline}");
    assert!(lines[1].ends_with("  Billing  Retrying the payment"), "{timeline}");
    assert!(screen.last_log().starts_with("Copied 2 marks as a timeline"), "{}", screen.last_log());

    screen.press("a Esc d");
    assert!(screen.controller.states[0].marks.is_empty() && screen.controller.states[0].notes.is_empty());
}

#[tokio::test]
async fn ctrl_o_jumps_back_to_where_you_were_and_alt_right_forward_again() {
    let mut screen = screen("jumps", json!({}));
    let at = screen.ui.now - Duration::from_secs(5);
    for n in 1..=200 {
        screen.controller.states[0].add_line(&format!("line {n}"), Hue::Gray, at);
    }
    screen.frame();
    screen.press("k k");
    assert_eq!(screen.ui.jumps.count(), None, "a line or two isn't a jump");
    screen.press("g g");
    let oldest = screen.controller.states[0].scroll;
    assert!(oldest > 2 && screen.ui.jumps.count() == Some((1, 1)), "gg is: {:?}", screen.ui.jumps.count());
    assert!(screen.frame().contains("↶ 1/1"), "{}", screen.frame());

    screen.press("Ctrl+o");
    assert_eq!(screen.controller.states[0].scroll, 2, "back where gg left");
    screen.press("Alt+Right");
    assert_eq!(screen.controller.states[0].scroll, oldest, "and forward again");

    screen.press("H");
    assert!(matches!(screen.ui.overlay, Some(Overlay::Requests(_))));
    screen.press("Alt+Left");
    assert!(screen.ui.overlay.is_none() && screen.controller.states[0].scroll == oldest, "the view was a jump");
    screen.press("Alt+Right");
    assert!(matches!(screen.ui.overlay, Some(Overlay::Requests(_))), "forward opens it again");

    screen.press("Esc G Ctrl+o");
    screen.controller.states[0].output = lazyrunner_core::output::Output::new(10);
    screen.press("Ctrl+o");
    assert!(screen.last_log().starts_with("Nowhere to jump back to"), "{}", screen.last_log());
    assert!(screen.controller.log.iter().any(|entry| entry.text.starts_with("Skipped a place")), "the gone one");
}

#[tokio::test]
async fn the_diagnostics_zip_holds_versions_settings_the_profile_logs_and_the_screen_with_secrets_masked() {
    use std::io::Read as _;
    let mut screen = screen("diagnostics", json!({ "githubToken": "gh-s3cret" }));
    let at = screen.ui.now - Duration::from_secs(5);
    screen.controller.states[0].add_line("Connecting: Server=sql;Database=Shop;Password=hunter2;", Hue::Gray, at);
    screen.press(", A z");
    let said = screen.ui.replays.pop().expect("writing in the background").recv_timeout(Duration::from_secs(60));
    let (hue, line) = said.expect("it finishes");
    assert_eq!(hue, Hue::Green, "{line}");
    let path = line.trim_start_matches("Diagnostics zip: ").split(" - ").next().unwrap_or_default().to_string();
    let mut zip = zip::ZipArchive::new(std::fs::File::open(&path).expect("the zip")).expect("a zip");
    let mut read = |name: &str| {
        let mut text = String::new();
        zip.by_name(name).unwrap_or_else(|_| panic!("{name} in it")).read_to_string(&mut text).expect("text");
        text
    };
    assert!(read("about.txt").contains(&format!("lazyrunner {}", lazyrunner_core::changelog::VERSION)));
    let settings = read("settings.json");
    assert!(settings.contains("\"githubToken\": \"***\"") && !settings.contains("gh-s3cret"), "{settings}");
    let orders = read("output/orders.log");
    assert!(orders.contains("Password=***;") && !orders.contains("hunter2"), "{orders}");
    assert!(
        read("screen.svg").starts_with("<svg")
            && !read("profile.json").is_empty()
            && !read("lazyrunner.log").is_empty()
    );

    screen.controller.add_log(line.clone(), hue);
    screen.frame();
    let hit = screen.ui.hits.iter().find(|hit| matches!(hit.target, Target::Reveal(_))).expect("the path, a link");
    let (x, y) = (hit.x, hit.y);
    screen.click_at(x, y);
    assert!(OPENED.with(|opened| opened.borrow().is_empty()), "a plain click opens nothing by default");
    screen.alt_click_at(x, y);
    assert_eq!(OPENED.with(|opened| opened.borrow().clone()), [format!("reveal {path}")]);
    assert!(screen.last_log().starts_with("Showing lazyrunner-diagnostics-"), "{}", screen.last_log());
}

#[tokio::test]
async fn a_held_key_acts_once_but_moving_and_typing_repeat() {
    use KeyEventKind::{Press, Release};
    let mut screen = screen("held", json!({}));
    let reduced = screen.controller.setting(Toggle::ReducedMotion);
    assert!(screen.event(',', Press) && !screen.event(',', Release));
    assert!(screen.event('f', Press));
    assert!(!screen.event('f', Press) && !screen.event('f', Press), "its repeats");
    assert_ne!(screen.controller.setting(Toggle::ReducedMotion), reduced, "toggled once");
    screen.event('f', Release);
    assert!(screen.event('f', Press), "pressed again");
    assert_eq!(screen.controller.setting(Toggle::ReducedMotion), reduced);
    screen.event('f', Release);

    screen.press("Esc");
    assert!(screen.event('j', Press) && screen.event('j', Press), "scrolling repeats");
    assert!(screen.event('l', Press) && screen.event('l', Press), "so does moving between columns");
    screen.press("/");
    assert!(screen.event('e', Press) && screen.event('e', Press) && screen.event('e', Press), "typing");
    assert_eq!(screen.ui.search.as_ref().map(|search| search.query.as_str()), Some("eee"));
}

#[tokio::test]
async fn a_spinner_of_your_own_spins_where_lazyrunners_would() {
    let mut screen = screen("spinner", json!({ "spinner": { "frames": ["◐", "◓", "◑", "◒"], "rate": 50 } }));
    super::spinner::refresh(&mut screen.ui, &mut screen.controller);
    assert!(screen.ui.spinner.custom);
    assert_eq!(super::columns::spinner(&screen.ui), ["◐", "◓", "◑", "◒"][(10_000 / 50) % 4]);

    screen.controller.states[0].status = lazyrunner_core::run::app::Status::Running;
    screen.press("z");
    let frame = screen.frame();
    assert!(frame.contains("◐ starting") || frame.contains("◐ compiling"), "starting spins it too: {frame}");

    screen.controller.config["spinner"] = json!({ "frames": [] });
    super::spinner::refresh(&mut screen.ui, &mut screen.controller);
    assert!(screen.last_log().contains("using lazyrunner's own"), "{}", screen.last_log());
    assert!(!screen.ui.spinner.custom);
    let logged = screen.controller.log.len();
    super::spinner::refresh(&mut screen.ui, &mut screen.controller);
    assert_eq!(screen.controller.log.len(), logged, "said once");
}

#[tokio::test]
async fn the_rail_maps_where_the_errors_are_and_a_click_jumps_there() {
    let mut screen = screen("rail", json!({}));
    let at = screen.ui.now - Duration::from_secs(5);
    let state = &mut screen.controller.states[0];
    state.output = lazyrunner_core::output::Output::new(1000);
    for n in 0..400 {
        let (text, hue) = if n == 10 { ("fail: boom".to_string(), Hue::Red) } else { (format!("line {n}"), Hue::Gray) };
        state.add_line(&text, hue, at);
    }
    let frame = screen.frame();
    assert!(!frame.contains("fail: boom") && frame.contains("█"), "{frame}");
    let top = screen
        .ui
        .hits
        .iter()
        .filter(|hit| matches!(hit.target, Target::Rail { column: 0, .. }))
        .min_by_key(|hit| hit.y)
        .map(|hit| (hit.x, hit.y, hit.target))
        .expect("a rail");
    screen.ui.hover = Some(top.2);
    let hovered = screen.frame();
    assert!(hovered.contains(" fail: boom "), "the pointer on it previews the error: {hovered}");
    screen.ui.hover = None;
    screen.click_at(top.0, top.1);
    assert!(screen.controller.states[0].scroll > 300, "{}", screen.controller.states[0].scroll);
    assert!(screen.frame().contains("fail: boom"), "{}", screen.frame());
    assert!(screen.frame().contains('⡇'), "ordinary rows are dotted by how fast they came: {}", screen.frame());

    let bottom = screen
        .ui
        .hits
        .iter()
        .filter(|hit| matches!(hit.target, Target::Rail { column: 0, .. }))
        .max_by_key(|hit| hit.y)
        .map(|hit| (hit.x, hit.y))
        .expect("a rail");
    let drag = crossterm::event::MouseEvent {
        kind: crossterm::event::MouseEventKind::Drag(crossterm::event::MouseButton::Left),
        column: bottom.0,
        row: bottom.1,
        modifiers: KeyModifiers::NONE,
    };
    assert!(mouse::wanted(&drag) && mouse::handle(&mut screen.ui, &mut screen.controller, drag));
    assert!(
        screen.controller.states[0].scroll < 20,
        "dragged down to the newest: {}",
        screen.controller.states[0].scroll
    );
}

#[tokio::test]
async fn the_start_sheet_shows_what_an_app_is_started_with_and_copies_it() {
    let mut screen = screen("sheet", json!({}));
    screen.press("x e");
    let frame = screen.frame();
    assert!(frame.contains("Start sheet · Orders"), "{frame}");
    assert!(frame.contains("Command line") && frame.contains("--launch-profile"), "{frame}");
    assert!(frame.contains("Launch profile") && frame.contains("Profile"), "{frame}");
    screen.press("y");
    let copied = copied();
    assert!(copied.last().is_some_and(|text| text.starts_with("Command line")), "{copied:?}");
    screen.press("Esc");
    assert!(screen.ui.overlay.is_none());
}

#[tokio::test]
async fn the_rail_sends_an_ember_to_a_new_error_shimmers_back_to_live_and_pulses_a_search_jump() {
    let mut screen = screen("rail-motion", json!({}));
    let at = screen.ui.now - Duration::from_secs(5);
    let state = &mut screen.controller.states[0];
    state.output = lazyrunner_core::output::Output::new(1000);
    for n in 0..400 {
        state.add_line(&format!("line {n}"), Hue::Gray, at);
    }
    state.scroll = 200;
    screen.frame();
    let now = screen.ui.now;
    let state = &mut screen.controller.states[0];
    state.add_line("fail: late", Hue::Red, now);
    state.errors += 1;
    assert!(screen.frame().contains('◉'), "an ember sets off: {}", screen.frame());
    assert!(super::rail_motion::animating(&screen.ui));
    screen.ui.now += Duration::from_millis(900);
    assert!(!screen.frame().contains('◉') && !super::rail_motion::animating(&screen.ui), "then it's done");

    screen.press("G");
    screen.frame();
    assert!(super::rail_motion::animating(&screen.ui), "back to live shimmers");
    screen.ui.now += Duration::from_millis(600);
    screen.frame();
    assert!(!super::rail_motion::animating(&screen.ui));

    let key = screen.controller.states[0].key.clone();
    screen.ui.search = Some(super::Search { query: "line 1".into(), key, index: Some(100), ..Default::default() });
    screen.frame();
    assert!(super::rail_motion::animating(&screen.ui), "a jump pulses its row");
    screen.ui.now += Duration::from_millis(450);
    screen.frame();
    assert!(!super::rail_motion::animating(&screen.ui));
}

#[tokio::test]
async fn the_rails_top_end_dims_while_the_oldest_lines_drop() {
    let mut screen = screen("rail-trimmed", json!({}));
    let at = screen.ui.now - Duration::from_secs(5);
    let state = &mut screen.controller.states[0];
    state.output = lazyrunner_core::output::Output::new(60);
    for n in 0..60 {
        state.add_line(&format!("line {n}"), Hue::Gray, at);
    }
    screen.frame();
    assert!(!super::rail_motion::animating(&screen.ui), "a full buffer seen for the first time isn't news");
    screen.controller.states[0].add_line("line 60", Hue::Gray, at);
    screen.frame();
    assert!(super::rail_motion::animating(&screen.ui), "the oldest line dropped: the top dims");
    screen.ui.now += Duration::from_millis(700);
    screen.frame();
    assert!(!super::rail_motion::animating(&screen.ui), "then it's as it was");
}

#[tokio::test]
async fn g_shimmers_from_where_the_bracket_was_while_the_view_glides_home() {
    let mut screen = screen("rail-shimmer", json!({}));
    screen.ui.motion.enabled = true;
    let at = screen.ui.now - Duration::from_secs(5);
    let state = &mut screen.controller.states[0];
    state.output = lazyrunner_core::output::Output::new(1000);
    for n in 0..400 {
        state.add_line(&format!("line {n}"), Hue::Gray, at);
    }
    state.scroll = 300;
    screen.frame();
    screen.ui.now += Duration::from_secs(1);
    screen.frame();
    let rows = screen.ui.hits.iter().filter(|hit| matches!(hit.target, Target::Rail { column: 0, .. })).count();
    screen.press("G");
    screen.frame();
    let key = screen.controller.states[0].key.clone();
    let from = super::rail_motion::shimmer_from(&screen.ui, &key).expect("G starts it at once, mid-glide");
    assert!(from + 4 < rows / 2, "from where the bracket was, well above the bottom: {from} of {rows}");
}

#[tokio::test]
async fn with_reduced_motion_nothing_moves_on_the_rail() {
    let mut screen = screen("rail-still", json!({ "reducedMotion": true }));
    let at = screen.ui.now - Duration::from_secs(5);
    let state = &mut screen.controller.states[0];
    state.output = lazyrunner_core::output::Output::new(1000);
    for n in 0..400 {
        state.add_line(&format!("line {n}"), Hue::Gray, at);
    }
    state.scroll = 200;
    screen.frame();
    let state = &mut screen.controller.states[0];
    state.add_line("fail: late", Hue::Red, at);
    state.errors += 1;
    assert!(!screen.frame().contains('◉') && !super::rail_motion::animating(&screen.ui));
}

#[tokio::test]
async fn an_empty_column_says_what_fills_it() {
    let mut screen = screen("empty-hints", json!({}));
    let frame = screen.frame();
    assert!(
        frame.contains("u starts it") && frame.contains("b builds it") && frame.contains("U starts all"),
        "{frame}"
    );
    screen.press("3");
    let npm = screen.frame();
    assert!(npm.contains("u starts it") && !npm.contains("b builds it"), "Shop's: npm builds as it starts: {npm}");
    assert_eq!(npm.matches("u starts it").count(), 1, "the keys act on the focused app only");

    screen.press("1");
    let at = screen.ui.now - Duration::from_secs(5);
    screen.controller.states[0].add_line("Now listening on: http://localhost:47201", Hue::Gray, at);
    assert!(!screen.frame().contains("u starts it"), "a column with output shows it");
    screen.press("f");
    let errors = screen.frame();
    assert!(errors.contains("No errors") && errors.contains("f shows all"), "{errors}");
}

#[tokio::test]
async fn r_in_the_checklist_never_reaches_the_app_behind_it() {
    let mut screen = screen("checklist-r-again", json!({}));
    let failed = Item { name: "git".into(), status: Status::Fail, detail: "not found on PATH".into() };
    let mut list = Checklist::fixed(vec![failed], Vec::new(), "checking…");
    list.manual = true;
    list.update(screen.ui.now, &mut screen.controller);
    screen.ui.overlay = Some(Overlay::Checklist(Box::new(list)));
    screen.press("r r r");
    let Some(Overlay::Checklist(list)) = &screen.ui.overlay else { panic!("the checklist stays open") };
    assert!(list.is_checking() && !list.skip, "r again while it retries: nothing");
    assert!(screen.ui.replay.is_none(), "no r goes on to the columns (it would restart the focused app)");

    let fine = Item { name: "git".into(), status: Status::Ok, detail: "2.50".into() };
    let mut list = Checklist::fixed(vec![fine], Vec::new(), "checking…");
    list.manual = true;
    list.update(screen.ui.now, &mut screen.controller);
    screen.ui.overlay = Some(Overlay::Checklist(Box::new(list)));
    screen.press("r");
    let Some(Overlay::Checklist(list)) = &screen.ui.overlay else { panic!("the checklist is still there") };
    assert!(list.skip, "nothing to retry: r closes it");
    assert!(screen.ui.replay.is_none(), "and isn't run on the columns");
}

#[tokio::test]
async fn alt_j_and_k_move_in_the_palette_and_the_theme_picker_while_plain_j_and_k_type() {
    let mut screen = screen("palette-alt-jk", json!({}));
    let palette = |screen: &Screen| match &screen.ui.overlay {
        Some(Overlay::Palette { index, query }) => (*index, query.clone()),
        _ => panic!("the palette is open"),
    };
    screen.press(": z o o m Alt+j");
    assert_eq!(palette(&screen), (1, "zoom".to_string()), "Alt+j moves down, and isn't typed");
    screen.press("Alt+k Alt+k");
    let count = super::actions::palette(&screen.ui, &screen.controller, "zoom").len();
    assert_eq!(palette(&screen).0, count - 1, "Alt+k moves up, wrapping around");
    screen.press("Ctrl+j");
    assert_eq!(palette(&screen).0, 0, "Ctrl+J too, where the terminal sends it");
    screen.press("j");
    assert_eq!(palette(&screen).1, "zoomj", "a plain j types");

    screen.press("Esc t Alt+j");
    let Some(Overlay::Themes { index, query, .. }) = &screen.ui.overlay else { panic!("the theme picker is open") };
    assert_eq!((*index, query.as_str()), (1, ""), "the theme picker moves the same way");
}

#[tokio::test]
async fn the_checklist_drains_its_bottom_edge_until_it_moves_on() {
    let mut screen = screen("checklist-bar", json!({}));
    let warned = Item { name: "Ports".into(), status: Status::Warn, detail: "47211 is in use".into() };
    let mut list = Checklist::fixed(vec![warned], Vec::new(), "checking…");
    list.update(screen.ui.now, &mut screen.controller);
    screen.ui.overlay = Some(Overlay::Checklist(Box::new(list)));
    let edge = |screen: &mut Screen| {
        let rows = super::overlays::checklist(&screen.ui, 120, 40);
        let bottom = rows.into_iter().rev().find(|row| row.iter().any(|seg| seg.text.contains('└'))).unwrap();
        bottom
            .iter()
            .filter(|seg| seg.text.contains('─'))
            .map(|seg| (seg.fg, seg.text.chars().count()))
            .collect::<Vec<_>>()
    };
    assert_eq!(edge(&mut screen).len(), 1, "full at first");
    screen.ui.now += Duration::from_secs(4);
    let drained = edge(&mut screen);
    assert_eq!(
        drained.iter().map(|(fg, _)| *fg).collect::<Vec<_>>(),
        [super::theme::C::Cyan, super::theme::C::DarkGray],
        "{drained:?}"
    );
    assert!(drained[0].1.abs_diff(drained[1].1) <= 2, "half gone at half time: {drained:?}");
}

#[tokio::test]
async fn copied_lines_light_up_in_their_column() {
    let mut screen = screen("copy-flash", json!({}));
    screen.ui.motion.enabled = true;
    let at = screen.ui.now;
    screen.controller.states[0].output.push("fail: Orders.Api[0]", at);
    screen.controller.states[0].output.push("      boom", at);
    screen.press("y y");
    let copied = screen.ui.motion.copied(0, screen.ui.now).cloned().unwrap_or_default();
    assert!(copied.contains("fail: Orders.Api[0]") && copied.contains("      boom"), "{copied:?}");
    screen.ui.now += Duration::from_millis(500);
    assert!(screen.ui.motion.copied(0, screen.ui.now).is_none(), "only for a moment");
}

#[tokio::test]
async fn a_saved_change_counts_down_to_the_rebuild_on_the_running_apis() {
    let mut screen = screen("countdown", json!({ "restartOnCodeChange": true }));
    screen.controller.states[0].status = AppStatus::Running;
    screen.controller.handle(Event::FileChanged(PathBuf::from("services/Orders/Checkout.cs")));
    let left = screen.controller.rebuild_in(Instant::now()).map(|(left, settle)| (left <= settle, settle.as_secs()));
    assert_eq!(left, Some((true, 2)));
    screen.ui.now = Instant::now();
    screen.press("z"); // Orders alone, wide enough for its whole title
    let frame = screen.frame();
    let orders = frame.lines().next().unwrap_or_default();
    assert!(orders.contains("change noticed") && orders.contains("rebuilding in"), "{frame}");
    assert!(screen.ui.is_animating(&screen.controller), "it counts down");
}

#[tokio::test]
async fn the_about_boxs_issue_links_open_the_repositorys_forms() {
    let mut screen = screen("about-forms", json!({}));
    screen.press(", A b f");
    let opened = OPENED.with(|opened| opened.borrow().clone());
    let os = if cfg!(windows) { "Windows" } else { "" };
    let bug = format!("issues/new?template=bug_report.yml&version={}&os={os}", lazyrunner_core::changelog::VERSION);
    assert!(opened.iter().any(|url| url.ends_with(&bug)), "{opened:?}");
    assert!(opened.iter().any(|url| url.ends_with("issues/new?template=feature_request.yml")), "{opened:?}");
}

#[tokio::test]
async fn a_settings_letter_also_highlights_the_setting_it_toggles() {
    let mut screen = screen("settings-letter", json!({}));
    screen.press(", f");
    let items = super::overlays::menu_items(&screen.ui, &screen.controller);
    let wanted = items.iter().position(|item| item.key == "f");
    let Some(Overlay::Settings { index }) = &screen.ui.overlay else { panic!("the settings stay open") };
    assert_eq!(Some(*index), wanted);
    assert!(screen.controller.setting(Toggle::ReducedMotion));
}

// ---------------------------------------------------------------- the command palette and the hints

#[tokio::test]
async fn the_palette_groups_its_keycaps_until_something_is_typed() {
    let mut screen = screen("palette-groups", json!({}));
    screen.press(":");
    let grouped = screen.frame();
    assert!(grouped.contains(" ▸ Run ─") && grouped.contains("Start Orders"), "{grouped}");
    assert!(grouped.contains("  u  "), "the key as a keycap: {grouped}");
    screen.press("z o o");
    let found = screen.frame();
    assert!(!found.contains(" ▸ Run ─") && found.contains("Zoom Orders"), "ranked, no headers: {found}");
}

#[tokio::test]
async fn the_palette_lists_what_you_ran_lately_first_and_remembers_it() {
    let mut screen = screen("palette-recent", json!({}));
    screen.press(": z o o m Enter");
    screen.press(": t h e m e Enter Esc :");
    let found = super::actions::palette(&screen.ui, &screen.controller, "");
    let first: Vec<(&str, bool)> = found.iter().take(2).map(|action| (action.key.as_str(), action.recent)).collect();
    assert_eq!(first, [("t", true), ("z", true)], "newest first");
    assert!(found.iter().skip(2).any(|action| action.key == "z" && !action.recent), "and still in its own group");
    let frame = screen.frame();
    assert!(frame.contains(" Recent ─"), "{frame}");
    assert_eq!(super::palette::recent(&screen.controller), ["t", "z"], "kept in the project's state");
}

#[tokio::test]
async fn enter_runs_at_once_while_the_palette_wipes_away() {
    let mut screen = screen("palette-enter", json!({}));
    screen.ui.motion.enabled = true;
    screen.frame();
    screen.press(": z o o m Enter");
    assert!(screen.ui.zoom && screen.ui.overlay.is_none(), "zoomed already");
    assert!(screen.ui.motion.palette_closing(screen.ui.now).is_some());
    let closing = screen.frame();
    assert!(closing.contains("─ Commands ─"), "the palette shows its pressed keycap for a frame: {closing}");
    screen.ui.now += Duration::from_millis(100);
    assert!(!screen.frame().contains("─ Commands ─"), "and is gone");
}

#[tokio::test]
async fn the_hints_follow_the_focused_apps_state() {
    let mut screen = screen("hints", json!({}));
    let hints = |screen: &mut Screen| {
        screen.frame().lines().find(|line| line.contains(":  commands")).unwrap_or_default().to_string()
    };
    let stopped = hints(&mut screen);
    assert!(
        stopped.contains(" u  start ") && stopped.contains(" b  build ") && stopped.contains(" U  start all "),
        "{stopped}"
    );
    screen.running();
    let running = hints(&mut screen);
    assert!(
        running.contains(" r  restart ") && running.contains(" S  stop all ") && !running.contains(" u  start "),
        "{running}"
    );
    assert!(!screen.frame().contains("[x] auto-update"), "the settings live in the , menu");
}

#[tokio::test]
async fn an_empty_requests_view_names_the_api_that_logs_somewhere_else() {
    let mut screen = screen("requests-silent", json!({}));
    screen.running();
    let at = screen.ui.now;
    screen.controller.states[0].output.clear();
    screen.controller.states[0].output.push("assignedTime: 11", at);
    screen.press("H");
    let frame = screen.frame();
    assert!(frame.contains("Orders prints no ASP.NET log to the console"), "{frame}");
    assert!(frame.contains("\"args\": [\"--Seq:ApiKey=\"]"), "{frame}");
}
