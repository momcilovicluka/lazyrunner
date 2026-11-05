//! What's new since you last looked: chips in the panel's header - `H 3 new errors`, `T 2 suites done · 1 red`,
//! `M 1 pending`, `A 3 new findings · 1 critical` - for what happened in a view since it was last open; and, while a
//! view is closed, what it's busy with in the background (`A ⠋ scanning 2 of 5`, `M ⠋ checking · 2 left`). Its key or a click on the chip opens the view
//! on the first new thing, and having it open clears the chip. Each view keeps a mark of what it had when it was last
//! open - the newest request's number, each suite's last finished run, the pending migrations - and until it has
//! been open, of what was there when lazyrunner first knew it.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Local};
use lazyrunner_core::run::Controller;
use lazyrunner_core::run::audit::Severity;
use lazyrunner_core::run::requests::Request;
use lazyrunner_core::run::suites::{Status, Suite};

use super::jumps::View;
use super::motion::Spot;
use super::suites::Row;
use super::theme::C;
use super::{Overlay, Ui, audit, migrations, requests, suites};

#[derive(Debug, Default)]
pub struct Seen {
    /// The newest request's number when the requests view was last open.
    requests: Option<u64>,
    /// Each suite's last finished run then.
    suites: Option<HashMap<String, Option<DateTime<Local>>>>,
    /// The pending migrations then: their context's key and their id.
    pending: Option<HashSet<(String, String)>>,
    /// The audit's findings then: each vulnerable package's app and name.
    findings: Option<HashSet<(String, String)>>,
    /// The chips shown last frame: a new one pops in.
    shown: Vec<&'static str>,
}

/// A view's news, as its chip says it.
pub struct Chip {
    pub key: &'static str,
    pub label: String,
    pub color: C,
    pub view: View,
}

fn open_view(ui: &Ui) -> Option<View> {
    // A box opened over a view keeps the view in `behind`: it's still open.
    [ui.overlay.as_ref(), ui.behind.as_ref()].into_iter().flatten().find_map(|overlay| match overlay {
        Overlay::Requests(_) => Some(View::Requests),
        Overlay::Tests { .. } => Some(View::Tests),
        Overlay::Migrations { .. } => Some(View::Migrations),
        Overlay::Audit { .. } => Some(View::Audit),
        _ => None,
    })
}

fn findings(controller: &Controller) -> HashSet<(String, String)> {
    let scans = controller.audits().iter();
    scans
        .flat_map(|(app, scan)| {
            let vulnerable = scan.packages.iter().filter(|package| package.severity.is_some());
            vulnerable.map(move |package| (app.clone(), package.name.clone()))
        })
        .collect()
}

fn finished(controller: &Controller) -> HashMap<String, Option<DateTime<Local>>> {
    controller.suites.iter().flatten().map(|suite| (suite.key.clone(), suite.last_run)).collect()
}

fn pending(controller: &Controller) -> HashSet<(String, String)> {
    let contexts = controller.contexts.iter().flatten();
    contexts
        .flat_map(|context| context.pending().into_iter().map(|migration| (context.key(), migration.id.clone())))
        .collect()
}

/// Each frame: an open view marks what it has as seen (and the first look at one does); a chip that appeared pops in.
pub fn update(ui: &mut Ui, controller: &Controller) {
    let open = open_view(ui);
    let seen = &mut ui.seen;
    if seen.requests.is_none() || open == Some(View::Requests) {
        seen.requests = Some(controller.requests.seq);
    }
    if controller.suites.is_some() && (seen.suites.is_none() || open == Some(View::Tests)) {
        seen.suites = Some(finished(controller));
    }
    if controller.contexts.is_some() && (seen.pending.is_none() || open == Some(View::Migrations)) {
        seen.pending = Some(pending(controller));
    }
    if seen.findings.is_none() || open == Some(View::Audit) {
        seen.findings = Some(findings(controller));
    }
    let keys: Vec<&'static str> = chips(ui, controller).iter().map(|chip| chip.key).collect();
    for key in keys.iter().filter(|key| !ui.seen.shown.contains(key)) {
        ui.motion.flash(Spot::Hint(format!("new {key}")), ui.now);
    }
    ui.seen.shown = keys;
}

fn new_errors<'a>(ui: &Ui, controller: &'a Controller) -> impl Iterator<Item = &'a Request> {
    let since = ui.seen.requests;
    controller
        .requests
        .items
        .iter()
        .filter(move |request| since.is_some_and(|since| request.seq > since) && request.status >= 400)
}

fn red(suite: &Suite) -> bool {
    matches!(suite.status, Status::Failed | Status::Error)
}

/// The suites that finished a run since, the red ones first.
fn done_suites<'a>(ui: &Ui, controller: &'a Controller) -> Vec<&'a Suite> {
    let (Some(seen), Some(suites)) = (&ui.seen.suites, &controller.suites) else { return Vec::new() };
    let mut done: Vec<&Suite> = suites
        .iter()
        .filter(|suite| suite.last_run.is_some() && !matches!(suite.status, Status::Running | Status::Queued))
        .filter(|suite| seen.get(&suite.key) != Some(&suite.last_run))
        .collect();
    done.sort_by_key(|suite| !red(suite));
    done
}

fn new_pending(ui: &Ui, controller: &Controller) -> Vec<(String, String)> {
    let Some(seen) = &ui.seen.pending else { return Vec::new() };
    let mut news: Vec<(String, String)> =
        pending(controller).into_iter().filter(|migration| !seen.contains(migration)).collect();
    news.sort();
    news
}

