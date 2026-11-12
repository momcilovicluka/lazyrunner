//! The Stack screen's Health tab (#35): the stack in a line - how many apps are healthy, warning, failing, and its
//! last minute - its share of the machine's memory and CPU, then a card per app in the profile's order: its probe, its
//! readings against its targets (`△` near one, `▲` over it) and a chart of its last minute - the same metric on every
//! card (`v`), the target dotted across - beside the picked app's panel (its targets, slowest endpoints, alerts and
//! last profile). Under them the slowest endpoints across the stack and the log of alerts. A breach fills a bar toward
//! the alert; an alert stays red until it's acknowledged (`a`, amber) or the app recovers (a green glint).

use std::time::{Duration, Instant};

use lazyrunner_core::run::Controller;
use lazyrunner_core::run::format_duration;
use lazyrunner_core::run::health::{Breach, Metric};
use lazyrunner_core::run::requests::{Endpoint, endpoints};

use super::health_draw::{self, Chart, Condition, NEAR};
use super::icons::{self, Icon};
use super::mouse::Target;
use super::seg::{Seg, aim, on, seg, width};
use super::theme::C;
use super::{Overlay, Ui};

/// A card's width, its gap, and its rows (borders included).
const CARD: usize = 40;
const GAP: usize = 1;
const CARD_ROWS: usize = 10;
/// Seconds between a chart's samples.
const SAMPLE_SECONDS: usize = 2;
/// A card's chart's rows.
const CHART_ROWS: usize = 3;
/// The side panel's width, and the gap before it.
const PANEL: usize = 46;
const PANEL_GAP: usize = 2;
/// How far back the slowest endpoints look.
const ENDPOINTS_WINDOW: Duration = Duration::from_secs(5 * 60);
/// The most endpoints listed, and their bars' cells.
const MOST_ENDPOINTS: usize = 8;
const BAR: usize = 20;
/// A new alert's card pulses twice: each pulse this long.
const PULSE: Duration = Duration::from_millis(300);
const PULSES: u32 = 2;
/// The sustained-time bar's cells.
const SUSTAINED: usize = 10;
const EIGHTHS: [&str; 8] = ["▏", "▎", "▍", "▌", "▋", "▊", "▉", "█"];

/// The ways `s` orders the cards: the profile's order first (they don't move by themselves).
pub const SORTS: [&str; 5] = ["the profile's order", "worst first", "cpu", "memory", "p95"];

/// `,`: the picked app's targets being edited - which app, which of `TARGETS`, what's typed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Editing {
    pub app: usize,
    pub field: usize,
    pub text: String,
}

/// The cards' order by `SORTS[sort]`: a metric's highest first, the worst condition first.
pub fn order(controller: &Controller, sort: usize) -> Vec<usize> {
    let mut order: Vec<usize> = (0..controller.apps.len()).collect();
    let rank = |condition: Condition| match condition {
        Condition::Failing => 0,
        Condition::Warning => 1,
        Condition::Healthy => 2,
        Condition::Stopped => 3,
    };
    let value = |at: usize, metric: Metric| controller.reading(at, metric).map_or(-1.0, |(value, _)| value);
    match SORTS.get(sort).copied().unwrap_or_default() {
        "worst first" => order.sort_by_key(|&at| rank(health_draw::condition(controller, at))),
        "cpu" => order.sort_by(|&a, &b| value(b, Metric::Cpu).total_cmp(&value(a, Metric::Cpu))),
        "memory" => order.sort_by(|&a, &b| value(b, Metric::Memory).total_cmp(&value(a, Metric::Memory))),
        "p95" => order.sort_by(|&a, &b| value(b, Metric::P95).total_cmp(&value(a, Metric::P95))),
        _ => {}
    }
    order
}

/// A target's value as `,` shows it to edit.
fn target_text(controller: &Controller, app: usize, field: usize) -> String {
    let (_, metric) = lazyrunner_core::run::controller::TARGETS[field];
    let target = metric.target(&controller.apps[app].health);
    if metric == Metric::Errors { format!("{target}") } else { format!("{target:.0}") }
}

/// Whether the tab takes typed text: a target being edited.
pub fn typing(ui: &Ui) -> bool {
    ui.health_edit.is_some()
}

