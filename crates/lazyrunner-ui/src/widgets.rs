//! Small pieces of the look: spinners, sparklines, shimmering progress bars, durations and counts.

use std::time::Duration;

use ratatui::style::{Color, Style};
use ratatui::text::Span;

use crate::theme::lerp;

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const SPARKS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
const PARTIAL_BLOCKS: [&str; 8] = ["", "▏", "▎", "▍", "▌", "▋", "▊", "▉"];
const SPINNER_FRAME: Duration = Duration::from_millis(80);
/// How fast the glint runs along a progress bar, in cells per second.
const GLINT_SPEED: f32 = 14.0;

pub fn spinner(elapsed: Duration) -> &'static str {
    SPINNER[(elapsed.as_millis() / SPINNER_FRAME.as_millis()) as usize % SPINNER.len()]
}

/// A sparkline of the last `width` values, scaled to the largest (but at least `floor`, so noise stays flat).
pub fn sparkline(values: &[f32], width: usize, floor: f32) -> String {
    let shown = &values[values.len().saturating_sub(width)..];
    let max = shown.iter().copied().fold(floor, f32::max);
    shown
        .iter()
        .map(|value| {
            let level = if max > 0.0 { (value / max * 7.0).round() as usize } else { 0 };
            SPARKS[level.min(7)]
        })
        .collect()
}

/// A progress bar `width` cells wide: whole blocks, an eighth-block edge, and a glint that runs along the filled part.
pub fn bar(
    fraction: f32,
    width: u16,
    elapsed: Duration,
    color: Color,
    glint: Color,
    edge: Color,
) -> Vec<Span<'static>> {
    let width = usize::from(width.max(1));
    let cells = fraction.clamp(0.0, 1.0) * width as f32;
    let full = cells.floor() as usize;
    let partial = if full < width { PARTIAL_BLOCKS[((cells - full as f32) * 8.0) as usize % 8] } else { "" };
    let travel = (full + 8) as f32;
    let position = (elapsed.as_secs_f32() * GLINT_SPEED) % travel - 4.0;
    let mut spans = Vec::with_capacity(full + 3);
    spans.push(Span::styled("▕", Style::new().fg(edge)));
    for cell in 0..full {
        let glow = (1.0 - (cell as f32 - position).abs() / 3.0).max(0.0);
        spans.push(Span::styled("█", Style::new().fg(lerp(color, glint, glow * 0.75))));
    }
    let rest = width - full - usize::from(!partial.is_empty());
    spans.push(Span::styled(format!("{partial}{}", " ".repeat(rest)), Style::new().fg(color)));
    spans.push(Span::styled("▏", Style::new().fg(edge)));
    spans
}

/// "4s", "1m05s".
pub fn duration(elapsed: Duration) -> String {
    let seconds = elapsed.as_secs();
    if seconds < 60 { format!("{seconds}s") } else { format!("{}m{:02}s", seconds / 60, seconds % 60) }
}

/// "950", "9.9k", "12k", "1.2M".
pub fn count(n: u64) -> String {
    match n {
        0..1_000 => n.to_string(),
        1_000..10_000 => format!("{:.1}k", n as f64 / 1e3),
        10_000..1_000_000 => format!("{}k", n / 1000),
        _ => format!("{:.1}M", n as f64 / 1e6),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sparklines_scale_to_the_largest_value_but_keep_a_floor() {
        assert_eq!(sparkline(&[0.0, 5.0, 10.0], 10, 1.0), "▁▅█");
        assert_eq!(sparkline(&[1.0, 1.0], 10, 100.0), "▁▁", "below the floor is flat");
        assert_eq!(sparkline(&[1.0, 2.0, 3.0, 4.0], 2, 0.0).chars().count(), 2, "only the newest fit");
    }

    #[test]
    fn a_bar_always_takes_its_full_width() {
        for fraction in [0.0, 0.07, 0.5, 0.99, 1.0, 3.0] {
            let spans = bar(fraction, 10, Duration::from_millis(700), Color::Yellow, Color::White, Color::Gray);
            let width: usize = spans.iter().map(Span::width).sum();
            assert_eq!(width, 12, "fraction {fraction}: two edges and ten cells");
        }
    }

    #[test]
    fn durations_and_counts_read_like_people_write_them() {
        assert_eq!(duration(Duration::from_secs(4)), "4s");
        assert_eq!(duration(Duration::from_secs(65)), "1m05s");
        assert_eq!(count(950), "950");
        assert_eq!(count(9_940), "9.9k");
        assert_eq!(count(12_400), "12k");
        assert_eq!(count(1_250_000), "1.2M");
    }

    #[test]
    fn the_spinner_moves_every_80_ms() {
        assert_eq!(spinner(Duration::ZERO), "⠋");
        assert_eq!(spinner(Duration::from_millis(85)), "⠙");
        assert_eq!(spinner(Duration::from_millis(800)), "⠋");
    }
}
