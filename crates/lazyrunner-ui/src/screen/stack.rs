//! The stack map (K, #51): the apps as boxes in columns by who calls whom - a caller left of what it calls, else
//! the profile's order - and the database after the apps that use it. Each edge is the calls one app made to
//! another (HttpClient's log, by the port they went to) or the profile's `dependsOn` (dashed until a call is seen),
//! coloured by its last minute: cyan with calls, red with a failure, with its calls a second on it. Under the map,
//! every edge's figures. `h` `j` `k` `l` choose an app, `Enter` goes to its column.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use lazyrunner_core::profile::AppKind;
use lazyrunner_core::run::app::{AppState, Status};
use lazyrunner_core::run::calls;
use lazyrunner_core::run::controller::Minute;
use lazyrunner_core::run::history::Kind;
use lazyrunner_core::run::{Controller, format_duration};
use unicode_width::UnicodeWidthStr;

use super::graph::{self, Map, Motion};
use super::icons::{self, Icon};
use super::motion::Spot;
use super::mouse::Target;
use super::seg::{Seg, aim, pad, seg};
use super::theme::C;
use super::{Overlay, Ui};

/// The edges draw in as it opens; a new failure pulses its edge twice.
const REVEAL: Duration = Duration::from_millis(300);
const PULSE: Duration = Duration::from_millis(800);
const PULSE_STEP: u128 = 200;
/// A dot's speed along a busy edge, cells a second: this, and this much more per call a second, up to the most.
const DOT_SPEED: f64 = 4.0;
const DOT_PER_CALL: f64 = 6.0;
const DOT_FASTEST: f64 = 30.0;
/// A box's edge kept this far from the view's side when it scrolls to the picked one.
const MARGIN: usize = 2;

/// K: opens the map on the focused app, the apps' config read again for where their calls go.
pub fn open(ui: &mut Ui, controller: &mut Controller) {
    controller.find_configured_calls();
    let index = ui.focus.min(controller.apps.len().saturating_sub(1));
    ui.overlay = Some(Overlay::Stack { index, opened: ui.now });
    ui.stack_dots = Dots::default();
}

fn view(ui: &Ui) -> Option<(usize, Instant)> {
    match &ui.overlay {
        Some(Overlay::Stack { index, opened }) => Some((*index, *opened)),
        _ => None,
    }
}

/// Whether its motion runs: not with reduced motion, nor in the screen tests' frozen frames.
fn moving(ui: &Ui) -> bool {
    ui.motion.enabled && !ui.reduced_motion
}

/// The map's parts: the boxes (the apps, then the database), the lines, how each moves, and each app edge's figures.
struct Model {
    nodes: Vec<graph::Node>,
    edges: Vec<graph::Edge>,
    motion: Vec<Motion>,
    figures: Vec<calls::Edge>,
}

/// What an app is doing, as its column's title says it: building (how far), waiting to build, installing its
/// packages, then its own state - a mark, words and a colour.
fn status(ui: &Ui, controller: &Controller, index: usize) -> (String, String, C) {
    let state = &controller.states[index];
    let key = &controller.apps[index].key;
    let busy = || if moving(ui) { super::columns::spinner(ui).to_string() } else { "◌".to_string() };
    if let Some(job) = controller.building.as_ref().filter(|job| job.key == *key) {
        let done = if job.total > 0 { format!(" {}/{}", job.done, job.total) } else { String::new() };
        return (busy(), format!("building{done}"), C::Yellow);
    }
    if controller.build_queue.iter().any(|queued| queued == key) {
        return ("◌".into(), "waiting to build".into(), C::DarkYellow);
    }
    if let Some(job) = controller.npm_ci.as_ref().filter(|job| job.key == *key) {
        return (busy(), job.what.to_string(), C::Yellow);
    }
    let (mark, word, color) = match &state.status {
        Status::Running if state.is_up() => ("●", "up", C::Green),
        Status::Running => ("◌", "starting", C::Yellow),
        Status::Exited(_) => ("✗", "exited", C::Red),
        Status::Error(_) => ("✗", "couldn't start", C::Red),
        Status::Stopped => ("○", "stopped", C::DarkGray),
    };
    (mark.into(), word.into(), color)
}

/// How long an app has run, in a few cells: "45s", "12m", "2h 14m", "3d 2h".
fn uptime(seconds: u64) -> String {
    match seconds {
        0..60 => format!("{seconds}s"),
        60..3600 => format!("{}m", seconds / 60),
        3600..86400 => format!("{}h {}m", seconds / 3600, seconds % 3600 / 60),
        _ => format!("{}d {}h", seconds / 86400, seconds % 86400 / 3600),
    }
}

