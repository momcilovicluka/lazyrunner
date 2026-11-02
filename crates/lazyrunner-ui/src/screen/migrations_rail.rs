//! The migrations view's rail: the selected context's migrations oldest at the top - `●` applied, `○` pending, dated
//! from their ids with a gap row where months pass, the older ones folded once they don't fit - and the database as a
//! bead on the rail, labelled where it is. Choosing a target lights the stretch between them: green with dashes flowing
//! down to apply, amber flowing up to revert (`⚠` on what reverting likely loses). While `dotnet ef` moves it, the
//! bead glides a node a hop as EF reports each migration done - in turn, faster once several wait - breathes while it
//! works, fills (or empties) each node it reaches with a glint, and pulses red where it fails; once done, a shimmer
//! runs along the stretch it travelled. With reduced motion the bead steps and nothing flows.

use std::time::{Duration, Instant};

use chrono::{Datelike, NaiveDate};
use lazyrunner_core::run::Controller;
use lazyrunner_core::run::migrations::{Context, Migration, Run};

use super::columns::fit;
use super::motion::ease;
use super::mouse::Target;
use super::seg::{Seg, aim, on, pad, seg};
use super::theme::C;
use super::{Overlay, Ui};

/// The bead's hop from one node to the next.
const HOP: Duration = Duration::from_millis(250);
/// More hops than this waiting: the bead speeds up to catch up.
const QUEUED: usize = 3;
/// How slowly the bead breathes while dotnet ef works.
const BREATH: Duration = Duration::from_millis(1600);
/// How long a node it reaches glints.
const GLINT: Duration = Duration::from_millis(200);
/// Where a run fails the bead pulses red: this many times, each this long.
const PULSES: u32 = 2;
const PULSE: Duration = Duration::from_millis(300);
/// The shimmer along the stretch a run travelled, once it's done.
const SHIMMER: Duration = Duration::from_millis(600);
/// A ghost path's dashes move a dot this often.
const FLOW: Duration = Duration::from_millis(90);
/// The progress bar in the contexts' list.
const BAR_CELLS: usize = 4;
/// A migration whose name starts with one of these likely drops what it added when it's reverted.
const DROPS_WHEN_REVERTED: [&str; 2] = ["Add", "Create"];
/// The rail's braille: its line (the left column of dots), a row's four dots on the left, and the bead's (both).
const BRAILLE: u32 = 0x2800;
const RAIL: u32 = 0x47;
const LEFT: [u32; 4] = [0x01, 0x02, 0x04, 0x40];
const BOTH: [u32; 4] = [0x09, 0x12, 0x24, 0xC0];

/// What the rail is drawn with: the time, when the screen opened (the dashes' phase), whether nothing moves, the
/// database's name, Nerd Font icons, the spinner's frame.
pub struct Look {
    pub now: Instant,
    pub since: Instant,
    pub still: bool,
    pub db: String,
    pub nerd: bool,
    pub spin: String,
}

/// Whether reverting it likely loses data: its Down() drops the table or column it added or created (a guess from
/// its name).
pub fn risky(migration: &Migration) -> bool {
    DROPS_WHEN_REVERTED.iter().any(|start| {
        migration.name.strip_prefix(start).is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_uppercase()))
    })
}

/// When each hop of the bead plays: in turn, from when EF said that migration was done or the hop before ended, a
/// `HOP` each - shorter while more than `QUEUED` wait, so it catches up.
fn hops(done: &[Instant]) -> Vec<(Instant, Instant)> {
    let mut hops: Vec<(Instant, Instant)> = Vec::with_capacity(done.len());
    for (at, when) in done.iter().enumerate() {
        let start = hops.last().map_or(*when, |(_, end)| (*end).max(*when));
        let waiting = done[at + 1..].iter().filter(|later| **later <= start).count();
        let took = if waiting > QUEUED { HOP.mul_f64(QUEUED as f64 / waiting as f64) } else { HOP };
        hops.push((start, start + took));
    }
    hops
}

/// How far the bead has come along a run's hops: the whole ones done, and the share of the one under way (with
/// reduced motion a hop is done as it starts).
fn travelled(hops: &[(Instant, Instant)], look: &Look) -> f64 {
    hops.iter()
        .map(|(start, end)| {
            if look.now >= *end || (look.still && look.now >= *start) {
                1.0
            } else if look.now <= *start {
                0.0
            } else {
                ease(look.now.duration_since(*start).as_secs_f64() / end.duration_since(*start).as_secs_f64())
            }
        })
        .sum()
}

