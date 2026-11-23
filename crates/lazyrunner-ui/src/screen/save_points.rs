//! The database screen's save points (#110), under the databases: the highlighted one's rail and what changed since
//! its picked save point. `s` makes a snapshot save point, `S` a backup one, `r` rewinds to the picked one (after a
//! yes that says what it loses), `R` pins it, `u` counts the rows again, `Tab` gives the rail the keys (`j` `k` pick a
//! save point there, `F2` renames it, `Del` drops it). The motion is worked out here each frame: a new save point
//! drops in with a ripple while its database's row glints, a rewind glides "you are here" down the rail as the server
//! restores, the change bars grow from their axis and tween to new counts, and the pinned chip breathes while a
//! pinned rewind holds a restart. With reduced motion everything steps.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use lazyrunner_core::output::Hue;
use lazyrunner_core::run::Controller;
use lazyrunner_core::run::controller::save_points::db_key;
use lazyrunner_core::run::controller::{Doing, Sessions};
use lazyrunner_core::run::save_points::{self as points, Kind, SavePoint};
use lazyrunner_core::run::secrets::Entry;

use super::migrations::{Confirm, Risk, SavePointYes};
use super::motion::ease;
use super::save_rail::{self, Bar, Node, Panel, Rail};
use super::seg::{Seg, seg};
use super::theme::C;
use super::{Overlay, Ui};

/// A new save point drops into place this long, then one ring ripples out from it this long; its database's row
/// glints once.
const DROP: Duration = Duration::from_millis(150);
const RIPPLE: Duration = Duration::from_millis(450);
const GLINT: Duration = super::motion::SWEEP;
/// "you are here" glides to each new share the server reports this long; after a rewind it rests on the save point
/// this long before it's back on top.
const GLIDE: Duration = Duration::from_millis(140);
const HOLD: Duration = Duration::from_millis(700);
/// A snapshot revert reports no progress: "you are here" eases towards this share of the way, about this fast, until
/// it's done.
const SNAPSHOT_MOST: f64 = 0.9;
const SNAPSHOT_PACE: f64 = 0.6;
/// The change bars grow from their axis this long, each row this long after the one above; new counts tween in this
/// long (the numbers rolling like an odometer).
const GROW: Duration = Duration::from_millis(150);
const STAGGER: Duration = Duration::from_millis(30);
const TWEEN: Duration = Duration::from_millis(400);
/// The pinned chip breathes this slowly while a pinned rewind holds a restart.
const BREATH: Duration = Duration::from_millis(1600);
/// Counts older than this are counted again when their database is highlighted.
const STALE: Duration = Duration::from_secs(10);
/// From this wide the rail and the changes are side by side, the rail taking this share.
const SIDE_BY_SIDE: usize = 100;
const RAIL_PERCENT: usize = 44;
/// The rows the save points get under the databases at least.
pub const LOWER_LEAST: usize = 9;
/// After a rewind the apps on the database show a dot each until they all answer, this long at most.
const APPS_WATCH: Duration = Duration::from_secs(60);

/// The save points' part of the database screen: whether the rail has the keys, the picked save point (the newest
/// first), a name being typed, the rewind being asked about (who else is connected, found in the background), and
/// the motion's state.
#[derive(Default)]
pub struct View {
    pub on_rail: bool,
    pub picked: usize,
    pub renaming: Option<String>,
    pub asking: Option<(String, Sessions)>,
    /// The database shown, by [`db_key`]; the save points seen (one more drops in).
    db: Option<String>,
    known: Option<Vec<String>>,
    dropped: Option<(String, Instant)>,
    glint: Option<(String, Instant)>,
    /// When the changes opened (their bars grow), and each table's bar tweening: from, to, since.
    opened: Option<Instant>,
    tweens: HashMap<String, (f64, f64, Instant)>,
}

/// The highlighted connection string.
fn highlighted(ui: &Ui) -> Option<Entry> {
    let picker = ui.picker.as_ref()?;
    picker.entries().get(picker.index).cloned()
}

