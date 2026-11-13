//! What's new since an app's previous run (`Alt+d`): in the focused column the lines its last run printed too fade to
//! dim, the new ones stay as they are with a mark in the gutter - `▲` a new error or warning, `●` any other new line -
//! and its top row counts them, with the errors and warnings of the last run that didn't come back (`fixed?` until the
//! app is up: they may only not have come yet). `n` / `N` step through the new errors and warnings, or the new lines
//! when there are none; `Enter` lists what was fixed - the last run's errors and warnings that didn't come back. The
//! rail marks only what's new, and sends its ember only for a new error. With reduced motion the lines dim at once and
//! the count doesn't flash. A run with new errors that diff mode wasn't on for puts a chip in the panel's header
//! (`1 Orders +2 new errors`): the column's number or a click turns diff mode on there, on the first of them.

use std::cell::Cell;
use std::collections::HashSet;
use std::time::{Duration, Instant};

use lazyrunner_core::output::{Hue, Level};
use lazyrunner_core::run::Controller;
use lazyrunner_core::run::app::AppState;
use lazyrunner_core::run::diff;

use super::Ui;
use super::columns::{self, fit};
use super::icons::{self, Icon};
use super::motion::{FLASH, Spot};
use super::seg::{Seg, seg};
use super::theme::C;

/// How long the lines take to dim, or to come back.
const FADE: Duration = Duration::from_millis(200);
/// What was fixed lists this many, then how many more.
pub const FIXED_SHOWN: usize = 5;

/// A column's diff mode, by its app's key in `Ui::diffs`.
pub struct Diff {
    pub on: bool,
    since: Instant,
    /// The line `n` / `N` went to last (its number).
    at: Option<u64>,
    /// The new errors counted at the last frame, and when that number last rose.
    counted: Cell<Option<usize>>,
    bumped: Cell<Option<Instant>>,
    /// The run it was turned on or off in (its first line's number): that run's chip is done with.
    run: u64,
    /// Whether what was fixed is listed (`Enter`).
    fixed: bool,
}

/// What a line of the column is to the run before.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// The last run printed it too, or it's from an earlier run.
    Seen,
    /// The last run didn't print it.
    New,
    /// A new error or warning: its first line (`head`), or one that belongs to it.
    Problem { level: Level, head: bool },
}

/// A column whose diff mode is on (or still fading off), as this frame draws it.
pub struct View {
    pub on: bool,
    /// How far the lines are dimmed (on) or back (off): 0 to 1.
    progress: f64,
    /// Whether there's a run to compare with.
    compared: bool,
    /// The first line of this run the column still has (its number), and what each line from there is.
    from: u64,
    kinds: Vec<Kind>,
    pub errors: usize,
    pub warnings: usize,
    pub lines: usize,
    pub fixed: usize,
    /// Whether the run is far enough along for `fixed` to count: up, or over.
    settled: bool,
    pub at: Option<u64>,
    /// While `Enter` lists them: the last run's errors and warnings that didn't come back - their first lines, and
    /// whether each was an error.
    pub gone: Vec<(String, bool)>,
}

impl View {
    /// What line `number` is to the run before; none with no run to compare with.
    pub fn kind(&self, number: u64) -> Option<Kind> {
        if !self.compared {
            return None;
        }
        let at = number.checked_sub(self.from).map(|at| at as usize);
        Some(at.map_or(Kind::Seen, |at| self.kinds.get(at).copied().unwrap_or(Kind::New)))
    }

    /// A line's colour as dimmed: a line seen before goes to its darker shade, then grey (an error or a warning
    /// stays its darker shade); new ones keep theirs.
    pub fn shade(&self, kind: Kind, color: C) -> C {
        if kind != Kind::Seen {
            return color;
        }
        let dimmed = if self.on { self.progress } else { 1.0 - self.progress };
        let darker = darker(color);
        let dimmest = if matches!(darker, C::DarkRed | C::DarkYellow) { darker } else { C::DarkGray };
        if dimmed < 0.5 {
            color
        } else if dimmed < 1.0 {
            darker
        } else {
            dimmest
        }
    }

