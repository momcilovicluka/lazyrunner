//! A pull on the database screen (#110), as a graph of its steps drawn like the stack map (`graph.rs`): a box per
//! step left to right, the switch fanning out to the APIs it restarts. Done steps are solid, the running one lit with
//! its `●` breathing and dots travelling into it (faster while data flows), the ones to come dashed. A step that
//! finishes turns solid, its `●` settling into `✓` with one glint, and the edge to the next draws itself in before
//! that one lights; a failed one pulses red twice and keeps a red border, the steps after it dim. Under the graph, the
//! picked step's line - what it runs, a real progress bar, its last output - and the error's first line. Done, one
//! light sweep runs over it, it holds a moment (each box with its duration, the slowest tinted), then folds into a
//! summary row. Narrower than its boxes, it scrolls sideways to the running step; much narrower, it's a list. An
//! export, an import and an update (#114) draw their own steps; an update's swap shows the two names trading places,
//! the old one sliding down and dimming as the new one rises into its place, and fans out to the apps it stops and
//! starts again.

use std::time::{Duration, Instant};

use lazyrunner_core::run::controller::pull::{PullRun, StepRun};
use lazyrunner_core::run::database_files as files;
use lazyrunner_core::run::pull::{Mode, State, Step};
use lazyrunner_core::run::{Controller, format_duration};
use unicode_width::UnicodeWidthStr;

use super::Ui;
use super::graph::{self, Map, Motion};
use super::motion::ease;
use super::mouse::Target;
use super::seg::{Seg, eighths, pad, seg, sweep, take_cells};
use super::theme::C;

/// A step box's text is this wide; an app box's this.
const INNER: usize = 14;
const APP_INNER: usize = 10;
/// An edge draws in this long before its step lights; a finished step glints this long; breathing takes this long.
const EDGE_IN: Duration = Duration::from_millis(250);
const GLINT: Duration = super::motion::SWEEP;
const BREATH: Duration = Duration::from_millis(1600);
/// A failed step pulses red twice: four phases this long.
const PULSE: Duration = Duration::from_millis(800);
const PULSE_STEP: u128 = 200;
/// Done: the light sweep, the hold, the fold into the summary row.
const SWEEP_ALL: Duration = Duration::from_millis(700);
const HOLD: Duration = Duration::from_millis(1500);
const FOLD: Duration = Duration::from_millis(250);
/// A share's glide to its newest value.
const SHARE_GLIDE: Duration = Duration::from_millis(150);
/// The dots' speed into the running step, cells a second: while data flows, and otherwise.
const DOT_FLOWING: f64 = 14.0;
const DOT_SLOW: f64 = 4.0;
/// A box kept this far from the view's side when it scrolls to the running one.
const MARGIN: usize = 2;
/// The progress bar's cells; below this width the graph is a list.
const BAR: usize = 20;
const LIST_BELOW: usize = 60;
/// A box's rows, and the section's rows besides the graph: its header, a blank row, the picked step's two.
const BOX_ROWS: usize = 5;
const AROUND: usize = 4;
/// The fewest rows the graph is drawn in whole: one row of boxes with its title and progress around it.
pub const LEAST: usize = BOX_ROWS + 2 + AROUND;
/// The swap's names trade places over this long, in a loop while it runs.
const TRADE: Duration = Duration::from_millis(1200);

/// Where the dots are along the edge into the running step: how far, when last moved, for which job and step.
#[derive(Debug, Default)]
pub struct Dots {
    travelled: f64,
    at: Option<Instant>,
    on: Option<(u64, usize)>,
}

impl Dots {
    fn advance(&mut self, on: (u64, usize), speed: f64, now: Instant) -> f64 {
        if self.on != Some(on) {
            *self = Self { travelled: 0.0, at: Some(now), on: Some(on) };
        }
        let last = self.at.unwrap_or(now);
        self.travelled += speed * now.saturating_duration_since(last).as_secs_f64();
        self.at = Some(now);
        self.travelled
    }
}

/// Whether the screen's motion runs.
fn moving(ui: &Ui) -> bool {
    ui.motion.enabled && !ui.reduced_motion
}

fn since(ui: &Ui, at: Instant) -> Duration {
    ui.now.saturating_duration_since(at)
}

