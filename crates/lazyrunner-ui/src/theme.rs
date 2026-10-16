//! Colours: one palette, drawn in truecolor where the terminal has it and in the xterm 256-colour palette where not.

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

#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub bg: Color,
    pub text: Color,
    pub dim: Color,
    pub faint: Color,
    pub accent: Color,
    pub accent2: Color,
    pub green: Color,
    pub yellow: Color,
    pub red: Color,
    pub magenta: Color,
    pub cyan: Color,
    /// What fresh output and light sweeps start from.
    pub glint: Color,
}

/// The default look (a night palette; P3 brings the theme files over).
pub const NIGHT: Palette = Palette {
    bg: Color::Rgb(0x1a, 0x1b, 0x26),
    text: Color::Rgb(0xc0, 0xca, 0xf5),
    dim: Color::Rgb(0x73, 0x7a, 0xa2),
    faint: Color::Rgb(0x41, 0x48, 0x68),
    accent: Color::Rgb(0x7a, 0xa2, 0xf7),
    accent2: Color::Rgb(0xbb, 0x9a, 0xf7),
    green: Color::Rgb(0x9e, 0xce, 0x6a),
    yellow: Color::Rgb(0xe0, 0xaf, 0x68),
    red: Color::Rgb(0xf7, 0x76, 0x8e),
    magenta: Color::Rgb(0xbb, 0x9a, 0xf7),
    cyan: Color::Rgb(0x7d, 0xcf, 0xff),
    glint: Color::Rgb(0xff, 0xff, 0xff),
};

/// Mixes `from` towards `to` by `t` (0..1). Non-RGB colours switch halfway.
pub fn lerp(from: Color, to: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    match (from, to) {
        (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) => {
            let mix = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * t).round() as u8;
            Color::Rgb(mix(r1, r2), mix(g1, g2), mix(b1, b2))
        }
        _ if t < 0.5 => from,
        _ => to,
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

    #[test]
    fn lerp_runs_from_one_colour_to_the_other() {
        let (black, white) = (Color::Rgb(0, 0, 0), Color::Rgb(255, 255, 255));
        assert_eq!(lerp(black, white, 0.0), black);
        assert_eq!(lerp(black, white, 1.0), white);
        assert_eq!(lerp(black, white, 0.5), Color::Rgb(128, 128, 128));
        assert_eq!(lerp(black, white, 7.0), white, "clamped");
    }
}