    /// The gutter cell of a line's `first` row (or a row it wraps to).
    pub fn gutter(&self, kind: Option<Kind>, first: bool) -> Seg {
        match kind {
            Some(Kind::Problem { level, head }) => {
                let (bright, dark) =
                    if level == Level::Error { (C::Red, C::DarkRed) } else { (C::Yellow, C::DarkYellow) };
                if head && first { seg("▲", bright) } else { seg("│", dark) }
            }
            Some(Kind::New) if first => seg("●", C::Cyan),
            _ => seg(" ", C::Gray),
        }
    }

    /// The rows the list of what was fixed takes under the top row.
    pub fn fixed_rows(&self) -> usize {
        self.gone.len().min(FIXED_SHOWN) + usize::from(self.gone.len() > FIXED_SHOWN)
    }

    /// Whether the column gives the marks a gutter: while it's on and has a run to compare with.
    pub fn has_gutter(&self) -> bool {
        self.on && self.compared
    }

    /// The new lines `n` / `N` step through, oldest first (their numbers): the new errors and warnings, or the new
    /// lines when there are none.
    fn targets(&self) -> Vec<u64> {
        let heads: Vec<u64> = self.numbered(|kind| matches!(kind, Kind::Problem { head: true, .. }));
        if heads.is_empty() { self.numbered(|kind| kind == Kind::New) } else { heads }
    }

    fn numbered(&self, wanted: impl Fn(Kind) -> bool) -> Vec<u64> {
        (self.from..).zip(&self.kinds).filter(|(_, kind)| wanted(**kind)).map(|(number, _)| number).collect()
    }
}

/// Alt+d: diff mode on the focused column, or off.
pub fn toggle(ui: &mut Ui, controller: &Controller) {
    let on = !focused_on(ui, controller);
    set(ui, controller, ui.focus, on);
}

/// Diff mode on app `index`'s column (a resumed session's).
pub fn turn_on(ui: &mut Ui, controller: &Controller, index: usize) {
    set(ui, controller, index, true);
}

/// Diff mode on app `index`'s column, or off.
fn set(ui: &mut Ui, controller: &Controller, index: usize, on: bool) {
    let Some(state) = controller.states.get(index) else { return };
    let key = state.key.clone();
    let (counted, bumped) = (Cell::new(None), Cell::new(None));
    let diff = Diff { on, since: ui.now, at: None, counted, bumped, run: state.run_from, fixed: false };
    ui.diffs.insert(key.clone(), diff);
    ui.rail_motion.forget(&key); // the error count it goes by changes: no ember for that
}

/// Whether the focused column has diff mode on.
pub fn focused_on(ui: &Ui, controller: &Controller) -> bool {
    controller.apps.get(ui.focus).is_some_and(|app| is_on(ui, &app.key))
}

/// `Enter` with diff mode on: lists what was fixed, or no longer.
pub fn toggle_fixed(ui: &mut Ui, controller: &Controller) {
    let Some(app) = controller.apps.get(ui.focus) else { return };
    if let Some(diff) = ui.diffs.get_mut(&app.key) {
        diff.fixed = !diff.fixed;
    }
}

/// Whether app `key`'s column has diff mode on.
pub fn is_on(ui: &Ui, key: &str) -> bool {
    ui.diffs.get(key).is_some_and(|diff| diff.on)
}

