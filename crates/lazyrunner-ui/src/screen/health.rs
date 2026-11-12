//! The Stack screen's Health tab (#35): a card per app, in the profile's order - its p95, 5xx rate, memory and CPU
//! against its targets, each with a braille sparkline, the target dotted across it and what's over it red - and under
//! the cards the slowest endpoints over the last minutes, as bars with each app's p95 target ticked across. A breach
//! fills a bar toward the alert; an alert stays red until it's acknowledged (`a`, amber) or the app recovers (a green
//! glint).

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use lazyrunner_core::run::Controller;
use lazyrunner_core::run::format_duration;
use lazyrunner_core::run::health::{Breach, Metric};
use lazyrunner_core::run::requests::{Endpoint, endpoints};

use super::icons::{self, Icon};
use super::mouse::Target;
use super::seg::{Seg, aim, on, seg, width};
use super::theme::C;
use super::{Overlay, Ui};

/// A card's width, its gap, and its rows (borders included).
const CARD: usize = 40;
const GAP: usize = 1;
const CARD_ROWS: usize = 8;
/// A sparkline's cells: two samples each.
const SPARK: usize = 10;
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
/// A reading this close to its target is amber.
const NEAR: f64 = 0.8;
const EIGHTHS: [&str; 8] = ["▏", "▎", "▍", "▌", "▋", "▊", "▉", "█"];

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
    let apps = controller.apps.len();
    let count = apps + slowest(controller).len();
    let across = ui.health_columns.max(1);
    let next = match name {
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
        "a" => return true,
        "Enter" => {
            enter(ui, controller, picked);
            return true;
        }
        "l" | "RightArrow" => picked + 1,
        "h" | "LeftArrow" => picked.saturating_sub(1),
        "j" | "DownArrow" if picked < apps => (picked + across).min(apps).min(count.saturating_sub(1)),
        "j" | "DownArrow" => picked + 1,
        "k" | "UpArrow" if picked > apps => picked - 1,
        "k" | "UpArrow" if picked == apps => {
            let last = apps.saturating_sub(1);
            last - last % across
        }
        "k" | "UpArrow" => picked.saturating_sub(across),
        "g" | "Home" | "<" => 0,
        "G" | "End" | ">" => count.saturating_sub(1),
        _ => return false,
    };
    ui.overlay = Some(Overlay::Health { index: next.min(count.saturating_sub(1)), opened });
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
    let Some(Target::Item(at)) = target else { return false };
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

/// A series as a braille sparkline, two samples a cell, scaled so its target sits three quarters up: the target dotted
/// across where the line is under it, a cell over it red.
fn braille(series: &[f32], target: f64) -> Vec<Seg> {
    const LEFT: [u32; 4] = [0x40, 0x04, 0x02, 0x01];
    const RIGHT: [u32; 4] = [0x80, 0x20, 0x10, 0x08];
    let newest: Vec<f64> = series.iter().rev().take(SPARK * 2).rev().map(|&value| f64::from(value)).collect();
    let top = newest.iter().copied().fold(target * 4.0 / 3.0, f64::max).max(f64::EPSILON);
    let level = |value: f64| if value <= 0.0 { 0 } else { ((value / top * 4.0).ceil() as usize).clamp(1, 4) };
    let line = ((target / top * 4.0).round() as usize).clamp(1, 4);
    let padding = SPARK * 2 - newest.len();
    let samples: Vec<Option<f64>> = std::iter::repeat_n(None, padding).chain(newest.into_iter().map(Some)).collect();
    samples
        .chunks(2)
        .map(|pair| {
            let mut bits = 0;
            for (at, column) in [LEFT, RIGHT].iter().enumerate() {
                let height = pair.get(at).copied().flatten().map_or(0, level);
                bits |= column.iter().take(height).fold(0, |all, bit| all | bit);
            }
            let first = pair.first().copied().flatten().map_or(0, level);
            if first < line {
                bits |= LEFT[line - 1];
            }
            let over = pair.iter().flatten().any(|&value| value > target);
            let glyph = char::from_u32(0x2800 + bits).unwrap_or(' ');
            seg(glyph.to_string(), if over { C::Red } else { C::DarkCyan })
        })
        .collect()
}

/// How close a reading is to its target: red over it, amber near it.
fn tone(value: f64, target: f64) -> C {
    match () {
        _ if value > target => C::Red,
        _ if value > target * NEAR => C::Yellow,
        _ => C::Gray,
    }
}

