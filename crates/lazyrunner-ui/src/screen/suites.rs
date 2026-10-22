//! T: the test suites. On the left the list, grouped by what runs them; on the right the selected suite: its live
//! output while it runs, then its failures (message and its own stack frames, links clickable), or its slowest tests
//! when all passed.

use std::collections::VecDeque;
use std::time::Instant;

use lazyrunner_core::links;
use lazyrunner_core::output::Hue;
use lazyrunner_core::run::Controller;
use lazyrunner_core::run::format_duration;
use lazyrunner_core::run::suites::{Outcome, Status, Suite, TestResult};

use super::columns::{chunk, fit, linked, spinner};
use super::mouse::Target;
use super::seg::{Seg, aim, on, pad, seg, take_cells};
use super::theme::C;
use super::{Overlay, Ui, copy};

/// The list's width: two fifths of the screen, within these.
const LIST_WIDTH: (usize, usize) = (30, 60);
/// Lines of live output turned into rows each frame (only the end shows).
const TAIL: usize = 80;
/// Lines of a failed test's message, and of its stack, shown.
const MESSAGE_LINES: usize = 8;
const STACK_LINES: usize = 4;
const ERROR_LINES: usize = 40;
const SLOWEST: usize = 5;
const SKIPPED_SHOWN: usize = 10;
/// Ctrl+d / Ctrl+u and the wheel scroll the details this far.
const PAGE: usize = 10;
const WHEEL: usize = 3;
/// Stack frames of the test framework and the runtime: shown only when a stack has nothing else.
const FRAMEWORK: [&str; 7] =
    ["at System.", "at Microsoft.", "at Xunit.", "at NUnit.", "at Castle.", "at Moq.", "node_modules"];

/// T: opens the view and looks for the suites (again, to find new ones).
pub fn open(ui: &mut Ui, controller: &mut Controller) {
    controller.find_suites();
    ui.overlay = Some(Overlay::Tests { index: 0, scroll: 0 });
}

/// j k choose, r / Enter run it, R run all, f / F the failed tests again, s stop, o its log, y copy its failures,
/// Ctrl+d / Ctrl+u scroll, Esc / q / T back.
pub fn key(ui: &mut Ui, controller: &mut Controller, name: &str) {
    let Some(Overlay::Tests { index, scroll }) = &mut ui.overlay else { return };
    let all: Vec<String> = controller.suites.iter().flatten().map(|suite| suite.key.clone()).collect();
    let selected: Vec<String> = all.get(*index).cloned().into_iter().collect();
    match name {
        "Escape" | "q" | "T" => ui.overlay = None,
        "j" | "DownArrow" => {
            *index = (*index + 1).min(all.len().saturating_sub(1));
            *scroll = 0;
        }
        "k" | "UpArrow" => {
            *index = index.saturating_sub(1);
            *scroll = 0;
        }
        "g" | "Home" => *scroll = 0,
        "Ctrl+D" | "PageDown" => *scroll += PAGE,
        "Ctrl+U" | "PageUp" => *scroll = scroll.saturating_sub(PAGE),
        "r" | "Enter" => controller.run_tests(&selected, false),
        "R" => controller.run_tests(&all, false),
        "f" => controller.run_tests(&selected, true),
        "F" => controller.run_tests(&all, true),
        "s" if controller.tests_active() => controller.stop_tests(false),
        "s" => controller.add_log("No tests are running.", Hue::DarkGray),
        "o" => open_log(ui, controller, selected.first()),
        "y" => copy_failures(ui, controller, selected.first()),
        _ => {}
    }
}

/// The wheel over the details scrolls them (`back`: up).
pub fn wheel(ui: &mut Ui, back: bool) {
    if let Some(Overlay::Tests { scroll, .. }) = &mut ui.overlay {
        *scroll = if back { scroll.saturating_sub(WHEEL) } else { *scroll + WHEEL };
    }
}

