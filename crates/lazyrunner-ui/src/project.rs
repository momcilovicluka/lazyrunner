//! Drawing a running project: one column per app (title chip, status, live numbers, output) and the panel below
//! (the header, the keys, the log). P3 brings the rest of the PowerShell version's look (overlays, menus, themes).

use std::time::{Duration, Instant};

use lazyrunner_core::run::Controller;
use lazyrunner_core::run::app::{AppState, Status};
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::theme::{Palette, lerp};
use crate::view::{draw_output, hue_color};
use crate::widgets::{bar, sparkline, spinner};

const MIN_SIZE: (u16, u16) = (60, 15);
const LOG_ROWS: u16 = 5;
const BAR_WIDTH: u16 = 10;
const SPARK_WIDTH: usize = 10;
const SLOW_RESPONSE_MS: u64 = 1000;
const DEFAULT_ICON: &str = "■";

/// What the screen shows besides the controller's state: focus, zoom, and where the last frame put things.
#[derive(Debug, Default)]
pub struct ViewState {
    pub focus: usize,
    pub zoom: bool,
    pub body_rows: u16,
    pub columns: Vec<Rect>,
    pub started: Option<Instant>,
}

pub fn draw(frame: &mut Frame, controller: &Controller, view: &mut ViewState, palette: &Palette, now: Instant) {
    let screen = frame.area();
    let buf = frame.buffer_mut();
    buf.set_style(screen, Style::new().bg(palette.bg).fg(palette.text));
    if screen.width < MIN_SIZE.0 || screen.height < MIN_SIZE.1 {
        let hint = format!(" lazyrunner: make the terminal a bit bigger ({}x{} at least)", MIN_SIZE.0, MIN_SIZE.1);
        buf.set_stringn(screen.x, screen.y, hint, usize::from(screen.width), Style::new().fg(palette.yellow));
        return;
    }
    let started = *view.started.get_or_insert(now);
    let [columns, panel] = Layout::vertical([Constraint::Fill(1), Constraint::Length(LOG_ROWS + 3)]).areas(screen);
    draw_columns(buf, columns, controller, view, palette, now - started);
    draw_panel(buf, panel, controller, palette, now - started);
}

fn draw_columns(
    buf: &mut Buffer,
    area: Rect,
    controller: &Controller,
    view: &mut ViewState,
    palette: &Palette,
    elapsed: Duration,
) {
    let count = controller.apps.len();
    view.focus = view.focus.min(count.saturating_sub(1));
    let shown: Vec<usize> = if view.zoom { vec![view.focus] } else { (0..count).collect() };
    let rects = Layout::horizontal(vec![Constraint::Fill(1); shown.len()]).spacing(1).split(area);
    view.columns = vec![Rect::default(); count];
    view.body_rows = area.height.saturating_sub(2);
    let now = Instant::now();
    for (slot, &index) in shown.iter().enumerate() {
        let rect = rects[slot];
        view.columns[index] = rect;
        draw_column(buf, rect, controller, index, index == view.focus, palette, elapsed, now);
        if slot + 1 < shown.len() {
            let tone = tone(controller, index, palette, now);
            for y in area.top()..area.bottom() {
                buf[(rect.right(), y)].set_symbol("│").set_fg(lerp(tone, palette.bg, 0.55));
            }
        }
    }
}

/// The app's colour: yellow building or starting, green up, red when it failed, grey when stopped (flashing red
/// for a moment after an error).
fn tone(controller: &Controller, index: usize, palette: &Palette, now: Instant) -> Color {
    let state = &controller.states[index];
    let blink =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |since| since.as_millis() / 250);
    if state.flash_until.is_some_and(|until| now < until) && blink.is_multiple_of(2) {
        return palette.red;
    }
    if tool_job(controller, &state.key).is_some() {
        return palette.yellow;
    }
    match &state.status {
        Status::Running if state.is_up() => palette.green,
        Status::Running => palette.yellow,
        Status::Exited(_) | Status::Error(_) => palette.red,
        Status::Stopped => palette.faint,
    }
}

/// The build or npm ci running for this app.
fn tool_job<'a>(
    controller: &'a Controller,
    key: &str,
) -> Option<(&'static str, &'a lazyrunner_core::run::controller::ToolJob)> {
    if let Some(job) = controller.building.as_ref().filter(|job| job.key == key) {
        return Some(("building", job));
    }
    controller.npm_ci.as_ref().filter(|job| job.key == key).map(|job| ("npm ci", job))
}