/// How a finished pull shows: sweeping, holding, folding (how far), or folded into its summary row.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Over {
    Sweeping(f64),
    Holding,
    Folding(f64),
    Folded,
}

fn over(ui: &Ui, pull: &PullRun) -> Option<Over> {
    let ended = pull.ended.filter(|_| pull.went_well())?;
    if ui.pull_view.focused {
        return Some(Over::Holding);
    }
    let after = since(ui, ended);
    let still = !moving(ui);
    Some(match after {
        _ if after < SWEEP_ALL && !still => Over::Sweeping(after.as_secs_f64() / SWEEP_ALL.as_secs_f64()),
        _ if after < SWEEP_ALL + HOLD => Over::Holding,
        _ if after < SWEEP_ALL + HOLD + FOLD && !still => {
            Over::Folding((after - SWEEP_ALL - HOLD).as_secs_f64() / FOLD.as_secs_f64())
        }
        _ => Over::Folded,
    })
}

/// Whether the pull's section shows the graph (else a summary row, or nothing).
pub fn graph_shown(ui: &Ui, controller: &Controller) -> bool {
    let Some(pull) = controller.pull.as_ref() else { return false };
    let dismissed = ui.pull_view.dismissed == Some(pull.job) && !ui.pull_view.focused;
    !dismissed && over(ui, pull) != Some(Over::Folded)
}

/// Whether anything of it moves.
pub fn animating(ui: &Ui, controller: &Controller) -> bool {
    let Some(pull) = controller.pull.as_ref().filter(|_| ui.picker.is_some() && moving(ui)) else { return false };
    if !pull.is_over() {
        return true;
    }
    let ended = pull.ended.map_or(Duration::MAX, |at| since(ui, at));
    ended < SWEEP_ALL + HOLD + FOLD || ended < PULSE
}

/// The step picked: the one `h` `l` picked, else the running one, the failed one, the last done.
pub fn picked(ui: &Ui, pull: &PullRun) -> Step {
    if let Some(step) = ui.pull_view.picked.and_then(|at| pull.steps().get(at)) {
        return *step;
    }
    let failed = pull.error.as_ref().map(|(step, _)| *step);
    let done = pull.steps().iter().rev().copied().find(|step| pull.step(*step).state == State::Done);
    pull.running().or(failed).or(done).unwrap_or(Step::Check)
}

/// Where the picked step is among the pull's steps.
pub fn picked_at(ui: &Ui, pull: &PullRun) -> usize {
    let step = picked(ui, pull);
    pull.steps().iter().position(|known| *known == step).unwrap_or_default()
}

/// Where a step is among the pull's steps (its box's place in the graph).
fn at(pull: &PullRun, step: Step) -> usize {
    pull.steps().iter().position(|known| *known == step).unwrap_or_default()
}

/// The step that restarts the apps: the switch, or an update's swap.
fn last(pull: &PullRun) -> Step {
    if pull.mode == Mode::Update { Step::Swap } else { Step::Switch }
}

/// How long a step took, once it's done.
fn took(run: &StepRun) -> Option<Duration> {
    Some(run.ended?.saturating_duration_since(run.started?))
}

/// The step that took longest, once the pull went well: its box is tinted.
fn slowest(pull: &PullRun) -> Option<Step> {
    pull.went_well().then(|| pull.steps().iter().copied().max_by_key(|step| took(pull.step(*step))))?
}

/// A step as it shows now: running only once the edge into it has drawn in.
fn shown_state(ui: &Ui, run: &StepRun, step: Step) -> State {
    let drawing = run.started.is_some_and(|at| step != Step::Check && moving(ui) && since(ui, at) < EDGE_IN);
    if run.state == State::Running && drawing { State::Waiting } else { run.state }
}

/// What a running step's `●` looks like this frame: breathing between the accent and its dark.
fn breath(ui: &Ui) -> C {
    let phase = ui.now.saturating_duration_since(ui.started).as_millis() % BREATH.as_millis();
    if moving(ui) && phase >= BREATH.as_millis() / 2 { C::DarkCyan } else { C::Cyan }
}

