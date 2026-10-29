//! The control panel below the columns: a rule with the theme's name, the header (the lazyrunner chip, the
//! summary, branch, database and folder), the keys that matter now as keycaps, a notice line, and the log. The
//! settings live in the , menu.

use lazyrunner_core::run::Controller;
use lazyrunner_core::run::app::Status as AppStatus;

use super::columns::{self, hue_color};
use super::icons::{self, Icon};
use super::motion::Spot;
use super::mouse::Target;
use super::seg::{Seg, on, seg, width};
use super::theme::{C, no_color};
use super::{Ui, status};

const SPARKLE: [char; 2] = ['✦', '✧'];
const DEFAULT_LOG_LINES: usize = 5;
/// "panelLogLines" beyond this is a typo, not a wish.
const MAX_LOG_LINES: usize = 50;

pub fn rows(ui: &Ui, controller: &Controller, cells: usize) -> Vec<Vec<Seg>> {
    let mut rows = Vec::new();
    let tag = if ui.theme.id != "default" || no_color() { format!(" {} ", ui.theme.name) } else { String::new() };
    rows.push(vec![seg("─".repeat(cells.saturating_sub(tag.chars().count())), C::DarkGray), seg(tag, C::DarkGray)]);
    rows.push(header(ui, controller));
    rows.push(hints(ui, controller, cells));
    rows.push(notice(ui, controller));
    let count = controller.config["panelLogLines"]
        .as_u64()
        .map_or(DEFAULT_LOG_LINES, |n| usize::try_from(n).unwrap_or(MAX_LOG_LINES).min(MAX_LOG_LINES));
    let entries: Vec<_> = controller.log.iter().rev().take(count).collect();
    for entry in entries.iter().rev() {
        rows.push(vec![seg(format!(" {} ", entry.time), C::DarkGray), seg(entry.text.clone(), hue_color(entry.hue))]);
    }
    for _ in entries.len()..count {
        rows.push(Vec::new());
    }
    rows
}

/// The lazyrunner chip shimmers while something builds; a sparkle banner celebrates "ready".
fn header(ui: &Ui, controller: &Controller) -> Vec<Seg> {
    let tick = ui.tick();
    let busy = controller.building.is_some() || controller.npm_ci.is_some();
    let reduced = ui.reduced_motion;
    let chip = if !reduced && busy && tick % 8 < 4 { C::Blue } else { C::Cyan };
    let summary = match &controller.celebrate {
        Some((text, _)) => {
            let spark = if reduced { SPARKLE[0] } else { SPARKLE[(tick / 2) as usize % 2] };
            seg(format!("  {spark} {text} {spark}"), C::Green)
        }
        None => {
            let summary = status::summary(controller);
            seg(format!("  {}", summary.line(ui.nerd)), summary.color)
        }
    };
    let mut segs = vec![on(" lazyrunner ", C::Black, chip), summary];
    if let Some((text, color)) = super::suites::badge(ui, controller) {
        segs.push(seg("   ", C::Gray));
        segs.push(seg(text, color).at(Target::Tests)); // a click opens T
    }
    segs.extend([
        seg(format!("   {} {}", icons::icon(ui.nerd, Icon::Branch), controller.branch), C::Magenta),
        seg(format!("   {} ", icons::icon(ui.nerd, Icon::Database)), C::DarkGray),
        seg(controller.db_label.clone().unwrap_or_else(|| "n/a".into()), C::Yellow),
        seg(format!("   {}", controller.root.display()), C::DarkGray),
    ]);
    segs
}

/// The keys that matter now, as keycaps: on the left the alerts and what the focused app can do as it is, on the
/// right what's always there (: and ? reach everything else). A key just pressed lights its keycap.
fn hints(ui: &Ui, controller: &Controller, cells: usize) -> Vec<Seg> {
    let mut now: Vec<(&str, &str, C)> = Vec::new();
    if !controller.conflicts.is_empty() {
        now.push(("!", "kill port user", C::Red));
    }
    if !controller.lock_changed_keys().is_empty() {
        now.push(("i", "install", C::Yellow));
    }
    now.extend(focused_hints(ui, controller));
    let always = [
        (":", "commands", C::Gray),
        ("x", "menu", C::Gray),
        ("?", "keys", C::Gray),
        (",", "settings", C::Gray),
        ("q", "quit", C::Gray),
    ];
    let pills = |items: &[(&str, &str, C)], room: usize| {
        let mut segs = Vec::new();
        for &(key, label, color) in items {
            let lit = ui.motion.lit(&Spot::Hint(key.to_string()), ui.now).is_some();
            let (cap, text) = match (lit, color) {
                (true, _) => (C::Cyan, C::Cyan),
                (false, C::Gray) => (C::Gray, C::Gray),
                (false, alert) => (alert, alert),
            };
            let item = [on(format!(" {key} "), C::Black, cap), seg(format!(" {label}  "), text)];
            if width(&segs) + width(&item) > room {
                break;
            }
            segs.extend(item);
        }
        segs
    };
    // What's always there gives way first (quit, settings, keys, then the menu; : stays), then the right now from its
    // end.
    let wanted = width(&pills(&now, usize::MAX));
    let mut right = Vec::new();
    for kept in (1..=always.len()).rev() {
        right = pills(&always[..kept], cells.saturating_sub(1));
        if wanted + width(&right) + 2 <= cells {
            break;
        }
    }
    let mut segs = vec![seg(" ", C::Gray)];
    segs.extend(pills(&now, cells.saturating_sub(width(&right) + 2)));
    let gap = cells.saturating_sub(width(&segs) + width(&right));
    segs.push(seg(" ".repeat(gap), C::Gray));
    segs.extend(right);
    segs
}