/// The most a box says (see [`app_node`]), and the most cells a line of it takes: a longer one (an endpoint, a
/// script) scrolls through.
const DETAIL_MOST: usize = 3;
const LINE_CELLS: usize = 30;

/// A line of a box, no wider than [`LINE_CELLS`].
fn line(ui: &Ui, text: String, color: C) -> Vec<Seg> {
    let text = if text.width() > LINE_CELLS { ui.scrolling(&text, LINE_CELLS) } else { text };
    vec![seg(text, color)]
}

/// A time in a few cells: "340 ms", "1.2 s".
fn ms(ms: f64) -> String {
    if ms >= 1000.0 { format!("{:.1} s", ms / 1000.0) } else { format!("{ms:.0} ms") }
}

/// A minute of requests or calls: "18 req/min · p95 40 ms", the failed ones red after; `nothing` without any.
fn minute(minute: &Minute, what: &str, nothing: &str, failed: &str) -> Vec<Seg> {
    if minute.count == 0 {
        return vec![seg(nothing, C::DarkGray)];
    }
    let p95 = minute.p95_ms.map_or(String::new(), |at| format!(" · p95 {}", ms(at)));
    let mut segs = vec![seg(format!("{} {what}/min{p95}", minute.count), C::Gray)];
    if minute.failed > 0 {
        segs.push(seg(format!(" · {} {failed}", minute.failed), C::Red));
    }
    segs
}

/// The errors and warnings an app printed.
fn printed(state: &AppState) -> Vec<Seg> {
    let count = |count: u64, word: &str| format!("{count} {word}{}", plural(count as usize));
    match (state.errors, state.warnings) {
        (0, 0) => vec![seg("no errors", C::DarkGray)],
        (0, warnings) => vec![seg(count(warnings, "warning"), C::Yellow)],
        (errors, 0) => vec![seg(count(errors, "error"), C::Red)],
        (errors, warnings) => {
            vec![seg(count(errors, "error"), C::Red), seg(format!(" · {}", count(warnings, "warning")), C::Yellow)]
        }
    }
}

/// A .NET app's last build: "built 4m ago in 22s", or "build failed 1m ago".
fn last_build(controller: &Controller, key: &str) -> Option<Vec<Seg>> {
    let run = controller.durations.item(Kind::Build, key)?.runs.last()?;
    let at = chrono::DateTime::parse_from_rfc3339(&run.at).ok()?;
    let ago = uptime((chrono::Local::now().fixed_offset() - at).num_seconds().max(0).unsigned_abs());
    Some(if run.ok {
        vec![seg(format!("built {ago} ago in {}", format_duration(run.sec)), C::DarkGray)]
    } else {
        vec![seg(format!("build failed {ago} ago"), C::Red)]
    })
}

/// An app's box, saying as much as `detail` lets it: 0 its name and state; 1 how long it has run, its CPU and
/// memory, and its minute's work (what a .NET app served, what an npm app called through its proxy); 2 the errors
/// and warnings it printed; 3 a .NET app's slowest endpoint and last build, an npm app's script.
fn app_node(ui: &Ui, controller: &Controller, index: usize, picked: bool, detail: usize) -> graph::Node {
    let app = &controller.apps[index];
    let state = &controller.states[index];
    let icon = icons::app(ui.nerd, app, controller.framework(&app.key));
    let (dot, word, color) = status(ui, controller, index);
    let port = controller.port(&app.key).map_or(String::new(), |port| format!(" · :{port}"));
    let name = seg(format!("{icon} {}", app.name), if picked { C::White } else { C::Gray });
    let border = if picked { C::Cyan } else { super::columns::border(controller, index) };
    let target = Some(Target::Item(index));
    if detail == 0 {
        let line = vec![seg(dot, color), seg(format!(" {word}{}", port.replace(" · ", "  ")), C::DarkGray)];
        return graph::Node { lines: vec![vec![name], line], pad: 1, border, dashed: false, target };
    }
    let running = matches!(state.status, Status::Running);
    let since = state
        .started_at
        .filter(|_| running)
        .map_or(String::new(), |at| format!(" · {}", uptime(ui.now.saturating_duration_since(at).as_secs())));
    let first = vec![seg(dot, color), seg(format!(" {word}{port}{since}"), C::DarkGray)];
    let load = match (state.cpu, state.memory_mb) {
        (Some(cpu), Some(memory)) if running => format!("cpu {cpu:.0}% · {memory:.0} MB"),
        _ => "not running".to_string(),
    };
    let dotnet = matches!(app.kind, AppKind::Dotnet { .. });
    let served = controller.served_last_minute(&app.key);
    let work = if dotnet {
        minute(&served, "req", "no requests (1 min)", "5xx")
    } else {
        minute(&controller.calls_last_minute(&app.key), "calls", "no calls (1 min)", "failed")
    };
    let mut lines = vec![vec![name], first, vec![seg(load, C::DarkGray)], work];
    if detail >= 2 {
        lines.push(printed(state));
    }
    if detail >= 3 && dotnet {
        if let Some((endpoint, took)) = served.slowest {
            lines.push(line(ui, format!("slowest {endpoint} {}", ms(took)), C::DarkGray));
        }
        lines.extend(last_build(controller, &app.key));
    } else if detail >= 3
        && let Some(script) = &state.script
    {
        let label = state.label.as_deref().map_or(String::new(), |label| format!(" · {label}"));
        lines.push(line(ui, format!("npm run {script}{label}"), C::DarkGray));
    }
    graph::Node { lines, pad: 2, border, dashed: false, target }
}