/// The two lines under a step's name: once the pull is over, the second says how long a done step took.
fn detail(controller: &Controller, pull: &PullRun, step: Step) -> [String; 2] {
    let [first, second] = doing(controller, pull, step);
    let run = pull.step(step);
    match took(run).filter(|_| pull.is_over() && run.state == State::Done) {
        Some(took) => [first, format_duration(took.as_secs_f64())],
        None => [first, second],
    }
}

/// The two lines under a step's name, as it goes.
fn doing(controller: &Controller, pull: &PullRun, step: Step) -> [String; 2] {
    let run = pull.step(step);
    let duration = took(run).map(|took| format_duration(took.as_secs_f64())).unwrap_or_default();
    let tables = pull.progress.tables.map(|(at, of)| format!("{at} / {of}")).unwrap_or_else(|| "reading…".into());
    let table = pull.progress.table.clone().unwrap_or_default();
    let split = |text: &str| -> [String; 2] {
        match text.rsplit_once(" · ") {
            Some((first, rest)) => [first.to_string(), rest.to_string()],
            None => [text.to_string(), String::new()],
        }
    };
    match (step, run.state) {
        (_, State::Failed) => ["failed".into(), duration],
        (_, State::Skipped) => split(&run.detail),
        (Step::Check, State::Running) => ["checking…".into(), pull.source.clone()],
        (Step::Check, State::Done) => split(&run.detail),
        (Step::Check, _) => [pull.source.clone(), String::new()],
        (Step::Export | Step::Import, State::Running) => [tables, table],
        (Step::Export, State::Done) => [run.detail.clone(), duration],
        (Step::Export, _) => ["a .bacpac".into(), String::new()],
        (Step::Import, State::Done) => [pull.database.clone(), duration],
        (Step::Import, _) => ["new database".into(), String::new()],
        (Step::AfterPull, State::Waiting) => {
            let scripts = controller.project.pulling.after_pull.len();
            [format!("{scripts} script{}", if scripts == 1 { "" } else { "s" }), String::new()]
        }
        (Step::AfterPull, State::Running) => {
            let at = run.output.iter().rev().find_map(|line| line.strip_prefix("▸ ")).unwrap_or_default();
            ["running".into(), at.rsplit(['/', '\\']).next().unwrap_or_default().to_string()]
        }
        (Step::Migrations, State::Waiting) => ["pending ones".into(), String::new()],
        (Step::Migrations, State::Running) => ["applying…".into(), String::new()],
        (Step::Switch, State::Done) => [run.detail.clone(), duration],
        (Step::Switch, _) => ["make it the".into(), "active one".into()],
        (Step::Swap, State::Running) => ["renaming…".into(), pull.target.clone().unwrap_or_default()],
        (Step::Swap, _) => [format!("→ {}", pull.target.as_deref().unwrap_or_default()), "names trade".into()],
        (_, State::Done) => [run.detail.clone(), duration],
    }
}

/// A box's lines: its mark and name, then its detail, each `inner` wide.
fn lines(title: Vec<Seg>, detail: [String; 2], color: C, inner: usize) -> Vec<Vec<Seg>> {
    let fit = |text: &str| {
        if text.width() > inner { format!("{}…", take_cells(text, inner - 1)) } else { pad(text, inner) }
    };
    let used: usize = title.iter().map(|part| part.text.width()).sum();
    let mut first = title;
    first.push(seg(" ".repeat(inner.saturating_sub(used)), C::Gray));
    vec![first, vec![seg(fit(&detail[0]), color)], vec![seg(fit(&detail[1]), C::DarkGray)]]
}

