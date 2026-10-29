//! Keys: vim-style navigation and search, the panel keys (from the action list), the overlays, the database
//! picker, and what each action does.

use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use lazyrunner_core::changelog;
use lazyrunner_core::output::Hue;
use lazyrunner_core::profile::AppKind;
use lazyrunner_core::run::secrets;
use lazyrunner_core::run::{Command, Controller, Origin};
use serde_json::{Map, Value, json};

use super::actions::{self, Action, Run};
use super::checklist::Checklist;
use super::columns;
use super::motion::Spot;
use super::overlays::menu_items;
use super::{Exit, Overlay, Picker, Search, Ui};

/// gg: the second g has to follow within this time.
const CHORD: Duration = Duration::from_millis(600);
const DEFAULT_WIDTH: f64 = 33.0;
const PICKER_PAGE: usize = 10;

/// A key as the actions name it: "a", "A", "/", "Space", "Ctrl+L", "Enter", "Escape", "UpArrow", "Shift+Tab".
pub fn key_name(key: &KeyEvent) -> String {
    if key.modifiers.contains(KeyModifiers::CONTROL)
        && let KeyCode::Char(c) = key.code
        && c.is_ascii_alphabetic()
    {
        return format!("Ctrl+{}", c.to_ascii_uppercase());
    }
    match key.code {
        KeyCode::Char(' ') => "Space".into(),
        KeyCode::Char(c) => c.to_string(),
        KeyCode::Enter => "Enter".into(),
        KeyCode::Esc => "Escape".into(),
        KeyCode::Tab => "Tab".into(),
        KeyCode::BackTab => "Shift+Tab".into(),
        KeyCode::Backspace => "Backspace".into(),
        KeyCode::Up => "UpArrow".into(),
        KeyCode::Down => "DownArrow".into(),
        KeyCode::Left => "LeftArrow".into(),
        KeyCode::Right => "RightArrow".into(),
        KeyCode::PageUp => "PageUp".into(),
        KeyCode::PageDown => "PageDown".into(),
        KeyCode::Home => "Home".into(),
        KeyCode::End => "End".into(),
        KeyCode::Insert => "Insert".into(),
        KeyCode::Delete => "Delete".into(),
        KeyCode::F(n) => format!("F{n}"),
        _ => String::new(),
    }
}

/// The user is in a prompt: the palette, a search, a filter, a note, the theme picker's search.
pub fn prompt(ui: &Ui) -> bool {
    ui.search.as_ref().is_some_and(|search| search.typing)
        || ui.picker.as_ref().is_some_and(|picker| picker.filtering || picker.new_name.is_some())
        || matches!(&ui.overlay, Some(Overlay::Palette { .. } | Overlay::Themes { .. }))
        || matches!(&ui.overlay, Some(Overlay::Marks { note: Some(_), .. }))
        || matches!(&ui.overlay, Some(Overlay::Requests(view)) if view.typing)
}

pub(super) fn typed(key: &KeyEvent) -> Option<char> {
    match key.code {
        // AltGr arrives as Ctrl+Alt on Windows: @ [ ] { } \ | on many layouts.
        KeyCode::Char(c)
            if (!key.modifiers.contains(KeyModifiers::CONTROL) || key.modifiers.contains(KeyModifiers::ALT))
                && !c.is_control() =>
        {
            Some(c)
        }
        _ => None,
    }
}

/// Handles one key.
pub fn press(ui: &mut Ui, controller: &mut Controller, key: KeyEvent) {
    ui.user_key_at = Some(ui.now);
    let before = super::jumps::before(ui, controller);
    press_key(ui, controller, key);
    super::jumps::noticed(ui, controller, before);
}

fn press_key(ui: &mut Ui, controller: &mut Controller, key: KeyEvent) {
    let name = key_name(&key);
    if name == "Ctrl+C" {
        ui.exit = Some(Exit::Quit);
        return;
    }
    if ui.follow.on {
        if name == "F" {
            super::follow::toggle(ui, controller);
        }
        return;
    }
    // A key ends the reload show at once, and does what it does (Esc only ends it).
    if ui.reload.take().is_some() && name == "Escape" {
        return;
    }
    if let Some(Overlay::Checklist(list)) = &mut ui.overlay {
        if name == "r" {
            // r is the checklist's own: it retries what failed, does nothing while checks run, else only closes it -
            // it never reaches the columns (pressed again during a retry it used to restart the focused app).
            if !list.retry(controller) && !list.is_checking() {
                list.skip = true;
            }
            return;
        }
        // Skips what's still running; the loop closes the checklist and runs the key right away.
        list.skip = true;
        if !matches!(name.as_str(), "Escape" | "Enter" | "Space") {
            ui.replay = Some(key);
        }
        return;
    }
    if let Some(back) = super::jumps::travel_key(&name, &key)
        && super::jumps::can_travel(ui)
    {
        super::jumps::travel(ui, controller, back);
        return;
    }
    if ui.overlay.is_some() {
        overlay_key(ui, controller, &name, &key);
        return;
    }
    if ui.picker.is_some() {
        picker_key(ui, controller, &name, &key);
        return;
    }
    if ui.search.as_ref().is_some_and(|search| search.typing) {
        search_key(ui, controller, &name, &key);
        return;
    }
    let chord = ui.pending_g.is_some_and(|at| ui.now.saturating_duration_since(at) < CHORD);
    ui.pending_g = None;
    if name == "g" {
        if chord {
            scroll(ui, controller, isize::MAX / 2);
        } else {
            ui.pending_g = Some(ui.now);
        }
        return;
    }
    if navigation(ui, controller, &name) {
        return;
    }
    action_key(ui, controller, &name);
}