/// The keys while a target is edited: `j` `k` another, digits, `Enter` saves it, `Esc` done.
fn edit_key(ui: &mut Ui, controller: &mut Controller, name: &str) -> bool {
    let Some(mut editing) = ui.health_edit.clone() else { return false };
    let fields = lazyrunner_core::run::controller::TARGETS.len();
    match name {
        "Escape" | "," => {
            ui.health_edit = None;
            return true;
        }
        "j" | "DownArrow" | "k" | "UpArrow" => {
            let step = if matches!(name, "j" | "DownArrow") { 1 } else { fields - 1 };
            editing.field = (editing.field + step) % fields;
            editing.text = target_text(controller, editing.app, editing.field);
        }
        "Backspace" => {
            editing.text.pop();
        }
        "Enter" => {
            let (field, _) = lazyrunner_core::run::controller::TARGETS[editing.field];
            let key = controller.apps[editing.app].key.clone();
            match editing.text.trim().parse::<f64>() {
                Ok(value) => {
                    if let Err(why) = controller.set_health_target(&key, field, value) {
                        controller.add_log(why, lazyrunner_core::output::Hue::Yellow);
                    }
                }
                Err(_) => {
                    controller.add_log(format!("{field}: a number, please"), lazyrunner_core::output::Hue::Yellow)
                }
            }
            editing.text = target_text(controller, editing.app, editing.field);
        }
        typed if typed.len() == 1 && typed.chars().all(|ch| ch.is_ascii_digit() || ch == '.') => {
            editing.text.push_str(typed)
        }
        _ => {}
    }
    ui.health_edit = Some(editing);
    true
}

/// The Health tab, on the first app.
pub fn open(ui: &mut Ui, _controller: &mut Controller) {
    ui.stack_tab = super::stack::Tab::Health;
    ui.overlay = Some(Overlay::Health { index: 0, opened: ui.now });
}

fn view(ui: &Ui) -> Option<(usize, Instant)> {
    match &ui.overlay {
        Some(Overlay::Health { index, opened }) => Some((*index, *opened)),
        _ => None,
    }
}

/// The slowest endpoints of the last minutes, slowest first.
fn slowest(controller: &Controller) -> Vec<Endpoint> {
    let since = chrono::Local::now() - chrono::Duration::from_std(ENDPOINTS_WINDOW).unwrap_or_default();
    let recent = controller.requests.items.iter().filter(|request| request.at >= since);
    let mut list = endpoints(recent);
    list.sort_by(|a, b| b.p95.total_cmp(&a.p95));
    list.truncate(MOST_ENDPOINTS);
    list
}

/// The keys of the Health tab; false for one it doesn't use.
pub fn key(ui: &mut Ui, controller: &mut Controller, name: &str) -> bool {
    let Some((picked, opened)) = view(ui) else { return false };
    if edit_key(ui, controller, name) {
        return true;
    }
    let apps = controller.apps.len();
    let count = apps + slowest(controller).len();
    let across = ui.health_columns.max(1);
    let order = order(controller, ui.health_sort);
    let picked_at = if picked < apps { order.iter().position(|&at| at == picked).unwrap_or(0) } else { picked };
    let next = match name {
        "," if picked < apps => {
            ui.health_edit = Some(Editing { app: picked, field: 0, text: target_text(controller, picked, 0) });
            return true;
        }
        "s" => {
            ui.health_sort = (ui.health_sort + 1) % SORTS.len();
            controller
                .add_log(format!("Health cards: {}", SORTS[ui.health_sort]), lazyrunner_core::output::Hue::DarkGray);
            return true;
        }
        "Escape" | "q" | "K" => {
            ui.overlay = None;
            return true;
        }
        "Tab" => {
            super::stack::open(ui, controller);
            return true;
        }
        "a" if picked < apps => {
            let key = controller.apps[picked].key.clone();
            controller.acknowledge_alerts(&key);
            return true;
        }
        "p" if picked < apps => {
            let key = controller.apps[picked].key.clone();
            super::flame::start(ui, controller, &key);
            return true;
        }
        "a" => return true,
        "v" => {
            ui.health_chart = (ui.health_chart + 1) % Chart::ALL.len();
            return true;
        }
        "Enter" => {
            enter(ui, controller, picked);
            return true;
        }
        "l" | "RightArrow" => picked_at + 1,
        "h" | "LeftArrow" => picked_at.saturating_sub(1),
        "j" | "DownArrow" if picked_at < apps => (picked_at + across).min(apps).min(count.saturating_sub(1)),
        "j" | "DownArrow" => picked_at + 1,
        "k" | "UpArrow" if picked_at > apps => picked_at - 1,
        "k" | "UpArrow" if picked_at == apps => {
            let last = apps.saturating_sub(1);
            last - last % across
        }
        "k" | "UpArrow" => picked_at.saturating_sub(across),
        "g" | "Home" | "<" => 0,
        "G" | "End" | ">" => count.saturating_sub(1),
        _ => return false,
    };
    let next = next.min(count.saturating_sub(1));
    let next = if next < apps { order[next] } else { next };
    ui.overlay = Some(Overlay::Health { index: next, opened });
    true
}

