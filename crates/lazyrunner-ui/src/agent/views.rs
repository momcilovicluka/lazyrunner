//! What an agent reads of the screens that had no tool of their own: the durations (`D`), the timeline (`~`), an
//! app's start sheet (`x`, `e`), the startup checks, the marks (`'`) and the changelog. Following, each opens its
//! screen ([`crate::screen::follow`]).

use std::time::Instant;

use lazyrunner_core::changelog::{self, CHANGELOG, VERSION};
use lazyrunner_core::output::Level;
use lazyrunner_core::run::Controller;
use lazyrunner_core::run::history;
use serde_json::{Value, json};

use super::Answer;
use crate::screen::actions::Run;
use crate::screen::{Overlay, Ui};

/// The runs of each item the durations give.
const RUNS: usize = 10;
/// The timeline's lines: by default, and at most.
const TIMELINE_LINES: usize = 100;
const MOST_TIMELINE_LINES: usize = 500;
/// The changelog's entries: by default, and at most.
const CHANGELOG_ENTRIES: usize = 3;
const MOST_CHANGELOG_ENTRIES: usize = 20;

fn tenths(seconds: f64) -> f64 {
    (seconds * 10.0).round() / 10.0
}

/// The durations (`D`): builds, starts, installs, test suites and tasks, each with its last runs, its usual time and
/// how the last compares (`kind` narrows them: "build", "start", "test"...).
pub fn durations(controller: &Controller, args: &Value) -> Value {
    let wanted = args["kind"].as_str().map(str::to_lowercase);
    let kinds: Vec<Value> = controller
        .durations
        .grouped()
        .into_iter()
        .filter(|(kind, _)| wanted.as_ref().is_none_or(|wanted| kind.title().to_lowercase().contains(wanted.as_str())))
        .map(|(kind, items)| {
            let items: Vec<Value> = items
                .iter()
                .map(|item| {
                    let runs: Vec<Value> = item
                        .runs
                        .iter()
                        .rev()
                        .take(RUNS)
                        .map(|run| json!({ "at": run.at, "seconds": tenths(run.sec), "ok": run.ok }))
                        .collect();
                    let trend = history::trend(&item.runs);
                    json!({
                        "key": item.key,
                        "label": item.label,
                        "usual_seconds": controller.durations.usual(kind, &item.key).map(tenths),
                        "last_vs_usual": trend.and_then(|trend| trend.change).map(|change| (change * 100.0).round() / 100.0),
                        "runs": runs,
                    })
                })
                .collect();
            json!({ "kind": kind.title(), "items": items })
        })
        .collect();
    json!({ "kinds": kinds, "hint": "runs newest first; last_vs_usual +0.18 is 18% slower than usual" })
}

/// The timeline (`~`): every app's output in the order it came, each line after how long ago and its app; `app`,
/// `grep` and `errors_only` narrow it, `count` lines (the newest).
pub fn timeline(ui: &Ui, controller: &Controller, args: &Value) -> Answer {
    let only = match args["app"].as_str() {
        Some(_) => Some(super::app_index(controller, args)?),
        None => None,
    };
    let grep = args["grep"].as_str().map(str::to_lowercase);
    let errors_only = args["errors_only"].as_bool().unwrap_or(false);
    let count = args["count"].as_u64().map_or(TIMELINE_LINES, |count| (count as usize).clamp(1, MOST_TIMELINE_LINES));
    let mut lines: Vec<(Instant, usize, String)> = controller
        .states
        .iter()
        .enumerate()
        .filter(|(app, _)| only.is_none_or(|only| only == *app))
        .flat_map(|(app, state)| state.output.newest_first(0).map(move |line| (app, line)))
        .filter(|(_, line)| !errors_only || line.severity() == Level::Error)
        .filter(|(_, line)| grep.as_ref().is_none_or(|grep| line.text.to_lowercase().contains(grep.as_str())))
        .map(|(app, line)| (line.born, app, line.text.clone()))
        .collect();
    lines.sort_by_key(|(born, _, _)| *born);
    let first = lines.len().saturating_sub(count);
    let text: Vec<String> = lines[first..]
        .iter()
        .map(|(born, app, text)| {
            let ago = ui.now.saturating_duration_since(*born).as_secs_f64();
            format!("-{ago:.1}s {} | {text}", controller.apps[*app].key)
        })
        .collect();
    Ok(json!({ "matching": lines.len(), "shown": text.len(), "text": text.join("\n") }))
}

/// An app's start sheet (`x`, `e`): what it's started with - its command line, its launch profile or its profile's
/// fields - and its environment with where each variable comes from, secrets masked. Without `app`, the focused one.
pub fn sheet(ui: &Ui, controller: &Controller, args: &Value) -> Answer {
    let app = match args["app"].as_str() {
        Some(_) => super::app_index(controller, args)?,
        None => ui.focus.min(controller.apps.len().saturating_sub(1)),
    };
    let text = crate::screen::sheet::copied(controller, app);
    Ok(json!({ "app": controller.apps[app].key, "text": text }))
}

/// The startup checks: the last checklist's results, or (`rerun`) the checks run again - the checklist opens and
/// fills in; call again for what they found.
pub fn checks(ui: &mut Ui, controller: &mut Controller, args: &Value) -> Value {
    let open = matches!(ui.overlay, Some(Overlay::Checklist(_)));
    if args["rerun"].as_bool() == Some(true) && !open {
        crate::screen::keys::run(ui, controller, &Run::Checklist);
    }
    let (items, ago) = match &ui.overlay {
        Some(Overlay::Checklist(list)) => (list.items.clone(), None),
        _ => match &ui.last_checks {
            Some((at, items)) => (items.clone(), Some(ui.now.saturating_duration_since(*at).as_secs())),
            None => (Vec::new(), None),
        },
    };
    let running = matches!(ui.overlay, Some(Overlay::Checklist(_)));
    let checks: Vec<Value> = items
        .iter()
        .map(|item| {
            let status = format!("{:?}", item.status).to_lowercase();
            json!({ "name": item.name, "status": status, "detail": item.detail })
        })
        .collect();
    let hint = match () {
        _ if running => "the checks run now: call again (without rerun) for what they found",
        _ if checks.is_empty() => "no checklist has run yet: rerun: true runs the checks",
        _ => "the last checklist's results: rerun: true runs them again",
    };
    json!({ "running": running, "seconds_ago": ago, "checks": checks, "hint": hint })
}

/// The marks (`'`): the lines the user marked in the columns, oldest first - what they want looked at.
pub fn marks(controller: &Controller) -> Value {
    let marks: Vec<Value> = crate::screen::marks::all(controller)
        .into_iter()
        .map(|(app, number)| {
            let text = controller.states[app].output.numbered(number, number).next().map(|line| line.text.clone());
            json!({ "app": controller.apps[app].key, "line": number, "text": text })
        })
        .collect();
    json!({ "marks": marks })
}

/// The changelog: what the newest versions changed (`count` of them), and the version that runs.
pub fn changelog(args: &Value) -> Value {
    let count =
        args["count"].as_u64().map_or(CHANGELOG_ENTRIES, |count| (count as usize).clamp(1, MOST_CHANGELOG_ENTRIES));
    let entries: Vec<Value> = changelog::entries(CHANGELOG, None, None)
        .into_iter()
        .take(count)
        .map(|entry| json!({ "version": entry.version, "date": entry.date, "lines": entry.lines }))
        .collect();
    json!({ "running": VERSION, "entries": entries })
}
