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
use super::icons::{self, Icon};
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
/// Stack frames of the picked test shown.
const PICKED_STACK_LINES: usize = 6;
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
    ui.overlay = Some(Overlay::Tests { index: FIRST_SUITE, scroll: 0, picking: None });
}

/// The selection when the view opens: the first suite (once there is a list).
pub(super) const FIRST_SUITE: usize = usize::MAX;

/// A row of the list: a side (Backend, Frontend), one of its apps (or the suites of no app), or a suite (by its
/// place among the suites).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Row {
    Side {
        backend: bool,
    },
    App {
        backend: bool,
        app: Option<String>,
    },
    Suite(usize),
    /// An app's projects with a test target but no test files, as one row.
    Untested {
        backend: bool,
        app: Option<String>,
    },
}

/// The list: the backend, then the frontend; in each, the profile's apps in their order (then the suites of no
/// app), each with its suites.
pub fn list_rows(controller: &Controller) -> Vec<Row> {
    let suites = controller.suites.as_deref().unwrap_or_default();
    let owner = |suite: &Suite| suite.app.clone().filter(|key| controller.app(key).is_some());
    let mut rows = Vec::new();
    for backend in [true, false] {
        let side: Vec<usize> = (0..suites.len()).filter(|&at| suites[at].runner.backend() == backend).collect();
        if side.is_empty() {
            continue;
        }
        rows.push(Row::Side { backend });
        for app in controller.apps.iter().map(|app| Some(app.key.clone())).chain([None]) {
            let mine: Vec<usize> = side.iter().copied().filter(|&at| owner(&suites[at]) == app).collect();
            if mine.is_empty() {
                continue;
            }
            let (tested, untested): (Vec<usize>, Vec<usize>) = mine.into_iter().partition(|&at| !suites[at].untested);
            rows.push(Row::App { backend, app: app.clone() });
            rows.extend(tested.into_iter().map(Row::Suite));
            if !untested.is_empty() {
                rows.push(Row::Untested { backend, app });
            }
        }
    }
    rows
}

/// The selection, in the list: the first suite at first, never past the end.
fn chosen(index: &mut usize, rows: &[Row]) -> usize {
    if *index == FIRST_SUITE && !rows.is_empty() {
        *index = rows.iter().position(|row| matches!(row, Row::Suite(_))).unwrap_or(0);
    }
    *index = (*index).min(rows.len().saturating_sub(1));
    *index
}

/// The suites of a row: the suite, or every suite under a side or an app.
fn under(controller: &Controller, rows: &[Row], at: usize) -> Vec<String> {
    let suites = controller.suites.as_deref().unwrap_or_default();
    let ends = |row: &Row| match rows.get(at) {
        Some(Row::Side { .. }) => matches!(row, Row::Side { .. }),
        Some(Row::App { .. }) => !matches!(row, Row::Suite(_)),
        _ => true,
    };
    let range: Vec<&Row> = match rows.get(at) {
        Some(Row::Suite(_)) => vec![&rows[at]],
        Some(_) => rows[at + 1..].iter().take_while(|row| !ends(row)).collect(),
        None => Vec::new(),
    };
    range
        .into_iter()
        .filter_map(|row| match row {
            Row::Suite(index) => suites.get(*index).map(|suite| suite.key.clone()),
            _ => None,
        })
        .collect()
}

/// The names of an Untested row's projects.
fn untested(controller: &Controller, row: &Row) -> Vec<String> {
    let Row::Untested { backend, app } = row else { return Vec::new() };
    let owner = |suite: &Suite| suite.app.clone().filter(|key| controller.app(key).is_some());
    let suites = controller.suites.as_deref().unwrap_or_default();
    suites
        .iter()
        .filter(|suite| suite.untested && suite.runner.backend() == *backend && owner(suite) == *app)
        .map(|suite| suite.name.clone())
        .collect()
}

/// A watched suite's mark: armed, not running - it holds still.
const WATCHED: &str = "◉";