/// When an ended run's motion is over: its last hop played, its shimmer or its pulses too; `None` while it runs.
fn settled(run: &Run) -> Option<Instant> {
    let (ended, _) = run.ended.as_ref()?;
    let last = hops(&run.done()).last().map_or(*ended, |(_, end)| *end);
    Some((*ended).max(last) + SHIMMER.max(PULSE * PULSES))
}

/// Whether the open view has something moving: a target's dashes, or a run's hops, glints, pulses or shimmer.
pub fn animating(ui: &Ui, controller: &Controller) -> bool {
    let Some(Overlay::Migrations { index, picking }) = &ui.overlay else { return false };
    let Some(context) = controller.contexts.as_deref().and_then(|all| all.get(*index)) else { return false };
    !ui.reduced_motion
        && (picking.is_some()
            || context.run.as_ref().is_some_and(|run| settled(run).is_none_or(|settled| ui.now < settled)))
}

/// A run under way: the migrations the bead has reached, and how many it moves.
pub fn progress(context: &Context, look: &Look) -> Option<(usize, usize)> {
    let run = context.run.as_ref().filter(|run| run.ended.is_none() && !run.planned.is_empty())?;
    Some((travelled(&hops(&run.done()), look).floor() as usize, run.planned.len()))
}

/// `▰▰▱▱ 2/4`.
pub fn bar(done: usize, total: usize) -> String {
    let filled = (done * BAR_CELLS / total.max(1)).min(BAR_CELLS);
    format!("{}{} {done}/{total}", "▰".repeat(filled), "▱".repeat(BAR_CELLS - filled))
}

fn date(id: &str) -> Option<NaiveDate> {
    let stamp = id.split('_').next().filter(|stamp| stamp.len() == 14)?;
    NaiveDate::parse_from_str(&stamp[..8], "%Y%m%d").ok()
}

fn month(date: NaiveDate) -> i32 {
    date.year() * 12 + date.month0() as i32
}

/// Each migration's date as the rail says it: `12 Apr`, with the year on the first one and where it changes.
fn dates(items: &[Migration]) -> Vec<String> {
    let mut year = None;
    items
        .iter()
        .map(|item| {
            let Some(date) = date(&item.id) else { return String::new() };
            let format = if year == Some(date.year()) { "%-d %b" } else { "%-d %b %Y" };
            year = Some(date.year());
            date.format(format).to_string()
        })
        .collect()
}

/// A row of the rail: a migration, the months that passed between two (two or more), or the older ones folded (how
/// many, whether all applied).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Row {
    Node(usize),
    Gap(i32),
    Fold(usize, bool),
}

fn layout(items: &[Migration]) -> Vec<Row> {
    let mut rows = Vec::with_capacity(items.len());
    for (at, item) in items.iter().enumerate() {
        let before = at.checked_sub(1).and_then(|before| date(&items[before].id)).map(month);
        if let (Some(before), Some(this)) = (before, date(&item.id).map(month))
            && this - before >= 2
        {
            rows.push(Row::Gap(this - before));
        }
        rows.push(Row::Node(at));
    }
    rows
}

/// The rows that fit in `room` around row `focus` - the newest when it's near them - the older ones above folded
/// into one row.
fn window(rows: &[Row], focus: usize, room: usize, items: &[Migration]) -> Vec<Row> {
    let room = room.max(2);
    if rows.len() <= room {
        return rows.to_vec();
    }
    let start = focus.saturating_sub(room / 2).min(rows.len() - room);
    let mut shown = rows[start..start + room].to_vec();
    if start > 0 {
        let older: Vec<usize> = rows[..=start]
            .iter()
            .filter_map(|row| match row {
                Row::Node(at) => Some(*at),
                _ => None,
            })
            .collect();
        shown[0] = Row::Fold(older.len(), older.iter().all(|at| items[*at].applied == Some(true)));
    }
    shown
}

/// The shown row a place on the rail is at (a node, fractional between two; -1: above the oldest), between rows when
/// between nodes.
fn row_of(shown: &[Row], place: f64) -> Option<f64> {
    let nodes: Vec<(usize, f64)> = shown
        .iter()
        .enumerate()
        .filter_map(|(row, line)| match line {
            Row::Node(at) => Some((*at, row as f64)),
            _ => None,
        })
        .collect();
    let (first, last) = (nodes.first()?, nodes.last()?);
    if place <= first.0 as f64 {
        return Some(first.1 - (first.0 as f64 - place).min(0.5));
    }
    if place >= last.0 as f64 {
        return Some(last.1);
    }
    let row = |node: usize| nodes.iter().find(|(at, _)| *at == node).map(|(_, row)| *row);
    let (low, high) = (place.floor() as usize, place.ceil() as usize);
    let (from, to) = (row(low)?, row(high)?);
    Some(from + (to - from) * (place - low as f64))
}