/// Whether nothing moves: reduced motion, or the motion turned off (the screen tests' frozen frames).
fn still(ui: &Ui) -> bool {
    ui.reduced_motion || !ui.motion.enabled
}

fn since(ui: &Ui, at: Instant) -> Duration {
    ui.now.saturating_duration_since(at)
}

/// As the screen opens: the motion starts afresh and the highlighted database's rows are counted.
pub fn opened(ui: &mut Ui, controller: &mut Controller) {
    ui.save_view = View { opened: Some(ui.now), ..View::default() };
    controller.know_database_apps();
    follow_highlight(ui, controller);
}

/// After a key: another database highlighted starts on its newest save point, its rows counted when they're stale.
pub fn follow_highlight(ui: &mut Ui, controller: &mut Controller) {
    let Some(entry) = highlighted(ui) else { return };
    let key = db_key(&entry.server, &entry.database);
    let view = &mut ui.save_view;
    if view.db.as_deref() != Some(key.as_str()) {
        view.db = Some(key);
        view.picked = 0;
        view.tweens.clear();
        view.opened = Some(ui.now);
    }
    let has = !controller.save_points_of(&entry.server, &entry.database).is_empty();
    if !has {
        ui.save_view.on_rail = false;
        return;
    }
    let stale = controller
        .counts(&entry.server, &entry.database)
        .and_then(|counts| counts.at)
        .is_none_or(|at| ui.now.saturating_duration_since(at) > STALE);
    if stale {
        controller.count_tables(&entry.server, &entry.database);
    }
}

/// Whether a save point's name is being typed (letters are text then).
pub fn typing(ui: &Ui) -> bool {
    ui.picker.is_some() && ui.save_view.renaming.is_some()
}

/// The save points' keys on the database screen; whether the key was one of theirs.
pub fn key(ui: &mut Ui, controller: &mut Controller, name: &str, typed: Option<char>) -> bool {
    if ui.save_view.renaming.is_some() {
        rename_key(ui, controller, name, typed);
        return true;
    }
    let Some(entry) = highlighted(ui) else { return false };
    let all: Vec<SavePoint> = controller.save_points_of(&entry.server, &entry.database).into_iter().cloned().collect();
    let picked = all.get(ui.save_view.picked.min(all.len().saturating_sub(1))).cloned();
    match name {
        "s" | "S" => {
            let kind = if name == "S" { Kind::Backup } else { Kind::Snapshot };
            if let Err(why) = controller.make_save_point(&entry.server, &entry.database, kind, None) {
                controller.add_log(format!("✗ {why}"), Hue::Red);
            }
        }
        "j" | "DownArrow" if ui.save_view.on_rail => {
            ui.save_view.picked = (ui.save_view.picked + 1).min(all.len().saturating_sub(1));
        }
        "k" | "UpArrow" if ui.save_view.on_rail => ui.save_view.picked = ui.save_view.picked.saturating_sub(1),
        "Escape" if ui.save_view.on_rail => ui.save_view.on_rail = false,
        "u" => controller.count_tables(&entry.server, &entry.database),
        "r" | "R" | "F2" | "Delete" => {
            let Some(point) = picked else {
                controller.add_log(format!("No save points of {} yet - s makes one.", entry.database), Hue::Gray);
                return true;
            };
            match name {
                "r" => ask_rewind(ui, controller, &point),
                "R" => {
                    if let Err(why) = controller.pin_save_point(&point.id) {
                        controller.add_log(format!("✗ {why}"), Hue::Red);
                    }
                }
                "F2" => ui.save_view.renaming = Some(point.label.clone()),
                _ => ask_forget(ui, &point),
            }
        }
        _ => return false,
    }
    true
}

