//! Draws the model: a header, one column per app (title chip and status, live stats, output), and the key hints.

use std::time::Instant;

use lazyrunner_core::output::{Level, OutputLine};
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::logo;
use crate::model::{Column, FADE, Model, Status};
use crate::theme::{Palette, lerp};
use crate::widgets::{bar, count, duration, sparkline, spinner};

const MIN_SIZE: (u16, u16) = (40, 8);
const BAR_WIDTH: u16 = 10;
const SPARK_WIDTH: usize = 12;
/// Timestamped lines look like "12:34:56.789 INF  ..."; the time and the level tag get their own colours.
const TIME_END: usize = 12;
const TAG_END: usize = 17;

pub fn draw(frame: &mut Frame, model: &mut Model, palette: &Palette, now: Instant) {
    let screen = frame.area();
    let buf = frame.buffer_mut();
    buf.set_style(screen, Style::new().bg(palette.bg).fg(palette.text));
    model.layout.screen = screen;
    if model.in_splash(now) {
        logo::splash(buf, screen, palette, now - model.started, model.columns.len());
        return;
    }
    if screen.width < MIN_SIZE.0 || screen.height < MIN_SIZE.1 {
        let hint = format!("lazyrunner: make the terminal a bit bigger ({}x{} at least)", MIN_SIZE.0, MIN_SIZE.1);
        buf.set_stringn(screen.x, screen.y, hint, usize::from(screen.width), Style::new().fg(palette.yellow));
        return;
    }
    let [header, middle, footer] =
        Layout::vertical([Constraint::Length(1), Constraint::Fill(1), Constraint::Length(1)]).areas(screen);
    draw_header(buf, header, model, palette, now);
    draw_columns(buf, middle, model, palette, now);
    draw_footer(buf, footer, palette);
}

fn draw_header(buf: &mut Buffer, area: Rect, model: &mut Model, palette: &Palette, now: Instant) {
    let starting = model.columns.iter().any(|column| matches!(column.status, Status::Starting { .. }));
    let pulse = if starting { ((now - model.started).as_secs_f32() * 4.0).sin() * 0.5 + 0.5 } else { 0.0 };
    let chip = lerp(palette.accent, palette.accent2, pulse);
    let running = model.columns.iter().filter(|column| matches!(column.status, Status::Running { .. })).count();
    let rate: u64 = model.columns.iter_mut().map(|column| u64::from(column.rate.per_second(now))).sum();
    let left = Line::from(vec![
        Span::styled(" lazyrunner ", Style::new().fg(palette.bg).bg(chip).add_modifier(Modifier::BOLD)),
        Span::styled(format!("  {} apps", model.columns.len()), Style::new().fg(palette.text)),
        Span::styled(format!(" · {running} running · "), Style::new().fg(palette.dim)),
        Span::styled(format!("{} lines/s", count(rate)), Style::new().fg(palette.text)),
    ]);
    buf.set_line(area.x, area.y, &left, area.width);
    let (cpu, memory) = model.own.map_or((0.0, 0.0), |own| (own.cpu, own.memory_bytes as f32 / 1_048_576.0));
    let right = format!("{:.0} fps · cpu {cpu:.1}% · {memory:.0} MB ", model.fps);
    let width = right.chars().count() as u16;
    if width + left.width() as u16 + 2 <= area.width {
        buf.set_string(area.right() - width, area.y, right, Style::new().fg(palette.faint));
    }
}

fn draw_columns(buf: &mut Buffer, area: Rect, model: &mut Model, palette: &Palette, now: Instant) {
    let shown: Vec<usize> = if model.zoom { vec![model.focus] } else { (0..model.columns.len()).collect() };
    let rects = Layout::horizontal(vec![Constraint::Fill(1); shown.len()]).spacing(1).split(area);
    model.layout.columns_area = area;
    model.layout.columns = vec![Rect::default(); model.columns.len()];
    model.layout.body_rows = area.height.saturating_sub(2);
    for (slot, &index) in shown.iter().enumerate() {
        let rect = rects[slot];
        model.layout.columns[index] = rect;
        draw_column(buf, rect, &mut model.columns[index], index == model.focus, model.started, palette, now);
        if slot + 1 < shown.len() {
            for y in area.top()..area.bottom() {
                buf[(rect.right(), y)].set_symbol("│").set_fg(palette.faint);
            }
        }
    }
}

