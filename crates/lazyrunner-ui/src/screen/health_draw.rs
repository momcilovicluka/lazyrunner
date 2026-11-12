//! The Health tab's pictures (#35): each card's chart - a filled braille area of the last minute, the target dotted
//! across, what's over it red - the stack's share of the machine's memory and CPU, the picked app's side panel (its
//! targets against its readings, its slowest endpoints, its alerts, its last profile), and the log of alerts.

use std::time::Duration;

use lazyrunner_core::run::Controller;
use lazyrunner_core::run::app::Status;
use lazyrunner_core::run::health::Metric;
use lazyrunner_core::run::profile::short;
use lazyrunner_core::run::requests::endpoints;

use super::seg::{Seg, seg};
use super::theme::C;

/// What a card's chart can draw: `v` steps through them, the same on every card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Chart {
    Cpu,
    Memory,
    P95,
    Errors,
    Requests,
}

impl Chart {
    pub const ALL: [Self; 5] = [Self::Cpu, Self::Memory, Self::P95, Self::Errors, Self::Requests];

    pub fn name(self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            Self::Memory => "memory",
            Self::P95 => "p95",
            Self::Errors => "5xx",
            Self::Requests => "requests/s",
        }
    }

    pub fn metric(self) -> Option<Metric> {
        match self {
            Self::Cpu => Some(Metric::Cpu),
            Self::Memory => Some(Metric::Memory),
            Self::P95 => Some(Metric::P95),
            Self::Errors => Some(Metric::Errors),
            Self::Requests => None,
        }
    }
}

/// The apps' colours in the capacity bars.
const APPS: [C; 8] = [C::Cyan, C::Green, C::Yellow, C::Magenta, C::Blue, C::DarkCyan, C::DarkGreen, C::DarkYellow];
/// Braille dots from the bottom up: the left column, the right one.
const LEFT: [u32; 4] = [0x40, 0x04, 0x02, 0x01];
const RIGHT: [u32; 4] = [0x80, 0x20, 0x10, 0x08];
/// The capacity bar's cells at most.
const CAPACITY: usize = 50;
/// The side panel's rows: its slowest endpoints and the log's lines.
const PANEL_ENDPOINTS: usize = 5;
pub const LOG_LINES: usize = 4;
const RECENT: Duration = Duration::from_secs(60);
const ENDPOINTS: Duration = Duration::from_secs(5 * 60);
/// A reading this close to its target is a warning.
pub const NEAR: f64 = 0.8;

/// How an app is: failing (crashed, or an alert not acknowledged), warning (a breach, an acknowledged alert, a reading
/// near its target), healthy, or stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Condition {
    Failing,
    Warning,
    Healthy,
    Stopped,
}

pub fn condition(controller: &Controller, index: usize) -> Condition {
    let state = &controller.states[index];
    let key = &controller.apps[index].key;
    let open_alert = controller.health.of(key).any(|breach| breach.alert() && !breach.acknowledged);
    if matches!(state.status, Status::Exited(_) | Status::Error(_)) || open_alert {
        return Condition::Failing;
    }
    if !state.is_up() {
        return Condition::Stopped;
    }
    let near = Metric::ALL
        .iter()
        .any(|&metric| controller.reading(index, metric).is_some_and(|(value, target)| value > target * NEAR));
    if controller.health.of(key).next().is_some() || near { Condition::Warning } else { Condition::Healthy }
}

/// App `index`'s series for `chart` (oldest first) and its target; none when it has no such reading.
pub fn series(controller: &Controller, index: usize, chart: Chart) -> Option<(Vec<f32>, Option<f64>)> {
    let (app, state) = (&controller.apps[index], &controller.states[index]);
    let kept = |series: &std::collections::VecDeque<f32>| series.iter().copied().collect::<Vec<f32>>();
    let target = chart.metric().map(|metric| metric.target(&app.health));
    let values = match chart {
        Chart::Cpu => kept(&state.cpu_history),
        Chart::Memory => kept(&state.memory_history),
        Chart::Requests if app.traits().counts_requests => kept(&state.request_history),
        Chart::P95 if app.traits().counts_requests => kept(controller.health.p95.get(&app.key)?),
        Chart::Errors if app.traits().counts_requests => kept(controller.health.errors.get(&app.key)?),
        _ => return None,
    };
    Some((values, target))
}