fn rename_key(ui: &mut Ui, controller: &mut Controller, name: &str, typed: Option<char>) {
    let Some(text) = ui.save_view.renaming.as_mut() else { return };
    match name {
        "Escape" => ui.save_view.renaming = None,
        "Backspace" => {
            text.pop();
        }
        "Enter" => {
            let label = text.clone();
            ui.save_view.renaming = None;
            let Some(entry) = highlighted(ui) else { return };
            let all = controller.save_points_of(&entry.server, &entry.database);
            let Some(id) = all.get(ui.save_view.picked).map(|point| point.id.clone()) else { return };
            if let Err(why) = controller.rename_save_point(&id, &label) {
                controller.add_log(format!("✗ {why}"), Hue::Red);
            }
        }
        _ => {
            if let Some(c) = typed {
                text.push(c);
            }
        }
    }
}

/// `r`: who else is connected is found first; the y/n box opens once it's known.
fn ask_rewind(ui: &mut Ui, controller: &mut Controller, point: &SavePoint) {
    if let Some(busy) = controller.database_busy() {
        controller.add_log(format!("lazyrunner is {busy} - rewind once it's done"), Hue::Yellow);
        return;
    }
    if point.gone {
        controller.add_log(format!("\"{}\"'s snapshot is no longer on the server.", point.label), Hue::Yellow);
        return;
    }
    let pending = controller.other_sessions(&point.server, &point.database);
    ui.save_view.asking = Some((point.id.clone(), pending));
}

/// `Del`: drops the picked save point, after a yes.
fn ask_forget(ui: &mut Ui, point: &SavePoint) {
    let mut lines = match point.kind {
        Kind::Snapshot if !point.gone => vec![
            format!("Drop save point \"{}\" ({}) of {}?", point.label, point.clock(), point.database),
            "Its snapshot is dropped on the server: you can't rewind to it any more.".to_string(),
        ],
        _ => vec![format!("Forget save point \"{}\" ({}) of {}?", point.label, point.clock(), point.database)],
    };
    if let Some(file) = point.backup.as_ref() {
        lines.push(format!("Its backup file stays where it is: {file}"));
    }
    let risk = if point.kind == Kind::Snapshot && !point.gone { Risk::LosesData } else { Risk::Revert };
    open_confirm(
        ui,
        Confirm { lines, jobs: Vec::new(), risk, save_point: Some(SavePointYes::Forget(point.id.clone())) },
    );
}

fn open_confirm(ui: &mut Ui, confirm: Confirm) {
    ui.behind = ui.overlay.take();
    ui.overlay = Some(Overlay::Confirm(Box::new(confirm)));
}

/// Each frame: the rewind's y/n box opens once who else is connected is known.
pub fn poll(ui: &mut Ui, controller: &Controller) {
    let Some((id, pending)) = ui.save_view.asking.as_ref() else { return };
    let Some(found) = pending.lock().ok().and_then(|mut slot| slot.take()) else { return };
    let id = id.clone();
    ui.save_view.asking = None;
    if ui.picker.is_none() || ui.overlay.is_some() {
        return;
    }
    let Some(point) = controller.save_point(&id).cloned() else { return };
    let lines = rewind_lines(controller, &point, found);
    open_confirm(
        ui,
        Confirm { lines, jobs: Vec::new(), risk: Risk::LosesData, save_point: Some(SavePointYes::Rewind(id)) },
    );
}

