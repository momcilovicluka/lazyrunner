//! M: EF Core migrations. On the left each DbContext of the apps with `"migrations"`; on the right the selected one's
//! migrations, newest first - applied, pending, the current one. `u` applies what's pending, `m` moves the context
//! to the chosen migration (forward, or back through their Down()), each after a yes in a box that names the
//! database.

use lazyrunner_core::output::Hue;
use lazyrunner_core::run::Controller;
use lazyrunner_core::run::controller::Work;
use lazyrunner_core::run::format_duration;
use lazyrunner_core::run::migrations::{Context, Migration, Status};

use super::columns::{fit, spinner};
use super::icons::{self, Icon};
use super::mouse::Target;
use super::overlays::boxed;
use super::seg::{Seg, aim, on, pad, seg, take_cells};
use super::theme::C;
use super::{Overlay, Ui};

/// The contexts' list: a third of the screen, within these.
const LIST_WIDTH: (usize, usize) = (30, 48);
/// Ctrl+d / Ctrl+u move this far.
const PAGE: usize = 10;
/// Migrations a confirmation names at most.
const NAMED: usize = 6;
/// The lines of dotnet ef's output shown while it works or after it failed.
const TAIL: usize = 30;

/// What the y/n box does on a yes: these contexts move (key, target migration, what the log says).
pub struct Confirm {
    pub lines: Vec<String>,
    pub jobs: Vec<(String, Option<String>, String)>,
}

/// M.
pub fn open(ui: &mut Ui, controller: &mut Controller) {
    if controller.open_migrations() {
        ui.overlay = Some(Overlay::Migrations { index: 0, picking: None });
    }
}

fn contexts(controller: &Controller) -> &[Context] {
    controller.contexts.as_deref().unwrap_or_default()
}

/// j k choose, l / Enter its migrations, h back, r check again, u apply pending, U apply pending everywhere, m
/// migrate to the chosen migration, Esc / q / M back; false for a key it doesn't use.
pub fn key(ui: &mut Ui, controller: &mut Controller, name: &str) -> bool {
    let all = contexts(controller).to_vec();
    let Some(Overlay::Migrations { index, picking }) = &mut ui.overlay else { return false };
    *index = (*index).min(all.len().saturating_sub(1));
    let selected = all.get(*index).cloned();
    let count = selected.as_ref().map_or(0, |context| context.items.len());
    let moved = |at: usize, by: isize, count: usize| at.saturating_add_signed(by).min(count.saturating_sub(1));
    let step = match name {
        "j" | "DownArrow" => Some(1),
        "k" | "UpArrow" => Some(-1),
        "Ctrl+D" | "PageDown" => Some(PAGE as isize),
        "Ctrl+U" | "PageUp" => Some(-(PAGE as isize)),
        "<" | "Home" => Some(isize::MIN / 2),
        ">" | "End" | "G" => Some(isize::MAX / 2),
        _ => None,
    };
    if let Some(by) = step {
        match picking {
            Some(at) => *at = moved(*at, by, count),
            None => *index = moved(*index, by, all.len()),
        }
        return true;
    }
    match name {
        "Escape" if picking.is_some() => *picking = None,
        "Escape" | "q" | "M" => ui.overlay = None,
        "l" | "RightArrow" | "Enter" | "Tab" if picking.is_none() => {
            if count > 0 {
                *picking = Some(0);
            } else {
                controller.add_log("No migrations to choose from yet - r checks again.", Hue::DarkGray);
            }
        }
        "h" | "LeftArrow" | "Tab" => *picking = None,
        "r" => controller.list_migrations(None),
        "u" => {
            let keys: Vec<String> = selected.iter().map(Context::key).collect();
            apply_pending(ui, controller, &keys);
        }
        "U" => {
            let keys: Vec<String> = all.iter().map(Context::key).collect();
            apply_pending(ui, controller, &keys);
        }
        "m" => match (selected, *picking) {
            (Some(context), Some(at)) => migrate_to(ui, controller, &context, at),
            _ => controller.add_log("Choose a migration first (l), then m.", Hue::DarkGray),
        },
        _ => return false,
    }
    true
}

/// Why a context can't be moved now, if it can't.
fn not_ready(context: &Context) -> Option<String> {
    if context.is_busy() {
        return Some(format!("{} is busy - wait a moment.", context.name));
    }
    if context.status != Status::Ok {
        return Some(format!("{} hasn't been checked - r checks it.", context.name));
    }
    (!context.known()).then(|| format!("{}'s migration history couldn't be read from the database.", context.name))
}