/// The database's box: its name; 1 its minute's commands and their time; 2 the slowest, and the requests that look
/// like an N+1.
fn database_node(ui: &Ui, controller: &Controller, picked: bool, detail: usize, index: usize) -> graph::Node {
    let label = controller.db_label.clone().unwrap_or_else(|| "database".into());
    let mut lines = vec![vec![seg(format!("{} {label}", icons::icon(ui.nerd, Icon::Database)), C::Gray)]];
    let minute = controller.database_minute();
    if detail >= 1 {
        lines.push(match minute.commands {
            0 => vec![seg("no commands (1 min)", C::DarkGray)],
            commands => vec![seg(format!("{commands} commands/min · {}", ms(minute.total_ms)), C::Gray)],
        });
    }
    if detail >= 2 && minute.commands > 0 {
        if let Some(slowest) = minute.slowest_ms {
            lines.push(vec![seg(format!("slowest {}", ms(slowest)), C::DarkGray)]);
        }
        if minute.repeated > 0 {
            let requests = format!("{} request{}", minute.repeated, plural(minute.repeated));
            lines.push(vec![seg(format!("{requests} look like N+1"), C::Yellow)]);
        }
    }
    let border = if picked { C::Cyan } else { C::DarkGray };
    let pad = if detail == 0 { 1 } else { 2 };
    graph::Node { lines, pad, border, dashed: false, target: Some(Target::Item(index)) }
}

/// An edge's colour and label: red with a failure in the last minute, cyan with calls, grey once seen, dim and
/// dashed while only declared.
fn look(edge: &calls::Edge) -> (C, String) {
    let color = if edge.failing() {
        C::Red
    } else if edge.calls > 0 {
        C::Cyan
    } else if edge.seen > 0 {
        C::Gray
    } else {
        C::DarkGray
    };
    let rate = rate(edge);
    let rate = if edge.served && !rate.is_empty() { format!("~{rate}") } else { rate };
    (color, rate)
}

/// Its calls over the last minute: "12/s", or "3/min" when they're fewer than one a second; nothing without any.
fn rate(edge: &calls::Edge) -> String {
    match edge.calls {
        0 => String::new(),
        _ if edge.per_second >= 1.0 => format!("{:.0}/s", edge.per_second),
        calls => format!("{calls}/min"),
    }
}

/// What an edge carried over the last minute, for its motion: how many a second, its dot's colour, its last failure.
/// Where each busy edge's dot is - how far it has travelled, in cells - and when it last moved: a dot moves on from
/// where it was, so a change in its edge's traffic changes its speed, never its place.
#[derive(Debug, Default)]
pub struct Dots(HashMap<String, (f64, Instant)>);

impl Dots {
    /// Moves an edge's dot on to `now` at `speed` cells a second (twice in a frame moves it once).
    fn advance(&mut self, edge: &str, speed: f64, now: Instant) -> f64 {
        let (travelled, last) = self.0.entry(edge.to_string()).or_insert((0.0, now));
        *travelled += speed * now.saturating_duration_since(*last).as_secs_f64();
        *last = now;
        *travelled
    }
}

struct Traffic {
    per_second: f64,
    dot: C,
    last_failed: Option<Instant>,
}

