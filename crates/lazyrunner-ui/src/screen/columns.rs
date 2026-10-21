//! The columns: one per app (title, live numbers, output), side by side, with borders in the state colour of the
//! column on their left, and heavy ones in the theme's focus colour beside the focused column.

use std::time::Duration;
use unicode_segmentation::UnicodeSegmentation;

use lazyrunner_core::output::{Hue, Level, OutputLine};
use lazyrunner_core::run::Controller;
use lazyrunner_core::run::app::{AppState, Status};
use lazyrunner_core::run::format_duration;
use unicode_width::UnicodeWidthStr;

use super::Ui;
use super::mouse::Target;
use super::seg::{Seg, aim, on, pad, seg, take_cells, width};
use super::theme::C;

const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
const ORBIT: [char; 4] = ['◐', '◓', '◑', '◒'];
pub const SPARKS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
const BLOCKS: [char; 8] = [' ', '▏', '▎', '▍', '▌', '▋', '▊', '▉'];
const DEFAULT_ICON: &str = "■";
const DEFAULT_WIDTH: f64 = 33.0;
const MIN_WIDTH: usize = 12;
const SLOW_RESPONSE_MS: u64 = 1000;
/// Fresh output is white this long, then a brighter shade of its colour until this.
const FRESH: Duration = Duration::from_millis(400);
const SETTLING: Duration = Duration::from_millis(1000);
const SPARK_WIDTH: usize = 10;
const HISTORY: usize = 30;

pub fn spinner(tick: u64) -> char {
    SPINNER[tick as usize % SPINNER.len()]
}

/// The relative widths from the settings (33 each by default), as cells adding up to `total`.
pub fn widths(controller: &Controller, keys: &[usize], total: usize) -> Vec<usize> {
    let configured = &controller.config["columnWidths"];
    let weights: Vec<f64> = keys
        .iter()
        .map(|&i| configured.get(&controller.apps[i].key).and_then(serde_json::Value::as_f64).unwrap_or(DEFAULT_WIDTH))
        .collect();
    let sum: f64 = weights.iter().sum::<f64>().max(1.0);
    let mut widths: Vec<usize> =
        weights.iter().map(|weight| ((total as f64 * weight / sum).floor() as usize).max(MIN_WIDTH)).collect();
    let used: usize = widths.iter().sum();
    if let Some(last) = widths.last_mut() {
        *last = (*last + total).saturating_sub(used).max(1);
    }
    widths
}

/// The building or npm ci progress for this app (both can run at once).
fn tool<'a>(
    controller: &'a Controller,
    key: &str,
) -> Option<(&'static str, &'a lazyrunner_core::run::controller::ToolJob)> {
    if let Some(job) = controller.building.as_ref().filter(|job| job.key == key) {
        return Some(("build", job));
    }
    controller.npm_ci.as_ref().filter(|job| job.key == key).map(|job| ("npmci", job))
}

/// The app's colour for its title chip; it flashes red for a moment when an error line arrives.
pub fn tone(ui: &Ui, controller: &Controller, index: usize) -> C {
    let state = &controller.states[index];
    if state.flash_until.is_some_and(|until| ui.now < until) && (ui.reduced_motion || ui.tick() % 4 < 2) {
        return C::Red;
    }
    if tool(controller, &state.key).is_some() {
        return C::Yellow;
    }
    match state.status {
        Status::Running if state.is_up() => C::Green,
        Status::Running => C::Yellow,
        Status::Exited(_) | Status::Error(_) => C::Red,
        Status::Stopped => C::DarkGray,
    }
}

/// The border colour: the state colour, dimmed (never flashes, so borders stay calm).
pub fn border(controller: &Controller, index: usize) -> C {
    let state = &controller.states[index];
    if tool(controller, &state.key).is_some() {
        return C::DarkYellow;
    }
    match state.status {
        Status::Running if state.is_up() => C::DarkGreen,
        Status::Running => C::DarkYellow,
        Status::Exited(_) | Status::Error(_) => C::DarkRed,
        Status::Stopped => C::DarkGray,
    }
}

/// A row cut or padded to exactly `cells`.
pub fn fit(segs: Vec<Seg>, cells: usize) -> Vec<Seg> {
    let mut out = Vec::new();
    let mut used = 0;
    for seg in segs {
        if used >= cells {
            break;
        }
        let text = if used + seg.text.width() > cells { take_cells(&seg.text, cells - used) } else { seg.text.clone() };
        used += text.width();
        out.push(Seg { text, ..seg });
    }
    if used < cells {
        out.push(seg(" ".repeat(cells - used), C::Gray));
    }
    out
}