/// The y/n box, over the view (it comes back after it).
fn ask(ui: &mut Ui, confirm: Confirm) {
    ui.behind = ui.overlay.take();
    ui.overlay = Some(Overlay::Confirm(Box::new(confirm)));
}

fn database(controller: &Controller) -> String {
    controller.db_label.clone().unwrap_or_else(|| "the database".into())
}

/// u / U: applies the pending migrations of these contexts, after a yes.
fn apply_pending(ui: &mut Ui, controller: &mut Controller, keys: &[String]) {
    let mut lines = vec![format!("Apply the pending migrations to {}?", database(controller)), String::new()];
    let mut jobs = Vec::new();
    let mut why = None;
    for context in keys.iter().filter_map(|key| controller.context(key)) {
        if let Some(reason) = not_ready(context) {
            why.get_or_insert(reason);
            continue;
        }
        let pending = context.pending();
        if pending.is_empty() {
            continue;
        }
        let names: Vec<&str> = pending.iter().take(3).map(|migration| migration.name.as_str()).collect();
        let more = if pending.len() > 3 { ", …" } else { "" };
        lines.push(format!("{}: {} pending - {}{more}", context.name, pending.len(), names.join(", ")));
        let plural = if pending.len() == 1 { "" } else { "s" };
        jobs.push((context.key(), None, format!("applied {} pending migration{plural}", pending.len())));
    }
    if jobs.is_empty() {
        let nothing = "Nothing pending to apply.".to_string();
        let text = if keys.len() == 1 { why.unwrap_or(nothing) } else { nothing };
        controller.add_log(text, Hue::DarkGray);
        return;
    }
    ask(ui, Confirm { lines, jobs });
}

/// m: moves a context to the chosen migration (`at`, newest first) - forward applies up to it, back reverts the
/// newer ones through their Down() - after a yes.
fn migrate_to(ui: &mut Ui, controller: &mut Controller, context: &Context, at: usize) {
    if let Some(reason) = not_ready(context) {
        controller.add_log(reason, Hue::DarkGray);
        return;
    }
    let Some(target) = context.items.len().checked_sub(at + 1) else { return };
    let migration = &context.items[target];
    let current = context.current();
    if current == Some(target) {
        controller.add_log(format!("{} is already at {}.", context.name, migration.name), Hue::DarkGray);
        return;
    }
    let db = database(controller);
    let from = current.map_or(0, |current| current + 1);
    let forward = current.is_none_or(|current| target > current);
    let moving: Vec<&Migration> = if forward {
        context.items[from..=target].iter().collect()
    } else {
        context.items[target + 1..from].iter().rev().collect()
    };
    let count = moving.len();
    let plural = if count == 1 { "" } else { "s" };
    let (name, to) = (&context.name, &migration.name);
    let mut lines = if forward {
        vec![format!("Apply {count} migration{plural} of {name} to {db}, up to {to}?")]
    } else {
        vec![
            format!("Revert {count} migration{plural} of {name} on {db}, back to {to}?"),
            "Their Down() runs - that can drop tables, columns and the data in them.".to_string(),
        ]
    };
    lines.push(String::new());
    lines.extend(moving.iter().take(NAMED).map(|migration| migration.name.clone()));
    if count > NAMED {
        lines.push("…".into());
    }
    let what = if forward { format!("applied up to {to}") } else { format!("reverted to {to}") };
    ask(ui, Confirm { lines, jobs: vec![(context.key(), Some(migration.id.clone()), what)] });
}

/// The y/n box: y / Enter does it, n / Esc doesn't.
pub fn confirm_key(ui: &mut Ui, controller: &mut Controller, name: &str) {
    match name {
        "y" | "Enter" => {
            if let Some(Overlay::Confirm(confirm)) = ui.overlay.take() {
                for (key, target, what) in confirm.jobs {
                    controller.migrate(&key, target, what);
                }
            }
        }
        "n" | "Escape" | "q" => ui.overlay = None,
        _ => {}
    }
}

/// A click on a context selects it, on a migration chooses it; a double click on a migration is m.
pub fn click(ui: &mut Ui, controller: &mut Controller, target: Option<Target>, double: bool) -> bool {
    let Some(Overlay::Migrations { index, picking }) = &mut ui.overlay else { return false };
    match target {
        Some(Target::Item(at)) => (*index, *picking) = (at, None),
        Some(Target::Migration(at)) => {
            *picking = Some(at);
            if double {
                key(ui, controller, "m");
            }
        }
        _ => return false,
    }
    true
}