/// What a side or app row says: "Backend · dotnet test", "Orders", "Other".
fn title(controller: &Controller, row: &Row) -> String {
    let suites = controller.suites.as_deref().unwrap_or_default();
    match row {
        Row::Side { backend: true } => "Backend · dotnet test".into(),
        Row::Side { backend: false } => {
            let mut runners: Vec<&str> = Vec::new();
            for suite in suites.iter().filter(|suite| !suite.runner.backend()) {
                let runner = suite.runner.group().rsplit(" · ").next().unwrap_or_default();
                if !runners.contains(&runner) {
                    runners.push(runner);
                }
            }
            format!("Frontend · {}", runners.join(" · "))
        }
        Row::App { app: Some(key), .. } => controller.app(key).map_or_else(|| key.clone(), |app| app.name.clone()),
        Row::App { app: None, .. } => "Other".into(),
        Row::Suite(at) => suites.get(*at).map(|suite| suite.name.clone()).unwrap_or_default(),
        Row::Untested { .. } => {
            let count = untested(controller, row).len();
            format!("{count} project{} without test files", if count == 1 { "" } else { "s" })
        }
    }
}

/// j k choose, l its tests (pick one), r / Enter run it, R run all, f / F the failed tests again, s stop, o its log,
/// y copy its failures, Ctrl+d / Ctrl+u scroll, Esc / q / T back; false for a key it doesn't use.
pub fn key(ui: &mut Ui, controller: &mut Controller, name: &str) -> bool {
    if matches!(ui.overlay, Some(Overlay::Tests { picking: Some(_), .. })) && pick_key(ui, controller, name) {
        return true;
    }
    let rows = list_rows(controller);
    let Some(Overlay::Tests { index, scroll, picking }) = &mut ui.overlay else { return false };
    let at = chosen(index, &rows);
    let all: Vec<String> = controller.suites.iter().flatten().map(|suite| suite.key.clone()).collect();
    let selected = under(controller, &rows, at);
    let a_suite = matches!(rows.get(at), Some(Row::Suite(_)));
    let label = rows.get(at).map(|row| title(controller, row)).unwrap_or_default();
    match name {
        "Escape" | "q" | "T" => ui.overlay = None,
        "j" | "DownArrow" => {
            *index = super::keys::wrapped(at, 1, rows.len());
            *scroll = 0;
        }
        "k" | "UpArrow" => {
            *index = super::keys::wrapped(at, -1, rows.len());
            *scroll = 0;
        }
        "l" | "RightArrow" | "Tab" => match selected.first().and_then(|key| controller.suite(key)) {
            Some(suite) if a_suite && suite.total() > 0 => *picking = Some(0),
            _ if !a_suite => controller.add_log("Choose a suite to pick one of its tests.", Hue::DarkGray),
            _ => controller.add_log("No results for this suite yet - r runs it.", Hue::DarkGray),
        },
        "<" => (*index, *scroll) = (0, 0),
        ">" => (*index, *scroll) = (rows.len().saturating_sub(1), 0),
        digit if digit.len() == 1 && ('1'..='9').contains(&digit.chars().next().unwrap_or('0')) => {
            // The apps' sections, numbered in the list.
            let nth = digit.parse::<usize>().unwrap_or(1) - 1;
            let apps = rows.iter().enumerate().filter(|(_, row)| matches!(row, Row::App { .. }));
            if let Some((at, _)) = apps.clone().nth(nth) {
                (*index, *scroll, *picking) = (at, 0, None);
            }
        }
        "g" | "Home" => *scroll = 0,
        "Ctrl+D" | "PageDown" => *scroll += PAGE,
        "Ctrl+U" | "PageUp" => *scroll = scroll.saturating_sub(PAGE),
        "r" | "Enter" | "f" if selected.is_empty() => {
            controller.add_log("Nothing to run here: these projects have no test files.", Hue::DarkGray);
        }
        "r" | "Enter" => controller.run_tests(&selected, false),
        "R" => controller.run_tests(&all, false),
        "a" => controller.toggle_watch(&selected),
        "A" => controller.toggle_watch(&all),
        "f" => controller.run_tests(&selected, true),
        "F" => controller.run_tests(&all, true),
        "s" if controller.tests_active() => controller.stop_tests(false),
        "s" => controller.add_log("No tests are running.", Hue::DarkGray),
        "o" if !a_suite => controller.add_log("o opens a suite's log - choose one.", Hue::DarkGray),
        "o" => open_log(ui, controller, selected.first()),
        "y" => copy_failures(ui, controller, &selected, &label),
        _ => return false,
    }
    true
}