fn open_log(ui: &Ui, controller: &mut Controller, key: Option<&String>) {
    match key.and_then(|key| controller.suite_log(key)) {
        Some(log) if (ui.opener)(&log.display().to_string()) => {}
        Some(log) => controller.add_log(format!("✗ Couldn't open {}", log.display()), Hue::Red),
        None => controller.add_log("No test log yet - r runs the suite.", Hue::DarkGray),
    }
}

/// y: every failure of the selected suite - name, message, stack - ready to paste.
fn copy_failures(ui: &Ui, controller: &mut Controller, key: Option<&String>) {
    let Some(suite) = key.and_then(|key| controller.suite(key)) else { return };
    let failed: Vec<&TestResult> = suite.tests.iter().filter(|test| test.outcome == Outcome::Failed).collect();
    if failed.is_empty() {
        let text = format!("Nothing to copy: no failed tests in {}.", suite.name);
        controller.add_log(text, Hue::DarkGray);
        return;
    }
    let text = failed
        .iter()
        .map(|test| {
            let mut parts = vec![format!("Failed: {}{}", test.name, took(test))];
            parts.extend([&test.message, &test.stack].into_iter().filter(|part| !part.is_empty()).cloned());
            parts.join(copy::NEWLINE)
        })
        .collect::<Vec<_>>()
        .join(&copy::NEWLINE.repeat(2));
    let what = format!("{} failed test{} from {}", failed.len(), if failed.len() == 1 { "" } else { "s" }, suite.name);
    copy::put(ui.clipboard, controller, &text, &what);
}

fn took(test: &TestResult) -> String {
    if test.ms > 0.0 { format!(" ({} ms)", test.ms.round()) } else { String::new() }
}

// ---------------------------------------------------------------- the view

pub fn rows(ui: &mut Ui, controller: &Controller, width: usize, rows: usize) -> Vec<Vec<Seg>> {
    let suites = controller.suites.as_deref().unwrap_or_default();
    let Some(Overlay::Tests { index, .. }) = &mut ui.overlay else { return Vec::new() };
    *index = (*index).min(suites.len().saturating_sub(1));
    let selected = *index;
    let list_width = (width * 2 / 5).clamp(LIST_WIDTH.0, LIST_WIDTH.1).min(width.saturating_sub(20));
    let detail_width = width.saturating_sub(list_width + 2);
    let height = rows.saturating_sub(2);
    let (list, at) = list(ui, suites, selected, list_width);
    let list_top = at.saturating_sub(height / 2).min(list.len().saturating_sub(height));
    let (detail, tail) = match suites.get(selected) {
        Some(suite) => details(ui, controller, suite, detail_width),
        None if controller.suites.is_none() => (vec![vec![seg("Looking for tests…", C::DarkGray)]], false),
        None => (vec![vec![seg("No test suites in this project.", C::DarkGray)]], false),
    };
    let most = detail.len().saturating_sub(height);
    let detail_top = match &mut ui.overlay {
        Some(Overlay::Tests { scroll, .. }) => {
            *scroll = (*scroll).min(most);
            if tail { most } else { *scroll }
        }
        _ => 0,
    };
    let mut out = vec![aim(header(ui, controller, width), Target::Box), aim(hint(width), Target::Box)];
    for row in 0..height {
        let left = list.get(list_top + row).cloned().unwrap_or_default();
        let right = detail.get(detail_top + row).cloned().unwrap_or_default();
        let mut line = fit(left, list_width);
        line.push(seg("│ ", C::FocusBorder));
        line.extend(fit(right, detail_width));
        out.push(aim(line, Target::Box));
    }
    out
}