/// A step's box.
fn step_node(ui: &Ui, controller: &Controller, pull: &PullRun, step: Step) -> graph::Node {
    let run = pull.step(step);
    let state = shown_state(ui, run, step);
    let after_failure = pull.error.as_ref().is_some_and(|(failed, _)| step.index() > failed.index());
    let (mark, mark_color) = match state {
        State::Done => ("✓", C::Green),
        State::Running => ("●", breath(ui)),
        State::Failed => ("✗", C::Red),
        State::Skipped => ("–", C::DarkGray),
        State::Waiting => ("○", C::DarkGray),
    };
    let name_color = match state {
        State::Running => C::White,
        State::Done | State::Failed => C::Gray,
        _ => C::DarkGray,
    };
    let mut title = vec![seg(format!("{mark} "), mark_color), seg(step.name(), name_color)];
    if let Some(ended) = run.ended.filter(|at| state == State::Done && moving(ui) && since(ui, *at) < GLINT) {
        title = sweep(title, since(ui, ended).as_secs_f64() / GLINT.as_secs_f64(), INNER, C::Black, C::Green);
    }
    let detail_color = if state == State::Running { C::Cyan } else { C::Gray };
    let mut node_lines = lines(title, detail(controller, pull, step), detail_color, INNER);
    if after_failure {
        node_lines = node_lines.into_iter().map(super::seg::dim).collect();
    }
    let pulse = run.ended.filter(|_| state == State::Failed && moving(ui)).map(|at| since(ui, at));
    let border = match state {
        _ if ui.pull_view.focused && picked(ui, pull) == step => C::White,
        State::Failed if pulse.is_some_and(|after| after < PULSE && (after.as_millis() / PULSE_STEP) % 2 == 1) => {
            C::White
        }
        State::Failed => C::Red,
        State::Running => C::Cyan,
        State::Done if slowest(pull) == Some(step) => C::Yellow,
        State::Done => C::Gray,
        _ => C::DarkGray,
    };
    if step == Step::Swap && matches!(state, State::Running | State::Done) {
        node_lines.truncate(1);
        node_lines.extend(trading(ui, pull, run));
    }
    let dashed = matches!(state, State::Waiting | State::Skipped);
    graph::Node { lines: node_lines, pad: 1, border, dashed, target: Some(Target::Step(at(pull, step))), bare: false }
}

/// The swap's two names trading places: the linked database's old copy sliding down and dimming, the new one rising
/// into its place - looping while it runs, settled once it's done (still: settled).
fn trading(ui: &Ui, pull: &PullRun, run: &StepRun) -> Vec<Vec<Seg>> {
    let target = pull.target.clone().unwrap_or_default();
    let old = pull.old.clone().unwrap_or_else(|| format!("{target}_old"));
    let fit = |text: &str| {
        if text.width() > INNER { format!("{}…", take_cells(text, INNER - 1)) } else { pad(text, INNER) }
    };
    let phase = match (run.state, run.started) {
        (State::Running, Some(at)) if moving(ui) => {
            let looped = since(ui, at).as_millis() % TRADE.as_millis();
            ease(looped as f64 / TRADE.as_millis() as f64)
        }
        (State::Running, _) => 0.0,
        _ => 1.0,
    };
    if phase < 0.5 {
        vec![vec![seg(fit(&target), C::Gray)], vec![seg(fit(&format!("↑ {}", pull.database)), C::Cyan)]]
    } else {
        vec![vec![seg(fit(&format!("✓ {target}")), C::Green)], vec![seg(fit(&format!("↓ {old}")), C::DarkGray)]]
    }
}

/// An API's box beyond the switch: waiting, restarting (lit, breathing) or answering again.
fn app_node(ui: &Ui, controller: &Controller, pull: &PullRun, (key, before): &(String, u64)) -> graph::Node {
    let switch = pull.step(last(pull)).state;
    let state = controller.state(key);
    let up = state.is_some_and(|state| state.generation > *before && state.is_up()) || switch == State::Done;
    let restarting = switch == State::Running && !up;
    let (mark, color, border) = match (up, restarting) {
        (true, _) => ("✓", C::Green, C::Gray),
        (false, true) => ("●", breath(ui), C::Cyan),
        _ => ("○", C::DarkGray, C::DarkGray),
    };
    let name = controller.app(key).map_or_else(|| key.clone(), |app| app.name.clone());
    let port = controller.port(key).map(|port| format!(":{port}")).unwrap_or_default();
    let title = vec![seg(format!("{mark} "), color), seg(take_cells(&name, APP_INNER - 2), C::Gray)];
    let node_lines = lines(title, ["restart".into(), port], C::DarkGray, APP_INNER);
    graph::Node { lines: node_lines, pad: 1, border, dashed: !up && !restarting, target: None, bare: false }
}

