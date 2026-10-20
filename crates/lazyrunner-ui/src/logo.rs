//! The splash: the logo with a light sweeping across it while the apps start.

use std::time::Duration;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use crate::theme::{Palette, lerp};
use crate::widgets::spinner;

pub const LOGO: [&str; 3] = [
    "█   ▄▀▄ ▀▀█ █ █ █▀▄ █ █ █▀▄ █▀▄ █▀▀ █▀▄",
    "█   █▀█ ▄▀  ▀█▀ █▀▄ █ █ █ █ █ █ █▀  █▀▄",
    "▀▀▀ ▀ ▀ ▀▀▀  ▀  ▀ ▀ ▀▀▀ ▀ ▀ ▀ ▀ ▀▀▀ ▀ ▀",
];
/// How fast the light crosses the logo, in cells per second, and how wide it is.
const SWEEP_SPEED: f32 = 42.0;
const SWEEP_WIDTH: f32 = 3.5;

pub fn width() -> u16 {
    LOGO[0].chars().count() as u16
}

/// Where the logo goes on a screen: centred, a little above the middle.
pub fn area(screen: Rect) -> Rect {
    let (w, h) = (width().min(screen.width), (LOGO.len() as u16).min(screen.height));
    Rect::new(screen.x + (screen.width - w) / 2, screen.y + screen.height.saturating_sub(h + 2) / 2, w, h)
}

pub fn splash(buf: &mut Buffer, screen: Rect, palette: &Palette, elapsed: Duration, apps: usize) {
    let logo = area(screen);
    let travel = f32::from(width()) + 24.0;
    let light = (elapsed.as_secs_f32() * SWEEP_SPEED) % travel - 12.0;
    for (row, text) in LOGO.iter().enumerate() {
        let y = logo.y + row as u16;
        if y >= logo.bottom() {
            break;
        }
        for (column, symbol) in text.chars().enumerate().take(usize::from(logo.width)) {
            let x = column as f32;
            let base = lerp(palette.accent, palette.accent2, x / f32::from(width()));
            let glow = (-((x - light + row as f32 * 1.5) / SWEEP_WIDTH).powi(2)).exp();
            let cell = &mut buf[(logo.x + column as u16, y)];
            cell.set_char(symbol);
            cell.set_style(Style::new().fg(lerp(base, palette.glint, glow * 0.9)));
        }
    }
    let caption = format!("{} starting {apps} apps", spinner(elapsed));
    let version = format!("v{}", env!("CARGO_PKG_VERSION"));
    let y = logo.bottom() + 1;
    for (offset, (text, style)) in [
        (version, Style::new().fg(palette.dim).add_modifier(Modifier::BOLD)),
        (caption, Style::new().fg(palette.faint)),
    ]
    .into_iter()
    .enumerate()
    {
        let row = y + offset as u16;
        if row < screen.bottom() {
            let w = text.chars().count() as u16;
            buf.set_stringn(
                screen.x + screen.width.saturating_sub(w) / 2,
                row,
                &text,
                usize::from(screen.width),
                style,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::NIGHT;

    #[test]
    fn the_logo_is_centred_and_its_rows_line_up() {
        assert!(LOGO.iter().all(|row| row.chars().count() == LOGO[0].chars().count()));
        let screen = Rect::new(0, 0, 100, 30);
        let logo = area(screen);
        assert_eq!((logo.x, logo.width, logo.height), ((100 - width()) / 2, width(), 3));
    }

    #[test]
    fn draws_the_logo_and_the_caption() {
        let screen = Rect::new(0, 0, 80, 20);
        let mut buf = Buffer::empty(screen);
        splash(&mut buf, screen, &NIGHT, Duration::from_millis(300), 5);
        let text = crate::dump::text(&buf);
        assert!(text.contains(LOGO[1]));
        assert!(text.contains("starting 5 apps"));
    }
}
