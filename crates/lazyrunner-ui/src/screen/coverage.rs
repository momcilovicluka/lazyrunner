//! Code coverage in the tests view (#33), with the `coverage` setting on. A suite's row says its last full run's
//! coverage and its change since the run before - `78.4% (+1.2)` - beside a bar of eighth blocks with the change drawn
//! at its end (a gain green, a loss red); a `coverage` capsule while a run collects it; and for a .NET suite without
//! coverlet, that it has none. Its details list the files whose coverage changed most (the drops first, in red), each
//! with a braille line of its last full runs, a link to its first line not covered; `O` opens them. A suite that lost
//! more than a point gets a chip in the panel, popping in as the others do. When a run finishes the number and the bar
//! ease from the old value to the new, then a glint sweeps once across the change and the bar's changed stretch, green
//! for a gain, red for a loss (not with reduced motion: they change at once, with no glint).

use std::ops::Range;
use std::time::Duration;

use lazyrunner_core::config::Toggle;
use lazyrunner_core::links;
use lazyrunner_core::run::Controller;
use lazyrunner_core::run::coverage::{self, FileChange, SuiteCoverage};
use lazyrunner_core::run::suites::{Status, Suite};
use serde_json::{Value, json};

use super::editor::Place;
use super::icons::{self, Icon};
use super::mouse::Target;
use super::seg::{Seg, eighth, on, seg};
use super::theme::C;
use super::{Overlay, Ui};

/// The bar's cells (fewer on a suite's row when it's narrow, never fewer than the least), and its blocks by how many
/// eighths of a cell they fill.
const BAR_CELLS: usize = 8;
const LEAST_BAR_CELLS: usize = 4;
/// How long the number and the bar take to ease to a run's new value; then the glint: when it starts sweeping across
/// the change, how long it takes, and how wide it is.
const EASE: Duration = Duration::from_millis(400);
const GLINT_AFTER: Duration = Duration::from_millis(250);
const GLINT: Duration = Duration::from_millis(500);
const GLINT_CELLS: usize = 3;
/// A loss of more than this many points is news: the panel's chip, which pops in as the other chips do.
const DROP: f64 = 1.0;
pub const CHIP_KEY: &str = "coverage";
/// The files the details list, at most.
const FILES_SHOWN: usize = 8;
/// What a .NET suite without coverlet says, on its row and in its details.
const NO_COLLECTOR: &str = "no coverlet";
const NO_COLLECTOR_LONG: &str = "no coverage - coverlet not referenced";
/// The capsule on a suite's row while its run collects coverage.
const CAPSULE: &str = "coverage";
/// Braille's dots, bottom up: a cell's left column, and its right; a trend is one row of them.
const LEFT: [u32; 4] = [0x40, 0x04, 0x02, 0x01];
const RIGHT: [u32; 4] = [0x80, 0x20, 0x10, 0x08];
const BRAILLE: u32 = 0x2800;
const TREND_LEVELS: usize = 4;
/// The minus sign changes are written with, and less than what rounds to a tenth.
const MINUS: char = '−';
const HALF_TENTH: f64 = 0.05;

/// Whether coverage is collected (the setting).
fn on_for(controller: &Controller) -> bool {
    controller.setting(Toggle::Coverage)
}

/// A .NET suite that can't collect coverage: its project doesn't reference coverlet.
fn missing_collector(suite: &Suite) -> bool {
    !coverage::collects(&suite.runner, suite.coverage.coverlet)
}

/// A change in points as the view writes it: `+1.2`, `−2.1`, `±0.0`.
pub fn signed(change: f64) -> String {
    let tenths = (change * 10.0).round() / 10.0;
    if tenths.abs() < HALF_TENTH {
        "±0.0".into()
    } else if tenths > 0.0 {
        format!("+{tenths:.1}")
    } else {
        format!("{MINUS}{:.1}", -tenths)
    }
}

/// A change's colour: a gain green, a loss red, none grey.
fn tone(change: f64) -> C {
    match signed(change).chars().next() {
        Some('+') => C::Green,
        Some(MINUS) => C::Red,
        _ => C::DarkGray,
    }
}

/// How far a run just finished is through a moment `lasts` long that starts `after` it finished, 0 to 1; none before
/// or after it, or with reduced motion.
fn moment(ui: &Ui, suite: &Suite, after: Duration, lasts: Duration) -> Option<f64> {
    let read = suite.coverage.read_at.filter(|_| !ui.reduced_motion)?;
    let since = ui.now.saturating_duration_since(read).checked_sub(after)?;
    let through = since.as_secs_f64() / lasts.as_secs_f64();
    (through < 1.0).then_some(through)
}