/// The audit's findings since, the worst first: their app, package and severity.
fn new_findings(ui: &Ui, controller: &Controller) -> Vec<(String, String, Severity)> {
    let Some(seen) = &ui.seen.findings else { return Vec::new() };
    let mut news: Vec<(String, String, Severity)> = controller
        .audits()
        .iter()
        .flat_map(|(app, scan)| {
            scan.packages.iter().filter_map(move |package| Some((app.clone(), package.name.clone(), package.severity?)))
        })
        .filter(|(app, name, _)| !seen.contains(&(app.clone(), name.clone())))
        .collect();
    news.sort_by_key(|(_, name, severity)| (std::cmp::Reverse(*severity), name.clone()));
    news
}

fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}

/// The views' news, a chip each.
pub fn chips(ui: &Ui, controller: &Controller) -> Vec<Chip> {
    let mut chips = Vec::new();
    let errors = new_errors(ui, controller).count();
    if errors > 0 {
        let label = format!("{errors} new error{}", plural(errors));
        chips.push(Chip { key: "H", label, color: C::Red, view: View::Requests });
    }
    let done = done_suites(ui, controller);
    if !done.is_empty() {
        let reds = done.iter().filter(|suite| red(suite)).count();
        let red_part = if reds > 0 { format!(" · {reds} red") } else { String::new() };
        let history =
            |suite: &Suite| controller.test_history.suite(&suite.key).map(|history| history.tally(&suite.tests));
        let new: usize = done.iter().filter_map(|suite| history(suite)).map(|tally| tally.new).sum();
        let new_part = if new > 0 { format!(" · {new} new failure{}", plural(new)) } else { String::new() };
        let label = format!("{} suite{} done{red_part}{new_part}", done.len(), plural(done.len()));
        let color = if reds > 0 { C::Red } else { C::Green };
        chips.push(Chip { key: "T", label, color, view: View::Tests });
    }
    let pending = new_pending(ui, controller).len();
    if pending > 0 {
        chips.push(Chip { key: "M", label: format!("{pending} pending"), color: C::Yellow, view: View::Migrations });
    }
    let found = new_findings(ui, controller);
    if let Some((_, _, worst)) = found.first() {
        let critical = found.iter().filter(|(.., severity)| *severity == Severity::Critical).count();
        let critical = if critical > 0 { format!(" · {critical} critical") } else { String::new() };
        let label = format!("{} new finding{}{critical}", found.len(), plural(found.len()));
        let color = if *worst >= Severity::High { C::Red } else { C::Yellow };
        chips.push(Chip { key: "A", label, color, view: View::Audit });
    }
    chips
}

/// What the closed views do in the background, a chip each: the audit's scans, the migrations' checks.
pub fn busy(ui: &Ui, controller: &Controller) -> Vec<Chip> {
    let open = open_view(ui);
    let spin = super::columns::spinner(ui);
    let mut chips = Vec::new();
    let scanning = controller.auditing();
    if !scanning.is_empty() && open != Some(View::Audit) {
        let label = match controller.audit_batch().filter(|(_, total)| *total > 1) {
            Some((done, total)) => format!("{spin} scanning {done} of {total}"),
            None => {
                let key = &scanning[0];
                format!("{spin} scanning {}", controller.app(key).map_or(key.as_str(), |app| app.name.as_str()))
            }
        };
        chips.push(Chip { key: "A", label, color: C::Yellow, view: View::Audit });
    }
    let left = controller.migration_jobs.len() + controller.migration_queue().len();
    if left > 0 && open != Some(View::Migrations) {
        let label = format!("{spin} checking · {left} left");
        chips.push(Chip { key: "M", label, color: C::Yellow, view: View::Migrations });
    }
    chips
}

/// Opens a view - its key, or its chip - on the first new thing in it.
pub fn open(ui: &mut Ui, controller: &mut Controller, view: View) {
    match view {
        View::Requests => match new_errors(ui, controller).map(|request| request.seq).min() {
            Some(seq) => {
                let first = requests::View { follow: false, selected: seq, ..Default::default() };
                ui.overlay = Some(Overlay::Requests(Box::new(first)));
            }
            None => requests::open(ui),
        },
        View::Tests => {
            let first = done_suites(ui, controller).first().map(|suite| suite.key.clone());
            suites::open(ui, controller);
            let all = controller.suites.as_deref().unwrap_or_default();
            let row = first.and_then(|key| {
                let at = all.iter().position(|suite| suite.key == key)?;
                suites::list_rows(controller).iter().position(|row| matches!(row, Row::Suite(suite) if *suite == at))
            });
            if let (Some(row), Some(Overlay::Tests { index, .. })) = (row, &mut ui.overlay) {
                *index = row;
            }
        }
        View::Migrations => {
            let first = new_pending(ui, controller).into_iter().next().map(|(context, _)| context);
            migrations::open(ui, controller);
            let all = controller.contexts.as_deref().unwrap_or_default();
            let at = first.and_then(|key| all.iter().position(|context| context.key() == key));
            if let (Some(at), Some(Overlay::Migrations { index, .. })) = (at, &mut ui.overlay) {
                *index = at;
            }
        }
        View::Audit => {
            let first = new_findings(ui, controller).into_iter().next().map(|(app, ..)| app);
            audit::open(ui, controller);
            let at = first.and_then(|key| controller.apps.iter().position(|app| app.key == key));
            if let (Some(at), Some(Overlay::Audit { app, .. })) = (at, &mut ui.overlay) {
                *app = at;
            }
        }
    }
}
