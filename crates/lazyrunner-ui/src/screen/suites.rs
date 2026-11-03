//! T: the test suites. On the left the list, grouped by what runs them; on the right the selected suite: its live
//! output while it runs, then its failures (message and its own stack frames, links clickable), or its slowest tests
//! when all passed. Each suite's history (`run::test_history`) tells its new failures from the known ones, its flaky
//! tests and the fixed ones; every test shows its last runs as a strip, two runs a cell, the newest on the right.
//! `v` lists all the suites, those with failures, or those with flaky tests.

use std::collections::VecDeque;
use std::time::Instant;

use crossterm::event::KeyEvent;

use lazyrunner_core::links;
use lazyrunner_core::output::Hue;
use lazyrunner_core::run::Controller;
use lazyrunner_core::run::format_duration;
use lazyrunner_core::run::suites::{Outcome, Status, Suite, TestResult};
use lazyrunner_core::run::test_history::{ABSENT, Story, Tally};

use super::columns::{chunk, fit, linked, spinner};
use super::icons::{self, Icon};
use super::mouse::Target;
use super::seg::{Seg, aim, band, on, pad, seg, take_cells};
use super::theme::C;
use super::{Overlay, Ui, copy};

/// The marks before a suite's counts in the list (`✓12 ×13 ≈1`).
const PASSED_MARK: &str = "✓";
const FAILED_MARK: &str = "×";
const FLAKY_MARK: &str = "≈";
/// A test's strip of last runs, two a cell: in the details, and beside each test to pick from.
const STRIP_CELLS: usize = 10;
const PICK_STRIP_CELLS: usize = 5;
/// A suite's line of times in the list.
const LIST_LINE_CELLS: usize = 5;
/// The project's runs listed under the list, at most.
const HISTORY_SHOWN: usize = 6;

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
    (ui.tests_only, ui.tests_run, ui.tests_typing) = (Only::All, None, false);
    ui.tests_query.clear();
    ui.overlay = Some(Overlay::Tests { index: FIRST_SUITE, scroll: 0, picking: None });
}

/// The selection when the view opens: the first suite (once there is a list).
pub(super) const FIRST_SUITE: usize = usize::MAX;

/// Which suites the list shows (`v`): all, those with failures, those with flaky tests.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Only {
    #[default]
    All,
    Failed,
    Flaky,
}

impl Only {
    const EACH: [Only; 3] = [Only::All, Only::Failed, Only::Flaky];

    fn next(self) -> Self {
        match self {
            Self::All => Self::Failed,
            Self::Failed => Self::Flaky,
            Self::Flaky => Self::All,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Failed => "failed",
            Self::Flaky => "flaky",
        }
    }

    fn admits(self, controller: &Controller, suite: &Suite) -> bool {
        match self {
            Self::All => true,
            Self::Failed => suite.failed > 0 || suite.status == Status::Error,
            Self::Flaky => tally(controller, suite).flaky > 0,
        }
    }
}

/// A suite's tests as its history sums them up (nothing to sum without one).
fn tally(controller: &Controller, suite: &Suite) -> Tally {
    controller.test_history.suite(&suite.key).map(|history| history.tally(&suite.tests)).unwrap_or_default()
}

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