/// A row on the rail as a dot: its cell, and which of the cell's four dots.
fn dot(row: f64) -> (usize, usize) {
    let sub = ((row + 0.5) * 4.0).floor().max(0.0) as usize;
    (sub / 4, sub % 4)
}

/// The stretch a target lights: from the database to it, forward or back, and what it says.
struct Ghost {
    from: f64,
    to: usize,
    forward: bool,
    label: String,
}

impl Ghost {
    fn holds(&self, at: usize) -> bool {
        if self.forward { at as f64 > self.from && at <= self.to } else { at > self.to && at as f64 <= self.from }
    }

    fn color(&self) -> C {
        if self.forward { C::Green } else { C::Yellow }
    }
}

fn ghost(context: &Context, to: usize) -> Option<Ghost> {
    let current = context.current();
    if Some(to) == current || to >= context.items.len() {
        return None;
    }
    let forward = current.is_none_or(|current| to > current);
    let label = if forward {
        let count = context.items[..=to].iter().filter(|item| item.applied != Some(true)).count();
        format!("◂ applies {count}")
    } else {
        format!("◂ reverts {} - may lose data", current.unwrap_or(to) - to)
    };
    Some(Ghost { from: current.map_or(-1.0, |at| at as f64), to, forward, label })
}

/// What the rail shows besides the migrations as checked.
struct Scene {
    /// Where the bead is (a place on the rail), its colour, and what it says.
    bead: f64,
    bead_color: C,
    label: String,
    label_color: C,
    /// Nodes a run shows applied or not other than as checked, and those glinting.
    filled: Vec<(usize, bool)>,
    glint: Vec<usize>,
    ghost: Option<Ghost>,
    /// The node a run failed on, and why.
    failed: Option<(usize, String)>,
    /// The finish shimmer: the stretch it runs along, and its head.
    shimmer: Option<(f64, f64, f64)>,
}

fn scene(context: &Context, picking: Option<usize>, look: &Look) -> Scene {
    let current = context.current().map_or(-1.0, |at| at as f64);
    let label =
        if look.db.is_empty() { "◂ database is here".into() } else { format!("◂ database is here · {}", look.db) };
    let mut scene = Scene {
        bead: current,
        bead_color: C::Cyan,
        label,
        label_color: C::Cyan,
        filled: Vec::new(),
        glint: Vec::new(),
        ghost: None,
        failed: None,
        shimmer: None,
    };
    if let Some(run) = &context.run {
        moving(&mut scene, context, run, look);
    }
    if context.run.as_ref().is_none_or(|run| run.ended.is_some()) {
        scene.ghost = picking.and_then(|to| ghost(context, to));
    }
    scene
}

/// A run on the rail: the bead along its hops, the nodes it reached, how it ended.
fn moving(scene: &mut Scene, context: &Context, run: &Run, look: &Look) {
    let index = |id: &str| context.items.iter().position(|item| item.id == id);
    let from = run.from.as_deref().and_then(index).map_or(-1.0, |at| at as f64);
    let done = run.done();
    if let Some(failure) = run.failure() {
        let failed = run.planned.get(done.len()).and_then(|id| index(id));
        let why = failure.lines().next().unwrap_or_default().to_string();
        scene.failed = failed.map(|at| (at, why));
        let since = run.ended.as_ref().map_or(Duration::ZERO, |(at, _)| look.now.saturating_duration_since(*at));
        let half = PULSE / 2;
        let dim = !look.still && since < PULSE * PULSES && (since.as_millis() / half.as_millis()) % 2 == 1;
        scene.bead_color = if dim { C::DarkRed } else { C::Red };
    }
    if settled(run).is_some_and(|settled| look.now >= settled) {
        return;
    }
    let hops = hops(&done);
    let travelled = travelled(&hops, look);
    let stops: Vec<f64> = std::iter::once(from)
        .chain(run.planned.iter().map(|id| index(id).map_or(from, |at| at as f64 - f64::from(u8::from(run.back)))))
        .collect();
    let whole = travelled.floor() as usize;
    scene.bead = match (stops.get(whole), stops.get(whole + 1)) {
        (Some(low), Some(high)) => low + (high - low) * (travelled - whole as f64),
        (Some(low), None) => *low,
        _ => from,
    };
    for (hop, id) in run.planned.iter().enumerate() {
        let Some(at) = index(id) else { continue };
        let reached = hops.get(hop).map(|(_, end)| *end).filter(|end| look.now >= *end);
        scene.filled.push((at, reached.is_some() != run.back));
        if reached.is_some_and(|end| !look.still && look.now < end + GLINT) {
            scene.glint.push(at);
        }
    }
    if run.ended.is_none() {
        let gliding = hops.iter().any(|(start, end)| look.now >= *start && look.now < *end);
        let breath = look.now.saturating_duration_since(look.since).as_millis() % BREATH.as_millis();
        scene.bead_color = if look.still {
            C::Cyan
        } else if gliding {
            C::White
        } else if breath < BREATH.as_millis() / 2 {
            C::Cyan
        } else {
            C::DarkCyan
        };
        let doing = if run.back { "reverting" } else { "applying" };
        let under = run.began.len().checked_sub(1).and_then(|at| run.planned.get(at)).and_then(|id| index(id));
        scene.label = match under {
            Some(at) => format!("◂ {doing} {}…", context.items[at].name),
            None => "◂ dotnet ef is starting…".into(),
        };
        scene.label_color = C::Yellow;
    } else if run.failure().is_none() && !look.still {
        let arrived = hops.last().map(|(_, end)| *end).or(run.ended.as_ref().map(|(at, _)| *at));
        let to = stops.last().copied().unwrap_or(from);
        if let Some(arrived) = arrived.filter(|arrived| look.now >= *arrived && from != to) {
            let share = look.now.duration_since(arrived).as_secs_f64() / SHIMMER.as_secs_f64();
            scene.shimmer = (share < 1.0).then(|| (from, to, from + (to - from) * ease(share)));
        }
    }
}