/// A key a view (tests, requests) doesn't use, run as on the columns; what it opens (settings, the key reference,
/// the database picker…) goes back to the view when it closes.
fn from_view(ui: &mut Ui, controller: &mut Controller, name: &str) {
    let view = ui.overlay.take();
    action_key(ui, controller, name);
    let opened = ui.overlay.is_some() || ui.picker.is_some() || ui.search.as_ref().is_some_and(|search| search.typing);
    if opened {
        ui.behind = view;
    } else {
        ui.overlay = view;
    }
}

/// The action a key runs, as the panel runs it: during a build only the safe ones (S cancels it, q quits).
fn action_key(ui: &mut Ui, controller: &mut Controller, name: &str) {
    let busy = controller.building.is_some();
    if busy && name == "S" {
        controller.cancel_start(); // like the PowerShell version: cancels the build and stops this start's apps
        return;
    }
    if busy && (name == "q" || name == "Q") {
        ui.exit = Some(Exit::Quit);
        return;
    }
    let Some(action) = actions::find(ui, controller, name) else { return };
    if busy && !action.safe {
        controller.add_log(format!("Building - press S to cancel it first, then: {}", action.label), Hue::Yellow);
        return;
    }
    ui.motion.flash(Spot::Hint(name.to_string()), ui.now);
    run(ui, controller, &action.run);
}

/// Where an action shows it was done: the title of the app it's about, or every title, or the setting it flips.
fn flash_for(ui: &mut Ui, action: &Run) {
    let spot = match action {
        Run::Toggle | Run::StartFocused | Run::StopFocused | Run::RestartFocused | Run::BuildFocused => {
            Spot::Title(ui.focus)
        }
        Run::StartAll | Run::StopAll | Run::RestartAll | Run::BuildAll => Spot::Titles,
        Run::Setting { toggle, .. } => Spot::Setting(*toggle),
        _ => return,
    };
    ui.motion.flash(spot, ui.now);
}

fn page(ui: &Ui) -> isize {
    ui.height.saturating_sub(16).max(2) as isize
}

/// Moving around: h l Tab ← → change the column, 1-9 jump to one; j k, Ctrl+u d b f, PgUp PgDn, gg G, Home End
/// scroll; n N step through search matches; Esc clears the search.
fn navigation(ui: &mut Ui, controller: &mut Controller, name: &str) -> bool {
    let page = page(ui);
    match name {
        "l" | "Tab" | "RightArrow" => move_focus(ui, controller, 1),
        "h" | "Shift+Tab" | "LeftArrow" => move_focus(ui, controller, -1),
        "k" | "UpArrow" => scroll(ui, controller, 1),
        "j" | "DownArrow" => scroll(ui, controller, -1),
        "Ctrl+U" => scroll(ui, controller, page / 2),
        "Ctrl+D" => scroll(ui, controller, -(page / 2)),
        "Ctrl+B" | "PageUp" => scroll(ui, controller, page),
        "Ctrl+F" | "PageDown" => scroll(ui, controller, -page),
        "Home" | "<" => scroll(ui, controller, isize::MAX / 2),
        "G" | "End" | ">" => {
            if let Some(state) = controller.states.get_mut(ui.focus) {
                state.scroll = 0;
            }
        }
        "n" => find_match(ui, controller, true),
        "N" => find_match(ui, controller, false),
        "Escape" => ui.search = None,
        digit if digit.len() == 1 && ('1'..='9').contains(&digit.chars().next().unwrap_or('0')) => {
            let index = digit.parse::<usize>().unwrap_or(1) - 1;
            if index < controller.apps.len() {
                ui.focus = index;
            }
        }
        _ => return false,
    }
    true
}

fn move_focus(ui: &mut Ui, controller: &Controller, step: isize) {
    let count = controller.apps.len().max(1) as isize;
    ui.focus = ((ui.focus as isize + step).rem_euclid(count)) as usize;
}

fn scroll(ui: &Ui, controller: &mut Controller, lines: isize) {
    scroll_column(controller, ui.focus, lines, ui.search.as_ref());
}

