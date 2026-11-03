//! The tests view's runs (T): a suite's last runs as a row of cells - `[` / `]` pick one, and its failed tests are
//! listed - with its times as a braille line two rows high, the usual dotted across, amber where a run was slower
//! than the usual and red where it failed; for a side or an app, a grid of its suites' runs, the newest on the right,
//! their lines on one scale; a line for each suite in the list; and the project's runs under it. A run just finished
//! glints in (not with reduced motion).

use std::time::Duration;

use lazyrunner_core::run::Controller;
use lazyrunner_core::run::format_duration;
use lazyrunner_core::run::history::{self, Kind, Timed};
use lazyrunner_core::run::suites::Suite;
use lazyrunner_core::run::test_history::{RUNS_KEPT, SuiteHistory};

use super::Ui;
use super::seg::{Seg, on, pad, seg, take_cells};
use super::theme::C;

/// How long a run just finished glints in.
const GLINT: Duration = Duration::from_millis(600);
/// A trend's cells: two runs a cell.
const TREND_CELLS: usize = 10;
/// A run this much slower than the usual is amber in a trend.
const SLOWER: f64 = 1.15;
/// The failed tests a picked run lists.
const FAILED_SHOWN: usize = 8;
/// Braille's dots, bottom up: the left column's, and the right's.
const LEFT: [u32; 4] = [0x40, 0x04, 0x02, 0x01];
const RIGHT: [u32; 4] = [0x80, 0x20, 0x10, 0x08];
const BRAILLE: u32 = 0x2800;

/// Whether `suite` finished a run a moment ago: its newest run glints in.
pub fn fresh(ui: &Ui, suite: &Suite) -> bool {
    !ui.reduced_motion && suite.finished.is_some_and(|at| ui.now.saturating_duration_since(at) < GLINT)
}

/// Whether a run is glinting in: frames keep coming until it's done.
pub fn animating(ui: &Ui, controller: &Controller) -> bool {
    controller.suites.iter().flatten().any(|suite| fresh(ui, suite))
}

/// A run's colour: red when a test failed in it, green when they passed, grey when all were skipped.
fn run_color(history: &SuiteHistory, run: usize) -> C {
    match history.counts(run) {
        (_, failed) if failed > 0 => C::Red,
        (passed, _) if passed > 0 => C::Green,
        _ => C::DarkGray,
    }
}

/// A suite's runs, a cell each, `RUNS_KEPT` wide with the newest on the right: `picked` (runs back from the newest)
/// marked, the newest glinting in when `glint`.
fn cells(history: &SuiteHistory, picked: Option<usize>, glint: bool) -> Vec<Seg> {
    let count = history.runs.len();
    let mut cells = vec![seg("·".repeat(RUNS_KEPT.saturating_sub(count)), C::DarkGray)];
    cells.extend((0..count).map(|run| {
        let (back, color) = (count - 1 - run, run_color(history, run));
        if picked == Some(back) {
            on("▾", C::Black, color)
        } else if glint && back == 0 {
            seg("█", C::White)
        } else {
            seg("█", color)
        }
    }));
    cells
}

/// A suite's runs' times, each in its colour, the last run's time, and the usual one.
type Times = (Vec<(f64, C)>, f64, Option<f64>);

/// Suite `key`'s last `kept` full runs' times, oldest first, each in its colour - red where it failed, amber where it
/// was slower than the usual - with the last run's time and the usual one.
fn times(controller: &Controller, key: &str, kept: usize) -> Option<Times> {
    let item = controller.durations.item(Kind::Test, key)?;
    let runs = &item.runs[item.runs.len().saturating_sub(kept)..];
    let last = runs.last()?.sec;
    let usual = history::trend(&item.runs).and_then(|trend| trend.usual);
    let color = |run: &Timed| match usual {
        _ if !run.ok => C::Red,
        Some(usual) if run.sec > usual * SLOWER => C::Yellow,
        _ => C::DarkCyan,
    };
    Some((runs.iter().map(|run| (run.sec, color(run))).collect(), last, usual))
}