/// `Enter`: a card's column, an endpoint's row in the requests view.
fn enter(ui: &mut Ui, controller: &mut Controller, picked: usize) {
    let apps = controller.apps.len();
    if picked < apps {
        ui.overlay = None;
        ui.focus = picked;
        ui.motion.flash(super::motion::Spot::Title(picked), ui.now);
        return;
    }
    let Some(endpoint) = slowest(controller).into_iter().nth(picked - apps) else { return };
    let mut view = super::requests::View {
        tab: super::requests::Tab::Endpoints,
        app: Some(endpoint.app.clone()),
        ..Default::default()
    };
    view.index = super::requests::endpoint_index(&view, controller, &endpoint).unwrap_or(0);
    ui.overlay = Some(Overlay::Requests(Box::new(view)));
}

/// A click picks a card or an endpoint; a double click does what `Enter` does.
pub fn click(ui: &mut Ui, controller: &mut Controller, target: Option<Target>, double: bool) -> bool {
    let Some((_, opened)) = view(ui) else { return false };
    let at = match target {
        Some(Target::Item(at) | Target::Chart { app: at, .. }) => at,
        _ => return false,
    };
    ui.overlay = Some(Overlay::Health { index: at, opened });
    if double {
        enter(ui, controller, at);
    }
    true
}

/// Whether it moves: a breach's bar filling, a new alert's pulses, a recovery's glint.
pub fn animating(ui: &Ui, controller: &Controller) -> bool {
    view(ui).is_some()
        && super::stack::moving(ui)
        && (!controller.health.breaches.is_empty() || !controller.health.recovered.is_empty())
}

/// How close a reading is to its target: red over it, amber near it.
fn tone(value: f64, target: f64) -> C {
    match () {
        _ if value > target => C::Red,
        _ if value > target * NEAR => C::Yellow,
        _ => C::Gray,
    }
}

/// A reading said with its mark: `△` near its target, `▲` over it; `-` without one.
fn reading(controller: &Controller, index: usize, metric: Metric, label: &str, rolled: &Rolled) -> Vec<Seg> {
    let label = seg(format!("{label} "), C::DarkGray);
    match controller.reading(index, metric) {
        Some((value, target)) => {
            let mark = match () {
                _ if value > target => " ▲",
                _ if value > target * NEAR => " △",
                _ => "",
            };
            let (shown, tint) = rolled.get(&(index, metric)).copied().unwrap_or((value, None));
            vec![label, seg(format!("{}{mark}", metric.say(shown)), tint.unwrap_or_else(|| tone(value, target)))]
        }
        None => vec![label, seg("-", C::DarkGray)],
    }
}

/// A card's state after its name: its probe's answer, or that it's stopped or failed.
fn probe(controller: &Controller, index: usize) -> Vec<Seg> {
    let state = &controller.states[index];
    match (state.is_up(), controller.last_probe(&controller.apps[index].key)) {
        (true, Some(probe)) if probe.ok => vec![seg(format!("● up {} ms", probe.ms), C::Green)],
        (true, Some(_)) => vec![seg("● not responding", C::Red)],
        (true, None) => vec![seg("● up", C::Green)],
        _ if matches!(
            state.status,
            lazyrunner_core::run::app::Status::Exited(_) | lazyrunner_core::run::app::Status::Error(_)
        ) =>
        {
            vec![seg("✗ stopped with an error", C::Red)]
        }
        _ if state.is_running() => vec![seg("◌ starting", C::Yellow)],
        _ => vec![seg("○ stopped", C::DarkGray)],
    }
}

/// A card's readings in two lines: its CPU and memory; its p95, 5xx rate and requests a second.
/// Each app's readings as they roll to their new values, and their tint a moment after one changed.
type Rolled = std::collections::HashMap<(usize, Metric), (f64, Option<C>)>;