/// The column of `state` as diff mode draws it this frame; none when it's off.
pub fn view(ui: &Ui, state: &AppState) -> Option<View> {
    let diff = ui.diffs.get(&state.key)?;
    let moving = ui.motion.enabled && !ui.reduced_motion;
    let progress = if moving { through(ui.now.saturating_duration_since(diff.since)) } else { 1.0 };
    if !diff.on && progress >= 1.0 {
        return None;
    }
    let settled = !state.is_running() || state.is_up();
    let mut view = View {
        on: diff.on,
        progress,
        compared: false,
        from: state.run_from,
        kinds: Vec::new(),
        errors: 0,
        warnings: 0,
        lines: 0,
        fixed: 0,
        settled,
        at: diff.at,
        gone: Vec::new(),
    };
    let Some(previous) = state.previous.as_ref().filter(|_| state.run_from > 0) else { return Some(view) };
    let lines = diff::numbered(&state.output, state.run_from);
    view.compared = true;
    view.from = lines.first().map_or(state.run_from, |(number, _)| *number);
    view.kinds = lines
        .iter()
        .map(|(_, line)| if previous.shapes.contains(&line.shape) { Kind::Seen } else { Kind::New })
        .collect();
    let known: HashSet<u64> = previous.problems.iter().map(|problem| problem.shape).collect();
    let mut now_shapes = HashSet::new();
    for block in diff::blocks(&lines) {
        now_shapes.insert(block.shape);
        let new = !known.contains(&block.shape);
        for number in block.first..=block.last {
            let kind = if new { Kind::Problem { level: block.level, head: number == block.first } } else { Kind::Seen };
            if let Some(slot) = view.kinds.get_mut((number - view.from) as usize) {
                *slot = kind;
            }
        }
        match (new, block.level) {
            (false, _) => {}
            (true, Level::Error) => view.errors += 1,
            (true, _) => view.warnings += 1,
        }
    }
    view.lines = view.kinds.iter().filter(|kind| **kind == Kind::New).count();
    let gone = previous.problems.iter().filter(|problem| !now_shapes.contains(&problem.shape));
    view.fixed = gone.clone().count();
    if diff.on && diff.fixed {
        view.gone = gone.map(|problem| (problem.text.clone(), problem.error)).collect();
    }
    if moving {
        let before = diff.counted.replace(Some(view.errors));
        if before.is_some_and(|before| view.errors > before) {
            diff.bumped.set(Some(ui.now));
        }
    }
    Some(view)
}

/// The column's top row: what's new, what's fixed, and the keys.
pub fn summary(ui: &Ui, state: &AppState, view: &View, cells: usize) -> Vec<Seg> {
    let mut segs = vec![seg(format!(" {} ", icons::icon(ui.nerd, Icon::Diff)), C::Cyan)];
    if !view.compared {
        segs.push(seg("nothing to compare with yet - the next run compares with this one", C::DarkGray));
        return fit(segs, cells);
    }
    let bumped = ui.diffs.get(&state.key).and_then(|diff| diff.bumped.get());
    let flashing = bumped.is_some_and(|at| ui.now.saturating_duration_since(at) < FLASH);
    let mut parts: Vec<Seg> = Vec::new();
    if view.errors > 0 {
        let color = if flashing { C::White } else { C::Red };
        parts.push(seg(format!("+{} new {}", view.errors, plural(view.errors, "error")), color));
    }
    if view.warnings > 0 {
        parts.push(seg(format!("+{} new {}", view.warnings, plural(view.warnings, "warning")), C::Yellow));
    }
    if view.fixed > 0 {
        let (mark, color) = if view.settled { ("", C::Green) } else { ("?", C::DarkGreen) };
        parts.push(seg(format!("{} fixed{mark}", view.fixed), color));
    }
    if view.lines > 0 {
        parts.push(seg(format!("{} new {}", view.lines, plural(view.lines, "line")), C::Gray));
    }
    if parts.is_empty() {
        parts.push(seg("nothing new since the last run", C::Green));
    }
    for (at, part) in parts.into_iter().enumerate() {
        if at > 0 {
            segs.push(seg(" · ", C::DarkGray));
        }
        segs.push(part);
    }
    let mut keys = Vec::new();
    if view.errors + view.warnings + view.lines > 0 {
        keys.push("n N step");
    }
    if view.fixed > 0 {
        keys.push(if view.gone.is_empty() { "Enter lists the fixed" } else { "Enter hides them" });
    }
    keys.push("Alt+d shows all");
    segs.push(seg(format!(" - {}", keys.join(", ")), C::DarkGray));
    fit(segs, cells)
}