/// Whether a run's coverage is still easing in or glinting: frames keep coming until it's done.
pub fn animating(ui: &Ui, controller: &Controller) -> bool {
    let moving = |suite: &Suite| moment(ui, suite, Duration::ZERO, GLINT_AFTER + GLINT).is_some();
    on_for(controller) && controller.suites.iter().flatten().any(moving)
}

/// The value to show now, and the run before's: the number and the bar ease from the old to the new as a run
/// finishes.
fn shown(ui: &Ui, suite: &Suite, runs: &SuiteCoverage) -> Option<(f64, Option<f64>)> {
    let now = runs.last()?.percent;
    let before = runs.before().map(|run| run.percent);
    let eased = match (moment(ui, suite, Duration::ZERO, EASE), before) {
        (Some(through), Some(old)) => old + (now - old) * super::motion::ease(through),
        _ => now,
    };
    Some((eased, before))
}

/// The suite's row's part (`selected`: on the selection's colours), at most `room` cells: a capsule while its run
/// collects coverage, `no coverlet` for a .NET suite without it, else `78.4% (+1.2)` and its bar - as many cells of
/// it as there's room for, down to the least; nothing with the setting off.
pub fn row(ui: &Ui, controller: &Controller, suite: &Suite, selected: bool, room: usize) -> Vec<Seg> {
    if !on_for(controller) {
        return Vec::new();
    }
    let paint = |text: String, color: C| if selected { on(text, color, C::FocusBack) } else { seg(text, color) };
    let parts: Vec<Seg> = if suite.status == Status::Running && suite.coverage.collecting {
        let icon = icons::icon(ui.nerd, Icon::Coverage);
        let label = if icon.is_empty() { format!(" {CAPSULE} ") } else { format!(" {icon} {CAPSULE} ") };
        vec![paint(" ".into(), C::Gray), on(label, C::Black, C::DarkCyan)]
    } else if missing_collector(suite) {
        let warn = icons::icon(ui.nerd, Icon::Warn);
        vec![paint(format!(" {warn} {NO_COLLECTOR}"), C::DarkGray)]
    } else {
        let Some((value, before)) = controller.coverage.suite(&suite.key).and_then(|runs| shown(ui, suite, runs))
        else {
            return Vec::new();
        };
        let old = before.unwrap_or(value);
        let mut parts = number(value, before, selected);
        let text = super::seg::width(&parts[..1])..super::seg::width(&parts);
        let cells = room.saturating_sub(text.end + 1).min(BAR_CELLS);
        let mut moved = 0..0;
        if cells >= LEAST_BAR_CELLS {
            parts.push(paint(" ".into(), C::Gray));
            parts.extend(bar(value, old, selected, cells));
            let changed = changed_cells(value, old, cells);
            moved = text.end + 1 + changed.start..text.end + 1 + changed.end;
        }
        glinted(ui, suite, parts, text, moved, value - old)
    };
    if super::seg::width(&parts) <= room { parts } else { Vec::new() }
}

/// ` 78.4% (+1.2)`: the number, and its change since the full run before.
fn number(value: f64, before: Option<f64>, selected: bool) -> Vec<Seg> {
    let base = if selected { C::FocusText } else { C::Gray };
    let paint = |text: String, color: C| if selected { on(text, color, C::FocusBack) } else { seg(text, color) };
    let mut parts = vec![paint(format!(" {value:.1}%"), base)];
    if let Some(before) = before {
        parts.push(paint(format!(" ({})", signed(value - before)), tone(value - before)));
    }
    parts
}

/// `segs` with the glint sweeping once across the cells `text` and then `cells` - the change, and the bar's changed
/// stretch - a moment after a run finished, in the change's colour.
fn glinted(ui: &Ui, suite: &Suite, segs: Vec<Seg>, text: Range<usize>, cells: Range<usize>, change: f64) -> Vec<Seg> {
    let Some(through) = moment(ui, suite, GLINT_AFTER, GLINT) else { return segs };
    let color = tone(change);
    if color == C::DarkGray {
        return segs;
    }
    let head = (through * (text.len() + cells.len() + GLINT_CELLS) as f64) as usize;
    let lit = head.saturating_sub(GLINT_CELLS)..head;
    let (in_text, in_bar) = (part(&lit, &text, 0), part(&lit, &cells, text.len()));
    let segs = super::seg::band(segs, in_text.start, in_text.end, C::Black, color);
    super::seg::band(segs, in_bar.start, in_bar.end, C::Black, color)
}