fn readings(controller: &Controller, index: usize, rolled: &Rolled) -> [Vec<Seg>; 2] {
    let mut first = reading(controller, index, Metric::Cpu, "cpu", rolled);
    first.push(seg("   ", C::Gray));
    first.extend(reading(controller, index, Metric::Memory, "mem", rolled));
    if !controller.apps[index].traits().counts_requests {
        return [first, vec![seg("no requests logged (not a .NET API)", C::DarkGray)]];
    }
    let mut second = reading(controller, index, Metric::P95, "p95", rolled);
    second.push(seg("  ", C::Gray));
    second.extend(reading(controller, index, Metric::Errors, "5xx", rolled));
    let rate = controller.states[index].request_history.back().copied().unwrap_or(0.0);
    second.push(seg(format!("  {rate:.1} req/s"), C::Gray));
    [first, second]
}

/// A card's chart of `chart` over its last minute, and the line under it.
fn chart_rows(ui: &Ui, controller: &Controller, index: usize, inner: usize) -> Vec<Vec<Seg>> {
    let chart = Chart::ALL[ui.health_chart % Chart::ALL.len()];
    let caption = |extra: String| vec![seg(format!("{} · last 60 s{extra} · v another", chart.name()), C::DarkGray)];
    match health_draw::series(controller, index, chart).filter(|(values, _)| !values.is_empty()) {
        Some((values, target)) => {
            let rows = health_draw::area(&values, target, (inner, CHART_ROWS), C::DarkCyan);
            let mut rows: Vec<Vec<Seg>> = rows
                .into_iter()
                .map(|row| {
                    row.into_iter()
                        .enumerate()
                        .map(|(cell, part)| part.at(Target::Chart { app: index, cell }))
                        .collect()
                })
                .collect();
            let hovered = match ui.hover {
                Some(Target::Chart { app, cell }) if app == index => Some(cell),
                _ => None,
            };
            match hovered {
                Some(cell) => rows.push(moment(&values, target, chart, (cell, inner))),
                None => {
                    let target = target
                        .zip(chart.metric())
                        .map_or(String::new(), |(target, metric)| format!(" · ┄ {}", metric.say(target)));
                    rows.push(caption(target));
                }
            }
            rows
        }
        None => {
            let why = if controller.states[index].is_up() { "no samples yet" } else { "not running" };
            let mut rows: Vec<Vec<Seg>> = (0..CHART_ROWS).map(|_| Vec::new()).collect();
            rows[CHART_ROWS / 2] = vec![seg(format!("{:^inner$}", format!("{} · {why}", chart.name())), C::DarkGray)];
            rows.push(caption(String::new()));
            rows
        }
    }
}

/// The reading under the pointer on a chart: its value against the target, and how long ago it was (samples are 2 s
/// apart).
fn moment(values: &[f32], target: Option<f64>, chart: Chart, (cell, cells): (usize, usize)) -> Vec<Seg> {
    let at = (cell * 2 * values.len() / (cells * 2).max(1)).min(values.len().saturating_sub(1));
    let value = f64::from(values[at]);
    let ago = (values.len() - 1 - at) * SAMPLE_SECONDS;
    let said = chart.metric().map_or_else(|| format!("{value:.1}/s"), |metric| metric.say(value));
    let color = target.map_or(C::White, |target| tone(value, target));
    let against =
        target.zip(chart.metric()).map_or(String::new(), |(target, metric)| format!(" of {}", metric.say(target)));
    vec![
        seg(format!("{} ", chart.name()), C::DarkGray),
        seg(said, color),
        seg(format!("{against} · {ago} s ago"), C::DarkGray),
    ]
}

/// The worst of an app's breaches: an open alert, an acknowledged one, then the breach nearest an alert.
fn worst<'a>(controller: &'a Controller, key: &'a str, now: Instant) -> Option<&'a Breach> {
    let rank = |breach: &Breach| (breach.alert() && !breach.acknowledged, breach.alert(), breach.progress(now));
    controller.health.of(key).max_by(|a, b| rank(a).partial_cmp(&rank(b)).unwrap_or(std::cmp::Ordering::Equal))
}