/// A filled braille area of `series`, `cells` wide and `rows` high (the top row first), stretched across the width:
/// the target dotted across where the area is under it, a cell over it red.
pub fn area(series: &[f32], target: Option<f64>, (cells, rows): (usize, usize), color: C) -> Vec<Vec<Seg>> {
    let columns = cells * 2;
    let levels = rows * 4;
    let blank = || vec![seg(" ".repeat(cells), C::Gray)];
    if series.is_empty() || columns == 0 {
        return (0..rows).map(|_| blank()).collect();
    }
    let value = |column: usize| f64::from(series[(column * series.len() / columns).min(series.len() - 1)]);
    let highest = series.iter().map(|&value| f64::from(value)).fold(0.0, f64::max);
    let top = target.map_or(highest, |target| highest.max(target * 4.0 / 3.0)).max(f64::EPSILON);
    let height = |column: usize| {
        let value = value(column);
        let level = ((value / top) * levels as f64).round() as usize;
        if value > 0.0 { level.clamp(1, levels) } else { 0 }
    };
    let line = target.map(|target| (((target / top) * levels as f64).round() as usize).clamp(1, levels));
    (0..rows)
        .map(|row| {
            let base = (rows - 1 - row) * 4;
            (0..cells)
                .map(|cell| {
                    let mut bits = 0;
                    for (side, dots) in [LEFT, RIGHT].iter().enumerate() {
                        let filled = height(cell * 2 + side).saturating_sub(base).min(4);
                        bits |= dots.iter().take(filled).fold(0, |all, dot| all | dot);
                    }
                    if let Some(line) = line.filter(|&line| line > base && line <= base + 4)
                        && height(cell * 2) < line
                        && cell % 2 == 0
                    {
                        bits |= LEFT[line - base - 1];
                    }
                    let over = target.is_some_and(|target| (0..2).any(|side| value(cell * 2 + side) > target));
                    let glyph = char::from_u32(0x2800 + bits).unwrap_or(' ');
                    seg(glyph.to_string(), if over { C::Red } else { color })
                })
                .collect()
        })
        .collect()
}

/// A value said in MB or GB.
fn megabytes(mb: f64) -> String {
    if mb >= 1024.0 { format!("{:.1} GB", mb / 1024.0) } else { format!("{mb:.0} MB") }
}

/// The stack's share of the machine: its memory and its CPU, a segment per app in its colour, the rest free.
pub fn capacity(controller: &Controller, machine_memory: f32, cells: usize) -> Vec<Vec<Seg>> {
    let width = cells.saturating_sub(34).min(CAPACITY);
    if width < 10 {
        return Vec::new();
    }
    let total_memory = f64::from(machine_memory).max(1.0);
    let used: Vec<(usize, f64, f64)> = (0..controller.apps.len())
        .filter(|&at| controller.states[at].is_running())
        .map(|at| {
            let state = &controller.states[at];
            (at, f64::from(state.memory_mb.unwrap_or(0.0)), f64::from(state.cpu.unwrap_or(0.0)))
        })
        .collect();
    let bar = |label: &str, share: &dyn Fn(&(usize, f64, f64)) -> f64, said: String| {
        let mut line = vec![seg(format!(" {label:<7}▕"), C::DarkGray)];
        let mut drawn = 0;
        for each in &used {
            let cells_of = (share(each) * width as f64).round() as usize;
            let cells_of = cells_of.min(width - drawn);
            if cells_of > 0 {
                line.push(seg("█".repeat(cells_of), APPS[each.0 % APPS.len()]));
                drawn += cells_of;
            }
        }
        line.push(seg("░".repeat(width - drawn), C::DarkGray));
        line.push(seg(format!("▏ {said}"), C::Gray));
        line
    };
    let memory: f64 = used.iter().map(|(_, memory, _)| memory).sum();
    let cpu: f64 = used.iter().map(|(_, _, cpu)| cpu).sum();
    let free = megabytes((total_memory - memory).max(0.0));
    let memory_said = format!("{} of {} · {free} free", megabytes(memory), megabytes(total_memory));
    let mut legend = vec![seg(format!(" {:<7} ", ""), C::Gray)];
    for (at, ..) in &used {
        legend.push(seg("■ ", APPS[at % APPS.len()]));
        legend.push(seg(format!("{}  ", controller.apps[*at].name), C::DarkGray));
    }
    vec![
        bar("memory", &|(_, memory, _)| memory / total_memory, memory_said),
        bar("cpu", &|(_, _, cpu)| cpu / 100.0, format!("{cpu:.0}% of the machine · {:.0}% headroom", 100.0 - cpu)),
        legend,
    ]
}