/// What the rewind's y/n box says: what it rewinds to, what's lost, which apps stop, who else is cut off, which save
/// points go.
fn rewind_lines(controller: &Controller, point: &SavePoint, others: Result<Vec<String>, String>) -> Vec<String> {
    let mut lines = vec![format!(
        "Rewind {} @ {} to \"{}\" ({}, {})?",
        point.database,
        point.server,
        point.label,
        point.clock(),
        point.kind.name()
    )];
    let changed = controller
        .counts(&point.server, &point.database)
        .filter(|counts| counts.at.is_some())
        .map(|counts| points::summary(&points::changes(&point.tables, &counts.tables)))
        .filter(|said| said != "nothing changed");
    lines.push(match changed {
        Some(changed) => format!("What changed since is lost: {changed}."),
        None => "What changed since is lost.".to_string(),
    });
    let apps = controller.apps_on(&point.server, &point.database);
    let running: Vec<String> = apps.into_iter().filter(|key| controller.is_running(key)).collect();
    if !running.is_empty() {
        lines.push(format!("{} stop while it restores, then start again.", controller.names(&running)));
    }
    match others {
        Ok(others) if !others.is_empty() => {
            lines.push(String::new());
            lines.push("Also connected - their sessions are cut off (SINGLE_USER WITH ROLLBACK IMMEDIATE):".into());
            lines.extend(others.into_iter().map(|other| format!("  {other}")));
        }
        Ok(_) => {}
        Err(why) => lines.push(format!("Who else is connected couldn't be read ({why}).")),
    }
    let lost = controller.lost_by_rewind(&point.id);
    if !lost.is_empty() {
        lines.push(String::new());
        lines.push("These save points go too - SQL Server reverts only from a database's one snapshot:".into());
        lines.extend(lost.iter().map(|other| format!("  \"{}\" ({})", other.label, other.clock())));
    }
    lines
}

/// A yes in the y/n box for a save point.
pub fn yes(controller: &mut Controller, yes: SavePointYes) {
    let result = match yes {
        SavePointYes::Rewind(id) => controller.rewind_to(&id, None),
        SavePointYes::Forget(id) => controller.forget_save_point(&id),
        _ => return,
    };
    if let Err(why) = result {
        controller.add_log(format!("✗ {why}"), Hue::Red);
    }
}

/// How far a share has come, gliding from the one before to the newest as the server reports it; with none (a
/// snapshot revert), easing towards most of the way over its usual time.
fn share_at(started: Instant, share: Option<(f64, f64, Instant)>, now: Instant, still: bool) -> f64 {
    match share {
        Some((_, newest, _)) if still => newest,
        Some((before, newest, at)) => {
            before + (newest - before) * ease(now.saturating_duration_since(at).as_secs_f64() / GLIDE.as_secs_f64())
        }
        None if still => 0.0,
        None => SNAPSHOT_MOST * (1.0 - (-now.saturating_duration_since(started).as_secs_f64() / SNAPSHOT_PACE).exp()),
    }
}

/// A rewind of the database `key` on the rail: the save point it goes to, how far it has come (0 to 1), whether it's
/// done; done ones rest on their save point a moment, then they're over.
fn rewinding(controller: &Controller, key: &str, now: Instant, still: bool) -> Option<(String, f64, bool)> {
    if let Some(work) = controller.db_work.as_ref().filter(|work| db_key(&work.server, &work.database) == key)
        && let Doing::Rewinding(id) = &work.doing
    {
        return Some((id.clone(), share_at(work.started, work.share, now, still), false));
    }
    let done = controller.db_done.as_ref().filter(|done| done.ok && db_key(&done.server, &done.database) == key)?;
    let Doing::Rewinding(id) = &done.doing else { return None };
    let after = now.saturating_duration_since(done.ended);
    if after > GLIDE + HOLD {
        return None;
    }
    let ended = share_at(done.started, done.share, done.ended, still);
    let share = if still { 1.0 } else { ended + (1.0 - ended) * ease(after.as_secs_f64() / GLIDE.as_secs_f64()) };
    Some((id.clone(), share, true))
}

/// The bars' scale while a rewind runs on the database: they shrink to the axis with it, and stay there until the
/// rows are counted again.
fn bar_scale(controller: &Controller, entry: &Entry, now: Instant, still: bool) -> f64 {
    let key = db_key(&entry.server, &entry.database);
    if let Some((_, share, _)) = rewinding(controller, &key, now, still) {
        return 1.0 - share;
    }
    let counted = controller.counts(&entry.server, &entry.database).and_then(|counts| counts.at);
    match controller.db_done.as_ref() {
        Some(done)
            if done.ok
                && matches!(done.doing, Doing::Rewinding(_))
                && db_key(&done.server, &done.database) == key
                && counted.is_none_or(|at| at < done.ended) =>
        {
            0.0
        }
        _ => 1.0,
    }
}