/// Scrolls a column `lines` back (negative: towards the newest), through the lines it shows.
pub fn scroll_column(controller: &mut Controller, index: usize, lines: isize, search: Option<&Search>) {
    let Some(shown) = controller.states.get(index).map(|state| columns::shown_lines(state, search).len()) else {
        return;
    };
    if let Some(state) = controller.states.get_mut(index) {
        state.scroll = state.scroll.saturating_add_signed(lines).min(shown.saturating_sub(1));
    }
}

// ---------------------------------------------------------------- search (/ then n older, N newer)

/// Typing a search: the query; Tab filters the column to the matches, Alt+r makes it a regex, Alt+c minds case.
fn search_key(ui: &mut Ui, controller: &mut Controller, name: &str, key: &KeyEvent) {
    let Some(search) = ui.search.as_mut() else { return };
    let alt = key.modifiers == KeyModifiers::ALT;
    match name {
        "Escape" => ui.search = None,
        "Backspace" => {
            search.query.pop();
            search.refresh();
        }
        "Tab" => search.filter = !search.filter,
        "r" | "R" if alt => {
            search.regex = !search.regex;
            search.refresh();
        }
        "c" | "C" if alt => {
            search.case = !search.case;
            search.refresh();
        }
        "Enter" => {
            search.typing = false;
            if search.query.is_empty() {
                ui.search = None;
            } else {
                find_match(ui, controller, true);
            }
        }
        _ => {
            if let Some(c) = typed(key) {
                search.query.push(c);
                search.refresh();
            }
        }
    }
}

/// Steps to the next match above (older) or below (newer) the current one (the first time, from the bottom of
/// what the column shows), wrapping around at the ends, and scrolls it into the middle of the column. Only the lines
/// the column shows count (its level filter, a search that filters).
fn find_match(ui: &mut Ui, controller: &mut Controller, older: bool) {
    let focus_key = controller.apps.get(ui.focus).map(|app| app.key.clone()).unwrap_or_default();
    let (body_rows, cells) = (ui.body_rows, ui.focus_cells);
    let Some(search) = ui.search.as_mut().filter(|search| !search.query.is_empty()) else {
        controller.add_log("Press / to search the focused column first.", Hue::DarkGray);
        return;
    };
    if search.key != focus_key {
        search.key = focus_key;
        search.index = None;
    }
    let Some(state) = controller.states.get(ui.focus) else { return };
    let shown = columns::shown_lines(state, Some(search));
    let hits: Vec<usize> = shown
        .iter()
        .enumerate()
        .filter(|(_, item)| !item.context && search.matches(&item.line.text))
        .map(|(at, _)| at)
        .collect();
    let (Some(&first), Some(&last)) = (hits.first(), hits.last()) else {
        search.note = "no match".into();
        search.total = 0;
        return;
    };
    let current = search.index.and_then(|index| shown.iter().position(|item| item.position == index));
    let start = current.map_or(if older { shown.len() as isize - state.scroll as isize } else { -1 }, |at| at as isize);
    let next = if older {
        hits.iter().rev().find(|&&at| (at as isize) < start)
    } else {
        hits.iter().find(|&&at| at as isize > start)
    };
    search.note.clear();
    let found = match next {
        Some(&at) => at,
        None => {
            search.note = "wrapped".into();
            if older { last } else { first }
        }
    };
    search.index = Some(shown[found].position);
    search.total = hits.len();
    search.position = hits.iter().filter(|&&at| at >= found).count(); // 1 = newest
    let scroll = columns::middle(&shown, found, cells, body_rows);
    let line = shown[found].position;
    if let Some(state) = controller.states.get_mut(ui.focus) {
        state.scroll = scroll;
    }
    ui.motion.flash(Spot::Match { app: ui.focus, line }, ui.now);
}

// ---------------------------------------------------------------- overlays

/// Scrolls a box of lines (the drawing keeps `top` in range).
fn scroll_box(top: &mut usize, name: &str) {
    match name {
        "j" | "DownArrow" => *top += 1,
        "k" | "UpArrow" => *top = top.saturating_sub(1),
        "Ctrl+D" | "PageDown" => *top += 10,
        "Ctrl+U" | "PageUp" => *top = top.saturating_sub(10),
        "g" | "Home" | "<" => *top = 0,
        "G" | "End" | ">" => *top = usize::MAX / 2,
        _ => {}
    }
}

