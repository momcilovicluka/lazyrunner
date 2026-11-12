//! D: how long each build, app start and npm ci took over its last runs, full screen like the other views. The table
//! has a row per item (bars per run, the last, the usual and the trend), so one that got slower stands out; `j` `k`
//! pick one, and its details show its runs as a taller chart - growing in as it's picked - and each run listed: when,
//! how long, and whether it failed. The details sit beside the table on a wide terminal, under it on a narrow one.

use std::time::{Duration, Instant};

use lazyrunner_core::output::Hue;
use lazyrunner_core::run::Controller;
use lazyrunner_core::run::format_duration;
use lazyrunner_core::run::history::{Item, Kind, Timed, Trend, trend};
use unicode_width::UnicodeWidthStr;

use super::columns::{SPARKS, fit};
use super::icons::{self, Icon};
use super::mouse::Target;
use super::seg::{Seg, aim, pad, seg, take_cells};
use super::theme::C;
use super::{Overlay, Ui};

/// The run count shows from this wide a table (at 80 columns too).
const COUNT_FROM: usize = 64;
/// What a row takes besides the name and the bars: "  " name " ", last, usual and the trend.
const ROW_FIXED: usize = 3 + 9 + 9 + 10;
/// The run count's cells.
const COUNT_CELLS: usize = 10;
/// The tallest bar is ▇, not █: a full block touches the bars in the row below and the rows blur together.
const TALLEST: usize = 6;
/// From this wide the picked item's details go beside the table (this wide), else under it.
const BESIDE_FROM: usize = 110;
const DETAIL_WIDTH: usize = 44;
/// The picked item's chart: up to this many rows tall (fewer when the details have little room), its bars growing in
/// over `GROW` as it's picked; the details' rows besides the chart and the runs.
const CHART_ROWS: usize = 6;
const GROW: Duration = Duration::from_millis(200);
const DETAIL_FIXED: usize = 7;
const KEYS: &str = " j k choose · g G first / last · Enter a build's map · Esc closes · usual = median of the earlier successful runs · \
     red = failed";

pub fn open(ui: &mut Ui, controller: &mut Controller) {
    if controller.durations.is_empty() {
        controller.add_log("No durations yet: they're recorded from the next build, start or npm ci.", Hue::DarkGray);
    } else {
        ui.overlay = Some(Overlay::Durations { index: 0, picked: ui.now });
    }
}

/// The items as the table lists them: by kind, each kind's by name.
fn items(controller: &Controller) -> Vec<(Kind, &Item)> {
    let grouped = controller.durations.grouped();
    grouped.into_iter().flat_map(|(kind, items)| items.into_iter().map(move |item| (kind, item))).collect()
}

/// j k choose an item, g G (Home End) the first and the last, Esc q D close.
pub fn key(ui: &mut Ui, controller: &mut Controller, name: &str) {
    if matches!(name, "Escape" | "q" | "D") {
        ui.overlay = None;
        return;
    }
    if name == "Enter" {
        open_build(ui, controller);
        return;
    }
    let count = items(controller).len();
    let now = ui.now;
    let Some(Overlay::Durations { index, picked }) = &mut ui.overlay else { return };
    let last = count.saturating_sub(1);
    let to = match name {
        "j" | "DownArrow" => super::keys::wrapped(*index, 1, count.max(1)),
        "k" | "UpArrow" => super::keys::wrapped(*index, -1, count.max(1)),
        "Ctrl+D" | "PageDown" => (*index + 10).min(last),
        "Ctrl+U" | "PageUp" => index.saturating_sub(10),
        "g" | "Home" | "<" => 0,
        "G" | "End" | ">" => last,
        _ => *index,
    };
    if to != *index {
        (*index, *picked) = (to, now);
    }
}

/// Enter on a build (or a project built): its app's build as a map, as it was kept.
fn open_build(ui: &mut Ui, controller: &mut Controller) {
    let Some(Overlay::Durations { index, .. }) = &ui.overlay else { return };
    let Some((kind, item)) = items(controller).get(*index).map(|(kind, item)| (*kind, (*item).clone())) else { return };
    let app = match kind {
        Kind::Build => item.key.clone(),
        Kind::Project => item.key.split('/').next().unwrap_or_default().to_string(),
        _ => return,
    };
    if controller.build_run(&app).is_none() {
        controller
            .add_log(format!("No build map of {} kept yet: its next build makes one.", item.label), Hue::DarkGray);
        return;
    }
    ui.overlay = None;
    super::code::open_build(ui, controller, app);
}

