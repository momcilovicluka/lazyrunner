//! Colour themes: one JSON file per theme (themes/README.md has the format). lazyrunner draws with the 16 console
//! colour names plus the Focus names; a theme maps each to a terminal colour. The bundled themes are built in; a
//! file of the same id in your themes folder replaces one, and new files add themes. `NO_COLOR` turns colour off
//! (backgrounds become reverse video).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use ratatui::style::{Color, Modifier, Style};
use serde_json::Value;

/// The colour names lazyrunner draws with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum C {
    Black,
    DarkRed,
    DarkGreen,
    DarkYellow,
    DarkBlue,
    DarkMagenta,
    DarkCyan,
    Gray,
    DarkGray,
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    White,
    FocusBack,
    FocusText,
    FocusBorder,
}

pub const PALETTE: [(&str, C); 16] = [
    ("Black", C::Black),
    ("DarkRed", C::DarkRed),
    ("DarkGreen", C::DarkGreen),
    ("DarkYellow", C::DarkYellow),
    ("DarkBlue", C::DarkBlue),
    ("DarkMagenta", C::DarkMagenta),
    ("DarkCyan", C::DarkCyan),
    ("Gray", C::Gray),
    ("DarkGray", C::DarkGray),
    ("Red", C::Red),
    ("Green", C::Green),
    ("Yellow", C::Yellow),
    ("Blue", C::Blue),
    ("Magenta", C::Magenta),
    ("Cyan", C::Cyan),
    ("White", C::White),
];

/// The bundled themes.
const BUNDLED: [(&str, &str); 26] = [
    ("default", include_str!("../../../../themes/default.json")),
    ("ayu-mirage", include_str!("../../../../themes/ayu-mirage.json")),
    ("catppuccin-latte", include_str!("../../../../themes/catppuccin-latte.json")),
    ("catppuccin-mocha", include_str!("../../../../themes/catppuccin-mocha.json")),
    ("dark", include_str!("../../../../themes/dark.json")),
    ("dracula", include_str!("../../../../themes/dracula.json")),
    ("evangelion", include_str!("../../../../themes/evangelion.json")),
    ("everforest", include_str!("../../../../themes/everforest.json")),
    ("github-dark", include_str!("../../../../themes/github-dark.json")),
    ("github-light", include_str!("../../../../themes/github-light.json")),
    ("gruvbox", include_str!("../../../../themes/gruvbox.json")),
    ("hatsune-miku", include_str!("../../../../themes/hatsune-miku.json")),
    ("high-contrast", include_str!("../../../../themes/high-contrast.json")),
    ("kanagawa", include_str!("../../../../themes/kanagawa.json")),
    ("light", include_str!("../../../../themes/light.json")),
    ("monokai-pro", include_str!("../../../../themes/monokai-pro.json")),
    ("night-owl", include_str!("../../../../themes/night-owl.json")),
    ("nord", include_str!("../../../../themes/nord.json")),
    ("one-dark", include_str!("../../../../themes/one-dark.json")),
    ("rem", include_str!("../../../../themes/rem.json")),
    ("rose-pine", include_str!("../../../../themes/rose-pine.json")),
    ("shades-of-purple", include_str!("../../../../themes/shades-of-purple.json")),
    ("solarized-dark", include_str!("../../../../themes/solarized-dark.json")),
    ("solarized-light", include_str!("../../../../themes/solarized-light.json")),
    ("tokyo-night", include_str!("../../../../themes/tokyo-night.json")),
    ("zero-two", include_str!("../../../../themes/zero-two.json")),
];

/// A resolved theme: an SGR code per colour name, for text and for backgrounds.
#[derive(Clone, Debug, Default)]
pub struct Theme {
    pub id: String,
    pub name: String,
    pub description: String,
    fg: HashMap<C, String>,
    bg: HashMap<C, String>,
    /// Recolours the terminal itself while lazyrunner runs: (foreground, background) as #rrggbb.
    pub terminal: Option<(Option<String>, Option<String>)>,
    /// Made for a light background: the picker lists it with the light themes.
    pub light: bool,
}

impl Theme {
    /// No colours at all (NO_COLOR): text in the terminal's colour, backgrounds as reverse video.
    pub fn no_color() -> Self {
        Self { id: "no-color".into(), name: "no-color".into(), ..Self::default() }
    }

    /// Text in `fg`, on `bg` when given (reverse video when the theme has no colour for it, as with NO_COLOR).
    pub fn style(&self, fg: C, bg: Option<C>) -> Style {
        let mut style = self.fg.get(&fg).map_or_else(|| Style::new().fg(Color::Reset), |code| sgr(code));
        if let Some(bg) = bg {
            match self.bg.get(&bg) {
                Some(code) => style = style.patch(sgr(code)),
                None => style = style.add_modifier(Modifier::REVERSED),
            }
        }
        style
    }
}

