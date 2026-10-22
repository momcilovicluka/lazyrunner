//! Ctrl+R: reload window, like VS Code's - the settings and themes read again, the terminal set up again (the loop
//! does that part), everything repainted; the apps keep running. It plays a little CRT show over the frames: the
//! screen powers off into a glowing line and a sparkle, the logo shimmers in rainbow colours over twinkling stars with
//! a rainbow progress bar, then the fresh UI powers on from the middle. With reduced motion it just reloads.

use std::time::{Duration, Instant};

use lazyrunner_core::config::Toggle;
use lazyrunner_core::output::Hue;
use lazyrunner_core::run::Controller;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;

use super::theme::{self, C};
use super::{Ui, overlays};

/// Power off: the screen closes into a line, the line into a sparkle.
const OFF: Duration = Duration::from_millis(300);
/// The logo over the stars.
const STAGE: Duration = Duration::from_millis(650);
/// Power on: the fresh UI opens from the middle.
const ON: Duration = Duration::from_millis(520);
/// The header celebrates this long once the show is over.
const CELEBRATE: Duration = Duration::from_secs(2);
const RAINBOW: [C; 6] = [C::Red, C::Yellow, C::Green, C::Cyan, C::Blue, C::Magenta];
const STARS: [C; 5] = [C::White, C::Cyan, C::Magenta, C::Yellow, C::Green];
const TWINKLES: [&str; 4] = ["·", "✧", "✦", "*"];
const BAR_CELLS: u16 = 30;

/// The show going on: when it began, and the seed of its stars.
#[derive(Clone, Copy, Debug)]
pub struct Show {
    started: Instant,
    seed: u64,
}

/// Ctrl+R.
pub fn start(ui: &mut Ui, controller: &mut Controller) {
    controller.reload_settings();
    let (themes, warnings) = theme::load(ui.user_themes.as_deref());
    ui.themes = themes;
    let wanted = controller.config["theme"].as_str().unwrap_or("default").to_string();
    ui.set_theme(&wanted);
    for warning in warnings {
        controller.add_log(warning, Hue::Yellow);
    }
    ui.reset_terminal = true;
    ui.reduced_motion = controller.setting(Toggle::ReducedMotion);
    let mut celebrate = CELEBRATE;
    if !ui.reduced_motion {
        let seed = ui.now.saturating_duration_since(ui.started).as_nanos() as u64 | 1;
        ui.reload = Some(Show { started: ui.now, seed });
        celebrate += OFF + STAGE + ON;
    }
    controller.celebrate = Some(("↻ window reloaded".into(), ui.now + celebrate));
    controller.add_log("↻ Window reloaded: the settings and themes read again, the terminal set up again", Hue::Cyan);
}

/// Paints the show over the frame just drawn; it ends by itself.
pub fn paint(buf: &mut Buffer, area: Rect, ui: &mut Ui) {
    let Some(show) = ui.reload else { return };
    let elapsed = ui.now.saturating_duration_since(show.started);
    if elapsed < OFF {
        let (glow, edge) = (ui.theme.style(C::White, None), ui.theme.style(C::Cyan, None));
        power_off(buf, area, elapsed.as_secs_f64() / OFF.as_secs_f64(), glow, edge);
    } else if elapsed < OFF + STAGE {
        let seconds = (elapsed - OFF).as_secs_f64();
        stage(buf, area, ui, show, seconds, seconds / STAGE.as_secs_f64());
    } else if elapsed < OFF + STAGE + ON {
        let progress = (elapsed - OFF - STAGE).as_secs_f64() / ON.as_secs_f64();
        power_on(buf, area, progress, ui.theme.style(C::White, None));
    } else {
        ui.reload = None;
    }
}

fn blank(buf: &mut Buffer, x: u16, y: u16) {
    if let Some(cell) = buf.cell_mut((x, y)) {
        cell.set_symbol(" ").set_style(Style::reset());
    }
}

fn put(buf: &mut Buffer, x: u16, y: u16, symbol: &str, style: Style) {
    if let Some(cell) = buf.cell_mut((x, y)) {
        cell.set_symbol(symbol).set_style(style);
    }
}