fn overlay_key(ui: &mut Ui, controller: &mut Controller, name: &str, key: &KeyEvent) {
    let palette_len = match &ui.overlay {
        Some(Overlay::Palette { query, .. }) => actions::palette(ui, controller, query).len(),
        _ => 0,
    };
    match &mut ui.overlay {
        Some(Overlay::Checklist(_)) => {}         // press() has it
        Some(Overlay::Help) => ui.overlay = None, // any key closes the reference
        Some(Overlay::Themes { .. }) => super::theme_picker::key(ui, controller, name, key),
        Some(Overlay::About { .. }) => super::about::key(ui, controller, name),
        Some(Overlay::Changelog { top, .. }) => match name {
            "Escape" | "q" | "c" => ui.overlay = None,
            _ => scroll_box(top, name),
        },
        Some(Overlay::Marks { .. }) => super::marks::key(ui, controller, name, key),
        Some(Overlay::Durations { top }) => match name {
            "Escape" | "q" | "D" => ui.overlay = None,
            _ => scroll_box(top, name),
        },
        // The views' own keys first; the rest do what they do on the columns (, settings, d database, ...).
        Some(Overlay::Tests { .. }) => {
            if !super::suites::key(ui, controller, name) {
                from_view(ui, controller, name);
            }
        }
        Some(Overlay::Copy { index }) => {
            use super::copy::PICKS;
            let pick = match name {
                "Escape" | "q" => {
                    ui.overlay = None;
                    None
                }
                "j" | "DownArrow" => {
                    *index = wrapped(*index, 1, PICKS.len());
                    None
                }
                "k" | "UpArrow" => {
                    *index = wrapped(*index, -1, PICKS.len());
                    None
                }
                "<" | "Home" => {
                    *index = 0;
                    None
                }
                ">" | "End" => {
                    *index = PICKS.len() - 1;
                    None
                }
                "Enter" | "Space" => PICKS.get(*index).map(|(pick, ..)| *pick),
                other => PICKS.iter().find(|(_, key, _)| *key == other).map(|(pick, ..)| *pick),
            };
            if let Some(pick) = pick {
                ui.overlay = None;
                super::copy::copy(ui, controller, pick);
            }
        }
        Some(Overlay::Install { index }) => match name {
            "Escape" | "q" | "i" => ui.overlay = None,
            "j" | "DownArrow" => *index = wrapped(*index, 1, 2),
            "k" | "UpArrow" => *index = wrapped(*index, -1, 2),
            ">" => *index = 1,
            "<" => *index = 0,
            "c" | "n" | "Enter" | "Space" => {
                let clean = name == "c" || (name != "n" && *index == 0);
                ui.overlay = None;
                run(ui, controller, if clean { &Run::NpmCi } else { &Run::NpmInstall });
            }
            _ => {}
        },
        Some(Overlay::Requests(_)) => {
            if !super::requests::key(ui, controller, name, key) {
                from_view(ui, controller, name);
            }
        }
        Some(Overlay::Migrations { .. }) => {
            if !super::migrations::key(ui, controller, name) {
                from_view(ui, controller, name);
            }
        }
        Some(Overlay::Confirm(_)) => super::migrations::confirm_key(ui, controller, name),
        Some(Overlay::Palette { query, index }) => match name {
            "Escape" => ui.overlay = None,
            _ if list_step(name, key).is_some() => {
                *index = wrapped(*index, list_step(name, key).unwrap_or_default(), palette_len);
            }
            "Backspace" => {
                query.pop();
                *index = 0;
            }
            "Enter" => {
                let (query, index) = (query.clone(), *index);
                let found = actions::palette(ui, controller, &query);
                if let Some(action) = found.get(index).cloned() {
                    super::palette::remember(ui, controller, &action.key);
                    ui.motion.close_palette(&query, index, ui.now);
                    run_from_overlay(ui, controller, &action, false);
                } else {
                    ui.overlay = None;
                }
            }
            _ => {
                if let Some(c) = typed(key) {
                    query.push(c);
                    *index = 0;
                }
            }
        },
        Some(Overlay::Menu { .. } | Overlay::Settings { .. }) => {
            let settings = matches!(ui.overlay, Some(Overlay::Settings { .. }));
            let items = menu_items(ui, controller);
            let close = if settings { "," } else { "x" };
            let Some(Overlay::Menu { index } | Overlay::Settings { index }) = &mut ui.overlay else { return };
            *index = (*index).min(items.len().saturating_sub(1));
            match name {
                "Escape" | "q" => ui.overlay = None,
                _ if name == close => ui.overlay = None,
                "S" if controller.building.is_some() => controller.cancel_start(),
                "j" | "DownArrow" | "Ctrl+N" => *index = wrapped(*index, 1, items.len()),
                "<" | "Home" => *index = 0,
                ">" | "End" => *index = items.len().saturating_sub(1),
                "k" | "UpArrow" | "Ctrl+P" => *index = wrapped(*index, -1, items.len()),
                "Enter" | "Space" => {
                    if let Some(item) = items.get(*index).cloned() {
                        run_from_overlay(ui, controller, &item, settings);
                    }
                }
                _ => {
                    // The item a key runs gets the highlight too, as if chosen with j k (a setting stays in view).
                    if let Some(at) = items.iter().position(|item| item.key == name) {
                        *index = at;
                        run_from_overlay(ui, controller, &items[at], settings);
                    }
                }
            }
        }
        None => {}
    }
}