/// What the focused app can do as it is: cancel what it's busy with, start or build it stopped, restart or stop or
/// open it running, look at what went wrong when it stopped with an error; then search, and start or stop all.
fn focused_hints(ui: &Ui, controller: &Controller) -> Vec<(&'static str, &'static str, C)> {
    let (Some(app), Some(state)) = (controller.apps.get(ui.focus), controller.states.get(ui.focus)) else {
        return Vec::new();
    };
    let busy = controller.building.as_ref().is_some_and(|job| job.key == app.key)
        || controller.build_queue.contains(&app.key)
        || controller.npm_ci.as_ref().is_some_and(|job| job.key == app.key);
    let mut hints = Vec::new();
    match &state.status {
        _ if busy => hints.push(("s", "cancel", C::Gray)),
        AppStatus::Stopped => {
            hints.push(("u", "start", C::Gray));
            if app.traits().builds {
                hints.push(("b", "build", C::Gray));
            }
        }
        AppStatus::Running if state.is_up() => {
            hints.extend([("r", "restart", C::Gray), ("s", "stop", C::Gray)]);
            if controller.port(&app.key).is_some() {
                hints.push(("w", "open", C::Gray));
            }
        }
        AppStatus::Running => hints.push(("s", "stop", C::Gray)),
        AppStatus::Exited(_) | AppStatus::Error(_) => {
            hints.extend([("r", "restart", C::Gray), ("y", "copy", C::Gray), ("o", "log", C::Gray)]);
        }
    }
    if !state.output.is_empty() {
        hints.push(("/", "search", C::Gray));
    }
    let running = controller.states.iter().any(|state| state.is_running());
    hints.push(if running { ("S", "stop all", C::Gray) } else { ("U", "start all", C::Gray) });
    hints
}

/// The search prompt or where the search is, else a port conflict, else a changed package-lock.json.
fn notice(ui: &Ui, controller: &Controller) -> Vec<Seg> {
    if let Some(search) = &ui.search {
        if search.typing {
            let name = controller.app(&search.key).map_or("", |app| app.name.as_str());
            let count = controller.state(&search.key).map_or(0, |state| {
                columns::shown_lines(state, None).iter().filter(|item| search.matches(&item.line.text)).count()
            });
            let matches = if count == 1 { "match" } else { "matches" };
            let switch = |on: bool, text: &str| seg(text.to_string(), if on { C::Cyan } else { C::DarkGray });
            let mut segs = vec![seg(" /", C::Cyan), seg(search.query.clone(), C::White), seg(ui.cursor(), C::Cyan)];
            if let Some(why) = &search.invalid {
                segs.push(seg(format!("   ! not a regex, so as plain text ({why})"), C::Yellow));
            }
            segs.extend([
                seg(format!("   {name} · {count} {matches} · "), C::DarkGray),
                switch(search.filter, "Tab filter"),
                seg(" · ", C::DarkGray),
                switch(search.regex, "Alt+r regex"),
                seg(" · ", C::DarkGray),
                switch(search.case, "Alt+c case"),
                seg(" · Enter finds · Esc cancels", C::DarkGray),
            ]);
            return segs;
        }
        let no_match = search.note == "no match";
        let switches = search.switches();
        let switches = if switches.is_empty() { switches } else { format!(" · {switches}") };
        let place = if no_match {
            format!("no match{switches}")
        } else {
            let note = if search.note.is_empty() { String::new() } else { format!(" · {}", search.note) };
            format!("match {} of {} (1 = newest){note}{switches}", search.position, search.total)
        };
        return vec![
            seg(format!(" /{}", search.query), C::Cyan),
            seg(format!("   {place} · n older · N newer · Esc clears"), if no_match { C::Yellow } else { C::DarkGray }),
        ];
    }
    if !controller.conflicts.is_empty() {
        let owners: Vec<String> = controller
            .conflicts
            .iter()
            .map(|c| format!("port {} is used by {} (PID {})", c.port, c.name, c.pid))
            .collect();
        return vec![
            seg(format!(" ! {} - ", owners.join(", ")), C::Red),
            seg("[!]", C::Cyan),
            seg(" kills it and starts", C::Red),
        ];
    }
    if let Some(busy) = ui.busy {
        let marker = if ui.reduced_motion { '●' } else { super::columns::spinner(ui.tick()) };
        return vec![seg(format!(" {marker} {busy}"), C::Yellow)];
    }
    let focused = controller.apps.get(ui.focus).map(|app| app.key.as_str());
    if !controller.lock_changed_keys().is_empty()
        && let Some(key) = controller.npm_ci_key(focused)
        && let Some(lazyrunner_core::profile::AppKind::Npm { dir, .. }) = controller.app(&key).map(|app| &app.kind)
    {
        let lock = lazyrunner_core::profile::join(dir, "package-lock.json");
        return vec![
            seg(format!(" ! {lock} changed since the last npm ci - "), C::Yellow),
            seg("[i]", C::Cyan),
            seg(" reinstalls the packages", C::Yellow),
        ];
    }
    vec![seg("", C::Gray)]
}