/// The rail for `context`, `room` rows tall and `width` wide.
pub fn rail(context: &Context, picking: Option<usize>, look: &Look, width: usize, room: usize) -> Vec<Vec<Seg>> {
    let items = &context.items;
    let scene = scene(context, picking, look);
    let all = layout(items);
    let focus_node = picking.or(context.current()).unwrap_or(items.len().saturating_sub(1));
    let focus = all.iter().position(|row| *row == Row::Node(focus_node)).unwrap_or(all.len().saturating_sub(1));
    let shown = window(&all, focus, room, items);
    let track = track(&shown, &scene, look);
    let bead = row_of(&shown, scene.bead).map(|row| dot(row).0);
    let dates = dates(items);
    let date_cells = dates.iter().map(|date| date.chars().count()).max().unwrap_or(0);
    let near = |at: usize, head: f64| (at as f64 - head).abs() < 1.0;
    shown
        .iter()
        .enumerate()
        .map(|(row, line)| {
            let (dots, color) = track[row];
            let mut segs = vec![seg(format!("{} ", char::from_u32(BRAILLE + dots).unwrap_or(' ')), color)];
            match *line {
                Row::Fold(older, applied) => {
                    let all = if applied { ", all applied" } else { "" };
                    segs.push(seg(format!("  … {older} older{all}"), C::DarkGray));
                }
                Row::Gap(months) => segs.push(seg(format!("  · {months} months ·"), C::DarkGray)),
                Row::Node(at) => {
                    let item = &items[at];
                    let applied = scene
                        .filled
                        .iter()
                        .rev()
                        .find(|(node, _)| *node == at)
                        .map_or(item.applied, |(_, filled)| Some(*filled));
                    let in_ghost = scene.ghost.as_ref().filter(|ghost| ghost.holds(at));
                    let (glyph, mut node_color) = match applied {
                        Some(true) => ("●", C::Green),
                        Some(false) => ("○", C::Yellow),
                        None => ("?", C::DarkGray),
                    };
                    if let Some(ghost) = in_ghost {
                        node_color = ghost.color();
                    }
                    let shimmering = scene.shimmer.is_some_and(|(from, to, head)| {
                        near(at, head) && (at as f64) >= from.min(to) && (at as f64) <= from.max(to)
                    });
                    if scene.glint.contains(&at) || shimmering {
                        node_color = C::White;
                    }
                    segs.push(seg(glyph, node_color));
                    let warn = if in_ghost.is_some_and(|ghost| !ghost.forward) && risky(item) { "⚠ " } else { "" };
                    let text = format!(" {}  {warn}{}", pad(&dates[at], date_cells), item.name);
                    segs.push(if picking == Some(at) {
                        on(text, C::FocusText, C::FocusBack)
                    } else {
                        seg(text, if applied == Some(false) { C::Yellow } else { C::Gray })
                    });
                    if let Some(ghost) = scene.ghost.as_ref().filter(|ghost| ghost.to == at) {
                        segs.push(seg(format!("  {}", ghost.label), ghost.color()));
                    }
                    if let Some((_, why)) = scene.failed.as_ref().filter(|(node, _)| *node == at) {
                        segs.push(seg(format!("  ✗ {why}"), C::Red));
                    }
                }
            }
            if bead == Some(row) {
                segs.push(seg(format!("  {}", scene.label), scene.label_color));
            }
            let segs = fit(segs, width);
            match line {
                Row::Node(at) => aim(segs, Target::Migration(*at)),
                _ => segs,
            }
        })
        .collect()
}

