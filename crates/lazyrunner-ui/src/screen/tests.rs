//! Snapshot tests of every screen (insta: `cargo insta review` after a deliberate change), and the keys that
//! change what the screen shows. The sample project, without starting anything: the apps' states are set by hand.

use std::cell::RefCell;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
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
use super::{Exit, Overlay, Picker, Ui, draw, keys, text};

const SIZE: (u16, u16) = (110, 32);

struct Screen {
    temp: TempDir,
    _events: mpsc::UnboundedReceiver<Event>,
    controller: Controller,
    ui: Ui,
}

thread_local! {
    /// What `y` copied in this test (each test runs on its own thread).
    static COPIED: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
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

fn opened(_: &str) -> bool {
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
    ui.clipboard = record;
    ui.now = started + Duration::from_secs(10);
    Screen { temp, _events: rx, controller, ui }
}

impl Screen {
    fn render(&mut self, width: u16, height: u16) -> String {
        for entry in &mut self.controller.log {
            entry.time = "12:00:00".into();
        }
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| draw(frame, &mut self.ui, &self.controller)).unwrap();
        // The version in titles changes with every release; the snapshots shouldn't.
        let version = format!("v{}", lazyrunner_core::changelog::VERSION);
        text(terminal.backend().buffer()).replace(&version, "v<version>")
    }

    fn frame(&mut self) -> String {
        self.render(SIZE.0, SIZE.1)
    }

    fn press(&mut self, spec: &str) {
        for key in crate::keys::parse(spec).unwrap() {
            keys::press(&mut self.ui, &mut self.controller, key);
        }
    }

    fn last_log(&self) -> String {
        self.controller.log.back().map(|entry| entry.text.clone()).unwrap_or_default()
    }

    /// Orders up with some output, Billing still starting, the rest stopped.
    fn running(&mut self) {
        let at = self.ui.now - Duration::from_secs(5);
        let orders: &[&str] = &[
            "info: Microsoft.Hosting.Lifetime[14]",
            "      Now listening on: http://localhost:58201",
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
    screen.press("> > >");
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
    assert_eq!(screen.last_log(), "Opened http://localhost:58201/swagger");
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
async fn the_palette_selection_stops_at_its_last_command() {
    let mut screen = screen("keys-palette-end", json!({}));
    screen.press(": z o o m Down Down Down Down");
    let Some(Overlay::Palette { index, .. }) = screen.ui.overlay else { panic!("the palette is open") };
    let count = super::actions::palette(&screen.ui, &screen.controller, "zoom").len();
    assert_eq!(index, count - 1);
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
    screen.press("y");
    assert_eq!(copied(), [ERROR_ENTRY[1..5].join(NEWLINE)]);
    assert_eq!(screen.last_log(), "Copied the last error from Orders (4 lines) to the clipboard");
}

#[tokio::test]
async fn y_copies_the_search_match_with_its_indented_lines() {
    let mut screen = screen("copy-search", json!({}));
    screen.output(&ERROR_ENTRY);
    screen.press("/ N u l l R e f Enter y");
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
    screen.press("y");
    assert_eq!(copied(), [full]);
    assert_eq!(screen.last_log(), "Copied the last error from Orders (1 line) to the clipboard");
}

#[tokio::test]
async fn y_says_when_there_is_nothing_to_copy_or_the_clipboard_refuses() {
    let mut screen = screen("copy-nothing", json!({}));
    screen.output(&["info: Now listening on: http://localhost:58201"]);
    screen.press("y");
    assert!(copied().is_empty());
    assert_eq!(screen.last_log(), "Nothing to copy: no error in Orders's output (/ finds a line to copy).");
    screen.output(&ERROR_ENTRY);
    screen.ui.clipboard = |_| Err("the clipboard is busy".into());
    screen.press("y");
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