/// A card's last line: its worst breach - an alert, or the bar filling toward one - a recovery, or that it's well.
fn state_line(ui: &Ui, controller: &Controller, index: usize) -> Vec<Seg> {
    let key = &controller.apps[index].key;
    let now = ui.now;
    let more = controller.health.of(key).count().saturating_sub(1);
    let more = if more > 0 { format!(" +{more}") } else { String::new() };
    if let Some(breach) = worst(controller, key, now) {
        let what = format!("{} {}", breach.metric.name(), breach.metric.say(breach.value));
        let lasted = format_duration(now.saturating_duration_since(breach.since).as_secs_f64());
        if breach.alert() {
            let (icon, hint, color) = if breach.acknowledged {
                (Icon::Acknowledged, " · ack", C::Yellow)
            } else {
                (Icon::Alert, " (a)", C::Red)
            };
            let text = format!("{} {what} for {lasted}{more}{hint}", icons::icon(ui.nerd, icon));
            return vec![seg(text, color)];
        }
        let filled = (breach.progress(now) * SUSTAINED as f64).round() as usize;
        let bar = format!("{}{}", "▰".repeat(filled), "▱".repeat(SUSTAINED.saturating_sub(filled)));
        return vec![seg(bar, C::Yellow), seg(format!(" {what} for {lasted}{more}"), C::Yellow)];
    }
    if Metric::ALL.iter().any(|&metric| controller.health.just_recovered(key, metric, now)) {
        return vec![seg(format!("{} back within its targets", icons::icon(ui.nerd, Icon::Ok)), C::Green)];
    }
    if !controller.states[index].is_up() {
        return vec![seg("not running", C::DarkGray)];
    }
    vec![seg(format!("{} within its targets", icons::icon(ui.nerd, Icon::Ok)), C::DarkGreen)]
}

/// A card's border: a new alert pulsing, picked, an alert (red, acknowledged amber), a recovery's glint, or quiet.
fn border(ui: &Ui, controller: &Controller, index: usize, picked: bool) -> C {
    let key = &controller.apps[index].key;
    let now = ui.now;
    let alerts: Vec<&Breach> = controller.health.of(key).filter(|breach| breach.alert()).collect();
    let pulsing = alerts.iter().filter_map(|alert| alert.fired).any(|fired| {
        let since = now.saturating_duration_since(fired);
        super::stack::moving(ui)
            && since < PULSE * PULSES * 2
            && (since.as_millis() / PULSE.as_millis()).is_multiple_of(2)
    });
    match () {
        _ if pulsing => C::White,
        _ if picked => C::Cyan,
        _ if alerts.iter().any(|alert| !alert.acknowledged) => C::Red,
        _ if !alerts.is_empty() => C::Yellow,
        _ if health_draw::condition(controller, index) == Condition::Failing => C::Red,
        _ if health_draw::condition(controller, index) == Condition::Warning => C::DarkYellow,
        _ if Metric::ALL.iter().any(|&metric| controller.health.just_recovered(key, metric, now)) => C::Green,
        _ => C::DarkGray,
    }
}

/// App `index`'s card, `CARD` wide, its rows aimed at it.
fn card(ui: &Ui, controller: &Controller, (index, picked): (usize, bool), rolled: &Rolled) -> Vec<Vec<Seg>> {
    let app = &controller.apps[index];
    let edge = border(ui, controller, index, picked);
    let inner = CARD - 4;
    let icon = icons::app(ui.nerd, app, controller.framework(&app.key));
    let mut title = vec![seg(format!("{icon} {}", app.name), if picked { C::White } else { C::Gray })];
    let state_said = probe(controller, index);
    let gap = inner.saturating_sub(width(&title) + width(&state_said));
    title.push(seg(" ".repeat(gap), C::Gray));
    title.extend(state_said);
    let [first, second] = readings(controller, index, rolled);
    let mut lines = vec![title, first, second];
    lines.extend(chart_rows(ui, controller, index, inner));
    lines.push(state_line(ui, controller, index));
    let mut rows = vec![vec![seg(format!("╭{}╮", "─".repeat(CARD - 2)), edge)]];
    for line in lines {
        let mut row = vec![seg("│ ", edge)];
        row.extend(super::columns::fit(line, inner));
        row.push(seg(" │", edge));
        rows.push(row);
    }
    rows.push(vec![seg(format!("╰{}╯", "─".repeat(CARD - 2)), edge)]);
    rows.into_iter().map(|row| aim(row, Target::Item(index))).collect()
}