/// In a list you type into (the palette, the theme picker), the keys that move instead of typing: ↑ ↓, Ctrl+P / Ctrl+N,
/// and vim's j / k with Alt - or Ctrl, where the terminal sends Ctrl+J as itself and not as Enter.
pub fn list_step(name: &str, key: &KeyEvent) -> Option<isize> {
    let alt = key.modifiers == KeyModifiers::ALT;
    match name {
        "UpArrow" | "Ctrl+P" | "Ctrl+K" => Some(-1),
        "DownArrow" | "Ctrl+N" | "Ctrl+J" => Some(1),
        "k" if alt => Some(-1),
        "j" if alt => Some(1),
        _ => None,
    }
}

/// Moves `by` through a list of `len` items: one step down from the last goes to the first, one step up from the first
/// to the last; a bigger jump (a page, Home, End) stops at the end.
pub fn wrapped(at: usize, by: isize, len: usize) -> usize {
    let last = len.saturating_sub(1);
    match by {
        1 if at >= last => 0,
        -1 if at == 0 => last,
        _ => at.saturating_add_signed(by).min(last),
    }
}

fn run_from_overlay(ui: &mut Ui, controller: &mut Controller, action: &Action, keep_open: bool) {
    if !keep_open {
        ui.overlay = None;
    }
    if controller.building.is_some() && !action.safe {
        controller.add_log(format!("Building - press S to cancel it first, then: {}", action.label), Hue::Yellow);
        return;
    }
    if !keep_open {
        controller.add_log(format!("▸ {}", action.label), Hue::DarkGray);
    }
    run(ui, controller, &action.run);
}

// ---------------------------------------------------------------- actions

const SOURCE_CHECK: &str = "Checking for source changes…";

/// Does what an action does. One that fingerprints the source first (a start that builds) waits for a frame that
/// says so; the loop then calls `run_deferred`.
pub fn run(ui: &mut Ui, controller: &mut Controller, action: &Run) {
    flash_for(ui, action);
    if ui.deferred.is_none() && checks_sources(ui, controller, action) {
        ui.busy = Some(SOURCE_CHECK);
        ui.deferred = Some(action.clone());
        return;
    }
    perform(ui, controller, action);
}

/// Runs the action that waited for the "Checking for source changes…" frame.
pub fn run_deferred(ui: &mut Ui, controller: &mut Controller) {
    ui.busy = None;
    if let Some(action) = ui.deferred.take() {
        perform(ui, controller, &action);
    }
}

/// Whether `action` builds an app, so lazyrunner fingerprints the source (git status) first.
fn checks_sources(ui: &Ui, controller: &Controller, action: &Run) -> bool {
    if controller.building.is_some() {
        return false;
    }
    let focused = controller.apps.get(ui.focus).map(|app| app.key.as_str());
    let all = || controller.apps.iter().any(|app| controller.builds(&app.key));
    match action {
        Run::Toggle => focused.is_some_and(|key| !controller.is_running(key) && controller.builds(key)),
        Run::StartFocused | Run::RestartFocused | Run::BuildFocused => {
            focused.is_some_and(|key| controller.builds(key))
        }
        Run::StartAll | Run::RestartAll | Run::BuildAll => all(),
        _ => false,
    }
}

/// Whether `action` needs a focused app (there is none in a project without apps).
fn needs_app(action: &Run) -> bool {
    matches!(
        action,
        Run::Toggle
            | Run::StartFocused
            | Run::StopFocused
            | Run::RestartFocused
            | Run::BuildFocused
            | Run::NpmCi
            | Run::NpmInstall
            | Run::OpenFocused
            | Run::OpenLog
            | Run::Copy
            | Run::Search
            | Run::Widen
            | Run::Narrow
    )
}