/// The body rows: the columns side by side, `rows` high.
pub fn rows(ui: &mut Ui, controller: &Controller, total: usize, rows: usize) -> Vec<Vec<Seg>> {
    ui.body_rows = rows.saturating_sub(2);
    let count = controller.apps.len();
    if count == 0 {
        return vec![Vec::new(); rows];
    }
    ui.focus = ui.focus.min(count - 1);
    let keys: Vec<usize> = if ui.zoom { vec![ui.focus] } else { (0..count).collect() };
    let widths = widths(controller, &keys, total.saturating_sub(keys.len() - 1));
    if let Some(cells) = keys.iter().position(|&index| index == ui.focus).map(|at| widths[at]) {
        // The text's width: a scrollbar takes the last cell.
        let scrollable = controller.states[ui.focus].output.len() > rows.saturating_sub(2);
        ui.focus_cells = cells.saturating_sub(usize::from(scrollable));
    }
    let columns: Vec<Vec<Vec<Seg>>> = keys
        .iter()
        .zip(&widths)
        .map(|(&index, &cells)| {
            let mut column = vec![
                aim(title(ui, controller, index, cells), Target::Title(index)),
                aim(info(controller, index, cells), Target::Title(index)),
            ];
            column.extend(
                body(ui, controller, index, cells, rows.saturating_sub(2))
                    .into_iter()
                    .map(|row| aim(row, Target::Output(index))),
            );
            column
        })
        .collect();
    // Each border takes the state colour of the column on its left; both borders of the focused column are heavy
    // lines in the theme's focus colour.
    let separators: Vec<(C, &str, &str)> = (0..keys.len().saturating_sub(1))
        .map(|i| {
            if ui.focus == keys[i] || ui.focus == keys[i + 1] {
                (C::FocusBorder, "┃", "┳")
            } else {
                (border(controller, keys[i]), "│", "┬")
            }
        })
        .collect();
    (0..rows)
        .map(|row| {
            let mut line = Vec::new();
            for (c, column) in columns.iter().enumerate() {
                if c > 0 {
                    let (color, side, top) = separators[c - 1];
                    line.push(seg(if row == 0 { top } else { side }, color));
                }
                line.extend(column.get(row).cloned().unwrap_or_else(|| vec![seg(" ".repeat(widths[c]), C::Gray)]));
            }
            line
        })
        .collect()
}

fn title(ui: &mut Ui, controller: &Controller, index: usize, cells: usize) -> Vec<Seg> {
    let app = &controller.apps[index];
    let focused = index == ui.focus;
    let tone = tone(ui, controller, index);
    let marker = if focused { "▸ " } else { "" };
    let icon = app.icon.as_deref().unwrap_or(DEFAULT_ICON);
    let chip_fg = if tone == C::DarkGray { C::White } else { C::Black };
    let mut segs = vec![on(format!(" {marker}{icon} {} ", app.name), chip_fg, tone)];
    segs.push(seg(controller.port(&app.key).map_or("  ".into(), |port| format!(" :{port}  ")), C::DarkGray));
    segs.extend(status(ui, controller, index));
    let used = width(&segs);
    let rule = if focused { "━" } else { "─" };
    segs.push(seg(format!(" {}", rule.repeat(cells.saturating_sub(used + 1))), border(controller, index)));
    if focused {
        // The focused pane's whole title bar takes the theme's focus colours (the state chip keeps its own).
        for seg in segs.iter_mut().skip(1) {
            seg.fg = C::FocusText;
            seg.bg = Some(C::FocusBack);
        }
    }
    fit(segs, cells)
}

/// A progress bar that glides towards its target and shimmers while it waits; `id` keeps its position.
pub fn bar(ui: &mut Ui, fraction: f64, id: &str, animated: bool) -> String {
    const WIDTH: usize = 10;
    let target = fraction.clamp(0.0, 1.0);
    let shown = if animated { ui.glide(id, target) } else { target };
    let cells = shown * WIDTH as f64;
    let full = cells.floor() as usize;
    let partial =
        if full < WIDTH { BLOCKS[((cells - full as f64) * 8.0) as usize % 8].to_string() } else { String::new() };
    let mut filled: Vec<char> = vec!['█'; full];
    if animated && full > 1 && shown < 1.0 {
        filled[ui.tick() as usize % full] = '▓'; // a light glint runs along the bar
    }
    let rest = WIDTH.saturating_sub(full + partial.chars().count());
    format!("▕{}{partial}{}▏ ", filled.into_iter().collect::<String>(), " ".repeat(rest))
}