/// An SGR code ("91", "1;93", "38;5;208", "38;2;r;g;b", "48;…") as a style.
pub fn sgr(code: &str) -> Style {
    let parts: Vec<u16> = code.split(';').filter_map(|part| part.trim().parse::<u16>().ok()).collect();
    let mut style = Style::new();
    let mut index = 0;
    while index < parts.len() {
        let part = parts[index];
        match part {
            0 => style = Style::new(),
            1 => style = style.add_modifier(Modifier::BOLD),
            2 => style = style.add_modifier(Modifier::DIM),
            3 => style = style.add_modifier(Modifier::ITALIC),
            4 => style = style.add_modifier(Modifier::UNDERLINED),
            7 => style = style.add_modifier(Modifier::REVERSED),
            30..=37 | 90..=97 => style = style.fg(named(part)),
            39 => style = style.fg(Color::Reset),
            40..=47 | 100..=107 => style = style.bg(named(part - 10)),
            49 => style = style.bg(Color::Reset),
            38 | 48 => {
                let color = match parts.get(index + 1) {
                    Some(5) => {
                        let color = parts.get(index + 2).and_then(|&n| u8::try_from(n).ok()).map(Color::Indexed);
                        index += 2;
                        color
                    }
                    Some(2) => {
                        let rgb = parts.get(index + 2..index + 5).and_then(|rgb| {
                            let [r, g, b] = [rgb[0], rgb[1], rgb[2]].map(|n| u8::try_from(n).ok());
                            Some(Color::Rgb(r?, g?, b?))
                        });
                        index += 4;
                        rgb
                    }
                    _ => None,
                };
                if let Some(color) = color {
                    style = if part == 38 { style.fg(color) } else { style.bg(color) };
                }
            }
            _ => {}
        }
        index += 1;
    }
    style
}

fn named(code: u16) -> Color {
    match code {
        30 => Color::Black,
        31 => Color::Red,
        32 => Color::Green,
        33 => Color::Yellow,
        34 => Color::Blue,
        35 => Color::Magenta,
        36 => Color::Cyan,
        37 => Color::Gray,
        90 => Color::DarkGray,
        91 => Color::LightRed,
        92 => Color::LightGreen,
        93 => Color::LightYellow,
        94 => Color::LightBlue,
        95 => Color::LightMagenta,
        96 => Color::LightCyan,
        _ => Color::White,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Layer {
    Fg,
    Bg,
}

/// "#rrggbb" → true colour; "91" / "1;91" / "38;5;208" as given; a palette name → that colour in `codes`.
fn theme_code(value: &str, layer: Layer, codes: &HashMap<C, String>) -> Result<String, String> {
    if let Some(hex) =
        value.strip_prefix('#').filter(|hex| hex.len() == 6 && hex.chars().all(|c| c.is_ascii_hexdigit()))
    {
        let channel = |at: usize| u8::from_str_radix(&hex[at..at + 2], 16).unwrap_or(0);
        let lead = if layer == Layer::Fg { 38 } else { 48 };
        return Ok(format!("{lead};2;{};{};{}", channel(0), channel(2), channel(4)));
    }
    if let Some(code) = PALETTE.iter().find(|(name, _)| *name == value).and_then(|(_, c)| codes.get(c)) {
        return Ok(code.clone());
    }
    if value.is_empty() || !value.split(';').all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit())) {
        return Err(format!("'{value}' is not #rrggbb, an SGR code like 91, or a colour name"));
    }
    if layer == Layer::Fg {
        return Ok(value.to_string());
    }
    // A foreground code used as a background: 38;… → 48;…, 30-37 / 90-97 → +10.
    if let Some(rest) = value.strip_prefix("38;") {
        return Ok(format!("48;{rest}"));
    }
    let mut parts: Vec<String> = value.split(';').map(str::to_string).collect();
    if let Some(last) = parts.last_mut().and_then(|last| last.parse::<u16>().ok().map(|n| (last, n)))
        && (matches!(last.1, 30..=37 | 90..=97 | 39))
    {
        *last.0 = (last.1 + 10).to_string();
    }
    Ok(parts.join(";"))
}

