//! D: how long each build, app start and npm ci took over its last runs (bars per run, the last, the usual and the
//! trend), so one that got slower stands out.

use lazyrunner_core::output::Hue;
use lazyrunner_core::run::Controller;
use lazyrunner_core::run::format_duration;
use lazyrunner_core::run::history::{Item, trend};
use unicode_width::UnicodeWidthStr;

use super::columns::SPARKS;
use super::overlays::boxed;
use super::seg::{Seg, pad, seg, take_cells};
use super::theme::C;
use super::{Overlay, Ui};

/// The box's widest inside.
const MAX_INNER: usize = 100;
/// The run count shows from this wide an inside (at 80 columns too).
const COUNT_FROM: usize = 64;
/// What a row takes besides the name and the bars: "  " name " ", last, usual and the trend.
const ROW_FIXED: usize = 3 + 9 + 9 + 10;
/// The run count's cells.
const COUNT_CELLS: usize = 10;

pub fn open(ui: &mut Ui, controller: &mut Controller) {
    if controller.durations.is_empty() {
        controller.add_log("No durations yet: they're recorded from the next build, start or npm ci.", Hue::DarkGray);
    } else {
        ui.overlay = Some(Overlay::Durations { top: 0 });
    }
}

pub fn rows(ui: &mut Ui, controller: &Controller, cells: usize, rows: usize) -> Vec<Vec<Seg>> {
    let Some(Overlay::Durations { top }) = &mut ui.overlay else { return Vec::new() };
    let inner = cells.saturating_sub(8).clamp(20, MAX_INNER);
    let bars = inner.saturating_sub(66).clamp(5, 20);
    let count = inner >= COUNT_FROM;
    let label = inner.saturating_sub(bars + ROW_FIXED + if count { COUNT_CELLS } else { 0 }).max(8);
    let runs = if bars >= 11 { format!("last {bars} runs") } else { "runs".to_string() };
    let mut lines = Vec::new();
    for (kind, items) in controller.durations.grouped() {
        let header = format!("{} {}  {:>7}  {:>7}", pad(kind.title(), label + 2), pad(&runs, bars), "last", "usual");
        lines.push(vec![seg(header, C::Cyan)]);
        lines.extend(items.into_iter().map(|item| line(item, label, bars, count)));
    }
    let visible = rows.saturating_sub(4).max(1);
    *top = (*top).min(lines.len().saturating_sub(visible));
    let shown: Vec<Vec<Seg>> = lines.iter().skip(*top).take(visible).cloned().collect();
    let more = if lines.len() > visible {
        format!(" · j k scroll ({}-{} of {})", *top + 1, *top + shown.len(), lines.len())
    } else {
        String::new()
    };
    let footer = format!("Esc closes{more} · usual = median of the earlier successful runs · red = failed");
    boxed("Durations", shown, &footer, cells, rows, MAX_INNER)
}

/// An item's row: its name, a bar per run, the last run, the usual, the trend and (with room) how many runs there are.
fn line(item: &Item, label: usize, bars: usize, count: bool) -> Vec<Seg> {
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
    // A failed run has no trend (its time says little); a small change is "≈".
    let percent = |change: f64| (change * 100.0).round().min(9999.0);
    let (text, color) = match (trend.change, trend.notable()) {
        (None, _) => (String::new(), C::DarkGray),
        (Some(_), Some(change)) if change > 0.0 => (format!("▲ +{}%", percent(change)), C::Red),
        (Some(_), Some(change)) => (format!("▼ {}%", percent(change)), C::Green),
        (Some(_), None) => ("≈".to_string(), C::DarkGray),
    };
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
            let level = ((run.sec / max * 7.999).floor() as usize).min(7);
            seg(SPARKS[level].to_string(), if run.ok { C::DarkCyan } else { C::Red })
        })
        .collect();
    segs.push(seg(" ".repeat(bars - recent.len()), C::Gray));
    segs
}