/// The stack's last minute: its p95, its requests a second and their share of 5xx, as the header says them.
pub fn stack_minute(controller: &Controller) -> Option<String> {
    let since = chrono::Local::now() - chrono::Duration::from_std(RECENT).unwrap_or_default();
    let mut times: Vec<f64> =
        controller.requests.items.iter().filter(|request| request.at >= since).map(|request| request.ms).collect();
    if times.is_empty() {
        return None;
    }
    let failed =
        controller.requests.items.iter().filter(|request| request.at >= since && request.status >= 500).count();
    let count = times.len();
    times.sort_by(f64::total_cmp);
    let p95 = times[((count as f64 * 0.95).ceil() as usize).clamp(1, count) - 1];
    let rate = count as f64 / RECENT.as_secs_f64();
    Some(format!("p95 {p95:.0} ms · {rate:.1} req/s · {:.1}% 5xx", failed as f64 / count as f64 * 100.0))
}

/// The side panel for app `index`: its targets against its readings, its slowest endpoints, its alerts, its last
/// profile.
pub fn panel(
    controller: &Controller,
    index: usize,
    now: std::time::Instant,
    width: usize,
    editing: Option<(usize, &str)>,
) -> Vec<Vec<Seg>> {
    let app = &controller.apps[index];
    let mut lines = vec![vec![seg(format!(" {} · targets", app.name), C::White)]];
    if let Some((field, text)) = editing {
        lines[0].push(seg(" · editing", C::Cyan));
        for (at, (name, metric)) in lazyrunner_core::run::controller::TARGETS.iter().enumerate() {
            let line = if at == field {
                vec![
                    seg(format!(" ▸ {name:<12}"), C::White),
                    super::seg::on(format!(" {text}▏"), C::FocusText, C::FocusBack),
                ]
            } else {
                vec![seg(format!("   {name:<12} {}", metric.say(metric.target(&app.health))), C::Gray)]
            };
            lines.push(line);
        }
        lines.push(vec![seg("   errorRate is a share: 0.02 is 2%", C::DarkGray)]);
        return lines.into_iter().map(|line| super::columns::fit(line, width)).collect();
    }
    lines.push(vec![seg(format!(" {:<14}{:>10}{:>10}", "", "target", "now"), C::DarkGray)]);
    for metric in Metric::ALL {
        let target = metric.target(&app.health);
        let (now_said, mark) = match controller.reading(index, metric) {
            Some((value, _)) if value > target => (metric.say(value), seg(" ✗", C::Red)),
            Some((value, _)) if value > target * NEAR => (metric.say(value), seg(" △", C::Yellow)),
            Some((value, _)) => (metric.say(value), seg(" ✓", C::Green)),
            None => ("-".to_string(), seg("", C::Gray)),
        };
        let mut row = vec![seg(format!(" {:<14}{:>10}{:>10}", metric.name(), metric.say(target), now_said), C::Gray)];
        row.push(mark);
        lines.push(row);
    }
    lines.push(Vec::new());
    lines.push(vec![seg(" SLOWEST ENDPOINTS · 5 min", C::DarkCyan)]);
    let since = chrono::Local::now() - chrono::Duration::from_std(ENDPOINTS).unwrap_or_default();
    let mine = controller.requests.items.iter().filter(|request| request.app == app.key && request.at >= since);
    let mut slow = endpoints(mine);
    slow.sort_by(|a, b| b.p95.total_cmp(&a.p95));
    if slow.is_empty() {
        lines.push(vec![seg(" none in the last 5 minutes", C::DarkGray)]);
    }
    for endpoint in slow.iter().take(PANEL_ENDPOINTS) {
        let path = format!("{} {}", endpoint.method, endpoint.endpoint);
        let tail = format!("{:>7.0} ms {:>4} err", endpoint.p95, endpoint.errors);
        let room = width.saturating_sub(tail.chars().count() + 2);
        let path: String = path.chars().take(room).collect();
        let errors = if endpoint.errors > 0 { C::Red } else { C::Gray };
        lines.push(vec![seg(format!(" {path:<room$} "), C::Gray), seg(tail, errors)]);
    }
    lines.push(Vec::new());
    lines.push(vec![seg(" ALERTS", C::DarkCyan)]);
    let breaches: Vec<_> = controller.health.of(&app.key).collect();
    if breaches.is_empty() {
        lines.push(vec![seg(" none", C::DarkGray)]);
    }
    for breach in breaches {
        let lasted = lazyrunner_core::run::format_duration(now.saturating_duration_since(breach.since).as_secs_f64());
        let (mark, color, state) = match () {
            _ if breach.alert() && breach.acknowledged => ("!", C::Yellow, " · acknowledged"),
            _ if breach.alert() => ("!", C::Red, " · a acknowledges"),
            _ => ("△", C::Yellow, " · not an alert yet"),
        };
        let text =
            format!(" {mark} {} > {} for {lasted}{state}", breach.metric.name(), breach.metric.say(breach.target));
        lines.push(vec![seg(text, color)]);
    }
    lines.push(Vec::new());
    lines.push(vec![seg(" PROFILE", C::DarkCyan)]);
    match controller.profiles_of(&app.key).last() {
        Some(last) => {
            let at = last.at.get(11..19).unwrap_or(&last.at);
            let hottest = last.top(1).first().map_or(String::new(), |hot| {
                format!(" · hottest {} {:.0}%", short(&hot.name), hot.own / last.total().max(f64::EPSILON) * 100.0)
            });
            lines.push(vec![seg(format!(" {at}{hottest}"), C::Gray)]);
            lines.push(vec![seg(" p profiles it again · Alt+p opens it", C::DarkGray)]);
        }
        None => lines.push(vec![seg(" p profiles its CPU", C::DarkGray)]),
    }
    lines.into_iter().map(|line| super::columns::fit(line, width)).collect()
}