/// An endpoint's p95 as an eighth-block bar against `scale`, its app's target ticked across it, what's over it red.
fn bar(p95: f64, target: f64, scale: f64) -> Vec<Seg> {
    let eighths = |value: f64| ((value / scale * (BAR * 8) as f64).round() as usize).min(BAR * 8);
    let (filled, tick) = (eighths(p95), (eighths(target) / 8).min(BAR - 1));
    (0..BAR)
        .map(|cell| {
            let here = filled.saturating_sub(cell * 8).min(8);
            let over = cell >= tick && p95 > target;
            match here {
                0 if cell == tick => seg("┊", C::DarkGray),
                0 => seg(" ", C::Gray),
                full => seg(EIGHTHS[full - 1], if over { C::Red } else { C::DarkCyan }),
            }
        })
        .collect()
}

/// The slowest endpoints, a row each: its app, method and path, its bar and its p95.
fn endpoint_rows(
    controller: &Controller,
    list: &[Endpoint],
    first: usize,
    picked: usize,
    cells: usize,
) -> Vec<Vec<Seg>> {
    let target = |app: &str| controller.app(app).map_or(1000.0, |app| f64::from(app.health.p95_ms));
    let scale = list.iter().map(|endpoint| endpoint.p95.max(target(&endpoint.app))).fold(1.0, f64::max);
    let name = |app: &str| controller.app(app).map_or_else(|| app.to_string(), |app| app.name.clone());
    list.iter()
        .enumerate()
        .map(|(at, endpoint)| {
            let path = format!("{:<10} {:<6} {}", name(&endpoint.app), endpoint.method, endpoint.endpoint);
            let room = cells.saturating_sub(BAR + 14);
            let lit = first + at == picked;
            let text = if lit { on(path, C::FocusText, C::FocusBack) } else { seg(path, C::Gray) };
            let mut row = vec![seg(" ", C::Gray)];
            row.extend(super::columns::fit(vec![text], room));
            row.push(seg("  ", C::Gray));
            let goal = target(&endpoint.app);
            row.extend(bar(endpoint.p95, goal, scale));
            row.push(seg(format!(" {:>7.0} ms", endpoint.p95), tone(endpoint.p95, goal)));
            aim(row, Target::Item(first + at))
        })
        .collect()
}

/// The panel's chip while the stack has alerts (`! K 1 alert`), red while one isn't acknowledged; none on this tab.
pub fn chip(ui: &Ui, controller: &Controller) -> Option<(String, C)> {
    let alerts: Vec<&Breach> = controller.health.alerts().collect();
    if alerts.is_empty() || view(ui).is_some() {
        return None;
    }
    let open = alerts.iter().any(|alert| !alert.acknowledged);
    let icon = icons::icon(ui.nerd, if open { Icon::Alert } else { Icon::Acknowledged });
    let count = alerts.len();
    let text = format!("{icon} K {count} alert{}", if count == 1 { "" } else { "s" });
    Some((text, if open { C::Red } else { C::Yellow }))
}

/// The header: the tabs, how many apps are healthy, warning, failing and stopped, the stack's last minute, the alerts.
fn header(ui: &Ui, controller: &Controller) -> Vec<Seg> {
    let mut segs = super::stack::tabs(super::stack::Tab::Health);
    let count = |wanted: Condition| {
        (0..controller.apps.len()).filter(|&at| health_draw::condition(controller, at) == wanted).count()
    };
    segs.push(seg(format!("   ● {} healthy", count(Condition::Healthy)), C::Green));
    let warning = count(Condition::Warning);
    segs.push(seg(format!(" · △ {warning} warning"), if warning > 0 { C::Yellow } else { C::DarkGray }));
    let failing = count(Condition::Failing);
    segs.push(seg(format!(" · ✗ {failing} failing"), if failing > 0 { C::Red } else { C::DarkGray }));
    let stopped = count(Condition::Stopped);
    if stopped > 0 {
        segs.push(seg(format!(" · {stopped} stopped"), C::DarkGray));
    }
    if let Some(sort) = SORTS.get(ui.health_sort).filter(|_| ui.health_sort > 0) {
        segs.push(seg(format!(" · sorted: {sort} (s)"), C::Cyan));
    }
    if let Some(minute) = health_draw::stack_minute(controller) {
        segs.push(seg(format!("   {minute}"), C::Gray));
    }
    let alerts: Vec<&Breach> = controller.health.alerts().collect();
    let open = alerts.iter().filter(|alert| !alert.acknowledged).count();
    match (alerts.len(), open) {
        (0, _) => segs.push(seg(" · no alerts", C::DarkGreen)),
        (all, 0) => segs.push(seg(format!(" · {all} acknowledged"), C::Yellow)),
        (_, open) => {
            let icon = icons::icon(ui.nerd, Icon::Alert);
            segs.push(seg(format!(" · {icon} {open} alert{}", if open == 1 { "" } else { "s" }), C::Red));
        }
    }
    segs
}