impl Traffic {
    /// A call between the apps: a white dot, a yellow one on a failing edge.
    fn of(edge: &calls::Edge) -> Self {
        let dot = if edge.failing() { C::Yellow } else { C::White };
        Self { per_second: edge.per_second, dot, last_failed: edge.last_failed }
    }
}

fn motion(ui: &Ui, opened: Instant, traffic: &Traffic, dots: &mut Dots, edge: &str) -> Motion {
    if !moving(ui) {
        return Motion::default();
    }
    let since = ui.now.saturating_duration_since(opened);
    let shown = since.as_secs_f64() / REVEAL.as_secs_f64();
    let speed = (DOT_SPEED + traffic.per_second * DOT_PER_CALL).min(DOT_FASTEST);
    let travelled = dots.advance(edge, if traffic.per_second > 0.0 { speed } else { 0.0 }, ui.now);
    let dot = (traffic.per_second > 0.0).then_some((travelled, traffic.dot));
    let pulse = traffic.last_failed.map(|at| ui.now.saturating_duration_since(at)).filter(|since| *since < PULSE);
    let color = pulse.filter(|since| (since.as_millis() / PULSE_STEP).is_multiple_of(2)).map(|_| C::White);
    Motion { shown, dot, color }
}

/// The map's parts with `detail` in its boxes, from `figures` (the edges as [`Controller::stack_edges`] has them).
fn model(
    ui: &Ui,
    controller: &Controller,
    (picked, opened): (usize, Instant),
    figures: &[calls::Edge],
    detail: usize,
    dots: &mut Dots,
) -> Model {
    let count = controller.apps.len();
    let mut nodes: Vec<graph::Node> =
        (0..count).map(|index| app_node(ui, controller, index, index == picked, detail)).collect();
    let at = |key: &str| controller.apps.iter().position(|app| app.key == key);
    let mut edges = Vec::new();
    let mut motions = Vec::new();
    let mut kept = Vec::new();
    for edge in figures.iter().cloned() {
        let (Some(from), Some(to)) = (at(&edge.from), at(&edge.to)) else { continue };
        let (color, label) = look(&edge);
        edges.push(graph::Edge { from, to, color, dashed: edge.seen == 0, label });
        let id = format!("{}>{}", edge.from, edge.to);
        motions.push(motion(ui, opened, &Traffic::of(&edge), dots, &id));
        kept.push(edge);
    }
    let users = controller.database_users();
    if !users.is_empty() {
        let commands: Vec<usize> = users.iter().map(|key| controller.database_commands(key)).collect();
        nodes.push(database_node(ui, controller, picked == count, detail, count));
        for ((user, key), commands) in users.iter().filter_map(|key| Some((at(key)?, key))).zip(commands) {
            let label = if commands > 0 { format!("{commands}/min") } else { String::new() };
            let color = if commands > 0 { C::Gray } else { C::DarkGray };
            let dashed = !controller.database_seen(key);
            edges.push(graph::Edge { from: user, to: count, color, dashed, label });
            let traffic = Traffic { per_second: commands as f64 / 60.0, dot: C::Cyan, last_failed: None };
            motions.push(motion(ui, opened, &traffic, dots, &format!("{key}>database")));
        }
    }
    Model { nodes, edges, motion: motions, figures: kept }
}

/// h j k l: the node to go to from `picked`.
fn step(layout: &graph::Layout, picked: usize, name: &str) -> usize {
    let mut order: Vec<usize> = (0..layout.boxes.len()).collect();
    order.sort_by_key(|&node| (layout.columns[node], layout.boxes[node].y));
    let at = order.iter().position(|&node| node == picked).unwrap_or(0);
    let column = layout.columns.get(picked).copied().unwrap_or(0);
    let y = layout.boxes.get(picked).map_or(0, |found| found.y);
    let nearest = |wanted: usize| {
        (0..layout.boxes.len())
            .filter(|&node| layout.columns[node] == wanted)
            .min_by_key(|&node| layout.boxes[node].y.abs_diff(y))
    };
    match name {
        "j" | "DownArrow" => order[(at + 1) % order.len()],
        "k" | "UpArrow" => order[(at + order.len() - 1) % order.len()],
        "l" | "RightArrow" => nearest(column + 1).unwrap_or(picked),
        "h" | "LeftArrow" => column.checked_sub(1).and_then(nearest).unwrap_or(picked),
        _ => picked,
    }
}