#[allow(clippy::too_many_arguments)]
fn draw_column(
    buf: &mut Buffer,
    area: Rect,
    controller: &Controller,
    index: usize,
    focused: bool,
    palette: &Palette,
    elapsed: Duration,
    now: Instant,
) {
    if area.height < 3 || area.width < 6 {
        return;
    }
    let state = &controller.states[index];
    draw_title(buf, Rect { height: 1, ..area }, controller, index, focused, palette, elapsed, now);
    draw_info(buf, Rect { y: area.y + 1, height: 1, ..area }, controller, index, palette);
    draw_output(
        buf,
        Rect { y: area.y + 2, height: area.height - 2, ..area },
        &state.output,
        state.scroll,
        palette,
        now,
    );
}

#[allow(clippy::too_many_arguments)]
fn draw_title(
    buf: &mut Buffer,
    area: Rect,
    controller: &Controller,
    index: usize,
    focused: bool,
    palette: &Palette,
    elapsed: Duration,
    now: Instant,
) {
    let app = &controller.apps[index];
    let state = &controller.states[index];
    let tone = tone(controller, index, palette, now);
    let marker = if focused { "▸ " } else { "" };
    let icon = app.icon.as_deref().unwrap_or(DEFAULT_ICON);
    let chip_fg = if tone == palette.faint { palette.text } else { palette.bg };
    let mut spans = vec![Span::styled(
        format!(" {marker}{icon} {} ", app.name),
        Style::new().fg(chip_fg).bg(tone).add_modifier(Modifier::BOLD),
    )];
    let port = controller.port(&app.key).map_or("  ".to_string(), |port| format!(" :{port}  "));
    spans.push(Span::styled(port, Style::new().fg(palette.dim)));
    spans.extend(status_spans(controller, state, palette, elapsed, now));
    let line = Line::from(spans);
    let (end, _) = buf.set_line(area.x, area.y, &line, area.width);
    let (rule, color) = if focused { ("━", palette.accent) } else { ("─", lerp(tone, palette.bg, 0.5)) };
    for x in (end + 1)..area.right() {
        buf[(x, area.y)].set_symbol(rule).set_fg(color);
    }
}