fn perform(ui: &mut Ui, controller: &mut Controller, action: &Run) {
    let key = controller.apps.get(ui.focus).map(|app| app.key.clone()).unwrap_or_default();
    if key.is_empty() && needs_app(action) {
        return;
    }
    match action {
        Run::Toggle => execute(controller, Command::ToggleApp(key)),
        Run::StartFocused => execute(controller, Command::Start(key)),
        Run::StopFocused => execute(controller, Command::Stop(key)),
        Run::RestartFocused => execute(controller, Command::Restart(key)),
        Run::BuildFocused => execute(controller, Command::Build(key)),
        Run::StartAll => execute(controller, Command::StartAll),
        Run::StopAll => execute(controller, Command::StopAll),
        Run::RestartAll => execute(controller, Command::RestartAll),
        Run::BuildAll => execute(controller, Command::BuildAll),
        Run::KillPortUser => execute(controller, Command::KillPortUser),
        Run::Quit => ui.exit = Some(Exit::Quit),
        Run::Database => open_picker(ui, controller),
        Run::NpmCi => execute(controller, Command::NpmCi(key)),
        Run::NpmInstall => execute(controller, Command::NpmInstall(key)),
        Run::Install => ui.overlay = Some(Overlay::Install { index: 0 }),
        Run::OpenFocused => open_focused(ui, controller, &key),
        Run::OpenAll => open_all(ui, controller),
        Run::OpenLog => open_log(ui, controller),
        Run::Copy => ui.overlay = Some(Overlay::Copy { index: 0 }),
        Run::Durations => super::durations::open(ui, controller),
        Run::Tests => super::seen::open(ui, controller, super::jumps::View::Tests),
        Run::Requests => super::seen::open(ui, controller, super::jumps::View::Requests),
        Run::Migrations => super::seen::open(ui, controller, super::jumps::View::Migrations),
        Run::GitTool => ui.git_tool_wanted = true,
        Run::Menu => ui.overlay = Some(Overlay::Menu { index: 0 }),
        Run::Search => {
            // / again on the same column brings its last search back, switches and all.
            let last = ui.search.take().filter(|search| search.key == key);
            ui.search = Some(match last {
                Some(search) => Search { typing: true, index: None, ..search },
                None => Search { typing: true, key, ..Search::default() },
            });
        }
        Run::Zoom => ui.zoom = !ui.zoom,
        Run::JumpBack => super::jumps::travel(ui, controller, true),
        Run::JumpForward => super::jumps::travel(ui, controller, false),
        Run::Follow => super::follow::toggle(ui, controller),
        Run::Diagnostics => super::bundle::write(ui, controller),
        Run::Mark => super::marks::toggle(ui, controller),
        Run::PrevMark => super::marks::jump(ui, controller, true),
        Run::NextMark => super::marks::jump(ui, controller, false),
        Run::Marks => super::marks::open(ui),
        Run::LevelFilter => {
            let Some(state) = controller.states.get(ui.focus) else { return };
            let (key, filter) = (state.key.clone(), state.filter.next());
            controller.set_level_filter(&key, filter);
            let name = controller.apps.get(ui.focus).map_or(key, |app| app.name.clone());
            controller.add_log(format!("{name}: {} (f: {})", filter.label(), filter.next().label()), Hue::Cyan);
        }
        Run::Widen => resize(controller, &key, 5.0),
        Run::Narrow => resize(controller, &key, -5.0),
        Run::EqualWidths => {
            controller.set_setting("columnWidths", json!({}));
            controller.add_log("Column widths reset to equal (saved)", Hue::Cyan);
        }
        Run::Layout => switch_layout(controller),
        Run::Reload => super::reload::start(ui, controller),
        Run::Clear => {
            for state in &mut controller.states {
                state.output.clear();
                state.scroll = 0;
            }
            controller.log.clear();
            ui.search = None;
        }
        Run::Theme => super::theme_picker::open(ui, controller),
        Run::Settings => ui.overlay = Some(Overlay::Settings { index: 0 }),
        Run::Help => ui.overlay = Some(Overlay::Help),
        Run::Palette => {
            ui.recent_commands = super::palette::recent(controller);
            ui.overlay = Some(Overlay::Palette { query: String::new(), index: 0 });
        }
        Run::Setting { toggle, label } => {
            let on = !controller.setting(*toggle);
            execute(controller, Command::SetToggle { toggle: *toggle, on });
            controller.add_log(format!("{label}: {} (saved)", if on { "on" } else { "off" }), Hue::Cyan);
        }
        Run::Checklist => {
            ui.overlay = None;
            ui.overlay = Some(Overlay::Checklist(Box::new(Checklist::start(controller, true, &[], false))));
        }
        Run::Changelog => {
            let lines = changelog::format(&changelog::entries(changelog::CHANGELOG, None, None));
            if lines.is_empty() {
                controller.add_log("No changelog in this lazyrunner.", Hue::DarkGray);
            } else {
                ui.overlay = Some(Overlay::Changelog { lines, top: 0 });
            }
        }
        Run::About => super::about::open(ui, controller),
        Run::OpenProfile => {
            let profile = controller.project.path.clone();
            super::links::open_file(ui, controller, &profile, None, None);
        }
        Run::ReloadProfile => {
            let start = controller.states.iter().any(|state| state.is_running());
            reopen(ui, controller, controller.root.clone(), start, "reloading the project profile");
        }
    }
}

/// A key's command (it names the focused app, which exists; anything refused goes to the log anyway).
fn execute(controller: &mut Controller, command: Command) {
    if let Err(why) = controller.execute(command, Origin::Panel) {
        controller.add_log(format!("✗ {why}"), Hue::Red);
    }
}

/// Asks to open lazyrunner again on `root` (another project, or this one's profile reloaded).
pub fn reopen(ui: &mut Ui, controller: &mut Controller, root: std::path::PathBuf, start: bool, reason: &str) {
    controller.add_log(format!("Reopening lazyrunner for {} ({reason})…", root.display()), Hue::Cyan);
    ui.exit = Some(Exit::Reopen { root, start });
}