/// Builds one theme, following "extends" (every theme but default extends default unless it says otherwise).
fn resolve(id: &str, files: &HashMap<String, Value>, chain: &[String]) -> Result<Theme, String> {
    if chain.iter().any(|seen| seen == id) {
        return Err(format!("extends loops: {} → {id}", chain.join(" → ")));
    }
    let data = files.get(id).ok_or_else(|| format!("extends '{id}', which isn't in the themes folder"))?;
    let parent_id = match data.get("extends") {
        Some(Value::String(parent)) => Some(parent.clone()),
        Some(_) => None, // "extends": null starts from the terminal's own colours
        None if id != "default" && files.contains_key("default") => Some("default".into()),
        None => None,
    };
    let mut chain = chain.to_vec();
    chain.push(id.to_string());
    let parent = match parent_id {
        Some(parent) => resolve(&parent, files, &chain)?,
        None => Theme::default(),
    };
    let (mut fg, mut bg) = (parent.fg.clone(), parent.bg.clone());
    for section in ["colors", "foregrounds", "backgrounds"] {
        let Some(map) = data.get(section).and_then(Value::as_object) else { continue };
        for (name, value) in map {
            let Some(&(_, color)) = PALETTE.iter().find(|(known, _)| known == name) else {
                let names: Vec<&str> = PALETTE.iter().map(|(name, _)| *name).collect();
                return Err(format!("{section}: unknown colour '{name}' (use: {})", names.join(", ")));
            };
            let value = crate::screen::seg::text_of(value);
            if section != "backgrounds" {
                let code = theme_code(&value, Layer::Fg, &fg)?;
                fg.insert(color, code);
            }
            if section != "foregrounds" {
                let code = theme_code(&value, Layer::Bg, &bg)?;
                bg.insert(color, code);
            }
        }
    }
    // The focused column: title bar background and text, and the heavy borders beside it.
    let focus = data.get("focus").cloned().unwrap_or(Value::Null);
    for (field, color, layer) in [
        ("background", C::FocusBack, Layer::Bg),
        ("text", C::FocusText, Layer::Fg),
        ("border", C::FocusBorder, Layer::Fg),
    ] {
        let Some(value) = focus.get(field).map(crate::screen::seg::text_of).filter(|value| !value.is_empty()) else {
            continue;
        };
        let codes = if layer == Layer::Fg { &mut fg } else { &mut bg };
        let code = theme_code(&value, layer, codes)?;
        codes.insert(color, code);
    }
    let terminal = match data.get("terminal") {
        Some(Value::Object(terminal)) => {
            let color = |field: &str| -> Result<Option<String>, String> {
                match terminal.get(field).and_then(Value::as_str) {
                    None => Ok(None),
                    Some(value)
                        if value.len() == 7
                            && value.starts_with('#')
                            && value[1..].chars().all(|c| c.is_ascii_hexdigit()) =>
                    {
                        Ok(Some(value.into()))
                    }
                    Some(_) => Err(format!("terminal.{field} must be #rrggbb")),
                }
            };
            Some((color("foreground")?, color("background")?))
        }
        Some(_) => None,
        None => parent.terminal.clone(),
    };
    let own_background = data.get("terminal").and_then(|terminal| terminal.get("background")).and_then(Value::as_str);
    let light = match (data.get("light").and_then(Value::as_bool), own_background) {
        (Some(light), _) => light,
        (None, Some(background)) => is_light(background),
        (None, None) => parent.light,
    };
    let text = |field: &str| data.get(field).and_then(Value::as_str).map(str::to_string);
    Ok(Theme {
        id: id.into(),
        name: text("name").unwrap_or_else(|| id.into()),
        description: text("description").unwrap_or_default(),
        fg,
        bg,
        terminal,
        light,
    })
}

/// Whether a #rrggbb colour is a light one, as a light theme's background is.
fn is_light(hex: &str) -> bool {
    let channel = |at: usize| f64::from(u8::from_str_radix(hex.get(at..at + 2).unwrap_or("00"), 16).unwrap_or(0));
    (0.2126 * channel(1) + 0.7152 * channel(3) + 0.0722 * channel(5)) / 255.0 > 0.5
}

