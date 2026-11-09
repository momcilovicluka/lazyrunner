//! What a change affects, in the tests view (#78): `v`'s fourth list keeps every suite but dims those nothing they
//! depend on changed for, each affected one ends in a `●` (a `?` when it can't be told: it runs with them), Alt+r runs
//! them, and the details say why - the projects from the suite to the changed one, then the file, hop by hop.

use std::collections::{BTreeSet, HashMap};
use std::time::{Duration, Instant};

use lazyrunner_core::links;
use lazyrunner_core::run::Controller;
use lazyrunner_core::run::impact::{Impact, Why};
use lazyrunner_core::run::suites::Suite;

use super::mouse::Target;
use super::seg::{Seg, band, on, seg};
use super::suites::Only;
use super::theme::C;
use super::{Overlay, Ui};

/// An affected suite's mark, and one it can't be told for.
const AFFECTED: &str = "●";
const UNKNOWN: &str = "?";
const ACCENT: C = C::Magenta;
/// The marks' cells at a row's end.
pub const MARK_CELLS: usize = 2;
/// The left-out rows fade this long as the list switches to the affected suites.
const FADE: Duration = Duration::from_millis(100);
/// A suite that becomes affected while the view is open: a sweep across its row, then its mark lands bright.
const SWEEP: Duration = Duration::from_millis(300);
const LAND: Duration = Duration::from_millis(150);
const BAND: usize = 6;
/// The why assembles a hop at a time.
const HOP: Duration = Duration::from_millis(60);
/// Hops a why has at most, for how long it assembles.
const MOST_HOPS: u32 = 8;
/// Reasons shown in the details, at most.
const WHYS_SHOWN: usize = 4;

/// What the tests view saw of what changes affect.
#[derive(Debug, Default)]
pub struct Seen {
    affected: Option<BTreeSet<String>>,
    landed: HashMap<String, Instant>,
    since: Option<Instant>,
    /// The selection goes to the first affected suite once that's known.
    pub seek: bool,
    why: Option<(String, Instant)>,
}

/// While the tests view is open, what changes affect is kept fresh.
pub fn keep_fresh(ui: &Ui, controller: &mut Controller) {
    if matches!(ui.overlay, Some(Overlay::Tests { .. })) {
        controller.want_impact();
        controller.want_causes();
    }
    super::code::keep_fresh(ui, controller);
}

/// `v` switched to the affected suites: the others fade, and the selection goes to the first affected one.
pub fn switched(ui: &mut Ui) {
    ui.tests_impact.since = Some(ui.now);
    ui.tests_impact.seek = true;
}

/// Each frame: the suites that became affected while the view is open land.
pub fn track(ui: &mut Ui, controller: &Controller) {
    if !controller.impacts_known() {
        return;
    }
    let now: BTreeSet<String> = controller.affected().into_iter().collect();
    let seen = &mut ui.tests_impact;
    if let Some(before) = &seen.affected
        && ui.tests_only == Only::Affected
        && !ui.reduced_motion
    {
        for key in now.difference(before) {
            seen.landed.insert(key.clone(), ui.now);
        }
    }
    seen.landed.retain(|_, at| ui.now.saturating_duration_since(*at) < SWEEP + LAND);
    seen.affected = Some(now);
}

/// Whether what changes affect runs this suite.
pub fn runs(controller: &Controller, key: &str) -> bool {
    controller.impact(key).is_some_and(Impact::runs)
}

/// The affected list's chip: "affected 2 of 6", or that nothing changed.
pub fn chip(controller: &Controller) -> String {
    if !controller.impacts_known() {
        return "affected · looking".into();
    }
    let total = controller.suites.iter().flatten().filter(|suite| !suite.untested).count();
    match controller.affected().len() {
        0 => "nothing changed since the last runs".into(),
        count => format!("affected {count} of {total}"),
    }
}

/// A suite's mark at its row's end: `●`, `?`, or room; after a landing's sweep, bright for a moment.
pub fn mark(ui: &Ui, controller: &Controller, key: &str, selected: bool) -> Vec<Seg> {
    let (text, color) = match controller.impact(key) {
        Some(Impact::Affected(_)) => (AFFECTED, ACCENT),
        Some(Impact::Unknown(_)) => (UNKNOWN, C::Yellow),
        _ => (" ", C::Gray),
    };
    let landing = ui.tests_impact.landed.get(key).map(|at| ui.now.saturating_duration_since(*at));
    let back = selected.then_some(C::FocusBack);
    let plain = |text: &str, color: C| match back {
        Some(back) => on(text, color, back),
        None => seg(text, color),
    };
    match landing {
        Some(since) if since < SWEEP => vec![plain("  ", C::Gray)],
        Some(_) => vec![plain(" ", C::Gray), on(text, C::White, color)],
        None => vec![plain(&format!(" {text}"), if selected { C::FocusText } else { color })],
    }
}