/// The widths of every column (33 when not set), with the focused one changed by `delta` (5..90), saved.
fn resize(controller: &mut Controller, key: &str, delta: f64) {
    let current = controller.config["columnWidths"].clone();
    let mut widths = Map::new();
    for app in &controller.apps {
        let width = current.get(&app.key).and_then(Value::as_f64).unwrap_or(DEFAULT_WIDTH);
        let width = if app.key == key { (width + delta).clamp(5.0, 90.0) } else { width };
        widths.insert(app.key.clone(), json!(width));
    }
    controller.set_setting("columnWidths", Value::Object(widths));
}

/// `|`: auto (stacked when narrow) → columns → stacked → auto, saved.
fn switch_layout(controller: &mut Controller) {
    let now = columns::layout(&controller.config);
    let at = columns::LAYOUTS.iter().position(|layout| *layout == now).unwrap_or(0);
    let next = columns::LAYOUTS[(at + 1) % columns::LAYOUTS.len()];
    controller.set_setting("layout", json!(next));
    let what = match next {
        "columns" => "columns side by side".to_string(),
        "stacked" => "stacked top to bottom".to_string(),
        _ => format!("auto - stacked when narrower than {} cells", columns::STACK_BELOW),
    };
    controller.add_log(format!("Layout: {what} (saved)"), Hue::Cyan);
}

// ---------------------------------------------------------------- opening things

fn open(ui: &Ui, controller: &mut Controller, target: &str) -> bool {
    let opened = (ui.opener)(target);
    if !opened {
        controller.add_log(format!("✗ Couldn't open {target}"), Hue::Red);
    }
    opened
}

/// An app's address: its own port, or for a watch build the port of the app that serves it.
fn app_url(controller: &Controller, key: &str) -> String {
    let port = controller.port(key).or_else(|| {
        controller.mode(key).and_then(|mode| mode.served_by.as_deref()).and_then(|server| controller.port(server))
    });
    format!("http://localhost:{}", port.map_or(String::new(), |port| port.to_string()))
}

fn open_app_page(ui: &Ui, controller: &mut Controller, key: &str) {
    let Some(app) = controller.app(key).cloned() else { return };
    if controller.mode(key).is_none() {
        let reason = lazyrunner_core::apptypes::no_mode_reason(&controller.root, &app, &controller.apps);
        controller.add_log(format!("{} doesn't run on this branch ({reason}).", app.name), Hue::Gray);
        return;
    }
    let url = app_url(controller, key);
    if open(ui, controller, &url) {
        controller.add_log(format!("Opened {} ({url}).", app.name), Hue::Gray);
    }
}

fn open_focused(ui: &Ui, controller: &mut Controller, key: &str) {
    let Some(app) = controller.app(key).cloned() else { return };
    match &app.kind {
        AppKind::Npm { .. } => open_app_page(ui, controller, key),
        AppKind::Dotnet { swagger, .. } => {
            let url = format!("{}{}", app_url(controller, key), swagger.clone().unwrap_or_default());
            if open(ui, controller, &url) {
                controller.add_log(format!("Opened {url}"), Hue::Gray);
            }
        }
    }
}

fn open_all(ui: &Ui, controller: &mut Controller) {
    for app in controller.apps.clone() {
        if let AppKind::Dotnet { swagger: Some(swagger), .. } = &app.kind {
            let url = format!("{}{swagger}", app_url(controller, &app.key));
            open(ui, controller, &url);
        }
    }
    controller.add_log("Opened Swagger for the APIs.", Hue::Gray);
    for app in controller.apps.clone().iter().filter(|app| app.traits().variants) {
        open_app_page(ui, controller, &app.key);
    }
}

fn open_log(ui: &Ui, controller: &mut Controller) {
    let Some(state) = controller.states.get_mut(ui.focus) else { return };
    state.flush_log();
    let own = state.log_path.clone();
    let failed = controller.last_failed_log.clone().filter(|path| controller.last_build_failed && path.exists());
    let Some(path) = failed.or(own).filter(|path| path.exists()) else {
        controller.add_log("No log yet.", Hue::Gray);
        return;
    };
    if open(ui, controller, &path.display().to_string()) {
        controller.add_log(format!("Opened {}", path.display()), Hue::Gray);
    }
}

// ---------------------------------------------------------------- the database picker

fn open_picker(ui: &mut Ui, controller: &mut Controller) {
    let path = secrets::secrets_path(&controller.root, &controller.apps);
    let groups = path
        .as_ref()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .map(|text| secrets::groups(&text))
        .unwrap_or_default();
    let Some(path) = path.filter(|_| !groups.is_empty()) else {
        let place = secrets::secrets_path(&controller.root, &controller.apps)
            .map_or("secrets.json".into(), |path| path.display().to_string());
        controller.add_log(format!("No SQL connection strings found in {place}"), Hue::Yellow);
        return;
    };
    let mut picker =
        Picker { path, groups, group: 0, index: 0, top: 0, filter: String::new(), filtering: false, new_name: None };
    picker.index = picker.active_index();
    ui.picker = Some(picker);
}