/// The wheel: over the contexts it chooses one, else it moves through the migrations (`back`: up).
pub fn wheel(ui: &mut Ui, controller: &mut Controller, target: Option<Target>, back: bool) {
    let Some(Overlay::Migrations { picking, .. }) = &mut ui.overlay else { return };
    if matches!(target, Some(Target::Item(_))) {
        *picking = None;
    } else if picking.is_none() {
        *picking = Some(0);
    }
    key(ui, controller, if back { "k" } else { "j" });
}

// ---------------------------------------------------------------- the view

pub fn rows(ui: &mut Ui, controller: &Controller, width: usize, rows: usize) -> Vec<Vec<Seg>> {
    let all = contexts(controller);
    let spin = if ui.reduced_motion { '●' } else { spinner(ui.tick()) };
    let (now, nerd) = (ui.now, ui.nerd);
    let Some(Overlay::Migrations { index, picking }) = &mut ui.overlay else { return Vec::new() };
    *index = (*index).min(all.len().saturating_sub(1));
    let (selected, picking) = (*index, *picking);
    let list_width = (width / 3).clamp(LIST_WIDTH.0, LIST_WIDTH.1).min(width.saturating_sub(20));
    let detail_width = width.saturating_sub(list_width + 2);
    let height = rows.saturating_sub(3);
    let working = match &controller.migration_job {
        Some(job) => {
            let what = match &job.work {
                Work::Find { .. } => "dbcontext list",
                Work::List { .. } => "migrations list",
                Work::Update { .. } => "database update",
            };
            let took = format_duration(now.saturating_duration_since(job.started).as_secs_f64());
            format!("   {spin} dotnet ef {what} · {took}")
        }
        None if !controller.migration_queue().is_empty() && controller.building.is_some() => {
            "   waiting for the build".into()
        }
        None => String::new(),
    };
    let db = controller.db_label.clone().unwrap_or_else(|| "n/a".into());
    let header = vec![
        on(format!(" {} Migrations ", icons::icon(nerd, Icon::Migrations)), C::Black, C::Cyan),
        seg("   database ", C::DarkGray),
        seg(db, C::Yellow),
        seg(working, C::Yellow),
    ];
    let hint = if picking.is_some() {
        " j k choose · m migrate to it (forward or back) · u apply pending · h back · Esc back"
    } else {
        " j k choose · l its migrations · u apply pending · U apply pending everywhere · r check again · Esc back"
    };
    let note =
        " dotnet ef uses each app's last build (b rebuilds it) and its Development settings - the database d picks";
    let mut out =
        vec![fit(header, width), fit(vec![seg(hint, C::DarkGray)], width), fit(vec![seg(note, C::DarkGray)], width)];
    let several_apps = all.iter().any(|context| all.first().is_some_and(|first| first.app != context.app));
    let list: Vec<Vec<Seg>> = all
        .iter()
        .enumerate()
        .map(|(at, context)| {
            let (icon, summary, color) = summary(context, spin);
            let label = if several_apps {
                let app = controller.app(&context.app).map_or(context.app.as_str(), |app| app.name.as_str());
                format!("{} · {app}", context.name)
            } else {
                context.name.clone()
            };
            let room = list_width.saturating_sub(summary.chars().count() + 7).max(4);
            let name = pad(&cut(&label, room), room);
            let row = if at == selected {
                let back = if picking.is_none() { C::FocusBack } else { C::DarkGray };
                vec![on(pad(&format!(" ▸ {icon} {name} {summary}"), list_width), C::FocusText, back)]
            } else {
                vec![seg(format!("   {icon} "), color), seg(format!("{name} "), C::Gray), seg(summary, color)]
            };
            aim(row, Target::Item(at))
        })
        .collect();
    let detail = all
        .get(selected)
        .map_or_else(Vec::new, |context| details(controller, context, picking, detail_width, height, spin));
    for row in 0..height {
        let mut line = fit(list.get(row).cloned().unwrap_or_default(), list_width);
        line.push(seg("│ ", C::FocusBorder));
        line.extend(fit(detail.get(row).cloned().unwrap_or_default(), detail_width));
        out.push(line);
    }
    out.into_iter().map(|row| aim(row, Target::Box)).collect()
}

fn cut(text: &str, cells: usize) -> String {
    if text.chars().count() > cells { format!("{}…", take_cells(text, cells.saturating_sub(1))) } else { text.into() }
}

/// What the list says about a context: its icon, its state, its colour.
fn summary(context: &Context, spin: char) -> (String, String, C) {
    match context.status {
        Status::Idle => ("·".into(), "not checked".into(), C::DarkGray),
        Status::Queued => ("◌".into(), "queued".into(), C::DarkGray),
        Status::Loading => (spin.to_string(), "checking…".into(), C::Yellow),
        Status::Migrating => (spin.to_string(), "migrating…".into(), C::Yellow),
        Status::Error => ("!".into(), "failed".into(), C::Red),
        Status::Ok if !context.known() => ("?".into(), "history unreadable".into(), C::Yellow),
        Status::Ok => match context.pending().len() {
            0 => ("✓".into(), "up to date".into(), C::Green),
            pending => ("◌".into(), format!("{pending} pending"), C::Yellow),
        },
    }
}