/// Picking a test (l): j k choose, r / Enter run it, y copy it, h / Esc / Tab back to the suites; false for the
/// keys the suites handle (s, o, R, ...).
fn pick_key(ui: &mut Ui, controller: &mut Controller, name: &str) -> bool {
    let rows = list_rows(controller);
    let Some(Overlay::Tests { index, picking, .. }) = &mut ui.overlay else { return false };
    let Some(Row::Suite(at)) = rows.get(chosen(index, &rows)) else { return false };
    let Some(suite) = controller.suites.as_deref().and_then(|suites| suites.get(*at)) else { return false };
    let names: Vec<String> = test_list(suite).iter().map(|test| test.name.clone()).collect();
    let key = suite.key.clone();
    let last = names.len().saturating_sub(1);
    let at = picking.get_or_insert(0);
    *at = (*at).min(last);
    match name {
        "h" | "LeftArrow" | "Escape" | "Tab" => *picking = None,
        "j" | "DownArrow" => *at = super::keys::wrapped(*at, 1, names.len()),
        "k" | "UpArrow" => *at = super::keys::wrapped(*at, -1, names.len()),
        "Ctrl+D" | "PageDown" => *at = (*at + PAGE).min(last),
        "Ctrl+U" | "PageUp" => *at = at.saturating_sub(PAGE),
        "g" | "Home" | "<" => *at = 0,
        "G" | "End" | ">" => *at = last,
        "r" | "Enter" => {
            if let Some(test) = names.get(*at) {
                controller.run_test(&key, test);
            }
        }
        "y" => {
            let test = names.get(*at).cloned();
            copy_test(ui, controller, &key, test.as_deref());
        }
        _ => return false,
    }
    true
}

/// A click on a side or an app runs its suites; on a suite it selects it (and leaves its tests), on a test it picks
/// it, and a double click runs it.
pub fn click(ui: &mut Ui, controller: &mut Controller, target: Option<Target>, double: bool) -> bool {
    let rows = list_rows(controller);
    let Some(Overlay::Tests { index, scroll, picking }) = &mut ui.overlay else { return false };
    let header = matches!(target, Some(Target::Item(at)) if !matches!(rows.get(at), Some(Row::Suite(_))));
    match target {
        Some(Target::Item(at)) => (*index, *scroll, *picking) = (at, 0, None),
        Some(Target::Test(at)) => *picking = Some(at),
        _ => return false,
    }
    // A header runs on the first click (the second of a double click would only say it already runs).
    if header != double {
        key(ui, controller, "Enter");
    }
    true
}

/// The wheel: over the list it chooses a suite, while picking a test it chooses one, else it scrolls the details
/// (`back`: up).
pub fn wheel(ui: &mut Ui, controller: &mut Controller, target: Option<Target>, back: bool) {
    let Some(Overlay::Tests { scroll, picking, .. }) = &mut ui.overlay else { return };
    let step = if back { "k" } else { "j" };
    if matches!(target, Some(Target::Item(_))) {
        *picking = None;
        key(ui, controller, step);
    } else if picking.is_some() {
        key(ui, controller, step);
    } else {
        *scroll = if back { scroll.saturating_sub(WHEEL) } else { *scroll + WHEEL };
    }
}

/// A suite's tests to pick from: the failed first, then the skipped, then the passed, each by name.
fn test_list(suite: &Suite) -> Vec<&TestResult> {
    let rank = |test: &TestResult| match test.outcome {
        Outcome::Failed => 0,
        Outcome::Skipped => 1,
        Outcome::Passed => 2,
    };
    let mut list: Vec<&TestResult> = suite.tests.iter().collect();
    list.sort_by(|a, b| rank(a).cmp(&rank(b)).then_with(|| a.name.cmp(&b.name)));
    list
}

/// y while picking: the test - its outcome, name, message and stack.
fn copy_test(ui: &Ui, controller: &mut Controller, key: &str, name: Option<&str>) {
    let Some(test) =
        controller.suite(key).and_then(|suite| suite.tests.iter().find(|test| Some(test.name.as_str()) == name))
    else {
        return;
    };
    let mut parts = vec![format!("{:?}: {}{}", test.outcome, test.name, took(test))];
    parts.extend([&test.message, &test.stack].into_iter().filter(|part| !part.is_empty()).cloned());
    let text = parts.join(copy::NEWLINE);
    let what = format!("the test {}", test.name);
    copy::put(ui.clipboard, controller, &text, &what);
}

