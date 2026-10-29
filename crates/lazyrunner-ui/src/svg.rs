//! A frame as a terminal window's picture (SVG): the README's screenshots, and the screen in a bug report.

use std::fmt::Write as _;

use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier};

/// Cell size and font in the SVG (px): a 14 px monospace font is about 8.4 px wide.
const CELL: (f64, f64) = (8.4, 18.0);
const FONT_SIZE: f64 = 14.0;
pub const FONT: &str = "'Cascadia Mono', 'Cascadia Code', Consolas, 'DejaVu Sans Mono', Menlo, monospace";
/// The window around the screen: padding and the title bar.
const PAD: f64 = 14.0;
const TITLE_BAR: f64 = 34.0;
/// A terminal's default text and background when the theme leaves them to it.
pub const DEFAULT_FG: &str = "#cccccc";
pub const DEFAULT_BG: &str = "#0c0c0c";

/// One run of cells in the same colours and weight.
struct Run {
    x: u16,
    cells: u16,
    text: String,
    fg: String,
    bg: Option<String>,
    bold: bool,
    underline: bool,
}

/// A frame as a terminal window's picture, `title` on its title bar; `default_fg` and `default_bg` stand for the
/// terminal's own colours (what the theme leaves to it).
pub fn render(buffer: &Buffer, title: &str, default_fg: &str, default_bg: &str) -> String {
    let mut svg = svg_head(buffer.area, title, default_fg, default_bg);
    svg.push_str(&svg_cells(buffer, default_fg));
    svg.push_str("</svg>\n");
    svg
}

/// The window: its size, background, title bar and buttons.
fn svg_head(area: ratatui::layout::Rect, title: &str, default_fg: &str, default_bg: &str) -> String {
    let width = PAD * 2.0 + f64::from(area.width) * CELL.0;
    let height = TITLE_BAR + PAD * 2.0 + f64::from(area.height) * CELL.1;
    let mut svg = String::new();
    let _ = write!(
        svg,
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{width:.0}" height="{height:.0}" viewBox="0 0 {width:.0} {height:.0}" font-family="{FONT}" font-size="{FONT_SIZE}">
<rect width="100%" height="100%" rx="10" fill="{default_bg}"/>
<rect width="100%" height="{TITLE_BAR}" rx="10" fill="#000000" fill-opacity="0.28"/>
<circle cx="22" cy="17" r="6" fill="#ff5f57"/><circle cx="42" cy="17" r="6" fill="#febc2e"/><circle cx="62" cy="17" r="6" fill="#28c840"/>
<text x="{:.0}" y="22" fill="{default_fg}" fill-opacity="0.6" font-size="13" text-anchor="middle">{}</text>
"##,
        width / 2.0,
        escape(title)
    );
    svg
}

/// Every cell: the backgrounds, then the text, a run of cells in the same colours at a time.
fn svg_cells(buffer: &Buffer, default_fg: &str) -> String {
    let area = buffer.area;
    let mut svg = String::new();
    for y in 0..area.height {
        let runs = row_runs(buffer, y, default_fg);
        let top = TITLE_BAR + PAD + f64::from(y) * CELL.1;
        for run in &runs {
            if let Some(bg) = &run.bg {
                let _ = writeln!(
                    svg,
                    r#"<rect x="{:.1}" y="{top:.1}" width="{:.1}" height="{}" fill="{bg}"/>"#,
                    PAD + f64::from(run.x) * CELL.0,
                    f64::from(run.cells) * CELL.0,
                    CELL.1
                );
            }
        }
        let baseline = top + CELL.1 * 0.76;
        for run in runs.iter().filter(|run| !run.text.trim().is_empty()) {
            let weight = if run.bold { r#" font-weight="bold""# } else { "" };
            let underline = if run.underline { r#" text-decoration="underline""# } else { "" };
            let _ = writeln!(
                svg,
                r#"<text x="{:.1}" y="{baseline:.1}" fill="{}" textLength="{:.1}" lengthAdjust="spacingAndGlyphs"{weight}{underline} xml:space="preserve">{}</text>"#,
                PAD + f64::from(run.x) * CELL.0,
                run.fg,
                f64::from(run.cells) * CELL.0,
                escape(&run.text)
            );
        }
    }
    svg
}

fn row_runs(buffer: &Buffer, y: u16, default_fg: &str) -> Vec<Run> {
    let area = buffer.area;
    let mut runs: Vec<Run> = Vec::new();
    for x in 0..area.width {
        let cell = &buffer[(x, y)];
        let (mut fg, mut bg) = (hex(cell.fg), hex(cell.bg));
        if cell.modifier.contains(Modifier::REVERSED) {
            (fg, bg) = (bg.or_else(|| Some(DEFAULT_BG.into())), fg.or_else(|| Some(default_fg.into())));
        }
        let fg = fg.unwrap_or_else(|| default_fg.into());
        let bold = cell.modifier.contains(Modifier::BOLD);
        let underline = cell.modifier.contains(Modifier::UNDERLINED);
        match runs.last_mut() {
            Some(run) if run.fg == fg && run.bg == bg && run.bold == bold && run.underline == underline => {
                run.cells += 1;
                run.text.push_str(cell.symbol());
            }
            _ => runs.push(Run { x, cells: 1, text: cell.symbol().to_string(), fg, bg, bold, underline }),
        }
    }
    runs
}

/// A colour as #rrggbb; `None` is the terminal's default.
fn hex(color: Color) -> Option<String> {
    let (r, g, b) = match color {
        Color::Reset => return None,
        Color::Rgb(r, g, b) => (r, g, b),
        Color::Indexed(n) => xterm(n),
        named => xterm(ansi_index(named)),
    };
    Some(format!("#{r:02x}{g:02x}{b:02x}"))
}

fn ansi_index(color: Color) -> u8 {
    match color {
        Color::Black => 0,
        Color::Red => 1,
        Color::Green => 2,
        Color::Yellow => 3,
        Color::Blue => 4,
        Color::Magenta => 5,
        Color::Cyan => 6,
        Color::Gray => 7,
        Color::DarkGray => 8,
        Color::LightRed => 9,
        Color::LightGreen => 10,
        Color::LightYellow => 11,
        Color::LightBlue => 12,
        Color::LightMagenta => 13,
        Color::LightCyan => 14,
        _ => 15,
    }
}

/// The xterm 256-colour palette (the first 16 as Windows Terminal's Campbell scheme has them).
fn xterm(n: u8) -> (u8, u8, u8) {
    const BASIC: [(u8, u8, u8); 16] = [
        (12, 12, 12),
        (197, 15, 31),
        (19, 161, 14),
        (193, 156, 0),
        (0, 55, 218),
        (136, 23, 152),
        (58, 150, 221),
        (204, 204, 204),
        (118, 118, 118),
        (231, 72, 86),
        (22, 198, 12),
        (249, 241, 165),
        (59, 120, 255),
        (180, 0, 158),
        (97, 214, 214),
        (242, 242, 242),
    ];
    match n {
        0..=15 => BASIC[usize::from(n)],
        16..=231 => {
            let n = n - 16;
            let level = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
            (level(n / 36), level((n / 6) % 6), level(n % 6))
        }
        _ => {
            let gray = 8 + (n - 232) * 10;
            (gray, gray, gray)
        }
    }
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}