/// What the app is doing, as the title says it.
fn status_spans(
    controller: &Controller,
    state: &AppState,
    palette: &Palette,
    elapsed: Duration,
    now: Instant,
) -> Vec<Span<'static>> {
    let yellow = Style::new().fg(palette.yellow);
    let dim = Style::new().fg(palette.dim);
    if let Some((verb, job)) = tool_job(controller, &state.key) {
        let took = now.saturating_duration_since(job.started);
        let mut spans = vec![Span::styled(format!("{} {verb} ", spinner(took)), yellow)];
        let kind = if verb == "building" { "build" } else { "npmci" };
        let (fraction, after) = if job.total > 0 && verb == "building" {
            (
                job.done.min(job.total) as f32 / job.total as f32,
                format!("{}/{} · {}", job.done.min(job.total), job.total, short(took)),
            )
        } else {
            match controller.timing_estimate(kind, &state.key) {
                Some(estimate) => (
                    took.as_secs_f32() / estimate as f32,
                    format!("{} / ~{}", short(took), short(Duration::from_secs_f64(estimate))),
                ),
                None => (0.0, short(took)),
            }
        };
        if job.total > 0 || controller.timing_estimate(kind, &state.key).is_some() {
            spans.extend(bar(fraction, BAR_WIDTH, elapsed, palette.yellow, palette.glint, palette.faint));
            spans.push(Span::raw(" "));
        }
        spans.push(Span::styled(after, dim));
        return spans;
    }
    match &state.status {
        Status::Running if state.is_up() => {
            let up = format!("  up in {}", short(Duration::from_secs_f64(state.up_after.unwrap_or_default())));
            if controller.port(&state.key).is_none() {
                return vec![Span::styled("● built, watching", Style::new().fg(palette.green)), Span::styled(up, dim)];
            }
            let probe = match controller.last_probe(&state.key) {
                None => Span::styled("checking…", dim),
                Some(result) if !result.ok => Span::styled("not responding", Style::new().fg(palette.red)),
                Some(result) => {
                    let code = if result.code >= 400 { format!(" HTTP {}", result.code) } else { String::new() };
                    if result.ms >= SLOW_RESPONSE_MS {
                        Span::styled(format!("slow {:.1}s{code}", result.ms as f64 / 1000.0), yellow)
                    } else {
                        Span::styled(format!("{} ms{code}", result.ms), Style::new().fg(palette.green))
                    }
                }
            };
            vec![Span::styled("● ", Style::new().fg(palette.green)), probe, Span::styled(up, dim)]
        }
        Status::Running => {
            // Not up yet: the dot pulses and the bar fills against the usual startup time.
            let took = state.started_at.map_or(Duration::ZERO, |started| now.saturating_duration_since(started));
            let dot = if (took.as_millis() / 300).is_multiple_of(2) { "●" } else { "○" };
            let verb = controller.app(&state.key).map_or("starting", |app| app.traits().start_verb);
            let mut spans = vec![Span::styled(format!("{dot} {verb} "), yellow)];
            match controller.timing_estimate("start", &state.key) {
                Some(estimate) => {
                    spans.extend(bar(
                        took.as_secs_f32() / estimate as f32,
                        BAR_WIDTH,
                        elapsed,
                        palette.yellow,
                        palette.glint,
                        palette.faint,
                    ));
                    spans.push(Span::styled(
                        format!(" {} / ~{}", short(took), short(Duration::from_secs_f64(estimate))),
                        dim,
                    ));
                }
                None => spans.push(Span::styled(short(took), dim)),
            }
            spans
        }
        Status::Exited(code) => {
            let code = code.map_or("?".into(), |code| code.to_string());
            vec![Span::styled(format!("✗ exited (code {code})"), Style::new().fg(palette.red))]
        }
        Status::Error(error) => vec![Span::styled(format!("✗ {error}"), Style::new().fg(palette.red))],
        Status::Stopped => vec![Span::styled("○ stopped", Style::new().fg(palette.faint))],
    }
}

fn short(took: Duration) -> String {
    lazyrunner_core::run::format_duration(took.as_secs_f64())
}

/// Second title line: live CPU and memory with sparklines, response times, requests, errors, the variant.
fn draw_info(buf: &mut Buffer, area: Rect, controller: &Controller, index: usize, palette: &Palette) {
    let state = &controller.states[index];
    let faint = Style::new().fg(palette.faint);
    let mut spans = vec![Span::raw(" ")];
    if state.scroll > 0 {
        spans.push(Span::styled(format!("⏸ ↑{} · G follows  ", state.scroll), Style::new().fg(palette.magenta)));
    }
    if state.is_running()
        && let Some(cpu) = state.cpu
    {
        let cpu_color = if cpu >= 80.0 {
            palette.red
        } else if cpu >= 40.0 {
            palette.yellow
        } else {
            palette.green
        };
        let history: Vec<f32> = state.cpu_history.iter().copied().collect();
        spans.push(Span::styled("cpu ", faint));
        spans.push(Span::styled(format!("{cpu:>3.0}%"), Style::new().fg(cpu_color)));
        spans.push(Span::styled(format!(" {}", sparkline(&history, SPARK_WIDTH, 25.0)), Style::new().fg(palette.cyan)));
        spans.push(Span::styled("  mem ", faint));
        spans.push(Span::styled(
            format!("{:.0} MB", state.memory_mb.unwrap_or_default()),
            Style::new().fg(palette.text),
        ));
    }
    if state.is_running() && !state.response_history.is_empty() {
        let history: Vec<f32> = state.response_history.iter().copied().collect();
        let slow = history.last().is_some_and(|ms| *ms >= SLOW_RESPONSE_MS as f32);
        spans.push(Span::styled("  resp ", faint));
        spans.push(Span::styled(
            sparkline(&history, SPARK_WIDTH, 200.0),
            Style::new().fg(if slow { palette.yellow } else { palette.cyan }),
        ));
    }
    if state.is_running()
        && controller.app(&state.key).is_some_and(|app| app.traits().counts_requests)
        && !state.request_history.is_empty()
    {
        let history: Vec<f32> = state.request_history.iter().copied().collect();
        spans.push(Span::styled("  req ", faint));
        spans.push(Span::styled(
            format!("{:.1}/s ", history.last().copied().unwrap_or_default()),
            Style::new().fg(palette.text),
        ));
        spans.push(Span::styled(sparkline(&history, SPARK_WIDTH, 5.0), Style::new().fg(palette.magenta)));
    }
    if state.errors > 0 {
        spans.push(Span::styled(format!("  ✗ {}", state.errors), Style::new().fg(palette.red)));
    }
    if state.warnings > 0 {
        spans.push(Span::styled(format!("  △ {}", state.warnings), Style::new().fg(palette.yellow)));
    }
    if let Some(mode) = controller.mode(&state.key) {
        spans.push(Span::styled(format!("  {} · {}", mode.label, mode.script), faint));
    }
    buf.set_line(area.x, area.y, &Line::from(spans), area.width);
}