/// h j k l choose a box, Enter goes to its app's column, Esc / q / K close; false for a key it doesn't use.
pub fn key(ui: &mut Ui, controller: &mut Controller, name: &str) -> bool {
    let Some((picked, opened)) = view(ui) else { return false };
    let count = controller.apps.len();
    match name {
        "Escape" | "q" | "K" => ui.overlay = None,
        "Enter" if picked < count => {
            ui.overlay = None;
            ui.focus = picked;
            ui.motion.flash(Spot::Title(picked), ui.now);
        }
        "Enter" => controller.add_log("The database has no column.", lazyrunner_core::output::Hue::DarkGray),
        "h" | "j" | "k" | "l" | "LeftArrow" | "DownArrow" | "UpArrow" | "RightArrow" => {
            let edges = controller.stack_edges();
            let model = model(ui, controller, (picked, opened), &edges, 0, &mut Dots::default());
            let next = step(&graph::layout(&model.nodes, &model.edges), picked, name);
            ui.overlay = Some(Overlay::Stack { index: next, opened });
        }
        _ => return false,
    }
    true
}

/// A click picks a box; a double click goes to its column.
pub fn click(ui: &mut Ui, controller: &mut Controller, target: Option<Target>, double: bool) -> bool {
    let Some((_, opened)) = view(ui) else { return false };
    let Some(Target::Item(at)) = target else { return false };
    ui.overlay = Some(Overlay::Stack { index: at, opened });
    if double {
        key(ui, controller, "Enter");
    }
    true
}

/// Whether the map moves: its edges drawing in, dots on the busy ones, a failure's pulse.
pub fn animating(ui: &Ui, controller: &Controller) -> bool {
    let Some((_, opened)) = view(ui) else { return false };
    if !moving(ui) {
        return false;
    }
    let pulsing = |edge: &calls::Edge| edge.last_failed.is_some_and(|at| ui.now.saturating_duration_since(at) < PULSE);
    let busy_database = controller.database_users().iter().any(|key| controller.database_commands(key) > 0);
    ui.now.saturating_duration_since(opened) < REVEAL
        || busy_database
        || controller.stack_edges().iter().any(|edge| edge.calls > 0 || pulsing(edge))
}

fn header(ui: &Ui, controller: &Controller, figures: &[calls::Edge], width: usize) -> Vec<Seg> {
    let per_second: f64 = figures.iter().map(|edge| edge.per_second).sum();
    let failing = figures.iter().filter(|edge| edge.failing()).count();
    let apps = controller.apps.len();
    let edges = figures.len() + controller.database_users().len();
    let mut segs = vec![
        super::viewbar::chip(ui.nerd, Icon::Stack, "Stack"),
        seg(format!("  {apps} app{} · {edges} edge{}", plural(apps), plural(edges)), C::DarkGray),
    ];
    if per_second > 0.0 {
        segs.push(seg(format!(" · {per_second:.1} calls/s"), C::DarkGray));
    }
    if failing > 0 {
        segs.push(seg(format!(" · {failing} failing"), C::Red));
    }
    super::columns::fit(segs, width)
}

fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}

/// Every edge's figures over the last minute, the picked app's bright: the calls between the apps, then the
/// database's commands.
fn figures(controller: &Controller, figures: &[calls::Edge], picked: Option<&str>) -> Vec<Vec<Seg>> {
    let name = |key: &str| controller.app(key).map_or_else(|| key.to_string(), |app| app.name.clone());
    let database = controller.db_label.clone().unwrap_or_else(|| "database".into());
    let users = controller.database_users();
    let pairs: Vec<String> = figures.iter().map(|edge| format!("{} → {}", name(&edge.from), name(&edge.to))).collect();
    let used: Vec<String> = users.iter().map(|key| format!("{} → {database}", name(key))).collect();
    let room = pairs.iter().chain(&used).map(|pair| pair.chars().count()).max().unwrap_or(0);
    let mut lines = vec![vec![seg(" CALLS · the last minute", C::DarkCyan)]];
    for (edge, pair) in figures.iter().zip(pairs) {
        let mine = picked.is_some_and(|key| edge.from == key || edge.to == key);
        let mut line = vec![seg(format!("   {}", pad(&pair, room)), if mine { C::White } else { C::Gray })];
        if edge.seen == 0 {
            let place = edge.declared_in.as_deref().unwrap_or("dependsOn");
            line.push(seg(format!("   {place} · no call seen yet"), C::DarkGray));
            lines.push(line);
            continue;
        }
        let rate = if edge.calls == 0 { "no calls".to_string() } else { rate(edge) };
        line.push(seg(format!("   {rate:>8}"), C::Gray));
        if let Some(p95) = edge.p95_ms {
            line.push(seg(format!("   p95 {p95:.0} ms"), C::DarkGray));
        }
        if edge.failed > 0 {
            line.push(seg(format!("   {} failed", edge.failed), C::Red));
        }
        if edge.served {
            line.push(seg(format!("   what {} served", name(&edge.to)), C::DarkGray));
        }
        lines.push(line);
    }
    for (key, pair) in users.iter().zip(used) {
        let mine = picked == Some(key.as_str());
        let commands = match controller.database_commands(key) {
            0 => "no commands".to_string(),
            count => format!("{count}/min"),
        };
        lines.push(vec![
            seg(format!("   {}", pad(&pair, room)), if mine { C::White } else { C::Gray }),
            seg(format!("   {commands:>8}"), C::Gray),
            seg("   database commands", C::DarkGray),
        ]);
    }
    if figures.is_empty() {
        lines.push(vec![seg(
            "   No calls between the apps yet: a .NET app's calls show once it makes one (HttpClient's log); a \
             config naming another app's port (a dev server's proxy, an environment file) or the profile's \
             dependsOn draws the others.",
            C::DarkGray,
        )]);
    }
    lines
}