/// The cells of `range` that the lit stretch `lit` covers, `range` coming `from` cells into the glint's way.
fn part(lit: &Range<usize>, range: &Range<usize>, from: usize) -> Range<usize> {
    let clamp = |at: usize| at.clamp(from, from + range.len()) - from + range.start;
    clamp(lit.start)..clamp(lit.end)
}

/// The cells of a bar `cells` wide that the change between `value` and `before` touches.
fn changed_cells(value: f64, before: f64, cells: usize) -> Range<usize> {
    let (low, high) = (eighths(value.min(before), cells), eighths(value.max(before), cells));
    if high == low { 0..0 } else { low / 8..high.div_ceil(8) }
}

/// A share of a bar `cells` wide, in eighths of a cell.
fn eighths(percent: f64, cells: usize) -> usize {
    ((percent.clamp(0.0, 100.0) / 100.0) * (cells * 8) as f64).round() as usize
}

/// The bar, `cells` wide: `value` in eighth blocks, the stretch between it and `before` drawn at its end in the
/// change's colour - what was gained green, what was lost red - so +0.3 and +4 look different.
pub fn bar(value: f64, before: f64, selected: bool, cells: usize) -> Vec<Seg> {
    let (low, high) = (eighths(value.min(before), cells), eighths(value.max(before), cells));
    let (base, change) = (if selected { C::FocusText } else { C::DarkCyan }, tone(value - before));
    let back = if selected { Some(C::FocusBack) } else { None };
    let paint = |text: &str, fg: C, bg: Option<C>| match bg {
        Some(bg) => on(text, fg, bg),
        None => seg(text, fg),
    };
    (0..cells)
        .map(|cell| {
            let start = cell * 8;
            let kept = low.saturating_sub(start).min(8);
            let top = high.saturating_sub(start).min(8);
            match (kept, top - kept) {
                (0, 0) => paint(" ", base, back),
                (kept, 0) => paint(eighth(kept), base, back),
                (0, moved) => paint(eighth(moved), change, back),
                (kept, _) if top == 8 => paint(eighth(kept), base, Some(change)),
                (kept, moved) => paint(eighth(top), if moved >= kept { change } else { base }, back),
            }
        })
        .collect()
}

/// A file's last full runs as a braille line, two runs a cell, the newest on the right: each run a step up or down
/// from the one before, the dots between them filled so it reads as one line.
fn trend(values: &[f64], color: C) -> Seg {
    let cells = coverage::RUNS_KEPT / 2;
    let shown = &values[values.len().saturating_sub(cells * 2)..];
    let first = cells * 2 - shown.len();
    let mut dots = vec![0u32; cells];
    let level = |value: f64| ((value.clamp(0.0, 100.0) / 100.0) * (TREND_LEVELS - 1) as f64).round() as usize;
    let mut before: Option<usize> = None;
    for (at, value) in shown.iter().enumerate() {
        let (column, now) = (first + at, level(*value));
        let from = before.unwrap_or(now);
        for dot in from.min(now)..=from.max(now) {
            dots[column / 2] |= if column % 2 == 1 { RIGHT[dot] } else { LEFT[dot] };
        }
        before = Some(now);
    }
    let glyph = |dots: u32| char::from_u32(BRAILLE + dots).unwrap_or(' ');
    seg(dots.into_iter().map(glyph).collect::<String>(), color)
}

/// A file's path as the view shows it: from the repo's root, with `/`.
fn relative(controller: &Controller, path: &str) -> String {
    let path = std::path::Path::new(path);
    path.strip_prefix(&controller.root).unwrap_or(path).display().to_string().replace('\\', "/")
}

/// A file of a report as a path to open: as it is when whole, else from the repo's root.
fn whole(controller: &Controller, path: &str) -> std::path::PathBuf {
    let path = std::path::Path::new(path);
    if path.is_absolute() { path.to_path_buf() } else { controller.root.join(path) }
}