/// The save points' part of the screen for the highlighted database, `width` wide and `rows` tall: the rail and the
/// changes side by side, or one over the other on a narrow screen.
pub fn rows(ui: &mut Ui, controller: &Controller, width: usize, rows: usize) -> Vec<Vec<Seg>> {
    let Some(entry) = highlighted(ui) else { return Vec::new() };
    note_new(ui, controller);
    let all: Vec<SavePoint> = controller.save_points_of(&entry.server, &entry.database).into_iter().cloned().collect();
    ui.save_view.picked = ui.save_view.picked.min(all.len().saturating_sub(1));
    let rail = rail_of(ui, controller, &entry, &all);
    let panel = all.get(ui.save_view.picked).map(|point| panel_of(ui, controller, &entry, point));
    let Some(panel) = panel else { return save_rail::rail(&rail, width, rows) };
    if width < SIDE_BY_SIDE {
        let mut lines = save_rail::rail(&rail, width, rows / 2);
        lines.push(Vec::new());
        let left = rows.saturating_sub(lines.len());
        lines.extend(save_rail::panel(&panel, width, left));
        return lines;
    }
    let left_width = width * RAIL_PERCENT / 100;
    let right_width = width.saturating_sub(left_width + 1);
    let mut left = save_rail::rail(&rail, left_width, rows);
    let mut right = save_rail::panel(&panel, right_width, rows);
    let tall = left.len().max(right.len());
    left.resize(tall, Vec::new());
    right.resize(tall, Vec::new());
    left.into_iter()
        .zip(right)
        .map(|(left, right)| {
            let mut row = super::columns::fit(left, left_width);
            let used = super::seg::width(&row);
            row.push(seg(" ".repeat(left_width.saturating_sub(used) + 1), C::DarkGray));
            row.extend(right);
            row
        })
        .collect()
}

/// A save point that wasn't there last frame drops in, its database's row glinting.
fn note_new(ui: &mut Ui, controller: &Controller) {
    let ids: Vec<String> = controller.save_points.iter().map(|point| point.id.clone()).collect();
    let fresh = ui
        .save_view
        .known
        .as_ref()
        .and_then(|known| controller.save_points.iter().find(|point| !known.contains(&point.id)))
        .map(|point| (point.id.clone(), db_key(&point.server, &point.database)));
    if let Some((id, key)) = fresh {
        ui.save_view.dropped = Some((id, ui.now));
        ui.save_view.glint = Some((key.clone(), ui.now));
        if ui.save_view.db.as_deref() == Some(key.as_str()) {
            ui.save_view.picked = 0;
        }
    }
    ui.save_view.known = Some(ids);
}

/// The note under the rail's title: a job on this database, the rewind being asked about, a name being typed.
fn note(ui: &Ui, controller: &Controller, entry: &Entry) -> Option<(String, C)> {
    let spin = if still(ui) { "·" } else { super::columns::spinner(ui) };
    if let Some(text) = &ui.save_view.renaming {
        return Some((format!("name: {text}{}  (Enter keeps it · Esc cancels)", ui.cursor()), C::Cyan));
    }
    if ui.save_view.asking.is_some() {
        return Some((format!("{spin} who else is connected…"), C::Yellow));
    }
    let work =
        controller.db_work.as_ref().filter(|work| work.server == entry.server && work.database == entry.database)?;
    let share = work.share.map(|(_, newest, _)| format!(" · {}%", (newest * 100.0).round()));
    let what = match &work.doing {
        Doing::Making(Kind::Snapshot) => "making a snapshot".to_string(),
        Doing::Making(Kind::Backup) => "backing up".to_string(),
        Doing::Rewinding(_) => "rewinding".to_string(),
        Doing::Dropping(_) => "dropping a snapshot".to_string(),
        Doing::Database(_) => return None,
    };
    Some((format!("{spin} {what}{}…", share.unwrap_or_default()), C::Yellow))
}