fn open_log(ui: &Ui, controller: &mut Controller, key: Option<&String>) {
    match key.and_then(|key| controller.suite_log(key)) {
        Some(log) if (ui.opener)(&log.display().to_string()) => {}
        Some(log) => controller.add_log(format!("✗ Couldn't open {}", log.display()), Hue::Red),
        None => controller.add_log("No test log yet - r runs the suite.", Hue::DarkGray),
    }
}

/// y: every failure of the selected suite (or of the suites under a side or an app) - name, message, stack -
/// ready to paste.
fn copy_failures(ui: &Ui, controller: &mut Controller, keys: &[String], label: &str) {
    let failed: Vec<&TestResult> = keys
        .iter()
        .filter_map(|key| controller.suite(key))
        .flat_map(|suite| suite.tests.iter().filter(|test| test.outcome == Outcome::Failed))
        .collect();
    if failed.is_empty() {
        let text = format!("Nothing to copy: no failed tests in {label}.");
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
    let what = format!("{} failed test{} from {label}", failed.len(), if failed.len() == 1 { "" } else { "s" });
    copy::put(ui.clipboard, controller, &text, &what);
}

fn took(test: &TestResult) -> String {
    if test.ms > 0.0 { format!(" ({} ms)", test.ms.round()) } else { String::new() }
}

// ---------------------------------------------------------------- the view

pub fn rows(ui: &mut Ui, controller: &Controller, width: usize, rows: usize) -> Vec<Vec<Seg>> {
    let suites = controller.suites.as_deref().unwrap_or_default();
    let list_rows = list_rows(controller);
    let Some(Overlay::Tests { index, picking, .. }) = &mut ui.overlay else { return Vec::new() };
    let selected = chosen(index, &list_rows);
    let suite = match list_rows.get(selected) {
        Some(Row::Suite(at)) => suites.get(*at),
        _ => None,
    };
    // Picking a test while the suite has some (a run can leave it none).
    let total = suite.map_or(0, Suite::total);
    *picking = picking.filter(|_| total > 0).map(|at| at.min(total.saturating_sub(1)));
    let picking = *picking;
    let list_width = (width * 2 / 5).clamp(LIST_WIDTH.0, LIST_WIDTH.1).min(width.saturating_sub(20));
    let detail_width = width.saturating_sub(list_width + 2);
    let height = rows.saturating_sub(2);
    let list = list(ui, controller, &list_rows, selected, list_width);
    let list_top = selected.saturating_sub(height / 2).min(list.len().saturating_sub(height));
    let (detail, tail) = match (suite, picking) {
        (Some(suite), Some(at)) => (pick(ui, suite, at, detail_width, height), false),
        (Some(suite), None) => details(ui, controller, suite, detail_width),
        (None, _) if controller.suites.is_none() => (vec![vec![seg("Looking for tests…", C::DarkGray)]], false),
        (None, _) if list_rows.is_empty() => (vec![vec![seg("No test suites in this project.", C::DarkGray)]], false),
        (None, _) => (group(ui, controller, &list_rows, selected), false),
    };
    let most = detail.len().saturating_sub(height);
    let detail_top = match &mut ui.overlay {
        Some(Overlay::Tests { scroll, .. }) => {
            *scroll = (*scroll).min(most);
            if tail { most } else { *scroll }
        }
        _ => 0,
    };
    let hint = hint(width, picking.is_some());
    let mut out = vec![aim(header(ui, controller, width), Target::Box), aim(hint, Target::Box)];
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
    let count = controller.suites.iter().flatten().filter(|suite| !suite.untested).count();
    let totals = controller.test_totals();
    let (summary, color) = if let Some(running) = status(controller) {
        let building = if controller.test_build.is_some() { " · building" } else { "" };
        let at_once = controller.test_parallelism();
        (format!("{} {running}{building} · up to {at_once} at once", icon(ui, Status::Running)), C::Yellow)
    } else if totals.ran > 0 {
        let mark = |icon| icons::icon(ui.nerd, icon);
        let skipped =
            if totals.skipped > 0 { format!("   {} {}", mark(Icon::Skip), totals.skipped) } else { String::new() };
        let errors = if totals.errors > 0 {
            format!("   {} {} didn't run", mark(Icon::Warn), totals.errors)
        } else {
            String::new()
        };
        let color = if totals.failed > 0 || totals.errors > 0 { C::Red } else { C::Green };
        (
            format!("{} {}   {} {}{skipped}{errors}", mark(Icon::Ok), totals.passed, mark(Icon::Fail), totals.failed),
            color,
        )
    } else {
        ("not run yet".to_string(), C::DarkGray)
    };
    let mut segs = vec![
        on(format!(" {} Tests ", icons::icon(ui.nerd, Icon::Tests)), C::FocusText, C::FocusBack),
        seg(format!("  {count} suite{} · ", if count == 1 { "" } else { "s" }), C::DarkGray),
        seg(summary, color),
    ];
    segs.extend(watching(ui, controller));
    fit(segs, width)
}

/// The test watch, when there is one: `◉ WATCH 2`, and while a save waits for the quiet, how soon it runs.
pub fn watching(ui: &Ui, controller: &Controller) -> Option<Seg> {
    let watched = controller.test_watch.suites.len();
    if watched == 0 {
        return None;
    }
    let soon = controller
        .test_watch
        .runs_in(ui.now)
        .map_or(String::new(), |left| format!(" · rerun in {:.1}s", left.as_secs_f64()));
    Some(seg(format!("   {WATCHED} WATCH {watched}{soon}"), C::Cyan))
}

fn hint(width: usize, picking: bool) -> Vec<Seg> {
    let text = if picking {
        " j k choose a test · r run it · y copy it · f failed again · s stop · h back to the suites"
    } else {
        " j k choose · 1-9 an app · < > first / last · r run it (a side or an app: all of it) · R run all · l its \
         tests · f / F failed again · a / A watch · s stop · o log · y copy failures · Esc back"
    };
    fit(vec![seg(text, C::DarkGray)], width)
}

/// Picking a test: the suite's tests on top (around the picked one), the picked test's message and stack below.
fn pick(ui: &mut Ui, suite: &Suite, at: usize, width: usize, height: usize) -> Vec<Vec<Seg>> {
    let list = test_list(suite);
    let running = if suite.status == Status::Running { "   running…" } else { "" };
    let mut lines = vec![vec![
        seg(suite.name.clone(), C::White),
        seg(format!("   {} tests · r runs the picked one", list.len()), C::DarkGray),
        seg(running, C::Yellow),
    ]];
    let room = (height.saturating_sub(2) * 11 / 20).max(3);
    let top = at.saturating_sub(room / 2).min(list.len().saturating_sub(room));
    for (index, test) in list.iter().enumerate().skip(top).take(room) {
        let (icon, color) = outcome_mark(ui.nerd, test.outcome);
        let ms = if test.ms > 0.0 { format!("{:>7} ms", test.ms.round()) } else { String::new() };
        let room = width.saturating_sub(ms.chars().count() + 4).max(8);
        let name = pad(&end_of(&test.name, room), room);
        let row = if index == at {
            vec![on(pad(&format!(" {icon} {name} {ms}"), width), C::FocusText, C::FocusBack)]
        } else {
            vec![seg(format!(" {icon} "), color), seg(format!("{name} "), C::Gray), seg(ms, C::DarkGray)]
        };
        lines.push(aim(row, Target::Test(index)));
    }
    lines.push(vec![seg("─".repeat(width), C::DarkGray)]);
    let Some(test) = list.get(at) else { return lines };
    let (_, color) = outcome_mark(ui.nerd, test.outcome);
    let mut out = Details { ui, lines, width };
    out.add(&test.name, C::White, 0);
    out.add(&format!("{:?}{}", test.outcome, took(test)), color, 0);
    for line in test.message.lines().filter(|line| !line.trim().is_empty()) {
        out.add(line.trim_end(), C::Gray, 2);
    }
    for frame in own_frames(&test.stack).into_iter().take(PICKED_STACK_LINES) {
        out.add(frame.trim(), C::DarkGray, 2);
    }
    out.lines
}

fn outcome_mark(nerd: bool, outcome: Outcome) -> (&'static str, C) {
    match outcome {
        Outcome::Failed => (icons::icon(nerd, Icon::Fail), C::Red),
        Outcome::Skipped => (icons::icon(nerd, Icon::Skip), C::DarkGray),
        Outcome::Passed => (icons::icon(nerd, Icon::Ok), C::Green),
    }
}

/// The end of a long name (it says the most): "…Tests.CartTests.Totals".
fn end_of(text: &str, cells: usize) -> String {
    let count = text.chars().count();
    if count <= cells {
        return text.to_string();
    }
    let rest: String = text.chars().skip(count + 1 - cells.max(1)).collect();
    format!("…{rest}")
}

/// The list's rows: sides and apps (the pointer on one says a click runs it), and suites.
fn list(ui: &Ui, controller: &Controller, rows: &[Row], selected: usize, width: usize) -> Vec<Vec<Seg>> {
    let suites = controller.suites.as_deref().unwrap_or_default();
    let mut out = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let hovered = ui.hover == Some(Target::Item(index)) && index != selected;
        let line = match row {
            Row::Suite(at) => {
                let Some(suite) = suites.get(*at) else { continue };
                let summary = summary(suite);
                let room = width.saturating_sub(summary.chars().count() + 9).max(4);
                let name = pad(&cut(&suite.name, room), room);
                let icon = icon(ui, suite.status);
                let watched = if controller.test_watch.suites.contains(&suite.key) { WATCHED } else { " " };
                if index == selected {
                    let row = format!("  {watched}▸ {icon} {name} {summary}");
                    vec![on(pad(&row, width), C::FocusText, C::FocusBack)]
                } else {
                    let color = tone(suite);
                    let name_color = if hovered { C::White } else { C::Gray };
                    vec![
                        seg(format!("   {watched}"), C::Cyan),
                        seg(format!(" {icon} "), color),
                        seg(format!("{name} "), name_color),
                        seg(summary, color),
                    ]
                }
            }
            Row::Untested { .. } => {
                let text = format!("     {}", title(controller, row));
                if index == selected {
                    vec![on(pad(&text, width), C::FocusText, C::FocusBack)]
                } else {
                    vec![seg(text, C::DarkGray)]
                }
            }
            header => {
                let keys = under(controller, rows, index);
                let (summary, color) = group_summary(controller, &keys);
                let summary = if hovered { "▶ click to run".to_string() } else { summary };
                // An app's number jumps to it (1-9).
                let number = rows[..index].iter().filter(|row| matches!(row, Row::App { .. })).count() + 1;
                let indent = match header {
                    Row::Side { .. } => " ".to_string(),
                    _ if number <= 9 => format!(" {number} "),
                    _ => "   ".to_string(),
                };
                let room = width.saturating_sub(summary.chars().count() + indent.len() + 2).max(4);
                let name = pad(&cut(&title(controller, header), room), room);
                if index == selected {
                    vec![on(pad(&format!("{indent}{name} {summary}"), width), C::FocusText, C::FocusBack)]
                } else {
                    let tone = if matches!(header, Row::Side { .. }) { C::DarkCyan } else { C::Cyan };
                    let mut name = seg(format!("{indent}{name} "), if hovered { C::White } else { tone });
                    name.underline = hovered;
                    vec![name, seg(summary, if hovered { C::White } else { color })]
                }
            }
        };
        out.push(aim(line, Target::Item(index)));
    }
    out
}