fn header(ui: &Ui, controller: &Controller, width: usize) -> Vec<Seg> {
    let count = controller.suites.as_ref().map_or(0, Vec::len);
    let totals = controller.test_totals();
    let (summary, color) = if let Some(running) = status(controller) {
        let building = if controller.test_build.is_some() { " · building" } else { "" };
        let at_once = controller.test_parallelism();
        (format!("{} {running}{building} · up to {at_once} at once", icon(ui, Status::Running)), C::Yellow)
    } else if totals.ran > 0 {
        let skipped = if totals.skipped > 0 { format!("   – {}", totals.skipped) } else { String::new() };
        let errors = if totals.errors > 0 { format!("   ! {} didn't run", totals.errors) } else { String::new() };
        let color = if totals.failed > 0 || totals.errors > 0 { C::Red } else { C::Green };
        (format!("✓ {}   ✗ {}{skipped}{errors}", totals.passed, totals.failed), color)
    } else {
        ("not run yet".to_string(), C::DarkGray)
    };
    let segs = vec![
        on(" ⚗ Tests ", C::FocusText, C::FocusBack),
        seg(format!("  {count} suite{} · ", if count == 1 { "" } else { "s" }), C::DarkGray),
        seg(summary, color),
    ];
    fit(segs, width)
}

fn hint(width: usize) -> Vec<Seg> {
    let text = " j k choose · r run · R run all · f / F failed again · s stop · o log · y copy failures · ^d ^u \
                scroll · Esc back";
    fit(vec![seg(text, C::DarkGray)], width)
}

/// The list's rows, grouped by what runs the suites, and the selected suite's row.
fn list(ui: &Ui, suites: &[Suite], selected: usize, width: usize) -> (Vec<Vec<Seg>>, usize) {
    let mut rows = Vec::new();
    let mut at = 0;
    let mut group = "";
    for (index, suite) in suites.iter().enumerate() {
        if suite.runner.group() != group {
            group = suite.runner.group();
            rows.push(vec![seg(format!(" {group}"), C::DarkCyan)]);
        }
        let summary = summary(suite);
        let room = width.saturating_sub(summary.chars().count() + 7).max(4);
        let name = if suite.name.chars().count() > room {
            format!("{}…", take_cells(&suite.name, room - 1))
        } else {
            pad(&suite.name, room)
        };
        let icon = icon(ui, suite.status);
        let row = if index == selected {
            at = rows.len();
            vec![on(pad(&format!(" ▸ {icon} {name} {summary}"), width), C::FocusText, C::FocusBack)]
        } else {
            let color = color(suite.status);
            vec![seg(format!("   {icon} "), color), seg(format!("{name} "), C::Gray), seg(summary, color)]
        };
        rows.push(aim(row, Target::Item(index)));
    }
    (rows, at)
}

fn icon(ui: &Ui, status: Status) -> String {
    match status {
        Status::Running if ui.reduced_motion => "●".into(),
        Status::Running => spinner(ui.tick()).to_string(),
        Status::Idle => "·".into(),
        Status::Queued => "◌".into(),
        Status::Passed => "✓".into(),
        Status::Failed => "✗".into(),
        Status::Error => "!".into(),
        Status::Cancelled => "■".into(),
    }
}

fn color(status: Status) -> C {
    match status {
        Status::Idle | Status::Queued => C::DarkGray,
        Status::Running | Status::Cancelled => C::Yellow,
        Status::Passed => C::Green,
        Status::Failed | Status::Error => C::Red,
    }
}

/// What the list says about a suite.
fn summary(suite: &Suite) -> String {
    match suite.status {
        Status::Idle => "not run".into(),
        Status::Queued if suite.note.is_empty() => "queued".into(),
        Status::Queued | Status::Error => suite.note.clone(),
        Status::Running => format!("{} done", suite.done),
        Status::Passed => format!("{} passed", suite.passed),
        Status::Failed => format!("{} of {} failed", suite.failed, suite.total()),
        Status::Cancelled => "stopped".into(),
    }
}

