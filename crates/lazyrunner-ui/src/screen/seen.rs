//! What's new since you last looked: chips in the panel's header - `H 3 new errors`, `T 2 suites done · 1 red`,
//! `M 1 pending` - for what happened in a view since it was last open. Its key or a click on the chip opens the view
//! on the first new thing, and having it open clears the chip. Each view keeps a mark of what it had when it was last
//! open - the newest request's number, each suite's last finished run, the pending migrations - and until it has
//! been open, of what was there when lazyrunner first knew it.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Local};
use lazyrunner_core::run::Controller;
use lazyrunner_core::run::requests::Request;
use lazyrunner_core::run::suites::{Status, Suite};

use super::jumps::View;
use super::motion::Spot;
use super::suites::Row;
use super::theme::C;
use super::{Overlay, Ui, migrations, requests, suites};

#[derive(Debug, Default)]
pub struct Seen {
    /// The newest request's number when the requests view was last open.
    requests: Option<u64>,
    /// Each suite's last finished run then.
    suites: Option<HashMap<String, Option<DateTime<Local>>>>,
    /// The pending migrations then: their context's key and their id.
    pending: Option<HashSet<(String, String)>>,
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
        _ => None,
    })
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
        let label = format!("{} suite{} done{red_part}", done.len(), plural(done.len()));
        let color = if reds > 0 { C::Red } else { C::Green };
        chips.push(Chip { key: "T", label, color, view: View::Tests });
    }
    let pending = new_pending(ui, controller).len();
    if pending > 0 {
        chips.push(Chip { key: "M", label: format!("{pending} pending"), color: C::Yellow, view: View::Migrations });
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
    }
}
