//! Colour depth: truecolor where the terminal has it, the xterm 256-colour palette where not (`downsample`).

use ratatui::buffer::Buffer;
use ratatui::style::Color;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorMode {
    TrueColor,
    Ansi256,
}

impl ColorMode {
    pub fn detect() -> Self {
        Self::from_env(|name| std::env::var(name).ok())
    }

    /// `LAZYRUNNER_COLORS=256|truecolor` wins. Otherwise truecolor when the terminal says so (`COLORTERM`), and in
    /// Windows Terminal, VS Code and Windows 10+ consoles, which all have it; 256 colours everywhere else.
    pub fn from_env(get: impl Fn(&str) -> Option<String>) -> Self {
        match get("LAZYRUNNER_COLORS").as_deref() {
            Some("256") => return Self::Ansi256,
            Some("truecolor" | "24bit") => return Self::TrueColor,
            _ => {}
        }
        let says_truecolor = matches!(get("COLORTERM").as_deref(), Some("truecolor" | "24bit"));
        let known_truecolor = get("WT_SESSION").is_some() || get("TERM_PROGRAM").as_deref() == Some("vscode");
        if says_truecolor || known_truecolor || cfg!(windows) { Self::TrueColor } else { Self::Ansi256 }
    }
}

/// Rewrites every RGB colour in the buffer to its nearest xterm 256-colour index.
pub fn downsample(buf: &mut Buffer) {
    for cell in &mut buf.content {
        cell.fg = to_ansi256(cell.fg);
        cell.bg = to_ansi256(cell.bg);
    }
}

const CUBE: [u8; 6] = [0, 95, 135, 175, 215, 255];

pub fn to_ansi256(color: Color) -> Color {
    let Color::Rgb(r, g, b) = color else { return color };
    let level = |v: u8| match v {
        0..=47 => 0,
        48..=114 => 1,
        _ => (usize::from(v) - 35) / 40,
    };
    let (ri, gi, bi) = (level(r), level(g), level(b));
    let cube = (CUBE[ri], CUBE[gi], CUBE[bi]);
    let average = (u32::from(r) + u32::from(g) + u32::from(b)) / 3;
    let gray_index = (average.saturating_sub(3) / 10).min(23);
    let gray = (8 + gray_index * 10) as u8;
    let distance = |(cr, cg, cb): (u8, u8, u8)| {
        let d = |a: u8, b: u8| (i32::from(a) - i32::from(b)).pow(2);
        d(cr, r) + d(cg, g) + d(cb, b)
    };
    if distance((gray, gray, gray)) < distance(cube) {
        Color::Indexed(232 + gray_index as u8)
    } else {
        Color::Indexed(16 + (36 * ri + 6 * gi + bi) as u8)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_truecolor_from_the_environment_and_256_otherwise() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |name: &str| pairs.iter().find(|(key, _)| *key == name).map(|(_, value)| value.to_string())
        };
        assert_eq!(
            ColorMode::from_env(env(&[("LAZYRUNNER_COLORS", "256"), ("COLORTERM", "truecolor")])),
            ColorMode::Ansi256
        );
        assert_eq!(ColorMode::from_env(env(&[("COLORTERM", "truecolor")])), ColorMode::TrueColor);
        assert_eq!(ColorMode::from_env(env(&[("TERM_PROGRAM", "vscode")])), ColorMode::TrueColor);
        if !cfg!(windows) {
            assert_eq!(ColorMode::from_env(env(&[("TERM", "xterm-256color")])), ColorMode::Ansi256);
        }
    }

    #[test]
    fn maps_to_the_nearest_xterm_colour() {
        assert_eq!(to_ansi256(Color::Rgb(0, 0, 0)), Color::Indexed(16));
        assert_eq!(to_ansi256(Color::Rgb(255, 255, 255)), Color::Indexed(231));
        assert_eq!(to_ansi256(Color::Rgb(255, 0, 0)), Color::Indexed(196));
        assert_eq!(to_ansi256(Color::Rgb(0x1a, 0x1b, 0x26)), Color::Indexed(234), "near-black greys use the grey ramp");
        assert_eq!(to_ansi256(Color::Yellow), Color::Yellow, "named colours stay");
    }
}