pub fn rows(ui: &mut Ui, controller: &Controller, cells: usize, rows: usize) -> Vec<Vec<Seg>> {
    let Some((picked, _)) = view(ui) else { return Vec::new() };
    let hint = if ui.health_edit.is_some() {
        " j k another target · type a number · Enter saves it (on this machine) · Esc done"
    } else {
        " h j k l choose · v another chart · s sort · , its targets · a acknowledge its alerts · p profile it · Enter its \
         column, an endpoint's requests · Tab the map · Esc back"
    };
    let mut lines = vec![super::columns::fit(header(ui, controller), cells)];
    lines.extend(super::viewbar::keys(hint, cells));
    lines.extend(health_draw::capacity(controller, (ui.machine_memory)(), cells));
    lines.push(Vec::new());
    let top = lines.len();
    let side = cells > CARD + PANEL_GAP + PANEL;
    let card_cells = if side { cells - PANEL_GAP - PANEL } else { cells };
    let across = ((card_cells.saturating_sub(1) + GAP) / (CARD + GAP)).max(1);
    ui.health_columns = across;
    let apps = controller.apps.len();
    let mut rolled = Rolled::new();
    for at in 0..apps {
        for metric in Metric::ALL {
            if let Some((value, _)) = controller.reading(at, metric) {
                let id = format!("health:{}:{}", controller.apps[at].key, metric.name());
                rolled.insert((at, metric), ui.roll(&id, value, false, true));
            }
        }
    }
    let mut body: Vec<Vec<Seg>> = Vec::new();
    let mut picked_rows = 0..0;
    for (row, chunk) in order(controller, ui.health_sort).chunks(across).enumerate() {
        let cards: Vec<Vec<Vec<Seg>>> =
            chunk.iter().map(|&at| card(ui, controller, (at, at == picked), &rolled)).collect();
        if chunk.contains(&picked) {
            picked_rows = row * CARD_ROWS..(row + 1) * CARD_ROWS;
        }
        for line in 0..CARD_ROWS {
            let mut joined = vec![seg(" ", C::Gray)];
            for (at, card) in cards.iter().enumerate() {
                if at > 0 {
                    joined.push(seg(" ".repeat(GAP), C::Gray));
                }
                joined.extend(card[line].clone());
            }
            body.push(joined);
        }
    }
    let picked_app = picked.min(apps.saturating_sub(1));
    let editing = ui
        .health_edit
        .as_ref()
        .filter(|editing| editing.app == picked_app)
        .map(|editing| (editing.field, editing.text.as_str()));
    let panel = if apps > 0 { health_draw::panel(controller, picked_app, ui.now, PANEL, editing) } else { Vec::new() };
    if side {
        let height = body.len().max(panel.len());
        body.resize(height, Vec::new());
        for (row, line) in body.iter_mut().enumerate() {
            let mut joined = super::columns::fit(std::mem::take(line), card_cells);
            joined.push(seg(" ".repeat(PANEL_GAP), C::Gray));
            joined.extend(panel.get(row).cloned().unwrap_or_default());
            *line = joined;
        }
    } else {
        body.push(Vec::new());
        body.extend(panel);
    }
    let list = slowest(controller);
    body.push(Vec::new());
    body.push(vec![seg(" SLOWEST ENDPOINTS · p95 over the last 5 minutes, ┊ its app's target", C::DarkGray)]);
    if list.is_empty() {
        body.push(vec![seg(" No requests in the last 5 minutes.", C::DarkGray)]);
    }
    if picked >= apps {
        let at = body.len() + picked - apps;
        picked_rows = at..at + 1;
    }
    body.extend(endpoint_rows(controller, &list, apps, picked, cells));
    body.push(Vec::new());
    body.extend(health_draw::log(controller, cells));
    let room = rows.saturating_sub(top);
    let from = picked_rows.end.saturating_sub(room).min(picked_rows.start);
    lines.extend(body.into_iter().skip(from).take(room));
    lines.truncate(rows);
    lines.resize(rows, Vec::new());
    lines.into_iter().map(|line| aim(line, Target::Box)).collect()
}