/// `values` as a braille line `rows` high and `cells` wide - two values a cell, the newest on the right, each joined
/// to the one before - scaled from the bottom dot at `low` to the top one at `top` (in the middle when they're the
/// same), with `usual` dotted across. Its rows, the top first; a cell takes the colour of the worst value in it.
fn line(values: &[(f64, C)], (low, top): (f64, f64), rows: usize, cells: usize, usual: Option<f64>) -> Vec<Vec<Seg>> {
    let highest = (rows * 4 - 1) as f64;
    let span = top - low;
    let level = |sec: f64| {
        let share = if span > f64::EPSILON { (sec - low) / span } else { 0.5 };
        (share * highest).round().clamp(0.0, highest) as usize
    };
    let columns = cells * 2;
    let shown = &values[values.len().saturating_sub(columns)..];
    let first = columns - shown.len();
    let (mut drawn, mut dotted) = (vec![vec![0; cells]; rows], vec![vec![0; cells]; rows]);
    let mut colors: Vec<Option<C>> = vec![None; cells];
    let mut before = None;
    for (at, (sec, color)) in shown.iter().enumerate() {
        let (column, now) = (first + at, level(*sec));
        let from: usize = before.unwrap_or(now);
        for dot in from.min(now)..=from.max(now) {
            set(&mut drawn, column, dot);
        }
        before = Some(now);
        colors[column / 2] = Some(worse(colors[column / 2], *color));
    }
    if let Some(usual) = usual {
        for column in (0..columns).step_by(2) {
            set(&mut dotted, column, level(usual));
        }
    }
    let glyph = |dots: u32| char::from_u32(BRAILLE + dots).unwrap_or(' ').to_string();
    (0..rows)
        .map(|row| {
            (0..cells)
                .map(|cell| match (drawn[row][cell], dotted[row][cell]) {
                    (0, 0) => seg(" ", C::Gray),
                    (0, dots) => seg(glyph(dots), C::DarkGray),
                    (dots, more) => seg(glyph(dots | more), colors[cell].unwrap_or(C::DarkCyan)),
                })
                .collect()
        })
        .collect()
}

/// The fastest and the slowest of `seconds`: a line's scale.
fn range(seconds: impl Iterator<Item = f64>) -> (f64, f64) {
    seconds.fold((f64::MAX, 0.0), |(low, top), sec| (low.min(sec), top.max(sec)))
}

/// The dot `level` up (0 = the bottom of the bottom row) of `column` in a braille grid, its rows top first.
fn set(grid: &mut [Vec<u32>], column: usize, level: usize) {
    let row = grid.len() - 1 - level / 4;
    grid[row][column / 2] |= if column % 2 == 1 { RIGHT[level % 4] } else { LEFT[level % 4] };
}

/// The worse of two colours in a line: red, then amber, then the rest.
fn worse(color: Option<C>, other: C) -> C {
    let rank = |color: C| match color {
        C::Red => 2,
        C::Yellow => 1,
        _ => 0,
    };
    match color {
        Some(color) if rank(color) >= rank(other) => color,
        _ => other,
    }
}

/// A suite's times as a line `cells` wide for its row in the list, on its own scale, and the last run's time.
pub fn list_line(controller: &Controller, key: &str, cells: usize) -> Option<(Vec<Seg>, f64)> {
    let (values, last, _) = times(controller, key, cells * 2)?;
    let range = range(values.iter().map(|(sec, _)| *sec));
    Some((line(&values, range, 1, cells, None).remove(0), last))
}

/// The project's last test runs, the newest first, at most `shown`, for under the list: when, how many passed and
/// failed, how long.
pub fn run_history(controller: &Controller, shown: usize) -> Vec<Vec<Seg>> {
    let runs = &controller.test_runs.0;
    if runs.is_empty() || shown == 0 {
        return Vec::new();
    }
    let today = chrono::Local::now().format("%Y-%m-%d ").to_string();
    let mut rows = vec![Vec::new(), vec![seg(" RUN HISTORY", C::DarkCyan)]];
    for run in runs.iter().rev().take(shown) {
        let when = match run.at.strip_prefix(&today) {
            Some(time) => time.get(..5).unwrap_or(time).to_string(),
            None => run.at.get(5..16).unwrap_or(&run.at).to_string(),
        };
        let mut row = vec![seg(format!(" {when:<11} "), C::DarkGray), seg(format!("✓{:<5} ", run.passed), C::Green)];
        row.push(match (run.failed, run.errors) {
            (0, 0) => seg("all green  ", C::Green),
            (failed, 0) => seg(format!("✗{failed:<9} "), C::Red),
            (failed, errors) => seg(format!("✗{failed} !{errors:<6} "), C::Red),
        });
        row.push(seg(format_duration(run.seconds), C::DarkGray));
        rows.push(row);
    }
    rows
}