fn tone(status: &Status, palette: &Palette) -> Color {
    match status {
        Status::Waiting => palette.faint,
        Status::Starting { .. } => palette.yellow,
        Status::Running { .. } => palette.green,
        Status::Exited(_) | Status::Failed(_) => palette.red,
    }
}

fn draw_column(
    buf: &mut Buffer,
    area: Rect,
    column: &mut Column,
    focused: bool,
    started: Instant,
    palette: &Palette,
    now: Instant,
) {
    if area.height < 3 || area.width < 4 {
        return;
    }
    let title = Rect { height: 1, ..area };
    let info = Rect { y: area.y + 1, height: 1, ..area };
    let body = Rect { y: area.y + 2, height: area.height - 2, ..area };
    draw_title(buf, title, column, focused, started, palette, now);
    draw_info(buf, info, column, palette, now);
    draw_body(buf, body, column, palette, now);
}

fn draw_title(
    buf: &mut Buffer,
    area: Rect,
    column: &Column,
    focused: bool,
    started: Instant,
    palette: &Palette,
    now: Instant,
) {
    let tone = tone(&column.status, palette);
    let marker = if focused { "▸ " } else { "" };
    let chip_fg = if matches!(column.status, Status::Waiting) { palette.text } else { palette.bg };
    let mut spans = vec![
        Span::styled(
            format!(" {marker}{} {} ", column.icon, column.name),
            Style::new().fg(chip_fg).bg(tone).add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
    ];
    match &column.status {
        Status::Waiting => spans.push(Span::styled("○ waiting", Style::new().fg(palette.faint))),
        Status::Starting { since, done, total } => {
            let elapsed = now.saturating_duration_since(*since);
            spans.push(Span::styled(format!("{} compiling ", spinner(elapsed)), Style::new().fg(palette.yellow)));
            spans.extend(bar(column.bar, BAR_WIDTH, now - started, palette.yellow, palette.glint, palette.faint));
            spans.push(Span::styled(format!(" {done}/{total} · {}", duration(elapsed)), Style::new().fg(palette.dim)));
        }
        Status::Running { up_after } => {
            spans.push(Span::styled("● running", Style::new().fg(palette.green)));
            spans.push(Span::styled(format!("  up in {:.1}s", up_after.as_secs_f32()), Style::new().fg(palette.dim)));
        }
        Status::Exited(code) => {
            let code = code.map_or("killed".to_string(), |code| format!("code {code}"));
            spans.push(Span::styled(format!("✗ exited ({code})"), Style::new().fg(palette.red)));
        }
        Status::Failed(error) => spans.push(Span::styled(format!("✗ {error}"), Style::new().fg(palette.red))),
    }
    let line = Line::from(spans);
    let (end, _) = buf.set_line(area.x, area.y, &line, area.width);
    let (rule, color) = if focused { ("━", palette.accent) } else { ("─", palette.faint) };
    for x in (end + 1)..area.right() {
        buf[(x, area.y)].set_symbol(rule).set_fg(color);
    }
}

fn draw_info(buf: &mut Buffer, area: Rect, column: &mut Column, palette: &Palette, now: Instant) {
    let mut spans = vec![Span::raw(" ")];
    if column.scroll > 0 {
        spans.push(Span::styled(format!("⏸ ↑{} · G follows  ", column.scroll), Style::new().fg(palette.magenta)));
    }
    let live = matches!(column.status, Status::Starting { .. } | Status::Running { .. });
    if live && let Some(&cpu) = column.cpu.back() {
        let cpu_color = match cpu {
            80.0.. => palette.red,
            40.0.. => palette.yellow,
            _ => palette.green,
        };
        let history: Vec<f32> = column.cpu.iter().copied().collect();
        spans.push(Span::styled("cpu ", Style::new().fg(palette.faint)));
        spans.push(Span::styled(format!("{cpu:>3.0}%"), Style::new().fg(cpu_color)));
        spans.push(Span::styled(format!(" {}", sparkline(&history, SPARK_WIDTH, 5.0)), Style::new().fg(palette.cyan)));
        spans.push(Span::styled("  mem ", Style::new().fg(palette.faint)));
        spans.push(Span::styled(format!("{:.0} MB", column.memory_mb), Style::new().fg(palette.text)));
    }
    let rate = column.rate.per_second(now);
    if live && (rate > 0 || column.output.total() > 0) {
        let history: Vec<f32> = column.rate.history(now).into_iter().map(|n| n as f32).collect();
        spans.push(Span::styled(format!("  {}/s ", count(u64::from(rate))), Style::new().fg(palette.text)));
        spans.push(Span::styled(sparkline(&history, SPARK_WIDTH, 10.0), Style::new().fg(palette.magenta)));
    }
    if column.output.errors() > 0 {
        spans.push(Span::styled(format!("  ✗ {}", count(column.output.errors())), Style::new().fg(palette.red)));
    }
    if column.output.warnings() > 0 {
        spans.push(Span::styled(format!("  △ {}", count(column.output.warnings())), Style::new().fg(palette.yellow)));
    }
    buf.set_line(area.x, area.y, &Line::from(spans), area.width);
}

/// Output, newest at the bottom, wrapped to the width. Fresh lines start bright and settle into their colour.
fn draw_body(buf: &mut Buffer, area: Rect, column: &Column, palette: &Palette, now: Instant) {
    let width = usize::from(area.width.saturating_sub(1)).max(1);
    let x = area.x + 1;
    let mut y = area.bottom();
    for line in column.output.newest_first(column.scroll) {
        if y <= area.top() {
            break;
        }
        let fade = 1.0 - (now.saturating_duration_since(line.born).as_secs_f32() / FADE.as_secs_f32()).min(1.0);
        let segments = segments(line, palette);
        let chunks = wrap(&line.text, width);
        for &(start, end) in chunks.iter().rev() {
            if y <= area.top() {
                break;
            }
            y -= 1;
            let mut cx = x;
            for &(seg_start, seg_end, color, bold) in &segments {
                let (from, to) = (start.max(seg_start), end.min(seg_end));
                if from >= to {
                    continue;
                }
                let mut style = Style::new().fg(lerp(color, palette.glint, fade * fade * 0.85));
                if bold {
                    style = style.add_modifier(Modifier::BOLD);
                }
                let (next, _) = buf.set_stringn(cx, y, &line.text[from..to], usize::from(area.right() - cx), style);
                cx = next;
            }
        }
    }
}

/// The line's coloured parts as byte ranges: time, level tag and message for timestamped lines, else one part.
fn segments(line: &OutputLine, palette: &Palette) -> Vec<(usize, usize, Color, bool)> {
    let text = line.text.as_bytes();
    let (message, tag) = match line.level {
        Level::Error => (palette.red, palette.red),
        Level::Warn => (palette.yellow, palette.yellow),
        Level::Debug => (palette.dim, palette.faint),
        Level::Info => (palette.text, palette.cyan),
    };
    let timestamped = text.len() > TAG_END && text[2] == b':' && text[5] == b':' && text[TIME_END] == b' ';
    if timestamped && line.text.is_char_boundary(TAG_END) {
        vec![(0, TIME_END, palette.faint, false), (TIME_END, TAG_END, tag, true), (TAG_END, text.len(), message, false)]
    } else {
        vec![(0, text.len(), message, false)]
    }
}

/// Splits `text` into byte ranges of at most `width` characters.
fn wrap(text: &str, width: usize) -> Vec<(usize, usize)> {
    let mut chunks = Vec::with_capacity(text.len() / width + 1);
    let mut start = 0;
    for (count, (index, _)) in text.char_indices().enumerate() {
        if count > 0 && count % width == 0 {
            chunks.push((start, index));
            start = index;
        }
    }
    chunks.push((start, text.len()));
    chunks
}

fn draw_footer(buf: &mut Buffer, area: Rect, palette: &Palette) {
    let hints = [("h/l", "focus"), ("1-9", "jump"), ("j/k", "scroll"), ("G", "follow"), ("z", "zoom"), ("q", "quit")];
    let mut spans = vec![Span::raw(" ")];
    for (key, label) in hints {
        spans.push(Span::styled(key, Style::new().fg(palette.accent).add_modifier(Modifier::BOLD)));
        spans.push(Span::styled(format!(" {label}   "), Style::new().fg(palette.faint)));
    }
    buf.set_line(area.x, area.y, &Line::from(spans), area.width);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::AppSpec;
    use crate::theme::NIGHT;
    use lazyrunner_core::process::{AppEvent, Launch};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn model(names: &[&str]) -> Model {
        let specs: Vec<AppSpec> = names
            .iter()
            .map(|name| AppSpec {
                name: name.to_string(),
                icon: "◆".into(),
                launch: Launch { program: "x".into(), args: vec![], dir: None },
            })
            .collect();
        Model::new(&specs, Instant::now(), None)
    }

    fn render(model: &mut Model, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let now = Instant::now();
        let frame = terminal.draw(|frame| draw(frame, model, &NIGHT, now)).unwrap();
        crate::dump::text(frame.buffer)
    }

    #[test]
    fn every_app_gets_a_titled_column_and_zoom_shows_only_the_focused_one() {
        let mut model = model(&["Orders", "Billing", "Users"]);
        let text = render(&mut model, 120, 20);
        assert!(text.contains("▸ ◆ Orders") && text.contains("◆ Billing") && text.contains("◆ Users"));
        assert!(text.lines().next().unwrap().contains(" lazyrunner   3 apps · 0 running"));
        model.focus = 1;
        model.zoom = true;
        let text = render(&mut model, 120, 20);
        assert!(text.contains("▸ ◆ Billing"));
        assert!(!text.contains("Orders") && !text.contains("Users"));
        assert_eq!(model.layout.columns[0], Rect::default(), "hidden columns have no area");
    }

    #[test]
    fn output_is_bottom_aligned_and_wraps() {
        let mut model = model(&["Orders"]);
        let now = Instant::now();
        model.apply(AppEvent::Started { app: 0, pid: Some(1) }, now);
        model.apply(AppEvent::Line { app: 0, text: "first".into(), stderr: false }, now);
        model.apply(AppEvent::Line { app: 0, text: "x".repeat(50), stderr: false }, now);
        let text = render(&mut model, 41, 10);
        let rows: Vec<&str> = text.lines().collect();
        assert_eq!(rows[8].trim(), "x".repeat(10), "the wrapped tail is on the last row above the hints");
        assert_eq!(rows[7].trim(), "x".repeat(40));
        assert_eq!(rows[6].trim(), "first");
    }

    #[test]
    fn starting_shows_progress_and_running_shows_the_startup_time() {
        let mut model = model(&["Orders"]);
        let now = Instant::now();
        model.apply(AppEvent::Started { app: 0, pid: Some(1) }, now);
        model.apply(AppEvent::Line { app: 0, text: "compiling 4/12 x".into(), stderr: false }, now);
        assert!(render(&mut model, 80, 10).contains("compiling ▕"));
        assert!(render(&mut model, 80, 10).contains(" 4/12 · 0s"));
        model.apply(AppEvent::Line { app: 0, text: "ready in 9 ms".into(), stderr: false }, now);
        assert!(render(&mut model, 80, 10).contains("● running  up in 0.0s"));
    }

    #[test]
    fn a_tiny_terminal_gets_a_hint_not_a_panic() {
        let mut model = model(&["Orders"]);
        assert!(render(&mut model, 30, 5).contains("make the terminal"));
        render(&mut model, 1, 1);
    }

    #[test]
    fn wraps_on_character_boundaries() {
        assert_eq!(wrap("abcdef", 4), [(0, 4), (4, 6)]);
        assert_eq!(wrap("", 4), [(0, 0)]);
        assert_eq!(wrap("äöüß", 2), [(0, 4), (4, 8)]);
    }
}