/// Cut to `cells`, with "…" when it was longer.
fn cut(text: &str, cells: usize) -> String {
    if text.chars().count() > cells { format!("{}…", take_cells(text, cells.saturating_sub(1))) } else { text.into() }
}

/// What a side or app row says about its suites: running, failed, passed, or not run.
fn group_summary(controller: &Controller, keys: &[String]) -> (String, C) {
    let suites: Vec<&Suite> = keys.iter().filter_map(|key| controller.suite(key)).collect();
    let running = suites.iter().filter(|suite| suite.is_active()).count();
    let unpacked = suites.iter().filter(|suite| suite.needs_packages.is_some()).count();
    let failed: usize = suites.iter().map(|suite| suite.failed).sum();
    let errors = suites.iter().filter(|suite| suite.status == Status::Error).count();
    let passed: usize = suites.iter().map(|suite| suite.passed).sum();
    if running > 0 {
        (format!("{running} running"), C::Yellow)
    } else if unpacked > 0 {
        ("needs packages".into(), C::Yellow)
    } else if failed > 0 {
        (format!("{failed} failed"), C::Red)
    } else if errors > 0 {
        (format!("{errors} didn't run"), C::Red)
    } else if suites.iter().any(|suite| suite.last_run.is_some()) {
        (format!("{passed} passed"), C::Green)
    } else {
        ("not run".into(), C::DarkGray)
    }
}