/// The screen closes into its middle line (first 60%), then the line into a sparkle.
fn power_off(buf: &mut Buffer, area: Rect, progress: f64, glow: Style, edge: Style) {
    let middle = area.y + area.height / 2;
    let center = area.x + area.width / 2;
    let (rows, cols) = if progress < 0.6 {
        (((1.0 - progress / 0.6) * f64::from(area.height) / 2.0) as u16, area.width / 2)
    } else {
        (0, ((1.0 - (progress - 0.6) / 0.4) * f64::from(area.width) / 2.0) as u16)
    };
    for y in area.y..area.y + area.height {
        for x in area.x..area.x + area.width {
            if y.abs_diff(middle) > rows || x.abs_diff(center) > cols {
                blank(buf, x, y);
            } else if y.abs_diff(middle) == rows
                && let Some(cell) = buf.cell_mut((x, y))
            {
                cell.set_style(edge); // the glowing edge
            }
        }
    }
    if progress >= 0.6 {
        put(buf, center, middle, "✦", glow);
    }
}

/// The logo in shimmering rainbow colours over twinkling stars, a rainbow bar filling under it.
fn stage(buf: &mut Buffer, area: Rect, ui: &Ui, show: Show, seconds: f64, progress: f64) {
    for y in area.y..area.y + area.height {
        for x in area.x..area.x + area.width {
            blank(buf, x, y);
        }
    }
    let mut random = show.seed;
    let mut next = || {
        random = random.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        random >> 33
    };
    let count = (u32::from(area.width) * u32::from(area.height) / 40).max(8);
    for _ in 0..count {
        let x = area.x + (next() % u64::from(area.width.max(1))) as u16;
        let y = area.y + (next() % u64::from(area.height.max(1))) as u16;
        let phase = (next() % 628) as f64 / 100.0;
        let speed = 5.0 + (next() % 900) as f64 / 100.0;
        let color = STARS[(next() % STARS.len() as u64) as usize];
        let shine = (seconds * speed + phase).sin();
        if shine > 0.2 {
            let twinkle = TWINKLES[((shine * TWINKLES.len() as f64) as usize).min(TWINKLES.len() - 1)];
            put(buf, x, y, twinkle, ui.theme.style(color, None));
        }
    }
    let logo = overlays::LOGO;
    let wide = logo[0].chars().count() as u16;
    let top = (area.y + area.height / 2).saturating_sub(3);
    let left = area.x + area.width.saturating_sub(wide) / 2;
    let shift = (seconds * 12.0) as usize;
    for (row, line) in logo.iter().enumerate() {
        for (at, symbol) in line.chars().enumerate().filter(|(_, symbol)| *symbol != ' ') {
            let color = RAINBOW[(at / 3 + shift) % RAINBOW.len()];
            put(buf, left + at as u16, top + row as u16, &symbol.to_string(), ui.theme.style(color, None));
        }
    }
    let bar_left = area.x + area.width.saturating_sub(BAR_CELLS) / 2;
    let filled = (progress.clamp(0.0, 1.0) * f64::from(BAR_CELLS)) as u16;
    for at in 0..BAR_CELLS {
        let (symbol, color) = if at < filled {
            ("█", RAINBOW[(usize::from(at) / 5 + shift) % RAINBOW.len()])
        } else {
            ("░", C::DarkGray)
        };
        put(buf, bar_left + at, top + 4, symbol, ui.theme.style(color, None));
    }
    let text = "↻ reloading";
    let text_left = area.x + area.width.saturating_sub(text.chars().count() as u16) / 2;
    for (at, symbol) in text.chars().enumerate() {
        put(buf, text_left + at as u16, top + 6, &symbol.to_string(), ui.theme.style(C::DarkGray, None));
    }
}

/// The fresh UI opens from its middle line outwards, between glowing edges.
fn power_on(buf: &mut Buffer, area: Rect, progress: f64, glow: Style) {
    let middle = area.y + area.height / 2;
    let rows = (progress.clamp(0.0, 1.0) * f64::from(area.height) / 2.0) as u16;
    for y in area.y..area.y + area.height {
        let distance = y.abs_diff(middle);
        for x in area.x..area.x + area.width {
            if distance > rows {
                blank(buf, x, y);
            } else if distance == rows
                && let Some(cell) = buf.cell_mut((x, y))
            {
                cell.set_style(glow);
            }
        }
    }
}