/// The graph's boxes, lines and motion.
fn model(ui: &mut Ui, controller: &Controller, pull: &PullRun) -> (Vec<graph::Node>, Vec<graph::Edge>, Vec<Motion>) {
    let mut nodes: Vec<graph::Node> = pull.steps().iter().map(|step| step_node(ui, controller, pull, *step)).collect();
    nodes.extend(pull.apps.iter().map(|app| app_node(ui, controller, pull, app)));
    let (mut edges, mut motion) = (Vec::new(), Vec::new());
    for pair in pull.steps().windows(2) {
        let (from, to) = (pair[0], pair[1]);
        let run = pull.step(to);
        let state = run.state;
        let failed_before = pull.error.as_ref().is_some_and(|(failed, _)| to.index() > failed.index());
        let (color, dashed) = match state {
            _ if failed_before => (C::DarkGray, true),
            State::Running => (C::Cyan, false),
            State::Failed => (C::Red, false),
            State::Done | State::Skipped => (C::Gray, false),
            State::Waiting => (C::DarkGray, true),
        };
        let mut look = Motion::default();
        if state == State::Running && moving(ui) {
            look.shown = run.started.map_or(1.0, |at| since(ui, at).as_secs_f64() / EDGE_IN.as_secs_f64());
            let speed = if to.flows() { DOT_FLOWING } else { DOT_SLOW };
            let travelled = ui.pull_view.dots.advance((pull.job, to.index()), speed, ui.now);
            look.dot = Some((travelled, C::White));
        }
        edges.push(graph::Edge {
            from: at(pull, from),
            to: at(pull, to),
            color,
            dashed,
            label: String::new(),
            quiet: false,
            top: state == State::Running,
        });
        motion.push(look);
    }
    let switch = pull.step(last(pull)).state;
    let steps = pull.steps().len();
    for (app, (key, before)) in pull.apps.iter().enumerate() {
        let restarted = controller.state(key).is_some_and(|state| state.generation > *before);
        let lit = switch == State::Done || (switch == State::Running && restarted);
        let color = if lit { C::Cyan } else { C::DarkGray };
        edges.push(graph::Edge {
            from: at(pull, last(pull)),
            to: steps + app,
            color: if switch == State::Done { C::Gray } else { color },
            dashed: !lit,
            label: String::new(),
            quiet: false,
            top: false,
        });
        motion.push(Motion::default());
    }
    (nodes, edges, motion)
}

/// The header: where from and to, how long it has taken and about how long is left.
fn header(ui: &Ui, pull: &PullRun, cells: usize) -> Vec<Seg> {
    let left = match pull.mode {
        Mode::Export => format!(" EXPORT {} → {}", pull.twin(), file_name(&pull.file)),
        Mode::Update => {
            format!(" UPDATE {} @ {} ← {}", pull.target.as_deref().unwrap_or_default(), pull.server, pull.twin())
        }
        Mode::Import => format!(" IMPORT {} → {} · {}", pull.twin(), pull.server, pull.database),
        Mode::Pull => format!(" PULL {} → {} · {}", pull.source, pull.server, pull.database),
    };
    let end = pull.ended.unwrap_or(ui.now);
    let mut right = format_duration(end.saturating_duration_since(pull.started).as_secs_f64());
    let running = pull.running().filter(|step| step.flows() && !pull.is_over());
    let share = shown_share(ui, pull);
    if let (Some(step), Some(share)) = (running, share)
        && let Some(left) = pull.step(step).started.and_then(|at| files::eta(share, since(ui, at)))
    {
        right.push_str(&format!(" · about {} left", format_duration(left.as_secs_f64())));
    }
    let gap = cells.saturating_sub(left.width() + right.width() + 1).max(2);
    vec![seg(left, C::DarkCyan), seg(" ".repeat(gap), C::Gray), seg(right, C::DarkGray)]
}

/// The share done, gliding to the newest the steps reported.
fn shown_share(ui: &Ui, pull: &PullRun) -> Option<f64> {
    let (before, newest, at) = pull.share?;
    if !moving(ui) {
        return Some(newest);
    }
    let glide = ease(since(ui, at).as_secs_f64() / SHARE_GLIDE.as_secs_f64());
    Some(before + (newest - before) * glide)
}

/// A path's file name.
fn file_name(path: &str) -> String {
    std::path::Path::new(path).file_name().map_or_else(String::new, |name| name.to_string_lossy().into_owned())
}