/// The selected context: its state, then its migrations, newest first (the chosen one highlighted).
fn details(
    controller: &Controller,
    context: &Context,
    picking: Option<usize>,
    width: usize,
    height: usize,
    spin: char,
) -> Vec<Vec<Seg>> {
    let app = controller.app(&context.app).map_or(context.app.as_str(), |app| app.name.as_str());
    let project = context.project.file_name().map_or(String::new(), |name| name.to_string_lossy().into_owned());
    let mut lines = vec![vec![seg(context.name.clone(), C::White), seg(format!("   {app} · {project}"), C::DarkGray)]];
    let output = |lines: &mut Vec<Vec<Seg>>| {
        let shown = context.output.iter().skip(context.output.len().saturating_sub(TAIL));
        lines.extend(shown.map(|line| vec![seg(format!("  {line}"), C::DarkGray)]));
    };
    if context.is_busy() && context.items.is_empty() {
        let doing = match context.status {
            Status::Queued => "◌ queued".to_string(),
            Status::Migrating => format!("{spin} migrating…"),
            _ => format!("{spin} checking…"),
        };
        lines.push(vec![seg(doing, C::Yellow)]);
        output(&mut lines);
        return lines;
    }
    if !context.error.is_empty() {
        lines.push(vec![seg(format!("! {}", context.error), C::Red)]);
        if context.status == Status::Error {
            output(&mut lines);
            return lines;
        }
        lines.push(Vec::new());
    }
    if context.items.is_empty() {
        lines.push(vec![seg("No migrations.", C::DarkGray)]);
        return lines;
    }
    let applied = context.items.iter().filter(|migration| migration.applied == Some(true)).count();
    let pending = context.pending().len();
    let checked = context
        .checked_at
        .map_or(String::new(), |at| format!(" · checked {} on {}", at.format("%H:%M:%S"), context.db));
    let color = if pending > 0 { C::Yellow } else { C::Green };
    lines.push(vec![seg(format!("{applied} applied · {pending} pending{checked}"), color)]);
    if !context.known() {
        let text = "The database's migration history couldn't be read - applied / pending is unknown.";
        lines.push(vec![seg(text, C::Yellow)]);
    }
    lines.push(Vec::new());
    let newest_first: Vec<&Migration> = context.items.iter().rev().collect();
    let current = context.current().map(|current| context.items.len() - 1 - current);
    let room = height.saturating_sub(lines.len()).max(1);
    let top = picking.map_or(0, |at| at.saturating_sub(room / 2).min(newest_first.len().saturating_sub(room)));
    for (at, migration) in newest_first.iter().enumerate().skip(top).take(room) {
        let (icon, color) = match migration.applied {
            Some(true) => ("✓", C::Green),
            Some(false) => ("◌", C::Yellow),
            None => ("?", C::DarkGray),
        };
        let stamp = migration.id.split('_').next().filter(|stamp| stamp.len() == 14).unwrap_or_default();
        let suffix = if current == Some(at) {
            "  ● current"
        } else if migration.applied == Some(false) {
            "  pending"
        } else {
            ""
        };
        let row = if picking == Some(at) {
            vec![on(pad(&format!(" {icon} {stamp}  {}{suffix}", migration.name), width), C::FocusText, C::FocusBack)]
        } else {
            let name_color = if migration.applied == Some(false) { C::Yellow } else { C::Gray };
            vec![
                seg(format!(" {icon} "), color),
                seg(format!("{stamp}  "), C::DarkGray),
                seg(migration.name.clone(), name_color),
                seg(suffix, if current == Some(at) { C::Cyan } else { C::Yellow }),
            ]
        };
        lines.push(aim(row, Target::Migration(at)));
    }
    lines
}

/// The y/n box.
pub fn confirm_rows(ui: &Ui, width: usize, rows: usize) -> Vec<Vec<Seg>> {
    let Some(Overlay::Confirm(confirm)) = &ui.overlay else { return Vec::new() };
    let lines = confirm
        .lines
        .iter()
        .map(|line| vec![seg(line.clone(), if line.contains("Down() runs") { C::Yellow } else { C::White })])
        .collect();
    boxed("Confirm", lines, "y / Enter: yes · n / Esc: no", width, rows, 90)
}