/// The suite's coverage in its details: its number and bar, then the files that changed most since the full run
/// before, each a link to its first line not covered; that it has none and why. Nothing with the setting off.
pub fn details(ui: &mut Ui, controller: &Controller, suite: &Suite, width: usize) -> Vec<Vec<Seg>> {
    if !on_for(controller) {
        return Vec::new();
    }
    let heading = |text: String| super::viewbar::heading(ui.nerd, (Icon::Coverage, C::DarkCyan), &text, C::DarkCyan);
    if missing_collector(suite) {
        let warn = icons::icon(ui.nerd, Icon::Warn);
        return vec![vec![seg(format!("{warn} {NO_COLLECTOR_LONG} (its test project's packages)"), C::DarkGray)]];
    }
    let Some(runs) = controller.coverage.suite(&suite.key) else {
        return vec![vec![seg("No coverage yet - a full run (r) collects it.", C::DarkGray)]];
    };
    let Some((value, before)) = shown(ui, suite, runs) else { return Vec::new() };
    let old = before.unwrap_or(value);
    let mut title = heading(format!("COVERAGE {value:.1}%"));
    let start = super::seg::width(&title);
    title.extend(number(value, before, false).into_iter().skip(1));
    let text = start..super::seg::width(&title);
    title.push(seg("  ", C::Gray));
    let changed = changed_cells(value, old, BAR_CELLS);
    let moved = text.end + 2 + changed.start..text.end + 2 + changed.end;
    title.extend(bar(value, old, false, BAR_CELLS));
    if let Some(was) = runs.before().map(|run| run.percent) {
        title.push(seg(format!("  was {was:.1}%"), C::DarkGray));
    }
    let title = glinted(ui, suite, title, text, moved, value - old);
    let mut lines = vec![ui.scrolling_row(title, 0, width)];
    let changed = runs.changed_files(FILES_SHOWN);
    if changed.is_empty() && before.is_some() {
        lines.push(vec![seg("  no file's coverage changed since the full run before", C::DarkGray)]);
    }
    if !changed.is_empty() {
        lines.push(vec![seg("  changed most since the full run before · O opens them", C::DarkGray)]);
    }
    for file in &changed {
        lines.push(file_row(ui, controller, runs, file, width));
    }
    if !lines.is_empty() {
        lines.push(Vec::new());
    }
    lines
}

/// A changed file: its trend, its change and coverage, and its path - a link to its first line not covered.
fn file_row(ui: &mut Ui, controller: &Controller, runs: &SuiteCoverage, file: &FileChange, width: usize) -> Vec<Seg> {
    let color = tone(file.change);
    let arrow = icons::icon(ui.nerd, if file.change < 0.0 { Icon::Faster } else { Icon::Slower });
    ui.links.push(links::Target::File {
        path: whole(controller, &file.path).display().to_string(),
        line: file.first_uncovered,
        column: None,
    });
    let line = file.first_uncovered.map_or(String::new(), |line| format!(":{line}"));
    let mut link = seg(format!("{}{line}", relative(controller, &file.path)), C::Gray)
        .at(Target::Link { column: ui.focus, link: ui.links.len() - 1 });
    link.underline = true;
    let row = vec![
        seg("  ", C::Gray),
        trend(&runs.trend(&file.path), color),
        seg(format!(" {arrow} {:>5}", signed(file.change)), color),
        seg(format!(" {:>5.1}%  ", file.percent), C::DarkGray),
        link,
    ];
    ui.scrolling_row(row, 4, width)
}

/// `O` on a suite without a failed test: the files whose coverage changed most, each at its first line not covered.
pub fn places(controller: &Controller, key: &str) -> Vec<Place> {
    let Some(runs) = controller.coverage.suite(key).filter(|_| on_for(controller)) else { return Vec::new() };
    let changed = runs.changed_files(FILES_SHOWN);
    changed
        .into_iter()
        .map(|file| Place {
            file: whole(controller, &file.path),
            line: file.first_uncovered,
            column: None,
            own: true,
            text: format!("{:.1}% ({})", file.percent, signed(file.change)),
        })
        .collect()
}

/// The suites that lost more than a point in a run finished since the tests view was last open, the worst first.
fn drops<'a>(ui: &Ui, controller: &'a Controller) -> Vec<(&'a Suite, f64)> {
    if !on_for(controller) {
        return Vec::new();
    }
    let mut found: Vec<(&Suite, f64)> = super::seen::done_suites(ui, controller)
        .into_iter()
        .filter(|suite| suite.coverage.read_at.is_some())
        .filter_map(|suite| Some((suite, controller.coverage.suite(&suite.key)?.change()?)))
        .filter(|(_, change)| *change < -DROP)
        .collect();
    found.sort_by(|a, b| a.1.total_cmp(&b.1));
    found
}