/// What a step runs.
fn runs(step: Step) -> &'static str {
    match step {
        Step::Check | Step::AfterPull => "sqlcmd",
        Step::Export | Step::Import => "SqlPackage",
        Step::Migrations => "dotnet ef",
        Step::Switch => "secrets.json",
        Step::Swap => "ALTER DATABASE … MODIFY NAME",
    }
}

/// The picked step's line: its mark and what it runs, its progress (a bar, the tables, the table it's on, the file's
/// size), else its last output line; and the error's first line when it failed.
fn picked_lines(ui: &Ui, pull: &PullRun, cells: usize) -> Vec<Vec<Seg>> {
    let step = picked(ui, pull);
    let run = pull.step(step);
    let (mark, color) = match run.state {
        State::Done => ("✓", C::Green),
        State::Running => ("●", breath(ui)),
        State::Failed => ("✗", C::Red),
        _ => ("○", C::DarkGray),
    };
    let mut line = vec![seg(format!(" {mark} "), color), seg(format!("{} · {}   ", step.name(), runs(step)), C::Gray)];
    let flowing = step.flows() && run.state == State::Running;
    match shown_share(ui, pull).filter(|_| flowing) {
        Some(share) => {
            line.push(seg(eighths(share, BAR), C::Cyan));
            let mut said = vec![format!("{:.0}%", share * 100.0)];
            said.extend(pull.progress.tables.map(|(at, of)| format!("{at} / {of} tables")));
            said.extend(pull.progress.table.clone());
            said.extend(pull.progress.bytes.map(|bytes| format!("{} so far", files::size(bytes))));
            line.push(seg(format!("   {}", said.join(" · ")), C::Gray));
        }
        None => {
            let last = run.output.iter().rev().find(|line| !line.trim().is_empty());
            let said = last.cloned().unwrap_or_else(|| run.detail.clone());
            line.push(seg(said, C::DarkGray));
        }
    }
    let mut lines = vec![super::columns::fit(line, cells)];
    if let Some((failed, why)) = &pull.error {
        let first = why.lines().next().unwrap_or_default();
        let said = if pull.cancelled {
            format!(" ■ cancelled at {} - the source is untouched", failed.name())
        } else {
            format!(" ✗ {} failed: {first}", failed.name())
        };
        lines.push(super::columns::fit(vec![seg(said, C::Red)], cells));
    }
    lines
}

/// The steps as a list, for a window narrower than the graph's boxes.
fn list(ui: &Ui, controller: &Controller, pull: &PullRun, cells: usize) -> Vec<Vec<Seg>> {
    pull.steps()
        .iter()
        .map(|step| {
            let run = pull.step(*step);
            let (mark, color) = match shown_state(ui, run, *step) {
                State::Done => ("✓", C::Green),
                State::Running => ("●", breath(ui)),
                State::Failed => ("✗", C::Red),
                State::Skipped => ("–", C::DarkGray),
                State::Waiting => ("○", C::DarkGray),
            };
            let [first, second] = detail(controller, pull, *step);
            let text = format!("{} {first} {second}", pad(step.name(), 11));
            super::columns::fit(vec![seg(format!("  {mark} "), color), seg(text, C::Gray)], cells)
        })
        .collect()
}

/// The summary row a finished pull folds into.
fn summary(pull: &PullRun) -> Vec<Seg> {
    let took = pull.ended.map(|at| format_duration(at.saturating_duration_since(pull.started).as_secs_f64()));
    let took = took.unwrap_or_default();
    let what = pull.mode.name();
    match &pull.error {
        None => {
            let said = match pull.mode {
                Mode::Pull => {
                    format!(" ✓ pulled {} in {took} · {} is active · Tab shows the steps", pull.source, pull.database)
                }
                Mode::Import => {
                    format!(" ✓ imported {} in {took} as {} · Tab shows the steps", pull.twin(), pull.database)
                }
                Mode::Export => {
                    format!(" ✓ exported {} in {took} · {} · Tab shows the steps", pull.twin(), file_name(&pull.file))
                }
                Mode::Update => format!(
                    " ✓ updated {} from {} in {took} · pulled just now · Tab shows the steps",
                    pull.target.as_deref().unwrap_or_default(),
                    pull.twin()
                ),
            };
            vec![seg(said, C::Green)]
        }
        Some((step, _)) if pull.cancelled => {
            vec![seg(format!(" ■ the {what} from {} was cancelled at {}", pull.source, step.name()), C::Yellow)]
        }
        Some((step, why)) => vec![seg(
            format!(
                " ✗ the {what} from {} failed at {}: {}",
                pull.source,
                step.name(),
                why.lines().next().unwrap_or_default()
            ),
            C::Red,
        )],
    }
}