fn rail_of(ui: &Ui, controller: &Controller, entry: &Entry, all: &[SavePoint]) -> Rail {
    let still = still(ui);
    let mut rail = Rail::new(&entry.database);
    rail.nodes = all.iter().map(node).collect();
    rail.picked = ui.save_view.picked;
    rail.focused = ui.save_view.on_rail;
    rail.note = note(ui, controller, entry);
    if let Some(point) = all.get(ui.save_view.picked)
        && let Some(counts) = controller.counts(&entry.server, &entry.database).filter(|counts| counts.at.is_some())
    {
        let changed = points::summary(&points::changes(&point.tables, &counts.tables));
        rail.since = Some(changed);
    }
    let key = db_key(&entry.server, &entry.database);
    if let Some((id, share, done)) = rewinding(controller, &key, ui.now, still)
        && let Some(at) = all.iter().position(|point| point.id == id)
    {
        rail.bead = share * save_rail::node_row(at) as f64;
        rail.drained = true;
        rail.since = None;
        rail.bead_color = if done { C::Green } else { C::White };
        rail.here = if done {
            format!("you are here · back at \"{}\"", all[at].label)
        } else {
            let real = controller.db_work.as_ref().and_then(|work| work.share).map(|(_, newest, _)| newest);
            real.map_or_else(
                || "you are here · rewinding…".into(),
                |real| format!("you are here · rewinding {}%", (real * 100.0).round()),
            )
        };
    }
    if let Some((id, at)) = ui.save_view.dropped.as_ref().filter(|_| !still)
        && all.first().is_some_and(|newest| newest.id == *id)
    {
        let after = since(ui, *at);
        if after < DROP {
            rail.dropping = Some(save_rail::node_row(0) as f64 * ease(after.as_secs_f64() / DROP.as_secs_f64()));
        } else if after < DROP + RIPPLE {
            rail.ripple = Some((0, (after - DROP).as_secs_f64() / RIPPLE.as_secs_f64()));
        }
    }
    rail
}

fn node(point: &SavePoint) -> Node {
    let detail = if point.gone {
        "gone - dropped outside lazyrunner".to_string()
    } else {
        let pinned = if point.pinned { " · ⟲ pinned" } else { "" };
        format!("{}{pinned}", point.kind.name())
    };
    Node {
        label: point.label.clone(),
        clock: point.clock(),
        detail,
        backup: point.kind == Kind::Backup,
        gone: point.gone,
        pinned: point.pinned,
    }
}

/// What changed since `point`, as bars tweening to the newest counts.
fn panel_of(ui: &mut Ui, controller: &Controller, entry: &Entry, point: &SavePoint) -> Panel {
    let still = still(ui);
    let now = ui.now;
    let title = format!("SINCE \"{}\" · {}", point.label, point.clock());
    let counts = controller.counts(&entry.server, &entry.database);
    let reading = counts.is_some_and(|counts| counts.reading.is_some());
    let note = match counts {
        Some(counts) if counts.error.is_some() => {
            Some((format!("✗ couldn't count the rows: {}", counts.error.clone().unwrap_or_default()), C::Red))
        }
        _ if reading => Some(("counting the rows…".to_string(), C::DarkGray)),
        None => Some(("u counts the rows".to_string(), C::DarkGray)),
        _ => None,
    };
    let changes = counts
        .filter(|counts| counts.at.is_some())
        .map(|counts| points::changes(&point.tables, &counts.tables))
        .unwrap_or_default();
    let scale = bar_scale(controller, entry, now, still);
    let view = &mut ui.save_view;
    if view.tweens.is_empty() && !changes.is_empty() {
        view.opened = Some(now); // the first counts: their bars grow from the axis
    }
    let mut shown: Vec<(String, u64, f64, bool)> = Vec::new();
    for change in &changes {
        let drawn = tween(&mut view.tweens, &change.table, change.delta() as f64, now, still);
        shown.push((change.table.clone(), change.then, drawn * scale, change.changed));
    }
    let leaving: Vec<String> =
        view.tweens.keys().filter(|table| !changes.iter().any(|change| change.table == **table)).cloned().collect();
    for table in leaving {
        let then = point.tables.iter().find(|old| old.shown() == table).map_or(0, |old| old.rows);
        let drawn = tween(&mut view.tweens, &table, 0.0, now, still);
        if drawn.abs() >= 0.5 {
            shown.push((table, then, drawn * scale, false));
        } else {
            view.tweens.remove(&table);
        }
    }
    let opened = view.opened.unwrap_or(now);
    let bars = shown
        .into_iter()
        .enumerate()
        .map(|(row, (table, then, delta, changed))| {
            let from = opened + STAGGER * row as u32;
            let grown =
                if still { 1.0 } else { ease(now.saturating_duration_since(from).as_secs_f64() / GROW.as_secs_f64()) };
            Bar { table, then, delta, changed, grown }
        })
        .collect();
    let count = changes.len();
    let right = match count {
        0 if counts.is_some_and(|counts| counts.at.is_some()) => "nothing changed · u counts again".to_string(),
        0 => String::new(),
        1 => "1 table changed · r rewinds".to_string(),
        count => format!("{count} tables changed · r rewinds"),
    };
    Panel { title, right, note, bars }
}