/// The selected suite's details, and whether they show their end (live output) rather than scroll from the top.
fn details(ui: &mut Ui, controller: &Controller, suite: &Suite, width: usize) -> (Vec<Vec<Seg>>, bool) {
    let spin = icon(ui, Status::Running);
    let now = ui.now;
    let elapsed = |started: Option<Instant>| {
        format_duration(started.map_or(0.0, |started| now.saturating_duration_since(started).as_secs_f64()))
    };
    let when = suite
        .last_run
        .map_or(String::new(), |at| format!("last run {} · {}", at.format("%H:%M:%S"), format_duration(suite.seconds)));
    let mut out = Details { ui, lines: Vec::new(), width };
    out.lines.push(vec![seg(suite.name.clone(), C::White), seg(format!("   {}", suite.runner.group()), C::DarkGray)]);
    match suite.status {
        Status::Running => {
            let only = if suite.failed_only { " · failed tests only" } else { "" };
            out.add(&format!("{spin} running · {} done · {}{only}", suite.done, elapsed(suite.started)), C::Yellow, 0);
            out.blank();
            for line in tail(&suite.output) {
                let bad = ["Failed ", "FAIL ", "error", "×"].iter().any(|word| line.contains(word));
                out.add(line, if bad { C::Red } else { C::DarkGray }, 2);
            }
            return (out.lines, true);
        }
        Status::Queued => {
            let build = controller.test_build.as_ref().filter(|build| build.keys.contains(&suite.key) && !suite.built);
            if let Some(build) = build {
                let started = elapsed(Some(build.started));
                let text = format!("{spin} building the test projects together · {} done · {started}", build.done);
                out.add(&text, C::Yellow, 0);
                out.blank();
                for line in tail(&build.output) {
                    out.add(line, if line.contains(": error ") { C::Red } else { C::DarkGray }, 2);
                }
                return (out.lines, true);
            }
            let note = if suite.note.is_empty() { "queued" } else { suite.note.as_str() };
            out.add(&format!("◌ {note}"), C::DarkGray, 0);
        }
        Status::Idle => {
            out.add("Not run yet - r runs this suite, R runs all of them.", C::DarkGray, 0);
            return (out.lines, false);
        }
        Status::Error => {
            out.add(&format!("! {} · {when}", suite.note), C::Red, 0);
            out.blank();
            if suite.output.is_empty() {
                out.add("o opens the log.", C::DarkGray, 0);
            }
            for line in suite.output.iter().skip(suite.output.len().saturating_sub(ERROR_LINES)) {
                out.add(line, if line.contains("error") { C::Red } else { C::DarkGray }, 2);
            }
            return (out.lines, false);
        }
        Status::Cancelled if suite.total() > 0 => {
            out.add("■ stopped - the results below are from the run before", C::Yellow, 0);
        }
        Status::Cancelled => out.add("■ stopped", C::Yellow, 0),
        Status::Passed | Status::Failed => {}
    }
    if suite.total() > 0 {
        results(&mut out, suite, &when);
    }
    (out.lines, false)
}