/// A click on an item picks it.
pub fn click(ui: &mut Ui, target: Option<Target>) -> bool {
    let now = ui.now;
    let Some(Overlay::Durations { index, picked }) = &mut ui.overlay else { return false };
    let Some(Target::Item(at)) = target else { return false };
    if *index != at {
        (*index, *picked) = (at, now);
    }
    true
}

/// Whether the picked item's chart is still growing in.
pub fn animating(ui: &Ui) -> bool {
    let Some(Overlay::Durations { picked, .. }) = &ui.overlay else { return false };
    ui.motion.enabled && !ui.reduced_motion && ui.now.saturating_duration_since(*picked) < GROW
}

/// How far the picked item's chart has grown: 1 at rest.
fn grown(ui: &Ui, picked: Instant) -> f64 {
    if !ui.motion.enabled || ui.reduced_motion {
        return 1.0;
    }
    let progress = ui.now.saturating_duration_since(picked).as_secs_f64() / GROW.as_secs_f64();
    1.0 - (1.0 - progress.clamp(0.0, 1.0)).powi(3)
}

/// The trend in words: slower or faster by how much (a failed run has none; a small change is "≈").
fn change(trend: Option<&Trend>, nerd: bool) -> (String, C) {
    let Some(trend) = trend else { return (String::new(), C::DarkGray) };
    let percent = |change: f64| (change * 100.0).round().min(9999.0);
    match (trend.change, trend.notable()) {
        (None, _) => (String::new(), C::DarkGray),
        (Some(_), Some(change)) if change > 0.0 => {
            (format!("{} +{}%", icons::icon(nerd, Icon::Slower), percent(change)), C::Red)
        }
        (Some(_), Some(change)) => (format!("{} {}%", icons::icon(nerd, Icon::Faster), percent(change)), C::Green),
        (Some(_), None) => ("≈".to_string(), C::DarkGray),
    }
}

/// An item's row: its name, a bar per run, the last run, the usual, the trend and (with room) how many runs there are.
fn line(item: &Item, label: usize, bars: usize, count: bool, nerd: bool) -> Vec<Seg> {
    let Some(trend) = trend(&item.runs) else { return Vec::new() };
    let name = if item.label.width() > label {
        format!("{}…", take_cells(&item.label, label.saturating_sub(1)))
    } else {
        item.label.clone()
    };
    let mut segs = vec![seg(format!("  {} ", pad(&name, label)), C::Gray)];
    segs.extend(bar_segs(item, bars));
    segs.push(seg(format!("  {:>7}", format_duration(trend.last)), if trend.last_ok { C::White } else { C::Red }));
    segs.push(seg(format!("  {:>7}", trend.usual.map_or("–".to_string(), format_duration)), C::DarkGray));
    let (text, color) = change(Some(&trend), nerd);
    segs.push(seg(format!("  {}", pad(&text, 8)), color));
    if count {
        let runs = item.runs.len();
        segs.push(seg(format!("  {runs:>3} run{}", if runs == 1 { "" } else { "s" }), C::DarkGray));
    }
    segs
}

/// One bar per run (the last `bars` of them), scaled to the slowest of those; a failed run's is red.
fn bar_segs(item: &Item, bars: usize) -> Vec<Seg> {
    let recent = &item.runs[item.runs.len().saturating_sub(bars)..];
    let max = recent.iter().map(|run| run.sec).fold(1.0, f64::max);
    let mut segs: Vec<Seg> = recent
        .iter()
        .map(|run| {
            let level = ((run.sec / max * (TALLEST as f64 + 0.999)).floor() as usize).min(TALLEST);
            seg(SPARKS[level].to_string(), if run.ok { C::DarkCyan } else { C::Red })
        })
        .collect();
    segs.push(seg(" ".repeat(bars - recent.len()), C::Gray));
    segs
}