/// The rail's cells: its line, a target's flowing path, the finish shimmer, the bead.
fn track(shown: &[Row], scene: &Scene, look: &Look) -> Vec<(u32, C)> {
    let mut track = vec![(RAIL, C::DarkGray); shown.len()];
    if let Some(ghost) = &scene.ghost
        && let (Some(from), Some(to)) = (row_of(shown, ghost.from), row_of(shown, ghost.to as f64))
    {
        let (low, high) = {
            let (from, to) = ((from + 0.5) * 4.0, (to + 0.5) * 4.0);
            (from.min(to).max(0.0) as usize, from.max(to).max(0.0) as usize)
        };
        let phase = (look.now.saturating_duration_since(look.since).as_millis() / FLOW.as_millis()) as i64;
        let direction = if ghost.forward { 1 } else { -1 };
        for (cell, (dots, color)) in track.iter_mut().enumerate().take(high / 4 + 1).skip(low / 4) {
            *dots = (0..4).fold(0, |dots, k| {
                let sub = cell * 4 + k;
                let lit = sub < low || sub > high || look.still || (sub as i64 - direction * phase).rem_euclid(4) < 2;
                if lit { dots | LEFT[k] } else { dots }
            });
            *color = ghost.color();
        }
    }
    if let Some((from, to, head)) = scene.shimmer
        && let (Some(from), Some(to), Some(head)) = (row_of(shown, from), row_of(shown, to), row_of(shown, head))
    {
        for (cell, (_, color)) in track.iter_mut().enumerate() {
            let cell = cell as f64;
            if (cell - head).abs() <= 1.0 && cell >= from.min(to).floor() && cell <= from.max(to).ceil() {
                *color = C::White;
            }
        }
    }
    if let Some((cell, k)) = row_of(shown, scene.bead).map(dot)
        && let Some(slot) = track.get_mut(cell)
    {
        *slot = (RAIL | BOTH[k], scene.bead_color);
    }
    track
}

#[cfg(test)]
mod tests {
    use super::*;

    fn migration(id: &str) -> Migration {
        Migration { id: id.into(), name: id[15..].into(), applied: Some(true) }
    }

    #[test]
    fn hops_play_in_turn_and_catch_up_once_several_wait() {
        let start = Instant::now();
        let at = |ms| start + Duration::from_millis(ms);
        assert_eq!(hops(&[at(0), at(1000)]), [(at(0), at(250)), (at(1000), at(1250))], "a HOP each, when it's done");
        assert_eq!(hops(&[at(0), at(5), at(10)])[1], (at(250), at(500)), "the next waits for the one before");
        let flood = hops(&[at(0); 9]);
        assert!(flood[0].1 - flood[0].0 < HOP, "eight waiting: it speeds up");
        assert_eq!(flood.last().map(|(start, end)| *end - *start), Some(HOP), "the last at the usual pace");
    }

    #[test]
    fn dates_come_from_the_ids_with_a_gap_where_months_pass() {
        let items = [
            migration("20251203000000_Init"),
            migration("20260112000000_AddOrders"),
            migration("20260412093011_Refunds"),
        ];
        assert_eq!(dates(&items), ["3 Dec 2025", "12 Jan 2026", "12 Apr"]);
        assert_eq!(layout(&items), [Row::Node(0), Row::Node(1), Row::Gap(3), Row::Node(2)]);
        assert!(risky(&items[1]) && !risky(&items[0]) && !risky(&migration("20260101000000_Address")));
        assert_eq!(bar(1, 4), "▰▱▱▱ 1/4");
    }

    #[test]
    fn a_long_history_folds_its_older_migrations() {
        let items: Vec<Migration> = (0..30).map(|at| migration(&format!("2026010100{at:04}_M{at}"))).collect();
        let rows: Vec<Row> = (0..30).map(Row::Node).collect();
        let newest = window(&rows, 29, 10, &items);
        assert_eq!((newest.len(), newest[0], newest[9]), (10, Row::Fold(21, true), Row::Node(29)));
        assert_eq!(window(&rows, 3, 10, &items)[0], Row::Node(0), "near the oldest: nothing folded");
    }
}