fn status(ui: &mut Ui, controller: &Controller, index: usize) -> Vec<Seg> {
    let state = &controller.states[index];
    let key = state.key.clone();
    let tick = ui.tick();
    let animated = !ui.reduced_motion;
    if let Some((kind, job)) = tool(controller, &key) {
        let elapsed = ui.now.saturating_duration_since(job.started).as_secs_f64();
        if kind == "build" && job.total > 0 {
            let done = job.done.min(job.total);
            let bar = bar(ui, done as f64 / job.total as f64, &format!("build:{key}"), animated);
            return vec![seg(
                format!(
                    "{} building {bar}{done}/{} · {}",
                    if animated { spinner(tick) } else { '●' },
                    job.total,
                    format_duration(elapsed)
                ),
                C::Yellow,
            )];
        }
        let estimate = controller.timing_estimate(kind, &key);
        let frame = if !animated {
            '●'
        } else if kind == "npmci" {
            ORBIT[tick as usize % ORBIT.len()]
        } else {
            spinner(tick)
        };
        let verb = if kind != "npmci" {
            "building"
        } else if job.start_after {
            "npm ci → start"
        } else {
            "npm ci"
        };
        let bar =
            estimate.map_or(String::new(), |estimate| bar(ui, elapsed / estimate, &format!("{kind}:{key}"), animated));
        let eta = estimate.map_or(String::new(), |estimate| format!(" / ~{}", format_duration(estimate)));
        return vec![seg(format!("{frame} {verb} {bar}{}{eta}", format_duration(elapsed)), C::Yellow)];
    }
    match &state.status {
        Status::Running if state.is_up() => {
            let up = format!("  up in {}", format_duration(state.up_after.unwrap_or_default()));
            if controller.port(&key).is_none() {
                return vec![seg("● built, watching", C::Green), seg(up, C::DarkGray)];
            }
            let mut segs = vec![seg("● ", C::Green)];
            segs.push(match controller.last_probe(&key) {
                None => seg("checking…", C::DarkGray),
                Some(probe) if !probe.ok => seg("not responding", C::Red),
                Some(probe) => {
                    let code = if probe.code >= 400 { format!(" HTTP {}", probe.code) } else { String::new() };
                    if probe.ms >= SLOW_RESPONSE_MS {
                        seg(format!("slow {:.1}s{code}", probe.ms as f64 / 1000.0), C::Yellow)
                    } else {
                        seg(format!("{} ms{code}", probe.ms), C::Green)
                    }
                }
            });
            segs.push(seg(up, C::DarkGray));
            segs
        }
        Status::Running => {
            // Not up yet: the dot pulses and the bar fills against the usual startup time.
            let elapsed =
                state.started_at.map_or(0.0, |started| ui.now.saturating_duration_since(started).as_secs_f64());
            let estimate = controller.timing_estimate("start", &key);
            let dot = if animated && tick % 6 >= 3 { '○' } else { '●' };
            let verb = controller.apps[index].traits().start_verb;
            let bar = estimate
                .map_or(String::new(), |estimate| bar(ui, elapsed / estimate, &format!("start:{key}"), animated));
            let eta = estimate.map_or(String::new(), |estimate| format!(" / ~{}", format_duration(estimate)));
            vec![seg(format!("{dot} {verb} {bar}{}{eta}", format_duration(elapsed)), C::Yellow)]
        }
        Status::Exited(code) => {
            vec![seg(format!("✗ exited (code {})", code.map_or("?".into(), |c| c.to_string())), C::Red)]
        }
        Status::Error(error) => vec![seg(format!("✗ {error}"), C::Red)],
        Status::Stopped => vec![seg("○ stopped", C::DarkGray)],
    }
}