/// The table: a title row per kind and a row per item (aimed at it, for a click); and each item's row in it.
fn table(controller: &Controller, width: usize, nerd: bool) -> (Vec<Vec<Seg>>, Vec<usize>) {
    let bars = width.saturating_sub(66).clamp(5, 20);
    let count = width >= COUNT_FROM;
    let label = width.saturating_sub(bars + ROW_FIXED + if count { COUNT_CELLS } else { 0 }).max(8);
    let runs = if bars >= 11 { format!("last {bars} runs") } else { "runs".to_string() };
    let (mut rows, mut at_item) = (Vec::new(), Vec::new());
    for (kind, items) in controller.durations.grouped() {
        let title = format!("{} {}  {:>7}  {:>7}", pad(kind.title(), label + 2), pad(&runs, bars), "last", "usual");
        rows.push(vec![seg(title, C::DarkCyan)]);
        for each in items {
            rows.push(aim(line(each, label, bars, count, nerd), Target::Item(at_item.len())));
            at_item.push(rows.len() - 1);
        }
    }
    (rows, at_item)
}

/// The picked item's chart: a column per run (as many of the last as fit), `height` rows tall in eighths, scaled to
/// the slowest of them and grown to `grown`; a failed run's in red, the last one bright.
fn chart(runs: &[Timed], width: usize, height: usize, grown: f64) -> Vec<Vec<Seg>> {
    let fit_runs = (width.saturating_sub(2) / 2).max(1);
    let recent = &runs[runs.len().saturating_sub(fit_runs)..];
    let max = recent.iter().map(|run| run.sec).fold(0.1, f64::max);
    let eighths: Vec<usize> =
        recent.iter().map(|run| ((run.sec / max * grown * (height * 8) as f64).round() as usize).max(1)).collect();
    (0..height)
        .map(|row| {
            let floor = (height - 1 - row) * 8;
            let mut line = vec![seg(" ", C::Gray)];
            for (at, (run, height)) in recent.iter().zip(&eighths).enumerate() {
                let filled = height.saturating_sub(floor).min(8);
                let block = if filled == 0 { " ".to_string() } else { SPARKS[filled - 1].to_string() };
                let color = match () {
                    _ if !run.ok => C::Red,
                    _ if at + 1 == recent.len() => C::Cyan,
                    _ => C::DarkCyan,
                };
                line.extend([seg(block, color), seg(" ", C::Gray)]);
            }
            line
        })
        .collect()
}

/// When a run finished, as it was recorded: 05 Nov 10:42.
fn when(run: &Timed) -> String {
    let at = chrono::DateTime::parse_from_rfc3339(&run.at);
    at.map_or_else(|_| run.at.clone(), |at| at.format("%d %b %H:%M").to_string())
}

/// The picked item's details: its name and kind, its last and usual and the trend, its chart, and its runs, newest
/// first, as many as fit in `room`.
fn details(ui: &Ui, kind: Kind, item: &Item, width: usize, room: usize, picked: Instant) -> Vec<Vec<Seg>> {
    let mut out = vec![vec![
        seg(format!(" {}", item.label.to_uppercase()), C::White),
        seg(format!(" · {}", kind.title()), C::DarkCyan),
    ]];
    if let Some(trend) = trend(&item.runs) {
        let usual = trend.usual.map_or("–".to_string(), format_duration);
        let (text, color) = change(Some(&trend), ui.nerd);
        let runs = item.runs.len();
        out.push(vec![
            seg(format!(" last {} · usual {usual} · ", format_duration(trend.last)), C::Gray),
            seg(if text.is_empty() { "–".into() } else { text }, color),
            seg(format!(" · {runs} run{}", if runs == 1 { "" } else { "s" }), C::DarkGray),
        ]);
    }
    let slowest = item.runs.iter().map(|run| run.sec).fold(0.0, f64::max);
    out.extend([Vec::new(), vec![seg(format!(" {} the slowest", format_duration(slowest)), C::DarkGray)]]);
    let height = room.saturating_sub(DETAIL_FIXED + 2).clamp(2, CHART_ROWS);
    out.extend(chart(&item.runs, width, height, grown(ui, picked)));
    out.extend([vec![seg(" oldest … newest", C::DarkGray)], Vec::new(), vec![seg(" RUNS", C::DarkCyan)]]);
    let left = room.saturating_sub(out.len());
    for run in item.runs.iter().rev().take(left) {
        let (mark, color) = if run.ok { ("✓", C::Green) } else { ("× failed", C::Red) };
        out.push(vec![seg(format!(" {}  {:>7}  ", when(run), format_duration(run.sec)), C::Gray), seg(mark, color)]);
    }
    out
}