/// The rows the section takes, out of `left`: none without a pull, its summary row once it's folded or put away.
pub fn height(ui: &Ui, controller: &Controller, left: usize) -> usize {
    if controller.pull.is_none() || left == 0 {
        return 0;
    }
    if !graph_shown(ui, controller) {
        return 1;
    }
    let apps = controller.pull.as_ref().map_or(0, |pull| pull.apps.len()).max(1);
    let graph = BOX_ROWS * apps + (apps - 1) + 2;
    (graph + AROUND).min(left)
}

/// A step's whole output, `Enter` on it: its newest lines, scrolled back `scroll`.
fn output(ui: &Ui, pull: &PullRun, cells: usize, rows: usize) -> Vec<Vec<Seg>> {
    let step = picked(ui, pull);
    let run = pull.step(step);
    let title = format!(
        " OUTPUT · {} · {} · {} lines · j k scroll · Enter or Esc back",
        step.name(),
        runs(step),
        run.output.len()
    );
    let mut lines = vec![vec![seg(title, C::DarkCyan)]];
    let room = rows.saturating_sub(1);
    let end = run.output.len().saturating_sub(ui.pull_view.scroll.min(run.output.len()));
    let start = end.saturating_sub(room);
    if run.output.is_empty() {
        lines.push(vec![seg("   nothing yet", C::DarkGray)]);
    }
    lines.extend(
        run.output[start..end].iter().map(|text| super::columns::fit(vec![seg(format!("   {text}"), C::Gray)], cells)),
    );
    lines
}

/// The pull's section, `rows` rows.
pub fn rows(ui: &mut Ui, controller: &Controller, cells: usize, rows: usize) -> Vec<Vec<Seg>> {
    let Some(pull) = controller.pull.as_ref() else { return Vec::new() };
    if rows == 0 {
        return Vec::new();
    }
    if !graph_shown(ui, controller) {
        return vec![summary(pull)];
    }
    if ui.pull_view.output {
        return output(ui, pull, cells, rows);
    }
    let mut lines = vec![header(ui, pull, cells)];
    let below = picked_lines(ui, pull, cells);
    let room = cells.saturating_sub(2);
    let graph_rows = rows.saturating_sub(lines.len() + below.len() + 1);
    if cells < LIST_BELOW {
        lines.extend(list(ui, controller, pull, cells).into_iter().take(graph_rows));
    } else {
        let (nodes, edges, motion) = model(ui, controller, pull);
        let map = Map::draw(&nodes, &edges, &motion);
        let keep = at(pull, pull.running().unwrap_or_else(|| picked(ui, pull)));
        let from = map.layout.boxes.get(keep).map_or(0, |at| (at.x + at.w + MARGIN).saturating_sub(room));
        let mut drawn: Vec<Vec<Seg>> = map.rows(from, room).into_iter().take(graph_rows).collect();
        match over(ui, pull) {
            Some(Over::Sweeping(through)) => {
                drawn = drawn.into_iter().map(|row| sweep(row, through, room, C::Black, C::Cyan)).collect();
            }
            Some(Over::Folding(folded)) => {
                let keep = ((drawn.len() as f64) * (1.0 - folded)).ceil() as usize;
                let skip = (drawn.len() - keep.min(drawn.len())) / 2;
                drawn = drawn.into_iter().skip(skip).take(keep).collect();
            }
            _ => {}
        }
        lines.extend(drawn.into_iter().map(|row| {
            let mut line = vec![seg("  ", C::Gray)];
            line.extend(row);
            line
        }));
    }
    lines.push(Vec::new());
    lines.extend(below);
    lines.truncate(rows);
    lines
}