fn draw_panel(buf: &mut Buffer, area: Rect, controller: &Controller, palette: &Palette, elapsed: Duration) {
    let faint = Style::new().fg(palette.faint);
    for x in area.left()..area.right() {
        buf[(x, area.y)].set_symbol("─").set_fg(palette.faint);
    }
    let busy = controller.building.is_some() || controller.npm_ci.is_some();
    let chip = if busy {
        lerp(palette.accent, palette.accent2, (elapsed.as_secs_f32() * 4.0).sin() * 0.5 + 0.5)
    } else {
        palette.accent
    };
    let running = controller.states.iter().filter(|state| state.is_running()).count();
    let up = controller.states.iter().filter(|state| state.is_up()).count();
    let summary = match &controller.celebrate {
        Some((text, _)) => Span::styled(format!("  ✦ {text} ✦"), Style::new().fg(palette.green)),
        None => Span::styled(
            format!("  {} apps · {running} running · {up} up", controller.apps.len()),
            Style::new().fg(palette.text),
        ),
    };
    let mut header = vec![
        Span::styled(" lazyrunner ", Style::new().fg(palette.bg).bg(chip).add_modifier(Modifier::BOLD)),
        summary,
        Span::styled(format!("   ⎇ {}", controller.branch), Style::new().fg(palette.magenta)),
    ];
    if let Some(db) = &controller.db_label {
        header.push(Span::styled("   DB ", faint));
        header.push(Span::styled(db.clone(), Style::new().fg(palette.yellow)));
    }
    header.push(Span::styled(format!("   {}", controller.root.display()), faint));
    buf.set_line(area.x, area.y + 1, &Line::from(header), area.width);

    let keys = [
        ("space", "start/stop"),
        ("U", "start all"),
        ("S", "stop all"),
        ("R", "restart all"),
        ("u s r b", "focused"),
        ("i", "npm ci"),
        ("h/l", "focus"),
        ("z", "zoom"),
        ("q", "quit"),
    ];
    let mut hints = vec![Span::raw(" ")];
    for (key, label) in keys {
        hints.push(Span::styled(key, Style::new().fg(palette.accent).add_modifier(Modifier::BOLD)));
        hints.push(Span::styled(format!(" {label}  "), faint));
    }
    buf.set_line(area.x, area.y + 2, &Line::from(hints), area.width);

    let rows = usize::from(area.height.saturating_sub(3));
    let entries: Vec<_> = controller.log.iter().rev().take(rows).collect();
    for (row, entry) in entries.iter().rev().enumerate() {
        let line = Line::from(vec![
            Span::styled(format!(" {} ", entry.time), faint),
            Span::styled(entry.text.clone(), Style::new().fg(hue_color(entry.hue, palette))),
        ]);
        buf.set_line(area.x, area.y + 3 + row as u16, &line, area.width);
    }
}

/// Whether the next frame differs even if nothing happens: something builds or starts, a column flashes, fresh
/// output still glows.
pub fn is_animating(controller: &Controller, now: Instant) -> bool {
    controller.building.is_some()
        || controller.npm_ci.is_some()
        || controller.celebrate.is_some()
        || controller.states.iter().any(|state| {
            let glowing =
                state.output.last_line_at().is_some_and(|at| now.saturating_duration_since(at) < crate::model::FADE);
            (state.is_running() && !state.is_up())
                || state.flash_until.is_some_and(|until| now < until)
                || (glowing && state.scroll == 0)
        })
}