/// The counts, each failure (its message and own frames), the slowest tests when all passed, and the skipped.
fn results(out: &mut Details, suite: &Suite, when: &str) {
    let counts =
        format!("✓ {} passed   ✗ {} failed   – {} skipped   · {when}", suite.passed, suite.failed, suite.skipped);
    out.add(&counts, if suite.failed > 0 { C::Red } else { C::Green }, 0);
    out.blank();
    for test in suite.tests.iter().filter(|test| test.outcome == Outcome::Failed) {
        out.add(&format!("✗ {}{}", test.name, took(test)), C::Red, 0);
        for line in test.message.lines().filter(|line| !line.trim().is_empty()).take(MESSAGE_LINES) {
            out.add(line.trim_end(), C::Gray, 4);
        }
        for frame in own_frames(&test.stack).into_iter().take(STACK_LINES) {
            out.add(frame.trim(), C::DarkGray, 4);
        }
        out.blank();
    }
    if suite.failed == 0 {
        out.add(&format!("All {} tests passed.", suite.passed), C::Green, 0);
        out.blank();
        out.add("Slowest:", C::DarkCyan, 0);
        let mut slowest: Vec<&TestResult> = suite.tests.iter().collect();
        slowest.sort_by(|a, b| b.ms.total_cmp(&a.ms));
        for test in slowest.into_iter().take(SLOWEST) {
            out.add(&format!("{:>7} ms  {}", test.ms.round(), test.name), C::DarkGray, 2);
        }
    }
    let skipped: Vec<&TestResult> = suite.tests.iter().filter(|test| test.outcome == Outcome::Skipped).collect();
    if !skipped.is_empty() {
        out.blank();
        out.add(&format!("Skipped ({}):", skipped.len()), C::DarkCyan, 0);
        for test in skipped.into_iter().take(SKIPPED_SHOWN) {
            out.add(&format!("– {}", test.name), C::DarkGray, 2);
        }
    }
}

/// The test's own stack frames; the framework's and the runtime's only when there is nothing else.
fn own_frames(stack: &str) -> Vec<&str> {
    let frames: Vec<&str> = stack.lines().filter(|line| !line.trim().is_empty()).collect();
    let own: Vec<&str> = frames
        .iter()
        .copied()
        .filter(|frame| {
            !FRAMEWORK.iter().any(|framework| frame.trim_start().starts_with(framework) || frame.contains(framework))
        })
        .collect();
    if own.is_empty() { frames } else { own }
}

fn tail(lines: &VecDeque<String>) -> impl Iterator<Item = &String> {
    lines.iter().skip(lines.len().saturating_sub(TAIL))
}

/// The details' rows: text wrapped to the width, its links underlined and clickable.
struct Details<'a> {
    ui: &'a mut Ui,
    lines: Vec<Vec<Seg>>,
    width: usize,
}

impl Details<'_> {
    fn add(&mut self, text: &str, color: C, indent: usize) {
        let room = self.width.saturating_sub(indent).max(10);
        // Links are found in the whole line, so one that wraps is a link on both rows.
        let found = links::find(text);
        let ids: Vec<usize> = found
            .iter()
            .map(|link| {
                self.ui.links.push(link.target.clone());
                self.ui.links.len() - 1
            })
            .collect();
        let mut start = 0;
        for piece in chunk(text, room) {
            let mut row = vec![seg(" ".repeat(indent), C::Gray)];
            if found.is_empty() {
                row.push(seg(piece.clone(), color));
            } else {
                row.extend(linked(&piece, start, &found, &ids, color, self.ui.focus));
            }
            start += piece.len();
            self.lines.push(row);
        }
    }

    /// An empty row, unless the last one is.
    fn blank(&mut self) {
        if self.lines.last().is_some_and(|row| !row.is_empty()) {
            self.lines.push(Vec::new());
        }
    }
}

/// What the panel's status says while tests run: "testing 2 +3…" (running, and queued).
pub fn status(controller: &Controller) -> Option<String> {
    if !controller.tests_active() {
        return None;
    }
    let count = |status| controller.suites.iter().flatten().filter(|suite| suite.status == status).count();
    let queued = count(Status::Queued);
    let waiting = if queued > 0 { format!(" +{queued} queued") } else { String::new() };
    Some(format!("testing {}{waiting}", count(Status::Running)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stack_shows_the_tests_own_frames() {
        let stack = "at Xunit.Assert.Equal()\nat Shop.Tests.CartTests.Totals() in C:\\src\\CartTests.cs:line 42\nat System.Runtime.Invoke()";
        assert_eq!(own_frames(stack), ["at Shop.Tests.CartTests.Totals() in C:\\src\\CartTests.cs:line 42"]);
        assert_eq!(own_frames("at System.A()\nat System.B()").len(), 2, "nothing else: all of them");
    }
}