/// A table's bar as drawn now: tweening from where it was to `target` once that changes.
fn tween(
    tweens: &mut HashMap<String, (f64, f64, Instant)>,
    table: &str,
    target: f64,
    now: Instant,
    still: bool,
) -> f64 {
    let at = |(from, to, since): (f64, f64, Instant)| {
        let share = now.saturating_duration_since(since).as_secs_f64() / TWEEN.as_secs_f64();
        if still { to } else { from + (to - from) * ease(share) }
    };
    match tweens.get(table).copied() {
        Some(current) if (current.1 - target).abs() < f64::EPSILON => at(current),
        Some(current) => {
            let drawn = at(current);
            tweens.insert(table.to_string(), (drawn, target, now));
            at((drawn, target, now))
        }
        None => {
            tweens.insert(table.to_string(), (target, target, now));
            target
        }
    }
}

/// Whether anything of the save points moves: a node dropping in or rippling, a row's glint, a rewind's glide, bars
/// growing or tweening, a job's spinner, the rewind being asked about, the pinned chip breathing.
pub fn animating(ui: &Ui, controller: &Controller) -> bool {
    if still(ui) {
        return ui.picker.is_some() && ui.save_view.asking.is_some();
    }
    let pinned_rewind = controller.db_work.as_ref().is_some_and(|work| work.pinned);
    if ui.picker.is_none() {
        return pinned_rewind;
    }
    let view = &ui.save_view;
    let moving = |at: Option<Instant>, lasts: Duration| at.is_some_and(|at| since(ui, at) < lasts);
    let tweening = view.tweens.values().any(|(from, to, at)| from != to && since(ui, *at) < TWEEN);
    let growing = moving(view.opened, GROW + STAGGER * view.tweens.len() as u32);
    let done = controller.db_done.as_ref().is_some_and(|done| since(ui, done.ended) < GLIDE + HOLD);
    pinned_rewind
        || controller.db_work.is_some()
        || view.asking.is_some()
        || view.renaming.is_some()
        || moving(view.dropped.as_ref().map(|(_, at)| *at), DROP + RIPPLE)
        || moving(view.glint.as_ref().map(|(_, at)| *at), GLINT)
        || tweening
        || growing
        || done
}

/// How far the database row's glint has come (0 to 1) after a save point of it was made.
pub fn glint(ui: &Ui, entry: &Entry) -> Option<f64> {
    let (key, at) = ui.save_view.glint.as_ref().filter(|_| !still(ui))?;
    let after = since(ui, *at);
    (*key == db_key(&entry.server, &entry.database) && after < GLINT).then(|| after.as_secs_f64() / GLINT.as_secs_f64())
}