/// A sparkline of the newest values; `floor` keeps small values small (3% CPU is a low bar, not a full one).
pub fn sparkline<'a>(values: impl DoubleEndedIterator<Item = &'a f32> + ExactSizeIterator, floor: f32) -> String {
    let recent: Vec<f32> = values.rev().take(SPARK_WIDTH).copied().collect::<Vec<_>>().into_iter().rev().collect();
    let max = recent.iter().copied().fold(floor, f32::max);
    recent.iter().map(|value| SPARKS[((value / max * 7.999).floor() as usize).min(7)]).collect()
}

/// Over the last minute the newest samples average 20% above the oldest ones.
fn memory_growing(state: &AppState) -> bool {
    let history = &state.memory_history;
    if history.len() < HISTORY {
        return false;
    }
    let average = |values: &mut dyn Iterator<Item = &f32>| values.copied().sum::<f32>() / 5.0;
    let old = average(&mut history.iter().take(5));
    let new = average(&mut history.iter().rev().take(5));
    old > 0.0 && new > old * 1.2
}

/// Second title line: live CPU and memory with sparklines, the response-time trend, requests, errors, the variant.
fn info(controller: &Controller, index: usize, cells: usize) -> Vec<Seg> {
    let state = &controller.states[index];
    let mut segs = vec![seg(" ", C::Gray)];
    if state.scroll > 0 {
        segs.push(seg(format!("⏸ ↑{} · G follows  ", state.scroll), C::Magenta));
    }
    let running = state.is_running();
    if running && let Some(cpu) = state.cpu {
        let color = if cpu >= 80.0 {
            C::Red
        } else if cpu >= 40.0 {
            C::Yellow
        } else {
            C::Green
        };
        segs.push(seg("cpu ", C::DarkGray));
        segs.push(seg(format!("{cpu:>3.0}%"), color));
        segs.push(seg(format!(" {}", sparkline(state.cpu_history.iter(), 25.0)), C::DarkCyan));
        segs.push(seg("  mem ", C::DarkGray));
        segs.push(seg(format!("{:.0} MB", state.memory_mb.unwrap_or_default()), C::Gray));
        if memory_growing(state) {
            segs.push(seg(" ↗", C::Yellow));
        }
    }
    if running && controller.port(&state.key).is_some() && !state.response_history.is_empty() {
        let slow = state.response_history.back().is_some_and(|ms| *ms >= SLOW_RESPONSE_MS as f32);
        segs.push(seg("  resp ", C::DarkGray));
        segs.push(seg(sparkline(state.response_history.iter(), 200.0), if slow { C::Yellow } else { C::DarkCyan }));
    }
    if running && !state.request_history.is_empty() {
        let last = state.request_history.back().copied().unwrap_or_default();
        segs.push(seg("  req ", C::DarkGray));
        segs.push(seg(format!("{} ", trim_number(last)), C::Gray));
        segs.push(seg(sparkline(state.request_history.iter(), 5.0), C::DarkMagenta));
    }
    if state.errors > 0 {
        segs.push(seg(format!("  ✗ {}", state.errors), C::Red));
    }
    if state.warnings > 0 {
        segs.push(seg(format!("  △ {}", state.warnings), C::Yellow));
    }
    if let Some(mode) = controller.mode(&state.key) {
        segs.push(seg(format!("  {} · {}", mode.label, mode.script), C::DarkGray));
    }
    fit(segs, cells)
}

/// "1.5/s", "2/s".
/// One decimal, rounded half away from zero like PowerShell's '{0:0.#}' (1.25 is 1.3, not 1.2).
fn trim_number(value: f32) -> String {
    let text = format!("{:.1}", (value * 10.0).round() / 10.0);
    format!("{}/s", text.trim_end_matches('0').trim_end_matches('.'))
}

/// A line's own colour.
pub fn line_color(line: &OutputLine) -> C {
    match line.hue {
        Some(hue) => hue_color(hue),
        None => match line.level {
            Level::Error => C::Red,
            Level::Warn => C::Yellow,
            Level::Debug => C::DarkGray,
            Level::Info => C::Gray,
        },
    }
}

pub fn hue_color(hue: Hue) -> C {
    match hue {
        Hue::Gray => C::Gray,
        Hue::DarkGray => C::DarkGray,
        Hue::Cyan => C::Cyan,
        Hue::DarkCyan => C::DarkCyan,
        Hue::Green => C::Green,
        Hue::DarkGreen => C::DarkGreen,
        Hue::Yellow => C::Yellow,
        Hue::DarkYellow => C::DarkYellow,
        Hue::Red => C::Red,
        Hue::Magenta => C::Magenta,
        Hue::White => C::White,
    }
}