/// A side or an app selected: its suites and how they did.
fn group(ui: &Ui, controller: &Controller, rows: &[Row], at: usize) -> Vec<Vec<Seg>> {
    if let Some(row @ Row::Untested { .. }) = rows.get(at) {
        let mut lines = vec![
            vec![seg(title(controller, row), C::White)],
            vec![seg("A test target, but no *.spec.* or *.test.* files: nothing to run.", C::DarkGray)],
            Vec::new(),
        ];
        lines.extend(untested(controller, row).into_iter().map(|name| vec![seg(format!("· {name}"), C::DarkGray)]));
        return lines;
    }
    let keys = under(controller, rows, at);
    let label = rows.get(at).map(|row| title(controller, row)).unwrap_or_default();
    let count = keys.len();
    let mut lines = vec![
        vec![seg(label, C::White)],
        vec![seg(
            format!(
                "{count} suite{} · r runs them all · f their failed tests again",
                if count == 1 { "" } else { "s" }
            ),
            C::DarkGray,
        )],
        Vec::new(),
    ];
    for suite in keys.iter().filter_map(|key| controller.suite(key)) {
        let color = tone(suite);
        lines.push(vec![
            seg(format!("{} ", icon(ui, suite.status)), color),
            seg(format!("{}  ", suite.name), C::Gray),
            seg(summary(suite), color),
        ]);
    }
    lines
}