/// The header: the view, how many items, and the one that got the most slower.
fn header(all: &[(Kind, &Item)], nerd: bool) -> Vec<Seg> {
    let mut segs = Vec::new();
    let plural = if all.len() == 1 { "" } else { "s" };
    segs.push(seg(format!("  {} item{plural}", all.len()), C::Gray));
    let slower = all
        .iter()
        .filter_map(|(_, item)| Some((item, trend(&item.runs)?.notable()?)))
        .filter(|(_, change)| *change > 0.0)
        .max_by(|a, b| a.1.total_cmp(&b.1));
    if let Some((item, change)) = slower {
        let icon = icons::icon(nerd, Icon::Slower);
        segs.push(seg(format!(" · slowed down most: {} {icon} +{}%", item.label, (change * 100.0).round()), C::Red));
    }
    segs
}

/// The rows that fit in `room`, the `selected` one in view.
fn window(rows: &[Vec<Seg>], selected: usize, room: usize) -> Vec<Vec<Seg>> {
    if rows.len() <= room {
        return rows.to_vec();
    }
    let first = selected.saturating_sub(room / 2).min(rows.len() - room);
    rows[first..first + room].to_vec()
}

/// The row lit across, as the selection bar is.
fn light(row: &mut [Seg]) {
    for part in row.iter_mut().filter(|part| part.bg.is_none()) {
        part.bg = Some(C::FocusBack);
        if matches!(part.fg, C::Gray | C::DarkGray | C::White) {
            part.fg = C::FocusText;
        }
    }
}

/// The view: its header and keys, then the table beside (or above) the picked item's details.
pub fn rows(ui: &mut Ui, controller: &Controller, cells: usize, rows: usize) -> Vec<Vec<Seg>> {
    let Some(Overlay::Durations { index, picked }) = &ui.overlay else { return Vec::new() };
    let (index, picked) = (*index, *picked);
    let all = items(controller);
    let index = index.min(all.len().saturating_sub(1));
    let Some(&(kind, item)) = all.get(index) else { return Vec::new() };
    let beside = cells >= BESIDE_FROM;
    let table_width = if beside { cells.saturating_sub(DETAIL_WIDTH + 3) } else { cells };
    let (mut table, at_item) = table(controller, table_width, ui.nerd);
    let row = at_item.get(index).copied().unwrap_or(0);
    let bar = ui.motion.bar(row, ui.now);
    if let Some(line) = table.get_mut(bar) {
        light(line);
    }
    let keys = super::viewbar::keys(KEYS, cells);
    let body = rows.saturating_sub(2 + keys.len());
    let mut out = vec![header(&all, ui.nerd)];
    out.extend(keys);
    out.push(Vec::new());
    if beside {
        let table = window(&table, row, body);
        let details = details(ui, kind, item, DETAIL_WIDTH, body, picked);
        for at in 0..body {
            let mut line = fit(table.get(at).cloned().unwrap_or_default(), table_width);
            line.push(seg(" │ ", C::DarkGray));
            line.extend(fit(details.get(at).cloned().unwrap_or_default(), DETAIL_WIDTH));
            out.push(line);
        }
    } else {
        let room = table.len().min(body / 2).max(3);
        out.extend(window(&table, row, room));
        out.push(vec![seg("─".repeat(cells), C::DarkGray)]);
        out.extend(details(ui, kind, item, cells, body.saturating_sub(room + 1), picked));
    }
    out.truncate(rows);
    out.into_iter().map(|line| aim(fit(line, cells), Target::Box)).collect()
}