/// The panel's chip for a suite that lost more than a point: `coverage −2.1`.
pub fn chip(ui: &Ui, controller: &Controller) -> Option<String> {
    let (_, worst) = drops(ui, controller).into_iter().next()?;
    Some(format!("{CAPSULE} {}", signed(worst)))
}

/// A click on the chip: the tests view on the suite that lost the most, its files at the top of its details.
pub fn open_drop(ui: &mut Ui, controller: &mut Controller) {
    let key = drops(ui, controller).first().map(|(suite, _)| suite.key.clone());
    super::suites::open(ui, controller);
    let Some(key) = key else { return };
    let all = controller.suites.as_deref().unwrap_or_default();
    let at = all.iter().position(|suite| suite.key == key);
    let rows = super::suites::list_rows(controller);
    let row = rows.iter().position(|row| matches!(row, super::suites::Row::Suite(suite) if Some(*suite) == at));
    if let (Some(row), Some(Overlay::Tests { index, .. })) = (row, &mut ui.overlay) {
        *index = row;
    }
}

/// A suite's coverage for an agent (`get_test_results`): its last full run's, its change since the one before, and
/// the files that changed most; that it can't be collected; nothing with the setting off.
pub fn json(controller: &Controller, suite: &Suite) -> Option<Value> {
    if !on_for(controller) {
        return None;
    }
    if missing_collector(suite) {
        return Some(json!({ "missing": "the test project doesn't reference coverlet.collector" }));
    }
    let runs = controller.coverage.suite(&suite.key)?;
    let round = |value: f64| (value * 10.0).round() / 10.0;
    let files: Vec<Value> = runs
        .changed_files(FILES_SHOWN)
        .iter()
        .map(|file| {
            json!({
                "file": relative(controller, &file.path),
                "percent": round(file.percent),
                "change": round(file.change),
                "first_uncovered_line": file.first_uncovered,
            })
        })
        .collect();
    Some(json!({
        "percent": runs.last().map(|run| round(run.percent)),
        "change": runs.change().map(round),
        "most_changed_files": files,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(segs: &[Seg]) -> String {
        segs.iter().map(|seg| seg.text.as_str()).collect()
    }

    #[test]
    fn a_change_is_signed_and_coloured() {
        assert_eq!((signed(1.24), signed(-2.06), signed(0.02)), ("+1.2".into(), "−2.1".into(), "±0.0".into()));
        assert_eq!((tone(1.2), tone(-2.1), tone(0.01)), (C::Green, C::Red, C::DarkGray));
    }

    #[test]
    fn the_bar_draws_the_change_at_its_end() {
        assert_eq!(text(&bar(50.0, 50.0, false, 8)), "████    ");
        assert_eq!(text(&bar(78.4, 78.4, false, 8)), "██████▎ ", "50.2 eighths: six cells and two eighths");
        assert_eq!(text(&bar(50.0, 50.0, false, 6)), "███   ", "narrower on a narrow row");
        let gained = bar(54.0, 50.0, false, 8);
        assert_eq!(text(&gained), "████▍   ");
        assert_eq!((gained[3].fg, gained[3].bg, gained[4].fg), (C::DarkCyan, None, C::Green), "the gain green");
        let lost = bar(46.0, 50.0, false, 8);
        assert_eq!(text(&lost), "███▋    ", "drawn to where it was");
        assert_eq!((lost[3].fg, lost[3].bg), (C::DarkCyan, Some(C::Red)), "the loss red, behind what's left");
        let small = bar(50.3, 50.0, false, 8);
        assert_eq!(text(&small), "████    ", "+0.3 is less than an eighth: unlike +4");
    }

    #[test]
    fn a_files_trend_is_a_braille_dot_a_run() {
        assert_eq!(trend(&[0.0, 100.0], C::Red).text, "⠀⠀⠀⣸", "a climb from the bottom to the top, filled between");
        assert_eq!(trend(&[50.0; 12], C::Red).text, "⠒⠒⠒⠒", "the last runs kept, halfway up");
        assert_eq!(trend(&[0.0, 0.0, 100.0, 100.0], C::Red).text, "⠀⠀⣀⡏", "one line, not dots");
    }
}