/// A card's line for one metric: its name, its reading, its sparkline and its target.
fn metric_line(controller: &Controller, index: usize, metric: Metric, series: &[f32]) -> Vec<Seg> {
    let app = &controller.apps[index];
    let label = seg(format!("{:<7}", metric.name()), C::DarkGray);
    let target = metric.target(&app.health);
    let requests = matches!(metric, Metric::P95 | Metric::Errors);
    if requests && !app.traits().counts_requests {
        return vec![label, seg("no requests logged", C::DarkGray)];
    }
    let Some((value, _)) = controller.reading(index, metric) else {
        let why = if controller.states[index].is_up() { "no reading yet" } else { "-" };
        return vec![label, seg(why, C::DarkGray)];
    };
    let mut line = vec![label, seg(format!("{:>7} ", metric.say(value)), tone(value, target))];
    line.extend(braille(series, target));
    line.push(seg(format!(" / {}", metric.say(target)), C::DarkGray));
    line
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
        _ if Metric::ALL.iter().any(|&metric| controller.health.just_recovered(key, metric, now)) => C::Green,
        _ => C::DarkGray,
    }
}

/// App `index`'s card, `CARD` wide, its rows aimed at it.
fn card(ui: &Ui, controller: &Controller, index: usize, picked: bool) -> Vec<Vec<Seg>> {
    let (app, state) = (&controller.apps[index], &controller.states[index]);
    let edge = border(ui, controller, index, picked);
    let inner = CARD - 4;
    let kept = |series: Option<&VecDeque<f32>>| series.map(|series| series.iter().copied().collect::<Vec<f32>>());
    let p95 = kept(controller.health.p95.get(&app.key)).unwrap_or_default();
    let errors = kept(controller.health.errors.get(&app.key)).unwrap_or_default();
    let icon = icons::app(ui.nerd, app, controller.framework(&app.key));
    let (dot, dot_color) = if state.is_up() { ("●", C::Green) } else { ("○", C::DarkGray) };
    let mut title = vec![seg(format!("{icon} {}", app.name), if picked { C::White } else { C::Gray })];
    let gap = inner.saturating_sub(width(&title) + 1);
    title.push(seg(" ".repeat(gap), C::Gray));
    title.push(seg(dot, dot_color));
    let lines = [
        title,
        metric_line(controller, index, Metric::P95, &p95),
        metric_line(controller, index, Metric::Errors, &errors),
        metric_line(controller, index, Metric::Memory, &kept(Some(&state.memory_history)).unwrap_or_default()),
        metric_line(controller, index, Metric::Cpu, &kept(Some(&state.cpu_history)).unwrap_or_default()),
        state_line(ui, controller, index),
    ];
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

/// The header: the tabs, how many apps, the alerts.
fn header(ui: &Ui, controller: &Controller) -> Vec<Seg> {
    let mut segs = super::stack::tabs(super::stack::Tab::Health);
    let apps = controller.apps.len();
    segs.push(seg(format!("   {apps} app{}", if apps == 1 { "" } else { "s" }), C::DarkGray));
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
    segs.push(seg(" · a reading over its target for a minute is an alert", C::DarkGray));
    segs
}

pub fn rows(ui: &mut Ui, controller: &Controller, cells: usize, rows: usize) -> Vec<Vec<Seg>> {
    let Some((picked, _)) = view(ui) else { return Vec::new() };
    let hint = " h j k l choose · a acknowledge its alerts · Enter its column, an endpoint's requests · Tab the map · \
                Esc back";
    let mut lines = vec![super::columns::fit(header(ui, controller), cells)];
    lines.extend(super::viewbar::keys(hint, cells));
    lines.push(Vec::new());
    let top = lines.len();
    let across = ((cells.saturating_sub(1) + GAP) / (CARD + GAP)).max(1);
    ui.health_columns = across;
    let apps = controller.apps.len();
    let mut body: Vec<Vec<Seg>> = Vec::new();
    let mut picked_rows = 0..0;
    for (row, chunk) in (0..apps).collect::<Vec<_>>().chunks(across).enumerate() {
        let cards: Vec<Vec<Vec<Seg>>> = chunk.iter().map(|&at| card(ui, controller, at, at == picked)).collect();
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
    let room = rows.saturating_sub(top);
    let from = picked_rows.end.saturating_sub(room).min(picked_rows.start);
    lines.extend(body.into_iter().skip(from).take(room));
    lines.truncate(rows);
    lines.resize(rows, Vec::new());
    lines.into_iter().map(|line| aim(line, Target::Box)).collect()
}