/// What app `index`'s chip in the panel says: its run's new errors, while diff mode wasn't on or off in this run.
pub fn chip(ui: &Ui, controller: &Controller, index: usize) -> Option<String> {
    let state = controller.states.get(index)?;
    let previous = state.previous.as_ref().filter(|_| state.run_from > 0)?;
    if ui.diffs.get(&state.key).is_some_and(|diff| diff.on || diff.run == state.run_from) {
        return None;
    }
    let (errors, _) = diff::new_problems(previous, &state.output, state.run_from);
    let name = &controller.apps.get(index)?.name;
    (errors > 0).then(|| format!("{name} +{errors} new {}", plural(errors, "error")))
}

/// Each app's chip, with its index.
pub fn chips(ui: &Ui, controller: &Controller) -> Vec<(usize, String)> {
    (0..controller.states.len()).filter_map(|index| Some((index, chip(ui, controller, index)?))).collect()
}

/// A chip's key or click: diff mode on its app's column, on the first new error or warning.
pub fn open_news(ui: &mut Ui, controller: &mut Controller, index: usize) {
    ui.focus = index;
    set(ui, controller, index, true);
    if let Some(diff) = controller.states.get(index).and_then(|state| ui.diffs.get_mut(&state.key)) {
        diff.at = Some(0);
    }
    step(ui, controller, false);
}

/// Whether `n` / `N` step through what's new in the focused column (diff mode on, no search to step through).
pub fn steps(ui: &Ui, controller: &Controller) -> bool {
    let searching = ui.search.as_ref().is_some_and(|search| !search.query.is_empty());
    !searching && focused_on(ui, controller)
}

/// `n` (older) / `N` (newer): to the next new error or warning - or new line - in the focused column, wrapping
/// around, brought to its middle.
pub fn step(ui: &mut Ui, controller: &mut Controller, older: bool) {
    let index = ui.focus;
    let Some(state) = controller.states.get(index) else { return };
    let Some(view) = view(ui, state) else { return };
    let targets = view.targets();
    let (Some(&first), Some(&last)) = (targets.first(), targets.last()) else {
        controller.add_log("Nothing new since the last run to step through.", Hue::DarkGray);
        return;
    };
    let shown = columns::shown_lines(state, None);
    let bottom = shown.len().saturating_sub(1 + state.scroll);
    let from = view.at.unwrap_or_else(|| {
        let number = shown.get(bottom).map_or(0, |item| state.output.number_at(item.position));
        if older { number + 1 } else { number.saturating_sub(1) }
    });
    let next = if older {
        targets.iter().rev().find(|&&number| number < from).copied().unwrap_or(last)
    } else {
        targets.iter().find(|&&number| number > from).copied().unwrap_or(first)
    };
    let Some(position) = state.output.position_of(next) else { return };
    let Some(found) = shown.iter().position(|item| item.position == position) else { return };
    let scroll = columns::middle(&shown, found, ui.focus_cells, ui.body_rows);
    let key = state.key.clone();
    if let Some(diff) = ui.diffs.get_mut(&key) {
        diff.at = Some(next);
    }
    controller.states[index].scroll = scroll;
    ui.motion.flash(Spot::Match { app: index, line: position }, ui.now);
}

/// Whether a column's lines are still dimming or coming back, or its count flashing.
pub fn animating(ui: &Ui) -> bool {
    if !ui.motion.enabled || ui.reduced_motion {
        return false;
    }
    ui.diffs.values().any(|diff| {
        ui.now.saturating_duration_since(diff.since) < FADE
            || diff.bumped.get().is_some_and(|at| ui.now.saturating_duration_since(at) < FLASH)
    })
}

fn through(elapsed: Duration) -> f64 {
    (elapsed.as_secs_f64() / FADE.as_secs_f64()).clamp(0.0, 1.0)
}

/// The darker shade of a colour, a step towards dim.
fn darker(color: C) -> C {
    match color {
        C::White => C::Gray,
        C::Red => C::DarkRed,
        C::Yellow => C::DarkYellow,
        C::Green => C::DarkGreen,
        C::Cyan => C::DarkCyan,
        C::Magenta => C::DarkMagenta,
        C::Blue => C::DarkBlue,
        C::DarkRed | C::DarkYellow => color,
        _ => C::DarkGray,
    }
}

fn plural(count: usize, word: &str) -> String {
    if count == 1 { word.to_string() } else { format!("{word}s") }
}