/// The brighter shade a fresh line settles through.
fn bright(color: C) -> C {
    match color {
        C::Gray => C::White,
        C::DarkGray => C::Gray,
        C::DarkCyan => C::Cyan,
        C::DarkYellow => C::Yellow,
        C::DarkGreen => C::Green,
        C::DarkRed => C::Red,
        C::DarkMagenta => C::Magenta,
        other => other,
    }
}

/// Output rows, newest at the bottom, wrapped to the width. New lines fade in; search matches are highlighted
/// (the current one in the focus colours, the others yellow).
fn body(ui: &Ui, controller: &Controller, index: usize, column: usize, rows: usize) -> Vec<Vec<Seg>> {
    let state = &controller.states[index];
    let search = ui.search.as_ref().filter(|search| !search.query.is_empty() && search.key == state.key);
    let total = state.output.len();
    // More lines than rows: the last cell is a scrollbar.
    let scrollable = total > rows && rows > 0 && column > 1;
    let cells = if scrollable { column - 1 } else { column };
    let mut result: Vec<Vec<Seg>> = Vec::with_capacity(rows);
    for (offset, line) in state.output.newest_first(state.scroll).enumerate() {
        if result.len() >= rows {
            break;
        }
        let position = total - 1 - state.scroll - offset; // 0 = the oldest line kept
        let mut color = line_color(line);
        let mut back = None;
        let age = ui.now.saturating_duration_since(line.born);
        if ui.reduced_motion {
            // New lines show in their own colour at once, no white flash fading out.
        } else if age < FRESH {
            color = C::White;
        } else if age < SETTLING {
            color = bright(color);
        }
        if let Some(search) = search
            && contains_ignore_case(&line.text, &search.query)
        {
            if search.index == Some(position) {
                (color, back) = (C::FocusText, Some(C::FocusBack));
            } else {
                (color, back) = (C::Black, Some(C::Yellow));
            }
        }
        let chunks = chunk(&line.text, cells);
        for text in chunks.iter().rev() {
            if result.len() >= rows {
                break;
            }
            let text = if back.is_some() { pad(text, cells) } else { text.clone() };
            result.push(fit(vec![Seg { text, fg: color, bg: back, style: None, target: None }], cells));
        }
    }
    while result.len() < rows {
        result.push(vec![seg(" ".repeat(cells), C::Gray)]);
    }
    result.reverse();
    if scrollable {
        add_scrollbar(&mut result, total, state.scroll);
    }
    result
}

/// The scrollbar: where the lines on screen are among all the column keeps (by lines, not wrapped rows); bright
/// while scrolled back.
fn add_scrollbar(rows: &mut [Vec<Seg>], total: usize, scroll: usize) {
    let height = rows.len();
    let thumb = (height * height / total).clamp(1, height);
    let newest_shown = total.saturating_sub(scroll).max(1); // the lines up to this one end at the bottom row
    let bottom = (newest_shown * height).div_ceil(total).clamp(thumb, height); // at the top it keeps its size
    let top = bottom.saturating_sub(thumb);
    let color = if scroll > 0 { C::Cyan } else { C::DarkGray };
    for (at, row) in rows.iter_mut().enumerate() {
        row.push(if (top..bottom).contains(&at) { seg("▐", color) } else { seg(" ", C::Gray) });
    }
}

/// Cuts text into pieces of `cells` cells.
pub fn chunk(text: &str, cells: usize) -> Vec<String> {
    if text.width() <= cells {
        return vec![text.to_string()];
    }
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut used = 0;
    for c in text.graphemes(true) {
        let w = c.width();
        if used + w > cells.max(1) {
            chunks.push(std::mem::take(&mut current));
            used = 0;
        }
        current.push_str(c);
        used += w;
    }
    chunks.push(current);
    chunks
}

pub fn contains_ignore_case(text: &str, query: &str) -> bool {
    text.to_lowercase().contains(&query.to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sparklines_and_chunks() {
        assert_eq!(sparkline([0.0f32, 50.0, 100.0].iter(), 1.0), "▁▄█");
        assert_eq!(chunk("abcdefg", 3), ["abc", "def", "g"]);
        assert_eq!(trim_number(1.25), "1.3/s");
        assert_eq!(trim_number(2.0), "2/s");
    }
}