fn picker_key(ui: &mut Ui, controller: &mut Controller, name: &str, key: &KeyEvent) {
    let Some(picker) = ui.picker.as_mut() else { return };
    if let Some(new_name) = picker.new_name.as_mut() {
        match name {
            "Escape" => picker.new_name = None,
            "Backspace" => {
                new_name.pop();
            }
            "Enter" => {
                let name = new_name.trim().to_string();
                picker.new_name = None;
                let Some(source) = picker.entries().get(picker.index).cloned().filter(|_| !name.is_empty()) else {
                    return;
                };
                match secrets::add(&picker.path, &source, &name) {
                    Ok(()) => {
                        controller.add_log(
                            format!(
                                "Added {name} @ {} to secrets.json (inactive) - Enter switches to it",
                                source.server
                            ),
                            Hue::Cyan,
                        );
                        let text = std::fs::read_to_string(&picker.path).unwrap_or_default();
                        picker.groups = secrets::groups(&text);
                        picker.filter.clear();
                        picker.index =
                            picker.entries().iter().position(|entry| entry.line == source.line + 1).unwrap_or(0);
                    }
                    Err(error) => controller.add_log(format!("✗ {error}"), Hue::Red),
                }
            }
            _ => {
                if let Some(c) = typed(key) {
                    new_name.push(c);
                }
            }
        }
        return;
    }
    if picker.filtering {
        match name {
            "Escape" => {
                picker.filter.clear();
                picker.filtering = false;
                picker.index = picker.active_index();
            }
            "Enter" => picker.filtering = false,
            "Backspace" => {
                picker.filter.pop();
                picker.index = 0;
            }
            _ => {
                if let Some(c) = typed(key) {
                    picker.filter.push(c);
                    picker.index = 0;
                    picker.top = 0;
                }
            }
        }
        return;
    }
    let count = picker.entries().len();
    match name {
        "n" | "+" | "Insert" => picker.new_name = Some(String::new()),
        "y" => {
            if let Some(entry) = picker.entries().get(picker.index).cloned() {
                let path = picker.path.clone();
                super::copy::copy_row(ui.clipboard, controller, &path, &entry);
            }
        }
        "/" => picker.filtering = true,
        "Escape" | "q" | "d" => {
            if picker.filter.is_empty() {
                ui.picker = None;
            } else {
                picker.filter.clear();
                picker.index = 0;
            }
        }
        "k" | "UpArrow" => picker.index = wrapped(picker.index, -1, count),
        "j" | "DownArrow" => picker.index = wrapped(picker.index, 1, count),
        "Ctrl+U" | "PageUp" => picker.index = picker.index.saturating_sub(PICKER_PAGE),
        "Ctrl+D" | "PageDown" => picker.index = (picker.index + PICKER_PAGE).min(count.saturating_sub(1)),
        "Home" | "<" => picker.index = 0,
        "G" | "End" | ">" => picker.index = count.saturating_sub(1),
        "h" | "l" | "LeftArrow" | "RightArrow" => {
            let groups = picker.groups.len().max(1);
            let step = if matches!(name, "l" | "RightArrow") { 1 } else { groups - 1 };
            picker.group = (picker.group + step) % groups;
            picker.filter.clear();
            picker.index = picker.active_index();
            picker.top = 0;
        }
        "Enter" => {
            if let Some(entry) = picker.entries().get(picker.index).cloned() {
                select_database(ui, controller, &entry);
            }
        }
        _ => {}
    }
}

fn select_database(ui: &mut Ui, controller: &mut Controller, entry: &secrets::Entry) {
    let Some(picker) = ui.picker.take() else { return };
    let text = std::fs::read_to_string(&picker.path).unwrap_or_default();
    let active_of_key =
        secrets::connections(&text).iter().filter(|other| other.key == entry.key && other.active).count();
    if entry.active && active_of_key == 1 {
        controller.add_log(format!("Already using {}.", entry.label), Hue::Gray);
        return;
    }
    controller.switch_database(entry);
}

#[cfg(test)]
mod tests {
    use super::wrapped;

    #[test]
    fn one_step_wraps_around_a_list_and_a_bigger_jump_stops_at_its_end() {
        assert_eq!(wrapped(0, -1, 5), 4, "up from the first: the last");
        assert_eq!(wrapped(4, 1, 5), 0, "down from the last: the first");
        assert_eq!(wrapped(2, 1, 5), 3);
        assert_eq!(wrapped(3, 10, 5), 4, "a page stops at the end");
        assert_eq!(wrapped(1, -10, 5), 0);
        assert_eq!(wrapped(9, -1, 5), 4, "past the end, as after the list got shorter");
        assert_eq!(wrapped(0, 1, 0), 0, "an empty list");
    }
}