/// The map with as much in its boxes as fits: the most detail whose map is no wider than the room and no taller than
/// its rows - fewer apps, a bigger terminal, more said - else the least.
fn fitting(
    ui: &Ui,
    controller: &Controller,
    view: (usize, Instant),
    figures: &[calls::Edge],
    (room, rows): (usize, usize),
    dots: &mut Dots,
) -> (Model, Map) {
    for detail in (1..=DETAIL_MOST).rev() {
        let model = model(ui, controller, view, figures, detail, dots);
        let map = Map::draw(&model.nodes, &model.edges, &model.motion);
        if map.layout.width <= room && map.layout.height < rows {
            return (model, map);
        }
    }
    let model = model(ui, controller, view, figures, 0, dots);
    let map = Map::draw(&model.nodes, &model.edges, &model.motion);
    (model, map)
}

pub fn rows(ui: &mut Ui, controller: &Controller, width: usize, rows: usize) -> Vec<Vec<Seg>> {
    let Some((picked, opened)) = view(ui) else { return Vec::new() };
    let room = width.saturating_sub(2);
    let edges = controller.stack_edges();
    let known = |key: &str| controller.app(key).is_some();
    let shown: Vec<calls::Edge> = edges.iter().filter(|edge| known(&edge.from) && known(&edge.to)).cloned().collect();
    let below = figures(controller, &shown, controller.apps.get(picked).map(|app| app.key.as_str()));
    let map_rows = rows.saturating_sub(3 + below.len() + 1).max(3);
    let mut dots = std::mem::take(&mut ui.stack_dots);
    let (model, map) = fitting(ui, controller, (picked, opened), &edges, (room, map_rows), &mut dots);
    ui.stack_dots = dots;
    let picked = picked.min(model.nodes.len().saturating_sub(1));
    let from = map.layout.boxes.get(picked).map_or(0, |at| (at.x + at.w + MARGIN).saturating_sub(room));
    let hint = " h j k l choose an app · Enter its column · Esc back";
    let mut lines =
        vec![header(ui, controller, &model.figures, width), super::viewbar::keys(ui, hint, width), Vec::new()];
    for row in map.rows(from, room).into_iter().take(map_rows) {
        let mut line = vec![seg("  ", C::Gray)];
        line.extend(row);
        lines.push(line);
    }
    lines.push(Vec::new());
    lines.extend(below);
    lines.truncate(rows);
    lines.resize(rows, Vec::new());
    lines.into_iter().map(|line| aim(line, Target::Box)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dot_moves_on_from_where_it_was_when_its_edge_gets_busier() {
        let start = Instant::now();
        let mut dots = Dots::default();
        assert_eq!(dots.advance("orders>billing", 4.0, start), 0.0);
        assert_eq!(dots.advance("orders>billing", 4.0, start + Duration::from_secs(1)), 4.0);
        let busier = dots.advance("orders>billing", 30.0, start + Duration::from_secs(1));
        assert_eq!(busier, 4.0, "a new call makes it faster, not further: it stays where it was this frame");
        let next = dots.advance("orders>billing", 30.0, start + Duration::from_millis(1100));
        assert!((next - 7.0).abs() < 1e-9, "then on from there: {next}");
    }
}