/// The apps on a database while a rewind brings them back: `○ Api  ● Admin` - each lit once it answers again.
pub fn apps_back(ui: &Ui, controller: &Controller, entry: &Entry) -> Option<Seg> {
    let key = db_key(&entry.server, &entry.database);
    let apps = match (&controller.db_work, &controller.db_done) {
        (Some(work), _) if matches!(work.doing, Doing::Rewinding(_)) && db_key(&work.server, &work.database) == key => {
            work.stopped.clone()
        }
        (_, Some(done))
            if matches!(done.doing, Doing::Rewinding(_))
                && db_key(&done.server, &done.database) == key
                && since(ui, done.ended) < APPS_WATCH =>
        {
            done.apps.clone()
        }
        _ => return None,
    };
    let up = |app: &String| controller.state(app).is_some_and(|state| state.is_up());
    if apps.is_empty() || apps.iter().all(up) {
        return None;
    }
    let text: Vec<String> = apps
        .iter()
        .filter_map(|app| Some((controller.app(app)?.name.clone(), up(app))))
        .map(|(name, up)| format!("{} {name}", if up { "●" } else { "○" }))
        .collect();
    Some(seg(text.join("  "), C::Yellow))
}

/// The status bar's chip while a save point is pinned: `⟲ before approval`, breathing while its rewind holds a
/// restart.
pub fn chip(ui: &Ui, controller: &Controller) -> Option<Seg> {
    let pinned = controller.save_points.iter().find(|point| point.pinned && !point.gone)?;
    let holding = controller.db_work.as_ref().is_some_and(|work| work.pinned);
    let breath = ui.now.saturating_duration_since(ui.started).as_millis() % BREATH.as_millis();
    let dim = holding && !still(ui) && breath >= BREATH.as_millis() / 2;
    Some(seg(format!("   ⟲ {}", pinned.label), if dim { C::DarkCyan } else { C::Cyan }))
}

/// The screen's keys while the rail has them, or a name is typed.
pub fn hint(ui: &Ui) -> Option<&'static str> {
    if ui.save_view.renaming.is_some() {
        return Some(" type the save point's name · Enter keeps it · Esc cancels");
    }
    ui.save_view.on_rail.then_some(
        " j k pick a save point · r rewind to it · R pin · F2 rename · Del drop · u count again · s S save point · \
         Tab on · Esc back",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rewind_glides_with_the_servers_share_and_a_snapshot_eases_towards_most_of_the_way() {
        let start = Instant::now();
        let at = |ms| start + Duration::from_millis(ms);
        let reported = Some((0.2, 0.6, at(1000)));
        assert!((share_at(start, reported, at(1000), false) - 0.2).abs() < 1e-9, "the glide starts where it was");
        assert!((share_at(start, reported, at(2000), false) - 0.6).abs() < 1e-9, "and ends at the newest share");
        assert_eq!(share_at(start, reported, at(1000), true), 0.6, "reduced motion: it steps");
        let snapshot = share_at(start, None, at(600), false);
        assert!(snapshot > 0.5 && snapshot < SNAPSHOT_MOST, "{snapshot}");
        assert!(share_at(start, None, at(60_000), false) <= SNAPSHOT_MOST);
    }

    #[test]
    fn a_bar_tweens_to_new_counts_and_steps_with_reduced_motion() {
        let mut tweens = HashMap::new();
        let start = Instant::now();
        assert_eq!(tween(&mut tweens, "Orders", 10.0, start, false), 10.0, "a first count shows as it is");
        assert_eq!(tween(&mut tweens, "Orders", 0.0, start, false), 10.0, "a new count starts from where it was");
        let half = tween(&mut tweens, "Orders", 0.0, start + TWEEN / 2, false);
        assert!(half > 0.0 && half < 10.0, "{half}");
        assert_eq!(tween(&mut tweens, "Orders", 0.0, start + TWEEN, false), 0.0);
        assert_eq!(tween(&mut tweens, "Drafts", 4.0, start, true), 4.0);
        assert_eq!(tween(&mut tweens, "Drafts", -3.0, start, true), -3.0);
    }
}