/// The log of alerts and recoveries, the newest first.
pub fn log(controller: &Controller, width: usize) -> Vec<Vec<Seg>> {
    let mut lines = vec![vec![seg(" RECENT ALERTS", C::DarkCyan)]];
    if controller.health.log.is_empty() {
        lines.push(vec![seg(" none yet - a reading over its target for a minute is one", C::DarkGray)]);
    }
    for logged in controller.health.log.iter().rev().take(LOG_LINES) {
        let (mark, color) = if logged.recovered { ("✓", C::Green) } else { ("!", C::Red) };
        lines.push(super::columns::fit(
            vec![seg(format!(" {} ", logged.at), C::DarkGray), seg(format!("{mark} {}", logged.text), color)],
            width,
        ));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_area_fills_up_to_each_reading_with_its_target_dotted_and_what_is_over_it_red() {
        let rows = area(&[0.0, 50.0, 100.0, 150.0], Some(100.0), (4, 2), C::Cyan);
        assert_eq!(rows.len(), 2);
        let text = |row: &Vec<Seg>| row.iter().map(|part| part.text.clone()).collect::<String>();
        assert_eq!(text(&rows[1]).chars().count(), 4);
        assert_eq!(rows[1][3].fg, C::Red, "150 is over 100");
        assert_eq!(rows[1][0].fg, C::Cyan);
        assert!(text(&rows[0]).chars().any(|ch| ch != '\u{2800}'), "the tallest reaches the top row");
        assert_eq!(area(&[], Some(1.0), (4, 2), C::Cyan).len(), 2, "no samples: blank rows");
    }
}
