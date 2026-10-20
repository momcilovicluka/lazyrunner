//! The control panel below the columns: a rule with the theme's name, the header (the lazyrunner chip, the
//! summary, branch, database and folder), the one-line menu, the settings, a notice line, and the log.

use lazyrunner_core::run::Controller;

use super::columns::hue_color;
use super::seg::{Seg, on, seg};
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
    rows.push(compact_menu(controller, cells));
    rows.extend(settings_rows(controller, cells));
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
    let chip = if busy && tick % 8 < 4 { C::Blue } else { C::Cyan };
    let summary = match &controller.celebrate {
        Some((text, _)) => {
            let spark = SPARKLE[(tick / 2) as usize % 2];
            seg(format!("  {spark} {text} {spark}"), C::Green)
        }
        None => {
            let summary = status::summary(controller);
            seg(format!("  {}", summary.text), summary.color)
        }
    };
    vec![
        on(" lazyrunner ", C::Black, chip),
        summary,
        seg(format!("   ⎇ {}", controller.branch), C::Magenta),
        seg("   DB ", C::DarkGray),
        seg(controller.db_label.clone().unwrap_or_else(|| "n/a".into()), C::Yellow),
        seg(format!("   {}", controller.root.display()), C::DarkGray),
    ]
}

/// Only what fits on one line; ? and : reach everything else. Lowercase = focused app, uppercase = all.
fn compact_menu(controller: &Controller, cells: usize) -> Vec<Seg> {
    let lock_changed = !controller.lock_changed_keys().is_empty();
    let mut items: Vec<(&str, &str, C)> = Vec::new();
    if !controller.conflicts.is_empty() {
        items.push(("!", "kill port user", C::Red));
    }
    if lock_changed {
        items.push(("i", "npm ci", C::Yellow));
    }
    let menu = [
        ("?", "keys"),
        (":", "commands"),
        ("space", "start/stop"),
        ("u/U", "start"),
        ("s/S", "stop"),
        ("r/R", "restart"),
        ("b/B", "rebuild"),
        ("d", "database"),
        ("w", "open"),
        ("x", "menu"),
        ("/", "search"),
        (",", "settings"),
        ("q", "quit"),
    ];
    items.extend(menu.iter().map(|(key, label)| (*key, *label, C::Gray)));
    if controller.apps.iter().any(|app| app.traits().installs) && !lock_changed {
        items.push(("i", "npm ci", C::Gray));
    }
    let mut segs = Vec::new();
    let mut used = 0;
    for (key, label, color) in items {
        let length = key.chars().count() + label.chars().count() + 5;
        if used + length > cells {
            break;
        }
        segs.push(seg(format!(" {key}"), C::Cyan));
        segs.push(seg(format!(" {label}  "), color));
        used += length;
    }
    segs
}

/// Every on/off setting, always visible (, opens the menu where the letter shown toggles it); wraps onto more
/// rows when the window is narrow.
fn settings_rows(controller: &Controller, cells: usize) -> Vec<Vec<Seg>> {
    let mut settings = vec![
        ("a", "auto-restart on branch change", controller.setting("autoRestartOnBranchChange")),
        ("u", "auto-start when lazyrunner opens", controller.setting("autoStartOnOpen")),
    ];
    if controller.apps.iter().any(|app| app.traits().frontend) {
        settings.push(("s", "stop/restart all include frontend", controller.setting("stopFrontendOnStop")));
    }
    settings.extend([
        ("e", "restart APIs on code change", controller.setting("restartOnCodeChange")),
        ("w", "startup checklist", controller.setting("startupChecklist")),
        ("v", "auto-update", controller.setting("autoUpdate")),
    ]);
    let mut rows = Vec::new();
    let mut current = vec![seg(" ,", C::Cyan), seg(" settings", C::DarkGray)];
    let mut used = 11;
    for (key, label, on) in settings {
        let check = if on { "[x]" } else { "[ ]" };
        let length = key.len() + check.len() + label.len() + 6;
        if used + length > cells && current.len() > 1 {
            rows.push(std::mem::take(&mut current));
            current.push(seg(" ".repeat(11), C::Gray));
            used = 11;
        }
        current.push(seg(format!("   {key} "), C::DarkCyan));
        current.push(seg(check, if on { C::Green } else { C::DarkGray }));
        current.push(seg(format!(" {label}"), if on { C::Gray } else { C::DarkGray }));
        used += length;
    }
    rows.push(current);
    rows
}

/// The search prompt or where the search is, else a port conflict, else a changed package-lock.json.
fn notice(ui: &Ui, controller: &Controller) -> Vec<Seg> {
    if let Some(search) = &ui.search {
        if search.typing {
            let name = controller.app(&search.key).map_or("", |app| app.name.as_str());
            return vec![
                seg(" /", C::Cyan),
                seg(search.query.clone(), C::White),
                seg(ui.cursor(), C::Cyan),
                seg(format!("   search {name} · Enter finds · Esc cancels"), C::DarkGray),
            ];
        }
        let no_match = search.note == "no match";
        let place = if no_match {
            "no match".to_string()
        } else {
            let note = if search.note.is_empty() { String::new() } else { format!(" · {}", search.note) };
            format!("match {} of {} (1 = newest){note}", search.position, search.total)
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
        return vec![seg(format!(" {} {busy}", super::columns::spinner(ui.tick())), C::Yellow)];
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
