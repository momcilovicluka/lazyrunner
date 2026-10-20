//! Keys: vim-style navigation and search, the panel keys (from the action list), the overlays, the database
//! picker, and what each action does.

use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use lazyrunner_core::changelog;
use lazyrunner_core::output::Hue;
use lazyrunner_core::profile::AppKind;
use lazyrunner_core::run::Controller;
use lazyrunner_core::run::secrets;
use serde_json::{Map, Value, json};

use super::actions::{self, Action, Run};
use super::checklist::Checklist;
use super::columns::{chunk, contains_ignore_case};
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
    let name = key_name(&key);
    if name == "Ctrl+C" {
        ui.exit = Some(Exit::Quit);
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
    let busy = controller.building.is_some();
    if busy && name == "S" {
        controller.cancel_start(); // like the PowerShell version: cancels the build and stops this start's apps
        return;
    }
    if busy && (name == "q" || name == "Q") {
        ui.exit = Some(Exit::Quit);
        return;
    }
    let Some(action) = actions::find(ui, controller, &name) else { return };
    if busy && !action.safe {
        controller.add_log(format!("Building - press S to cancel it first, then: {}", action.label), Hue::Yellow);
        return;
    }
    run(ui, controller, &action.run);
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
        "Home" => scroll(ui, controller, isize::MAX / 2),
        "G" | "End" => {
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
    if let Some(state) = controller.states.get_mut(ui.focus) {
        let max = state.output.len().saturating_sub(1);
        state.scroll = state.scroll.saturating_add_signed(lines).min(max);
    }
}

// ---------------------------------------------------------------- search (/ then n older, N newer)

fn search_key(ui: &mut Ui, controller: &mut Controller, name: &str, key: &KeyEvent) {
    let Some(search) = ui.search.as_mut() else { return };
    match name {
        "Escape" => ui.search = None,
        "Backspace" => {
            search.query.pop();
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
            }
        }
    }
}

/// Steps to the next match above (older) or below (newer) the current one (the first time, from the bottom of
/// what the column shows), wrapping around at the ends, and scrolls it into the middle of the column.
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
    let Some(state) = controller.states.get_mut(ui.focus) else { return };
    let lines: Vec<&str> =
        state.output.newest_first(0).map(|line| line.text.as_str()).collect::<Vec<_>>().into_iter().rev().collect();
    let count = lines.len() as isize;
    if count == 0 {
        search.note = "no match".into();
        search.total = 0;
        return;
    }
    let start = match search.index {
        Some(index) if (index as isize) < count => index as isize,
        _ if older => count - state.scroll as isize,
        _ => -1,
    };
    let step = if older { -1 } else { 1 };
    search.note.clear();
    let mut found = None;
    for n in 1..=count {
        let i = (start + step * n).rem_euclid(count);
        if contains_ignore_case(lines[i as usize], &search.query) {
            if (step < 0 && i > start) || (step > 0 && i < start) {
                search.note = "wrapped".into();
            }
            found = Some(i as usize);
            break;
        }
    }
    let Some(found) = found else {
        search.note = "no match".into();
        search.total = 0;
        return;
    };
    search.index = Some(found);
    search.total = lines.iter().filter(|line| contains_ignore_case(line, &search.query)).count();
    search.position = lines[found..].iter().filter(|line| contains_ignore_case(line, &search.query)).count(); // 1 = newest
    // The match in the middle of the column, counting the rows its newer lines wrap to.
    let mut rows_below = 0;
    let mut scroll = lines.len() - 1 - found;
    for line in lines[found + 1..].iter().rev() {
        let rows = chunk(line, cells).len();
        if rows_below + rows > body_rows / 2 {
            break;
        }
        rows_below += rows;
        scroll -= 1;
    }
    state.scroll = scroll;
}

// ---------------------------------------------------------------- overlays

