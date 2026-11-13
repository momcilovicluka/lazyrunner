//! The startup doctor closing: its box's outline shrinks down into the status line, where what it found lives on.

use std::time::Duration;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::Ui;
use super::theme::C;

/// How long the outline takes to shrink away, and the doctor box's widest.
const GHOST: Duration = Duration::from_millis(350);
const DOCTOR_CELLS: f64 = 154.0;

/// How far it has shrunk (0 to 1, eased), while it shrinks.
fn progress(ui: &Ui) -> Option<f64> {
    let (closed, _) = ui.last_checks.as_ref()?;
    let since = ui.now.saturating_duration_since(*closed);
    if ui.reduced_motion || ui.overlay.is_some() || since >= GHOST {
        return None;
    }
    let share = since.as_secs_f64() / GHOST.as_secs_f64();
    Some(1.0 - (1.0 - share).powi(3))
}

pub fn animating(ui: &Ui) -> bool {
    progress(ui).is_some()
}

/// The outline, from the box's size and place to the status line's.
pub fn paint(buf: &mut Buffer, area: Rect, ui: &Ui) {
    let Some(shrunk) = progress(ui) else { return };
    let (width, height) = (f64::from(area.width), f64::from(area.height));
    let lerp = |from: f64, to: f64| from + (to - from) * shrunk;
    let start_width = DOCTOR_CELLS.min(width - 2.0);
    let left = lerp((width - start_width) / 2.0, 0.0).round() as u16;
    let right = lerp((width + start_width) / 2.0 - 1.0, width - 1.0).round() as u16;
    let top = lerp(1.0, height - 2.0).round() as u16;
    let bottom = lerp(height - 3.0, height - 1.0).round().max(f64::from(top)) as u16;
    let style = ui.theme.style(if shrunk < 0.5 { C::Cyan } else { C::DarkCyan }, None);
    let mut put = |x: u16, y: u16, symbol: &str| {
        if let Some(cell) = buf.cell_mut((area.x + x, area.y + y)) {
            cell.set_symbol(symbol).set_style(style);
        }
    };
    for x in left..=right {
        put(x, top, "─");
        put(x, bottom, "─");
    }
    for y in top..=bottom {
        put(left, y, "│");
        put(right, y, "│");
    }
    for (x, y, corner) in [(left, top, "┌"), (right, top, "┐"), (left, bottom, "└"), (right, bottom, "┘")] {
        put(x, y, corner);
    }
}