/// A row left out of the affected suites: dimmed, after a short fade as the list switches.
pub fn dimmed(ui: &Ui, row: Vec<Seg>) -> Vec<Seg> {
    let fading = ui.tests_impact.since.is_some_and(|since| ui.now.saturating_duration_since(since) < FADE);
    let color = if fading && !ui.reduced_motion { C::Gray } else { C::DarkGray };
    row.into_iter().map(|part| Seg { fg: color, bg: None, ..part }).collect()
}

/// A row that just became affected, with the sweep across it.
pub fn swept(ui: &Ui, key: &str, row: Vec<Seg>, width: usize) -> Vec<Seg> {
    let Some(since) = ui.tests_impact.landed.get(key).map(|at| ui.now.saturating_duration_since(*at)) else {
        return row;
    };
    if since >= SWEEP {
        return row;
    }
    let at = (since.as_secs_f64() / SWEEP.as_secs_f64() * (width + BAND) as f64) as usize;
    band(row, at.saturating_sub(BAND), at, C::Black, ACCENT)
}

/// Whether something about what changes affect moves: frames keep coming until it's still.
pub fn animating(ui: &Ui) -> bool {
    if ui.reduced_motion || !matches!(ui.overlay, Some(Overlay::Tests { .. })) {
        return false;
    }
    let seen = &ui.tests_impact;
    let within = |at: Option<Instant>, long: Duration| at.is_some_and(|at| ui.now.saturating_duration_since(at) < long);
    !seen.landed.is_empty() || within(seen.since, FADE) || within(seen.why.as_ref().map(|(_, at)| *at), HOP * MOST_HOPS)
}

/// The details' block under the run cells: why the suite runs for what changed, or that it can't be told.
pub fn why(ui: &mut Ui, controller: &Controller, suite: &Suite, width: usize) -> Vec<Vec<Seg>> {
    let Some(impact) = controller.impact(&suite.key) else { return Vec::new() };
    let started = match &ui.tests_impact.why {
        Some((key, at)) if *key == suite.key => *at,
        _ => {
            ui.tests_impact.why = Some((suite.key.clone(), ui.now));
            ui.now
        }
    };
    let hops = if ui.reduced_motion {
        usize::MAX
    } else {
        1 + (ui.now.saturating_duration_since(started).as_millis() / HOP.as_millis()) as usize
    };
    match impact {
        Impact::Unchanged if ui.tests_only == Only::Affected => {
            vec![vec![seg("Nothing it depends on changed since its last run.", C::DarkGray)]]
        }
        Impact::Unchanged => Vec::new(),
        Impact::Unknown(reason) => {
            let told = format!("{UNKNOWN} can't tell: {reason} - it runs with the affected ones");
            vec![
                vec![seg("AFFECTED?", C::Yellow)],
                ui.scrolling_row(vec![seg("  ", C::Gray), seg(told, C::Yellow)], 1, width),
            ]
        }
        Impact::Affected(whys) => {
            let mut lines = vec![vec![seg("AFFECTED BECAUSE", ACCENT)]];
            for why in whys.iter().take(WHYS_SHOWN) {
                lines.extend(reason(ui, controller, why, hops, width));
            }
            if whys.len() > WHYS_SHOWN {
                lines.push(vec![seg(format!("  +{} more", whys.len() - WHYS_SHOWN), C::DarkGray)]);
            }
            lines
        }
    }
}

/// One reason: the projects from the suite to the changed one (+ how many more files), then the file, a link.
fn reason(ui: &mut Ui, controller: &Controller, why: &Why, hops: usize, width: usize) -> Vec<Vec<Seg>> {
    let chain: Vec<String> =
        if why.chain.is_empty() { vec!["every .NET suite".to_string()] } else { why.chain.clone() };
    let shown = chain.iter().take(hops).cloned().collect::<Vec<_>>().join(" ← ");
    let more = match why.files.len() {
        0 | 1 => String::new(),
        files => format!("  +{} more", files - 1),
    };
    let mut head = vec![seg("  ", C::Gray), seg(shown, C::White)];
    if hops > chain.len() {
        head.push(seg(more, C::DarkGray));
    }
    let mut lines = vec![ui.scrolling_row(head, 1, width)];
    let Some(file) = why.files.first().filter(|_| hops > chain.len()) else { return lines };
    ui.links.push(links::Target::File {
        path: controller.root.join(file).display().to_string(),
        line: None,
        column: None,
    });
    let mut link = seg(file.clone(), C::Gray).at(Target::Link { column: ui.focus, link: ui.links.len() - 1 });
    link.underline = true;
    lines.push(ui.scrolling_row(vec![seg("    ← ", C::DarkGray), link], 1, width));
    lines
}