fn overlay_key(ui: &mut Ui, controller: &mut Controller, name: &str, key: &KeyEvent) {
    let palette_len = match &ui.overlay {
        Some(Overlay::Palette { query, .. }) => actions::palette(ui, controller, query).len(),
        _ => 0,
    };
    match &mut ui.overlay {
        Some(Overlay::Checklist(list)) => list.skip = true,
        Some(Overlay::Help) => ui.overlay = None, // any key closes the reference
        Some(Overlay::Themes { .. }) => super::theme_picker::key(ui, controller, name, key),
        Some(Overlay::Changelog { top, .. }) => match name {
            "Escape" | "q" | "c" => ui.overlay = None,
            "j" | "DownArrow" => *top += 1,
            "k" | "UpArrow" => *top = top.saturating_sub(1),
            "Ctrl+D" | "PageDown" => *top += 10,
            "Ctrl+U" | "PageUp" => *top = top.saturating_sub(10),
            "g" | "Home" => *top = 0,
            "G" | "End" => *top = usize::MAX / 2,
            _ => {}
        },
        Some(Overlay::Palette { query, index }) => match name {
            "Escape" => ui.overlay = None,
            "UpArrow" | "Ctrl+P" => *index = index.saturating_sub(1),
            "DownArrow" | "Ctrl+N" => *index = (*index + 1).min(palette_len.saturating_sub(1)),
            "Backspace" => {
                query.pop();
                *index = 0;
            }
            "Enter" => {
                let (query, index) = (query.clone(), *index);
                let found = actions::palette(ui, controller, &query);
                if let Some(action) = found.get(index).cloned() {
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
                "j" | "DownArrow" | "Ctrl+N" => *index = (*index + 1).min(items.len().saturating_sub(1)),
                "k" | "UpArrow" | "Ctrl+P" => *index = index.saturating_sub(1),
                "Enter" | "Space" => {
                    if let Some(item) = items.get(*index).cloned() {
                        run_from_overlay(ui, controller, &item, settings);
                    }
                }
                _ => {
                    if let Some(item) = items.iter().find(|item| item.key == name).cloned() {
                        run_from_overlay(ui, controller, &item, settings);
                    }
                }
            }
        }
        None => {}
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

/// Whether `action` starts an app that builds, so lazyrunner fingerprints the source (git status) first.
fn checks_sources(ui: &Ui, controller: &Controller, action: &Run) -> bool {
    if controller.building.is_some() {
        return false;
    }
    let focused = controller.apps.get(ui.focus).map(|app| app.key.as_str());
    let all = || controller.apps.iter().any(|app| controller.builds(&app.key));
    match action {
        Run::Toggle => focused.is_some_and(|key| !controller.is_running(key) && controller.builds(key)),
        Run::StartFocused | Run::RestartFocused | Run::RebuildFocused => {
            focused.is_some_and(|key| controller.builds(key))
        }
        Run::StartAll | Run::RestartAll | Run::RebuildAll => all(),
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
            | Run::RebuildFocused
            | Run::NpmCi
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
    let only = [key.clone()];
    match action {
        Run::Toggle => controller.toggle(&key),
        Run::StartFocused => controller.start(Some(&only), false),
        Run::StopFocused => controller.stop_focused(&key),
        Run::RestartFocused => controller.restart("requested from the panel", Some(&only), false),
        Run::RebuildFocused => controller.restart("full rebuild", Some(&only), true),
        Run::StartAll => controller.start(None, false),
        Run::StopAll => controller.stop_all(),
        Run::RestartAll => controller.restart("requested from the panel", None, false),
        Run::RebuildAll => controller.restart("full rebuild", None, true),
        Run::KillPortUser => controller.resolve_port_conflicts(),
        Run::Quit => ui.exit = Some(Exit::Quit),
        Run::Database => open_picker(ui, controller),
        Run::NpmCi => controller.npm_ci(Some(&key)),
        Run::OpenFocused => open_focused(ui, controller, &key),
        Run::OpenAll => open_all(ui, controller),
        Run::OpenLog => open_log(ui, controller),
        Run::Copy => super::copy::copy(ui, controller),
        Run::Menu => ui.overlay = Some(Overlay::Menu { index: 0 }),
        Run::Search => ui.search = Some(Search { typing: true, key, ..Search::default() }),
        Run::Zoom => ui.zoom = !ui.zoom,
        Run::Widen => resize(controller, &key, 5.0),
        Run::Narrow => resize(controller, &key, -5.0),
        Run::EqualWidths => {
            controller.set_setting("columnWidths", json!({}));
            controller.add_log("Column widths reset to equal (saved)", Hue::Cyan);
        }
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
        Run::Palette => ui.overlay = Some(Overlay::Palette { query: String::new(), index: 0 }),
        Run::Setting { name, label } => {
            let on = !controller.setting(name);
            controller.set_setting(name, json!(on));
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
        Run::ReloadProfile => {
            let start = controller.states.iter().any(|state| state.is_running());
            reopen(ui, controller, controller.root.clone(), start, "reloading the project profile");
        }
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
        "k" | "UpArrow" => picker.index = picker.index.saturating_sub(1),
        "j" | "DownArrow" => picker.index = (picker.index + 1).min(count.saturating_sub(1)),
        "Ctrl+U" | "PageUp" => picker.index = picker.index.saturating_sub(PICKER_PAGE),
        "Ctrl+D" | "PageDown" => picker.index = (picker.index + PICKER_PAGE).min(count.saturating_sub(1)),
        "Home" => picker.index = 0,
        "G" | "End" => picker.index = count.saturating_sub(1),
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