/// A suite's runs in its details: a row of its last runs (`[` `]` pick one), a trend of their durations, and the
/// picked run - when it finished, its counts, whether it ran on the newest run's code - with its failed tests.
pub fn rows(ui: &Ui, controller: &Controller, suite: &Suite) -> Vec<Vec<Seg>> {
    let mut rows = Vec::new();
    let Some(history) = controller.test_history.suite(&suite.key).filter(|history| !history.runs.is_empty()) else {
        return rows;
    };
    let newest = history.runs.len() - 1;
    let picked = ui.tests_run.map(|back| back.min(newest));
    let mut runs = vec![seg("runs   ", C::DarkGray)];
    runs.extend(cells(history, picked, fresh(ui, suite)));
    let keys = if picked.is_some() { "   ] newer · [ older" } else { "   [ picks a run" };
    runs.push(seg(keys, C::DarkGray));
    rows.push(runs);
    if let Some((values, last, usual)) = times(controller, &suite.key, TREND_CELLS * 2) {
        let range = range(values.iter().map(|(sec, _)| *sec).chain(usual));
        let usual_text = usual.map_or(String::new(), |usual| format!("usual {} (dotted) · ", format_duration(usual)));
        for (at, drawn) in line(&values, range, 2, TREND_CELLS, usual).into_iter().enumerate() {
            let label = if at == 0 { "time   " } else { "       " };
            let mut row = vec![seg(label, C::DarkGray), seg(" ".repeat(RUNS_KEPT - TREND_CELLS), C::Gray)];
            row.extend(drawn);
            if at == 0 {
                row.push(seg(format!("   {usual_text}last {}", format_duration(last)), C::DarkGray));
            }
            rows.push(row);
        }
    }
    let Some(back) = picked else { return rows };
    let run = newest - back;
    let (passed, failed) = history.counts(run);
    let which = match back {
        0 => "the newest run".to_string(),
        1 => "1 run back".to_string(),
        back => format!("{back} runs back"),
    };
    let today = chrono::Local::now().format("%Y-%m-%d ").to_string();
    let when = history.when(run).map_or(String::new(), |at| format!(" · {}", at.strip_prefix(&today).unwrap_or(at)));
    let code = &history.runs[run];
    let same = if back == 0 {
        ""
    } else if !code.is_empty() && *code == history.runs[newest] {
        " · on the newest run's code"
    } else {
        " · on other code"
    };
    let color = if failed > 0 { C::Red } else { C::Green };
    rows.push(vec![seg(format!("{which}{when} · ✓{passed} ✗{failed}{same}"), color)]);
    let names = history.failed_in(run);
    for name in names.iter().take(FAILED_SHOWN) {
        rows.push(vec![seg(format!("  ✗ {name}"), C::Red)]);
    }
    if names.len() > FAILED_SHOWN {
        rows.push(vec![seg(format!("  … and {} more", names.len() - FAILED_SHOWN), C::DarkGray)]);
    }
    rows
}

/// A side's or an app's suites (`keys`): their last runs as a grid, the newest on the right, and their trends on one
/// axis, `width` wide; none before any of them has a history.
pub fn grid(ui: &Ui, controller: &Controller, keys: &[String], width: usize) -> Vec<Vec<Seg>> {
    let suites: Vec<&Suite> = keys.iter().filter_map(|key| controller.suite(key)).collect();
    if !suites.iter().any(|suite| controller.test_history.suite(&suite.key).is_some()) {
        return Vec::new();
    }
    let range = range(
        suites
            .iter()
            .filter_map(|suite| controller.durations.item(Kind::Test, &suite.key))
            .flat_map(|item| item.runs[item.runs.len().saturating_sub(TREND_CELLS * 2)..].iter().map(|run| run.sec)),
    );
    let name_cells = width.saturating_sub(RUNS_KEPT + TREND_CELLS + 14).clamp(8, 28);
    let mut rows = vec![vec![seg("Last runs, the newest on the right · their times on one scale:", C::DarkCyan)]];
    for suite in suites {
        let name = pad(&take_cells(&suite.name, name_cells), name_cells);
        let mut row = vec![seg(format!("{name} "), C::Gray)];
        match controller.test_history.suite(&suite.key) {
            Some(history) => row.extend(cells(history, None, fresh(ui, suite))),
            None => row.push(seg("·".repeat(RUNS_KEPT), C::DarkGray)),
        }
        if let Some((values, last, _)) = times(controller, &suite.key, TREND_CELLS * 2) {
            row.push(seg("  ", C::Gray));
            row.extend(line(&values, range, 1, TREND_CELLS, None).remove(0));
            row.push(seg(format!(" {}", format_duration(last)), C::DarkGray));
        }
        rows.push(row);
    }
    rows
}