/// Every theme, default first, then by display name; and the files that couldn't be used, with why.
pub fn load(user_dir: Option<&Path>) -> (Vec<Theme>, Vec<String>) {
    let mut warnings = Vec::new();
    let mut files: HashMap<String, Value> = HashMap::new();
    for (id, text) in BUNDLED {
        if let Ok(data) = serde_json::from_str(text) {
            files.insert(id.to_string(), data);
        }
    }
    let user_files: Vec<PathBuf> = user_dir
        .and_then(|dir| std::fs::read_dir(dir).ok())
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.extension().is_some_and(|e| e.eq_ignore_ascii_case("json")))
                .collect()
        })
        .unwrap_or_default();
    for path in user_files {
        let id = path.file_stem().map_or(String::new(), |stem| stem.to_string_lossy().to_lowercase());
        let file = path.file_name().map_or(String::new(), |name| name.to_string_lossy().into_owned());
        match lazyrunner_core::json::read_text(&path)
            .map_err(|e| e.to_string())
            .and_then(|text| serde_json::from_str::<Value>(&text).map_err(|e| e.to_string()))
        {
            Ok(data) if data.is_object() => {
                files.insert(id, data);
            }
            Ok(_) => warnings.push(format!("Theme {file} skipped: the file must hold one JSON object")),
            Err(error) => warnings.push(format!("Theme {file} skipped: {error}")),
        }
    }
    let mut themes = Vec::new();
    let mut ids: Vec<&String> = files.keys().collect();
    ids.sort();
    for id in ids {
        match resolve(id, &files, &[]) {
            Ok(theme) => themes.push(theme),
            Err(error) => warnings.push(format!("Theme {id}.json skipped: {error}")),
        }
    }
    themes.sort_by(|a, b| {
        (a.id != "default").cmp(&(b.id != "default")).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    if themes.is_empty() {
        themes.push(Theme { id: "default".into(), name: "default".into(), ..Theme::default() });
        warnings.push("No themes found - using plain colours".into());
    }
    (themes, warnings)
}

/// Whether NO_COLOR is set (to anything but empty).
pub fn no_color() -> bool {
    std::env::var_os("NO_COLOR").is_some_and(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sgr_codes_become_styles() {
        assert_eq!(sgr("91"), Style::new().fg(Color::LightRed));
        assert_eq!(sgr("1;93"), Style::new().fg(Color::LightYellow).add_modifier(Modifier::BOLD));
        assert_eq!(sgr("38;5;208"), Style::new().fg(Color::Indexed(208)));
        assert_eq!(sgr("48;2;1;2;3"), Style::new().bg(Color::Rgb(1, 2, 3)));
        assert_eq!(sgr("104"), Style::new().bg(Color::LightBlue));
    }

    #[test]
    fn every_bundled_theme_loads_default_first() {
        let (themes, warnings) = load(None);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(themes.len(), BUNDLED.len());
        assert_eq!(themes[0].id, "default");
        let default = &themes[0];
        assert_eq!(default.style(C::Gray, None), Style::new().fg(Color::Gray));
        assert_eq!(default.style(C::Black, Some(C::Cyan)), Style::new().fg(Color::Black).bg(Color::LightCyan));
        assert_eq!(default.style(C::FocusText, Some(C::FocusBack)), Style::new().fg(Color::White).bg(Color::LightBlue));
        let tokyo = themes.iter().find(|theme| theme.id == "tokyo-night").unwrap();
        assert_eq!(tokyo.style(C::Red, None), Style::new().fg(Color::Rgb(0xf7, 0x76, 0x8e)));
        assert_eq!(tokyo.terminal.as_ref().and_then(|t| t.1.as_deref()), Some("#1a1b26"));
    }

    #[test]
    fn light_themes_are_told_from_dark_ones() {
        let (themes, _) = load(None);
        let light = |id: &str| themes.iter().find(|theme| theme.id == id).unwrap().light;
        for id in ["light", "github-light", "catppuccin-latte", "solarized-light"] {
            assert!(light(id), "{id} is a light theme");
        }
        for id in ["default", "dark", "dracula", "solarized-dark", "evangelion"] {
            assert!(!light(id), "{id} is a dark theme");
        }
        assert!(is_light("#fdf6e3") && !is_light("#282a36"));
    }

    #[test]
    fn a_users_theme_extends_and_a_broken_one_is_skipped_with_why() {
        let dir = std::env::temp_dir().join(format!("lazyrunner-themes-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("mine.json"),
            r##"{ "name": "Mine", "extends": "dracula", "colors": { "Red": "#010203" } }"##,
        )
        .unwrap();
        std::fs::write(dir.join("bad.json"), r##"{ "colors": { "Pink": "#ffffff" } }"##).unwrap();
        let (themes, warnings) = load(Some(&dir));
        let mine = themes.iter().find(|theme| theme.id == "mine").unwrap();
        assert_eq!(mine.style(C::Red, None), Style::new().fg(Color::Rgb(1, 2, 3)));
        assert!(
            warnings.iter().any(|warning| warning.contains("bad.json skipped: colors: unknown colour 'Pink'")),
            "{warnings:?}"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn without_colours_backgrounds_are_reverse_video() {
        let theme = Theme::no_color();
        assert_eq!(
            theme.style(C::Black, Some(C::Cyan)),
            Style::new().fg(Color::Reset).add_modifier(Modifier::REVERSED)
        );
    }
}
