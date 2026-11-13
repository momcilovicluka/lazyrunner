//! `.` repeats the last action that changed something (#103), as in vim: a start, stop, restart or build (on the
//! apps it acted on then, not on what has the focus now), npm ci or install, a task, a test run (the same suites, or
//! the same test), a request sent again, applying the pending migrations (asking again, as it does). Not views,
//! scrolling, searches or settings. The panel's hints say what it would do (`. restart Orders`), the palette lists it
//! first; with nothing to repeat `.` shakes the hints.

use lazyrunner_core::output::Hue;
use lazyrunner_core::run::Controller;
use lazyrunner_core::run::requests::Request;

use super::Ui;
use super::actions::{self, Run};

/// What `.` does again.
#[derive(Clone, Debug)]
pub enum Again {
    /// An action from the list; the app it acted on, by its key (none: every app).
    Action { run: Run, app: Option<String> },
    /// These suites run (only their failed tests).
    Tests { keys: Vec<String>, failed_only: bool },
    /// One test of a suite.
    Test { suite: String, test: String },
    /// A request sent again, to this path.
    Request { request: Box<Request>, path: String },
    /// The pending migrations of these contexts applied.
    Migrations(Vec<String>),
}

/// The actions `.` repeats: they change something, and they run on their own.
fn repeatable(run: &Run) -> bool {
    matches!(
        run,
        Run::StartFocused
            | Run::StopFocused
            | Run::RestartFocused
            | Run::BuildFocused
            | Run::StartAll
            | Run::StopAll
            | Run::RestartAll
            | Run::BuildAll
            | Run::NpmCi
            | Run::NpmInstall
            | Run::Task(_)
    )
}

/// Notes an action as it runs from a key, the palette or a menu - Space as the start or stop it is now.
pub fn note(ui: &mut Ui, controller: &Controller, run: &Run) {
    let focused = controller.apps.get(ui.focus).map(|app| app.key.clone());
    let run = match run {
        Run::Toggle if focused.as_ref().is_some_and(|key| controller.is_running(key)) => Run::StopFocused,
        Run::Toggle => Run::StartFocused,
        other => other.clone(),
    };
    if !repeatable(&run) {
        return;
    }
    let on_focused = matches!(
        run,
        Run::StartFocused | Run::StopFocused | Run::RestartFocused | Run::BuildFocused | Run::NpmCi | Run::NpmInstall
    );
    ui.again = Some(Again::Action { run, app: focused.filter(|_| on_focused) });
}

fn name(controller: &Controller, key: &str) -> String {
    controller.app(key).map_or_else(|| key.to_string(), |app| app.name.clone())
}

/// What `.` would do, as the hints and the palette say it: `restart Orders`, `run 2 suites`.
pub fn said(ui: &Ui, controller: &Controller) -> Option<String> {
    Some(match ui.again.as_ref()? {
        Again::Action { run, app } => {
            let on = app.as_deref().map_or("all".to_string(), |key| name(controller, key));
            match run {
                Run::StartFocused | Run::StartAll => format!("start {on}"),
                Run::StopFocused | Run::StopAll => format!("stop {on}"),
                Run::RestartFocused | Run::RestartAll => format!("restart {on}"),
                Run::BuildFocused | Run::BuildAll => format!("build {on}"),
                Run::NpmCi => format!("npm ci {on}"),
                Run::NpmInstall => format!("npm install {on}"),
                Run::Task(key) => {
                    let task = controller.project.tasks.iter().find(|task| task.key == *key);
                    format!("run {}", task.map_or(key.as_str(), |task| task.name.as_str()))
                }
                _ => return None,
            }
        }
        Again::Tests { keys, failed_only } => {
            let what = if *failed_only { "the failed tests of " } else { "" };
            match keys.as_slice() {
                [one] => {
                    format!("run {what}{}", controller.suite(one).map_or(one.as_str(), |suite| suite.name.as_str()))
                }
                many => format!("run {what}{} suites", many.len()),
            }
        }
        Again::Test { test, .. } => format!("run {test}"),
        Again::Request { request, path } => format!("send {} {path}", request.method),
        Again::Migrations(_) => "apply the migrations".to_string(),
    })
}

/// `.`: the last action again, on what it acted on then; with none, the hints shake.
pub fn repeat(ui: &mut Ui, controller: &mut Controller) {
    let Some(again) = ui.again.clone() else {
        ui.shake_at = Some(ui.now);
        controller
            .add_log("Nothing to repeat yet: . runs the last start, stop, test run or the like again.", Hue::DarkGray);
        return;
    };
    let said = said(ui, controller).unwrap_or_default();
    match again {
        Again::Action { run, app } => {
            if let Some(key) = app {
                let Some(at) = controller.apps.iter().position(|each| each.key == key) else {
                    controller.add_log(format!("Can't repeat: {key} is no longer in the profile."), Hue::DarkGray);
                    return;
                };
                ui.focus = at;
            }
            let safe = actions::actions(ui, controller).iter().any(|action| action.run == run && action.safe);
            if controller.building.is_some() && !safe {
                controller.add_log(format!("Building - press S to cancel it first, then . to {said}"), Hue::Yellow);
                return;
            }
            controller.add_log(format!(". {said}"), Hue::DarkGray);
            super::keys::run(ui, controller, &run);
        }
        Again::Tests { keys, failed_only } => controller.run_tests(&keys, failed_only),
        Again::Test { suite, test } => controller.run_test(&suite, &test),
        Again::Request { request, path } => super::requests::send_again(ui, controller, &request, &path),
        Again::Migrations(keys) => super::migrations::apply_pending(ui, controller, &keys),
    }
}