/// The list as `v` and `/` narrow it: the suites they admit, under the sides and apps that still have one.
fn shown_rows(controller: &Controller, only: Only, query: &str) -> Vec<Row> {
    let rows = list_rows(controller);
    if only == Only::All && query.is_empty() {
        return rows;
    }
    let suites = controller.suites.as_deref().unwrap_or_default();
    let query = query.to_lowercase();
    let admits = |suite: &Suite| only.admits(controller, suite) && suite.name.to_lowercase().contains(&query);
    (0..rows.len())
        .filter(|&at| match &rows[at] {
            Row::Suite(index) => suites.get(*index).is_some_and(admits),
            Row::Untested { .. } => false,
            Row::Side { .. } | Row::App { .. } => {
                under(controller, &rows, at).iter().filter_map(|key| controller.suite(key)).any(admits)
            }
        })
        .map(|at| rows[at].clone())
        .collect()
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

/// The countdown capsule's cells: it drains over the real quiet a save waits for (a countdown, so linear).
const CAPSULE: usize = 8;
/// How long the sweep across a suite's row takes as the watch queues it, and how wide its band is.
const SWEEP: std::time::Duration = std::time::Duration::from_millis(300);
const BAND: usize = 6;
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
    let rows = shown_rows(controller, ui.tests_only, &ui.tests_query);
    let Some(Overlay::Tests { index, scroll, picking }) = &mut ui.overlay else { return false };
    let at = chosen(index, &rows);
    let all: Vec<String> = controller.suites.iter().flatten().map(|suite| suite.key.clone()).collect();
    let selected = under(controller, &rows, at);
    let a_suite = matches!(rows.get(at), Some(Row::Suite(_)));
    let label = rows.get(at).map(|row| title(controller, row)).unwrap_or_default();
    match name {
        "Escape" if !ui.tests_query.is_empty() => {
            ui.tests_query.clear();
            (*index, *scroll, *picking) = (FIRST_SUITE, 0, None);
        }
        "/" => ui.tests_typing = true,
        "Escape" | "q" | "T" => ui.overlay = None,
        "j" | "DownArrow" => {
            *index = super::keys::wrapped(at, 1, rows.len());
            (*scroll, ui.tests_run) = (0, None);
        }
        "k" | "UpArrow" => {
            *index = super::keys::wrapped(at, -1, rows.len());
            (*scroll, ui.tests_run) = (0, None);
        }
        "l" | "RightArrow" | "Tab" => match selected.first().and_then(|key| controller.suite(key)) {
            Some(suite) if a_suite && suite.total() > 0 => *picking = Some(0),
            _ if !a_suite => controller.add_log("Choose a suite to pick one of its tests.", Hue::DarkGray),
            _ => controller.add_log("No results for this suite yet - r runs it.", Hue::DarkGray),
        },
        "v" => {
            (ui.tests_only, ui.tests_run) = (ui.tests_only.next(), None);
            (*index, *scroll, *picking) = (FIRST_SUITE, 0, None);
        }
        "[" if a_suite => {
            let runs = selected.first().and_then(|key| controller.test_history.suite(key)).map_or(0, |h| h.runs.len());
            if runs > 0 {
                ui.tests_run = Some(ui.tests_run.map_or(0, |back| (back + 1).min(runs - 1)));
            }
        }
        "]" if a_suite => ui.tests_run = ui.tests_run.and_then(|back| back.checked_sub(1)),
        "Alt+f" => controller.run_flaky(&selected),
        "<" => (*index, *scroll) = (0, 0),
        ">" => (*index, *scroll) = (rows.len().saturating_sub(1), 0),
        digit if digit.len() == 1 && ('1'..='9').contains(&digit.chars().next().unwrap_or('0')) => {
            // The apps' sections, numbered in the list.
            let nth = digit.parse::<usize>().unwrap_or(1) - 1;
            let apps = rows.iter().enumerate().filter(|(_, row)| matches!(row, Row::App { .. }));
            if let Some((at, _)) = apps.clone().nth(nth) {
                (*index, *scroll, *picking, ui.tests_run) = (at, 0, None, None);
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
    let rows = shown_rows(controller, ui.tests_only, &ui.tests_query);
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
        "a" => {
            if let Some(test) = names.get(*at) {
                controller.pin_watch(&key, test);
            }
        }
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
/// Typing the filter (`/`): the suites whose names hold it; Backspace, Enter keeps it, Esc drops it. False when it
/// isn't being typed.
pub fn typing_key(ui: &mut Ui, name: &str, key: &KeyEvent) -> bool {
    if !ui.tests_typing {
        return false;
    }
    match name {
        "Escape" => {
            ui.tests_query.clear();
            ui.tests_typing = false;
        }
        "Enter" => ui.tests_typing = false,
        "Backspace" => {
            ui.tests_query.pop();
        }
        _ => ui.tests_query.extend(super::keys::typed(key)),
    }
    if let Some(Overlay::Tests { index, scroll, picking }) = &mut ui.overlay {
        (*index, *scroll, *picking) = (FIRST_SUITE, 0, None);
    }
    true
}

pub fn click(ui: &mut Ui, controller: &mut Controller, target: Option<Target>, double: bool) -> bool {
    let rows = shown_rows(controller, ui.tests_only, &ui.tests_query);
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
    let only = ui.tests_only;
    let query = ui.tests_query.clone();
    let list_rows = shown_rows(controller, only, &query);
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
    let mut list = list(ui, controller, &list_rows, selected, list_width);
    // The project's last runs under the list, when there is room.
    let spare = height.saturating_sub(list.len() + 2);
    if spare > 0 {
        list.extend(super::suite_runs::run_history(controller, spare.min(HISTORY_SHOWN)));
    }
    let list_top = selected.saturating_sub(height / 2).min(list.len().saturating_sub(height));
    let (detail, tail) = match (suite, picking) {
        (Some(suite), Some(at)) => (pick(ui, controller, suite, at, detail_width, height), false),
        (Some(suite), None) => details(ui, controller, suite, detail_width),
        (None, _) if controller.suites.is_none() => (vec![vec![seg("Looking for tests…", C::DarkGray)]], false),
        (None, _) if list_rows.is_empty() && !query.is_empty() => {
            (vec![vec![seg(format!("No suites match /{query} - Esc shows them all."), C::DarkGray)]], false)
        }
        (None, _) if list_rows.is_empty() && only != Only::All => {
            (vec![vec![seg(format!("No {} suites - v shows them all.", only.label()), C::DarkGray)]], false)
        }
        (None, _) if list_rows.is_empty() => (vec![vec![seg("No test suites in this project.", C::DarkGray)]], false),
        (None, _) => (group(ui, controller, &list_rows, selected, detail_width), false),
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
        seg("   ", C::Gray),
    ];
    for only in Only::EACH {
        let chip = format!(" {} ", only.label());
        segs.push(if only == ui.tests_only { on(chip, C::FocusText, C::FocusBack) } else { seg(chip, C::DarkGray) });
    }
    if ui.tests_typing || !ui.tests_query.is_empty() {
        let cursor = if ui.tests_typing { ui.cursor() } else { "" };
        segs.push(seg(format!("  /{}{cursor}", ui.tests_query), C::Cyan));
    }
    segs.extend(watching(ui, controller));
    fit(segs, width)
}

/// The test watch, when there is one: `◉ WATCH 2`, and while a save waits for the quiet, how soon it runs.
pub fn watching(ui: &Ui, controller: &Controller) -> Option<Seg> {
    let watched = controller.test_watch.suites.len();
    if watched == 0 {
        return None;
    }
    let soon = match (controller.test_watch.runs_in(ui.now), controller.test_watch.quiet_left(ui.now)) {
        (Some(left), _) if ui.reduced_motion => format!(" · rerun in {:.1}s", left.as_secs_f64()),
        (_, Some(share)) => {
            let full = ((share * CAPSULE as f64).ceil() as usize).min(CAPSULE);
            format!(" · {}{}", "▰".repeat(full), "▱".repeat(CAPSULE - full))
        }
        _ => String::new(),
    };
    let pinned = match controller.test_watch.pinned.values().collect::<Vec<_>>().as_slice() {
        [] => String::new(),
        [test] => format!(" · pinned: {test}"),
        tests => format!(" · {} pinned", tests.len()),
    };
    Some(seg(format!("   {WATCHED} WATCH {watched}{pinned}{soon}"), C::Cyan))
}

/// How far through its sweep suite `key`'s row is, 0 to 1, just after the watch queued it (never with reduced
/// motion).
fn sweep(ui: &Ui, controller: &Controller, key: &str) -> Option<f64> {
    let queued = controller.test_watch.queued_at(key).filter(|_| !ui.reduced_motion)?;
    let through = ui.now.saturating_duration_since(queued).as_secs_f64() / SWEEP.as_secs_f64();
    (through < 1.0).then_some(through)
}

/// A suite's row with the watch's sweep across it, once, as the watch queues it.
fn swept(ui: &Ui, controller: &Controller, key: &str, row: Vec<Seg>, width: usize) -> Vec<Seg> {
    let Some(through) = sweep(ui, controller, key) else { return row };
    let at = (through * (width + BAND) as f64) as usize;
    band(row, at.saturating_sub(BAND), at, C::Black, C::Cyan)
}

/// Whether a watched suite's row is sweeping: frames keep coming until it's done.
pub fn sweeping(ui: &Ui, controller: &Controller) -> bool {
    controller.test_watch.suites.iter().any(|key| sweep(ui, controller, key).is_some())
}

fn hint(width: usize, picking: bool) -> Vec<Seg> {
    let text = if picking {
        " j k choose a test · r run it · a watch only it · y copy it · f failed again · s stop · h back to the suites"
    } else {
        " j k choose · 1-9 an app · < > first / last · r run it (a side or an app: all of it) · R run all · l its \
         tests · f / F failed again · Alt+f flaky again · [ ] a run · a / A watch · v all / failed / flaky · / filter · s stop · o log · y copy failures · Esc back"
    };
    fit(vec![seg(text, C::DarkGray)], width)
}

/// Picking a test: the suite's tests on top (around the picked one), the picked test's message and stack below.
fn pick(ui: &mut Ui, controller: &Controller, suite: &Suite, at: usize, width: usize, height: usize) -> Vec<Vec<Seg>> {
    let list = test_list(suite);
    let history = controller.test_history.suite(&suite.key);
    let (strip_cells, gap) = history.map_or((0, ""), |_| (PICK_STRIP_CELLS + 1, " "));
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
        let room = width.saturating_sub(ms.chars().count() + 4 + strip_cells).max(8);
        let name = pad(&end_of(&test.name, room), room);
        let marks = history.map_or_else(Vec::new, |history| strip(history.line(&test.name), PICK_STRIP_CELLS, false));
        let row = if index == at {
            let mut row = vec![on(format!(" {icon} "), C::FocusText, C::FocusBack)];
            row.extend(marks);
            let rest = width.saturating_sub(3 + strip_cells - gap.len());
            row.push(on(pad(&format!("{gap}{name} {ms}"), rest), C::FocusText, C::FocusBack));
            row
        } else {
            let mut row = vec![seg(format!(" {icon} "), color)];
            row.extend(marks);
            row.extend([seg(format!("{gap}{name} "), C::Gray), seg(ms, C::DarkGray)]);
            row
        };
        lines.push(aim(row, Target::Test(index)));
    }
    lines.push(vec![seg("─".repeat(width), C::DarkGray)]);
    let Some(test) = list.get(at) else { return lines };
    let (_, color) = outcome_mark(ui.nerd, test.outcome);
    let mut out = Details { ui, lines, width };
    out.add(&test.name, C::White, 0);
    out.add(&format!("{:?}{}", test.outcome, took(test)), color, 0);
    let story = history.map(|history| history.story(&test.name)).unwrap_or_default();
    let told = match (story.new, story.fixed, story.flaky) {
        (_, _, Some((failures, runs))) => format!("↻ flaky - it failed {failures} of its last {runs} runs"),
        (true, _, _) if test.outcome == Outcome::Failed => "a new failure: it didn't fail the run before".into(),
        (_, true, _) => "fixed: it failed the run before".into(),
        _ => String::new(),
    };
    if !told.is_empty() {
        out.add(&told, if story.flaky.is_some() { C::Yellow } else { color }, 0);
    }
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
                let summary = match controller.test_watch.pinned.get(&suite.key) {
                    Some(test) => format!("{} · {WATCHED} pinned: {test}", summary(controller, suite)),
                    None => summary(controller, suite),
                };
                let news = tally(controller, suite).new;
                let badge = if news > 0 { format!(" {news} new ") } else { String::new() };
                let beside = if badge.is_empty() { 0 } else { badge.chars().count() + 1 };
                // Its time and a line of its last times, as long as the name keeps some room.
                let roomy = width.saturating_sub(summary.chars().count() + beside + 9);
                let timed = super::suite_runs::list_line(controller, &suite.key, LIST_LINE_CELLS)
                    .map(|(line, last)| (line, format!(" {}", format_duration(last))))
                    .filter(|(_, last)| roomy >= last.chars().count() + LIST_LINE_CELLS + 1 + 10);
                let timing = timed.as_ref().map_or(0, |(_, last)| last.chars().count() + LIST_LINE_CELLS + 1);
                let room = roomy.saturating_sub(timing).max(4);
                let name = pad(&cut(&suite.name, room), room);
                let icon = icon(ui, suite.status);
                let watched = if controller.test_watch.suites.contains(&suite.key) { WATCHED } else { " " };
                let (line, last) = timed.unwrap_or_default();
                let row = if index == selected {
                    let mut row =
                        vec![on(format!("  {watched}▸ {icon} {name} {summary}{last}"), C::FocusText, C::FocusBack)];
                    if !line.is_empty() {
                        row.push(on(" ", C::FocusText, C::FocusBack));
                        row.extend(line);
                    }
                    let rest = width.saturating_sub(super::seg::width(&row));
                    row.push(on(pad(&format!(" {badge}"), rest), C::FocusText, C::FocusBack));
                    row
                } else {
                    let color = tone(suite);
                    let name_color = if hovered { C::White } else { C::Gray };
                    let mut row = vec![
                        seg(format!("   {watched}"), C::Cyan),
                        seg(format!(" {icon} "), color),
                        seg(format!("{name} "), name_color),
                        seg(summary, color),
                        seg(last, C::DarkGray),
                    ];
                    if !line.is_empty() {
                        row.push(seg(" ", C::Gray));
                        row.extend(line);
                    }
                    if !badge.is_empty() {
                        row.extend([seg(" ", C::Gray), on(badge, C::Black, C::Red)]); // failures the run before didn't have
                    }
                    row
                };
                swept(ui, controller, &suite.key, row, width)
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
fn group(ui: &Ui, controller: &Controller, rows: &[Row], at: usize, width: usize) -> Vec<Vec<Seg>> {
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
            seg(summary(controller, suite), color),
        ]);
    }
    let grid = super::suite_runs::grid(ui, controller, &keys, width);
    if !grid.is_empty() {
        lines.push(Vec::new());
        lines.extend(grid);
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

/// What the list says about a suite: once it ran, how many passed, failed and are flaky. Marks narrower than a cell,
/// which a font's Nerd Font icons aren't (they run into the number), and which Nerd Fonts have themselves.
fn summary(controller: &Controller, suite: &Suite) -> String {
    if suite.needs_packages.is_some() && !suite.is_active() {
        return "needs packages".into();
    }
    match suite.status {
        Status::Passed | Status::Failed => {
            let mut text = format!("{PASSED_MARK}{}", suite.passed);
            if suite.failed > 0 {
                text.push_str(&format!(" {FAILED_MARK}{}", suite.failed));
            }
            let flaky = tally(controller, suite).flaky;
            if flaky > 0 {
                text.push_str(&format!(" {FLAKY_MARK}{flaky}"));
            }
            text
        }
        Status::Idle => "not run".into(),
        Status::Queued if suite.note.is_empty() => "queued".into(),
        Status::Queued | Status::Error => suite.note.clone(),
        Status::Running => format!("{} done", suite.done),
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
        results(&mut out, controller, suite, &when);
    }
    (out.lines, false)
}

/// The counts, each failure (the new ones first, each with its last runs, its message and own frames), the fixed and
/// the flaky tests, the slowest tests when all passed, and the skipped.
fn results(out: &mut Details, controller: &Controller, suite: &Suite, when: &str) {
    let history = controller.test_history.suite(&suite.key);
    let tally = tally(controller, suite);
    let story = |test: &TestResult| history.map(|history| history.story(&test.name)).unwrap_or_default();
    let line = |test: &TestResult| history.map(|history| history.line(&test.name));
    let split = match (tally.new, tally.known) {
        (0, _) | (_, 0) => String::new(),
        (new, known) => format!(" ({new} new · {known} known)"),
    };
    let flaky = if tally.flaky > 0 { format!("   ↻ {} flaky", tally.flaky) } else { String::new() };
    let counts = format!(
        "✓ {} passed   ✗ {} failed{split}   – {} skipped{flaky}   · {when}",
        suite.passed, suite.failed, suite.skipped
    );
    out.add(&counts, if suite.failed > 0 { C::Red } else { C::Green }, 0);
    out.blank();
    let runs = super::suite_runs::rows(out.ui, controller, suite);
    if !runs.is_empty() {
        out.lines.extend(runs);
        out.blank();
    }
    let glint = super::suite_runs::fresh(out.ui, suite);
    let mut failed: Vec<&TestResult> = suite.tests.iter().filter(|test| test.outcome == Outcome::Failed).collect();
    failed.sort_by_key(|test| !story(test).new);
    if !failed.is_empty() {
        let mut header = vec![seg(format!("FAILURES {}", failed.len()), C::Red)];
        if tally.new > 0 {
            header.extend([seg("  ", C::Gray), on(format!(" NEW {} ", tally.new), C::Black, C::Red)]);
        }
        out.lines.push(header);
    }
    for test in failed {
        let story = story(test);
        out.lines.push(test_row(out.width, line(test), &story, test, C::Red, glint));
        if let Some((failures, runs)) = story.flaky {
            out.add(&format!("↻ flaky - it failed {failures} of its last {runs} runs"), C::Yellow, 4);
        }
        for line in test.message.lines().filter(|line| !line.trim().is_empty()).take(MESSAGE_LINES) {
            out.add(line.trim_end(), C::Gray, 4);
        }
        for frame in own_frames(&test.stack).into_iter().take(STACK_LINES) {
            out.add(frame.trim(), C::DarkGray, 4);
        }
        out.blank();
    }
    let passed = |wanted: fn(&Story) -> bool| -> Vec<&TestResult> {
        suite.tests.iter().filter(|test| test.outcome == Outcome::Passed && wanted(&story(test))).collect()
    };
    for (title, tests, color) in [
        ("Fixed since the run before", passed(|story| story.fixed), C::Green),
        ("Flaky", passed(|story| story.flaky.is_some()), C::Yellow),
    ] {
        if tests.is_empty() {
            continue;
        }
        out.add(&format!("{title} ({}):", tests.len()), C::DarkCyan, 0);
        // A test fixed by the run just finished glints with it.
        let color = if glint && color == C::Green { C::White } else { color };
        for test in tests {
            out.lines.push(test_row(out.width, line(test), &story(test), test, color, glint));
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

/// A test's row in the details: its last runs (once its suite has a history), a badge when its failure is new, its
/// mark and name.
fn test_row(width: usize, runs: Option<&str>, story: &Story, test: &TestResult, color: C, glint: bool) -> Vec<Seg> {
    let mark = match test.outcome {
        Outcome::Failed => "✗",
        Outcome::Passed if story.flaky.is_some() && !story.fixed => "↻",
        Outcome::Passed => "✓",
        Outcome::Skipped => "–",
    };
    let new = story.new && test.outcome == Outcome::Failed;
    let took = took(test);
    let strip_cells = runs.map_or(0, |_| STRIP_CELLS + 1);
    let used = strip_cells + 2 + if new { 6 } else { 0 } + took.chars().count();
    let mut row = Vec::new();
    if let Some(runs) = runs {
        row.extend(strip(runs, STRIP_CELLS, glint));
        row.push(seg(" ", C::Gray));
    }
    if new {
        row.extend([on(" NEW ", C::Black, C::Red), seg(" ", C::Gray)]);
    }
    row.push(seg(format!("{mark} {}{took}", end_of(&test.name, width.saturating_sub(used).max(8))), color));
    row
}

/// A test's last runs (its history's line), `cells` wide: two runs a cell, the older's colour on the left half and
/// the newer's on the right, the newest on the right - green passed, red failed, grey skipped, nothing where it
/// wasn't in a run. The newest glints in when `glint`.
fn strip(runs: &str, cells: usize, glint: bool) -> Vec<Seg> {
    let marks: Vec<char> = runs.chars().collect();
    let shown = &marks[marks.len().saturating_sub(cells * 2)..];
    let mut padded = vec![ABSENT; cells * 2 - shown.len()];
    padded.extend_from_slice(shown);
    let newest = run_color(padded[padded.len() - 1]).filter(|_| glint).map(|_| C::White);
    let last = padded.len() / 2 - 1;
    padded
        .chunks(2)
        .enumerate()
        .map(|(at, pair)| {
            let newer = if at == last { newest.or(run_color(pair[1])) } else { run_color(pair[1]) };
            match (run_color(pair[0]), newer) {
                (Some(older), Some(newer)) => on("▌", older, newer),
                (Some(older), None) => seg("▌", older),
                (None, Some(newer)) => seg("▐", newer),
                (None, None) => seg(" ", C::Gray),
            }
        })
        .collect()
}

/// A run's colour in a strip; none where the test wasn't in it.
fn run_color(mark: char) -> Option<C> {
    match mark {
        'p' => Some(C::Green),
        'f' => Some(C::Red),
        's' => Some(C::DarkGray),
        _ => None,
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