fn icon(ui: &Ui, status: Status) -> String {
    match status {
        Status::Running if ui.reduced_motion => "●".into(),
        Status::Running => spinner(ui).to_string(),
        Status::Idle => "·".into(),
        Status::Queued => icons::icon(ui.nerd, Icon::Queued).into(),
        Status::Passed => icons::icon(ui.nerd, Icon::Ok).into(),
        Status::Failed => icons::icon(ui.nerd, Icon::Fail).into(),
        Status::Error => icons::icon(ui.nerd, Icon::Warn).into(),
        Status::Cancelled => icons::icon(ui.nerd, Icon::Cancelled).into(),
    }
}

/// A suite's colour in the list: its status's, yellow while it needs packages.
fn tone(suite: &Suite) -> C {
    if suite.needs_packages.is_some() && !suite.is_active() { C::Yellow } else { color(suite.status) }
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
    if suite.needs_packages.is_some() && !suite.is_active() {
        return "needs packages".into();
    }
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
    if let Some(problem) = suite.needs_packages.as_ref().filter(|_| !suite.is_active()) {
        out.add(&format!("! {problem} - i installs the packages"), C::Yellow, 0);
    }
    match suite.status {
        Status::Running => {
            let only = match suite.rerun.as_slice() {
                [test] => format!(" · {}", end_of(test, 60)),
                [] => String::new(),
                _ => " · failed tests only".to_string(),
            };
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

/// The tests on the panel's status line, once there are results or a run: "⚗ ✓438 ✗13 !1" (red when something
/// failed or didn't run), or "⚗ ⠋ testing 6 +10 queued" while they run.
pub fn badge(ui: &Ui, controller: &Controller) -> Option<(String, C)> {
    if let Some(running) = status(controller) {
        let flask = icons::icon(ui.nerd, Icon::Tests);
        return Some((format!("{flask} {} {running}", icon(ui, Status::Running)), C::Yellow));
    }
    let totals = controller.test_totals();
    if totals.ran == 0 {
        return None;
    }
    let errors = if totals.errors > 0 { format!(" !{}", totals.errors) } else { String::new() };
    let color = if totals.failed > 0 || totals.errors > 0 { C::Red } else { C::Green };
    let flask = icons::icon(ui.nerd, Icon::Tests);
    Some((format!("{flask} ✓{} ✗{}{errors}", totals.passed, totals.failed), color))
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
