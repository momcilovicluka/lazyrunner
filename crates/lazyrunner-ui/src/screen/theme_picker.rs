//! `t`: the theme picker. A narrow panel on the right, so the rest of the screen shows the theme on the real columns.
//! Moving through the list applies each theme at once (a preview, terminal background included; nothing is saved);
//! typing filters by name (by description only when no name matches); Enter keeps and saves the selected theme for
//! every project, Tab for this project alone (Tab on this project's own theme undoes it); Esc puts back the one the
//! picker opened with. Themes are read again every time it opens.

use std::time::{Duration, Instant};

use crossterm::event::KeyEvent;
use lazyrunner_core::output::Hue;
use lazyrunner_core::run::Controller;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use serde_json::json;

use super::actions::fuzzy_score;
use super::icons::{self, Icon};
use super::mouse::Target;
use super::seg::{Seg, aim, on, seg, styled};
use super::theme::{self, C, Theme};
use super::{Overlay, Ui, keys};

const PAGE: usize = 10;
/// The colours each row's swatch shows, in the theme's own colours.
const SWATCH: [C; 6] = [C::Red, C::Yellow, C::Green, C::Cyan, C::Blue, C::Magenta];
/// Lines of the selected theme's description under the list.
const DESCRIPTION_LINES: usize = 3;
/// How long a light theme stays selected before it lights up the whole terminal's background too.
pub const LIGHT_SETTLE: Duration = Duration::from_millis(350);
/// Keeping a theme sends a ripple of its colours over the screen, this long.
pub const RIPPLE: Duration = Duration::from_millis(550);
/// The ripple's ring: how deep (in cells across; half as many rows), in the new theme's colours from the front back.
const RING: f64 = 7.0;
const RING_COLORS: [C; 6] = [C::Red, C::Yellow, C::Green, C::Cyan, C::Blue, C::Magenta];
/// Behind the ring the new theme glows a little brighter for this many cells.
const GLOW: f64 = 4.0;
/// The sparkles on the ring, and how often they move.
const SPARKLES: [&str; 4] = ["✦", "✧", "⋆", "·"];
const TWINKLE_MS: u128 = 70;

/// A theme just kept, rippling out over the screen from where it was picked (the picker's side when unknown).
#[derive(Clone, Copy, Debug)]
pub struct Ripple {
    pub started: Instant,
    pub from: Option<(u16, u16)>,
}

/// `t`: reads the themes again and opens the picker on the theme in use.
pub fn open(ui: &mut Ui, controller: &mut Controller) {
    let (themes, warnings) = theme::load(ui.user_themes.as_deref());
    for warning in warnings {
        controller.add_log(warning, Hue::Yellow);
    }
    ui.themes = themes;
    let index = matches(&ui.themes, "").iter().position(|&at| ui.themes[at].id == ui.theme.id).unwrap_or(0);
    let own = controller.own_setting("theme").and_then(|id| id.as_str().map(str::to_string));
    ui.overlay = Some(Overlay::Themes { query: String::new(), index, original: ui.theme.id.clone(), own });
}

/// The themes the picker lists (indexes into `themes`): Default first, then the dark themes, then the light ones -
/// all of them, or the fuzzy matches for `query` in that order.
pub fn matches(themes: &[Theme], query: &str) -> Vec<usize> {
    let mut found = found(themes, query);
    found.sort_by_key(|&at| group(&themes[at])); // stable: each group keeps its order
    found
}

/// Where a theme is listed: Default (your terminal's own colours), the dark themes, the light themes.
fn group(theme: &Theme) -> u8 {
    if theme.id == "default" {
        0
    } else if theme.light {
        2
    } else {
        1
    }
}

/// While the picker is open: whether the terminal's background should stay as it is a moment longer. A light theme
/// only passed on the way through (or wrapped to from the top of the list) never flashes the whole terminal white.
pub fn holds_terminal(ui: &Ui, lit: bool) -> bool {
    matches!(ui.overlay, Some(Overlay::Themes { .. }))
        && ui.theme.light
        && !lit
        && ui.now.saturating_duration_since(ui.theme_changed) < LIGHT_SETTLE
}

/// All the themes, or the fuzzy matches for `query` - by name and id, and by description only when no name matches
/// (a few letters match some description far too easily).
fn found(themes: &[Theme], query: &str) -> Vec<usize> {
    if query.trim().is_empty() {
        return (0..themes.len()).collect();
    }
    let by = |text: &dyn Fn(&Theme) -> String| {
        let mut scored: Vec<(u32, usize)> = themes
            .iter()
            .enumerate()
            .filter_map(|(at, theme)| {
                Some(fuzzy_score(query, &text(theme))).filter(|&score| score > 0).map(|s| (s, at))
            })
            .collect();
        scored.sort_by_key(|&(score, _)| std::cmp::Reverse(score)); // stable: ties keep the list's order
        scored.into_iter().map(|(_, at)| at).collect::<Vec<_>>()
    };
    let by_name = by(&|theme| format!("{} {}", theme.name, theme.id));
    if by_name.is_empty() { by(&|theme| theme.description.clone()) } else { by_name }
}

/// A key while the picker is open.
pub fn key(ui: &mut Ui, controller: &mut Controller, name: &str, key: &KeyEvent) {
    let Some(Overlay::Themes { query, index, original, own }) = &mut ui.overlay else { return };
    match name {
        "Escape" => {
            let original = original.clone();
            ui.overlay = None;
            ui.set_theme(&original);
            return;
        }
        "Enter" | "Tab" => {
            let picked = matches(&ui.themes, query).get(*index).map(|&at| ui.themes[at].id.clone());
            let row = Target::Item(*index);
            let from = ui.hits.iter().find(|hit| hit.target == row).map(|hit| (hit.x + hit.width / 2, hit.y));
            let (original, own) = (original.clone(), own.clone());
            ui.overlay = None;
            match picked {
                Some(id) if name == "Tab" && own.as_deref() == Some(id.as_str()) => give_back(ui, controller),
                Some(id) => save(ui, controller, &id, name == "Tab"),
                None => {
                    ui.set_theme(&original);
                    return;
                }
            }
            ripple(ui, from);
            return;
        }
        _ if keys::list_step(name, key).is_some() => {
            let by = keys::list_step(name, key).unwrap_or_default();
            *index = keys::wrapped(*index, by, matches(&ui.themes, query).len());
        }
        "PageUp" => *index = index.saturating_sub(PAGE),
        "PageDown" => *index += PAGE,
        "Home" => *index = 0,
        "End" => *index = usize::MAX / 2,
        "Backspace" => {
            query.pop();
            *index = 0;
        }
        _ => {
            if let Some(c) = keys::typed(key) {
                query.push(c);
                *index = 0;
            }
        }
    }
    let found = matches(&ui.themes, query);
    *index = (*index).min(found.len().saturating_sub(1));
    let preview = found.get(*index).map(|&at| ui.themes[at].id.clone());
    if let Some(id) = preview.filter(|id| *id != ui.theme.id) {
        ui.set_theme(&id); // the live preview
    }
}

/// Enter: the theme is kept for every project (this one's own theme, if it had one, goes); Tab: for this project
/// alone. Saved, and named in the log.
fn save(ui: &mut Ui, controller: &mut Controller, id: &str, own: bool) {
    if own {
        controller.set_own_setting("theme", Some(json!(id)));
    } else {
        controller.set_setting("theme", json!(id));
        if controller.own_setting("theme").is_some() {
            controller.set_own_setting("theme", None);
        }
    }
    ui.set_theme(id);
    let description =
        if ui.theme.description.is_empty() { String::new() } else { format!(" - {}", ui.theme.description) };
    let no_color = if theme::no_color() { " (NO_COLOR is set, so colours stay off)" } else { "" };
    let saved = if own {
        format!("saved for {} in {}, Tab on it again undoes it", controller.project.name, settings_file(controller))
    } else {
        "saved".to_string()
    };
    controller.add_log(format!("Theme: {}{description}{no_color} ({saved})", ui.theme.name), Hue::Cyan);
}

/// Tab on this project's own theme: it goes, and the project uses every project's theme again.
fn give_back(ui: &mut Ui, controller: &mut Controller) {
    controller.set_own_setting("theme", None);
    let shared = controller.config["theme"].as_str().unwrap_or("default").to_string();
    ui.set_theme(&shared);
    let (project, file) = (controller.project.name.clone(), settings_file(controller));
    controller.add_log(
        format!("Theme: {}, as every project has it ({project}'s own theme undone in {file})", ui.theme.name),
        Hue::Cyan,
    );
}

/// A kept theme ripples out over the screen (not with reduced motion).
fn ripple(ui: &mut Ui, from: Option<(u16, u16)>) {
    if ui.motion.enabled && !ui.reduced_motion {
        ui.theme_ripple = Some(Ripple { started: ui.now, from });
    }
}

/// Paints the ripple over the frame just drawn: a ring in the new theme's own colours spreads from where it was
/// picked, round (a cell is about twice as tall as wide), easing out, sparkles twinkling on it; behind it the new
/// theme, glowing a moment, ahead of it the screen dimmed until the wave gets there. It ends by itself.
pub fn paint_ripple(buf: &mut Buffer, area: Rect, ui: &mut Ui) {
    let Some(ripple) = ui.theme_ripple else { return };
    let elapsed = ui.now.saturating_duration_since(ripple.started);
    let progress = elapsed.as_secs_f64() / RIPPLE.as_secs_f64();
    if progress >= 1.0 {
        ui.theme_ripple = None;
        return;
    }
    let side = (area.right().saturating_sub(area.width / 6), area.y + area.height / 2);
    let (ox, oy) = ripple.from.unwrap_or(side);
    let distance = |x: u16, y: u16| (f64::from(x) - f64::from(ox)).hypot(2.0 * (f64::from(y) - f64::from(oy)));
    let corners = [(area.x, area.y), (area.right(), area.y), (area.x, area.bottom()), (area.right(), area.bottom())];
    let farthest = corners.into_iter().map(|(x, y)| distance(x, y)).fold(0.0, f64::max);
    let radius = (1.0 - (1.0 - progress).powi(2)) * (farthest + RING + GLOW);
    let ring = RING_COLORS.map(|color| ui.theme.style(C::Black, Some(color)));
    let band =
        |depth: f64, width: f64| ring[((depth / width * RING_COLORS.len() as f64) as usize).min(RING_COLORS.len() - 1)];
    let (sparkle, glow, dim) = (
        ui.theme.style(C::White, None).add_modifier(Modifier::BOLD),
        Style::new().add_modifier(Modifier::BOLD),
        Style::new().add_modifier(Modifier::DIM),
    );
    let twinkle = elapsed.as_millis() / TWINKLE_MS;
    for y in area.y..area.bottom() {
        for x in area.x..area.right() {
            let behind = radius - distance(x, y);
            let Some(cell) = buf.cell_mut((x, y)) else { continue };
            if behind < 0.0 {
                cell.set_style(dim);
            } else if behind < RING {
                cell.set_style(band(behind, RING));
                if let Some(star) = sparkle_at(x, y, twinkle).filter(|_| cell.symbol() == " ") {
                    cell.set_symbol(star).set_style(sparkle);
                }
            } else if behind < RING + GLOW {
                cell.set_style(glow);
            }
        }
    }
}

/// Whether a cell on the ring sparkles at this twinkle, and with what: about one in nine, moving as it twinkles.
fn sparkle_at(x: u16, y: u16, twinkle: u128) -> Option<&'static str> {
    let seed = u64::from(x).wrapping_mul(73_856_093)
        ^ u64::from(y).wrapping_mul(19_349_663)
        ^ (twinkle as u64).wrapping_mul(83_492_791);
    let roll = seed % 97;
    (roll < 11).then(|| SPARKLES[roll as usize % SPARKLES.len()])
}

/// The settings file's name, where a project's own theme is kept (not the project's .lazyrunner.json).
fn settings_file(controller: &Controller) -> String {
    let path = &controller.paths().config_file;
    path.file_name().map_or_else(|| path.display().to_string(), |name| name.to_string_lossy().into_owned())
}

/// The panel: the themes found, each with a swatch of its own colours; the selected one's description below.
pub fn rows(ui: &Ui, saved: &str, cells: usize, rows: usize) -> Vec<Vec<Seg>> {
    let Some(Overlay::Themes { query, index, own, .. }) = &ui.overlay else { return Vec::new() };
    let found = matches(&ui.themes, query);
    let index = (*index).min(found.len().saturating_sub(1));
    let mut lines = vec![
        vec![
            on(format!(" {} ", icons::titled(ui.nerd, Icon::Brush, "Themes")), C::FocusText, C::FocusBack),
            seg(format!("  {} of {}", found.len(), ui.themes.len()), C::DarkGray),
        ],
        vec![seg(" › ", C::Cyan), seg(query.clone(), C::White), seg(ui.cursor(), C::Cyan)],
        vec![seg("─".repeat(cells), C::DarkGray)],
    ];
    let mut footer = vec![vec![seg("─".repeat(cells), C::DarkGray)]];
    if let Some(selected) = found.get(index).map(|&at| &ui.themes[at]) {
        for line in wrap(&selected.description, cells.saturating_sub(2)).into_iter().take(DESCRIPTION_LINES) {
            footer.push(vec![seg(format!(" {line}"), C::DarkGray)]);
        }
    }
    let pin = icons::icon(ui.nerd, Icon::Pin);
    if own.is_some() {
        footer.push(vec![seg(format!(" {pin} project's own · Tab undoes"), C::DarkGray)]);
    }
    footer.push(vec![seg(" ↑↓ Alt+jk preview · Esc undo", C::DarkGray)]);
    footer.push(vec![seg(" Enter keep · Tab this project", C::DarkGray)]);
    enum Entry {
        Heading(&'static str),
        Theme(usize, usize),
    }
    let mut entries = Vec::new();
    let mut last_group = None;
    for (at, &theme_at) in found.iter().enumerate() {
        let theme = &ui.themes[theme_at];
        if group(theme) != 0 && last_group != Some(group(theme)) {
            entries.push(Entry::Heading(if theme.light { "Light" } else { "Dark" }));
        }
        last_group = Some(group(theme));
        entries.push(Entry::Theme(at, theme_at));
    }
    let selected = entries.iter().position(|entry| matches!(entry, Entry::Theme(at, _) if *at == index)).unwrap_or(0);
    let visible = rows.saturating_sub(lines.len() + footer.len()).max(1);
    let first = selected.saturating_sub(visible / 2).min(entries.len().saturating_sub(visible));
    let name_cells = cells.saturating_sub(15).max(8);
    for entry in entries.iter().skip(first).take(visible) {
        let (at, theme_at) = match entry {
            Entry::Heading(label) => {
                let icon = if *label == "Light" { Icon::Sun } else { Icon::Moon };
                lines.push(heading(&icons::titled(ui.nerd, icon, label), cells));
                continue;
            }
            Entry::Theme(at, theme_at) => (*at, *theme_at),
        };
        let theme = &ui.themes[theme_at];
        let mark = match own {
            Some(own) if *own == theme.id => pin,
            _ if theme.id == saved => "•",
            _ => " ",
        };
        let twin = ui.themes.iter().filter(|other| other.name == theme.name).count() > 1;
        let name = if twin { format!("{} · {}", theme.name, theme.id) } else { theme.name.clone() };
        let name = fit_name(&name, name_cells);
        let mut row = if at == index {
            vec![on(format!(" ▸{mark} {name} "), C::FocusText, C::FocusBack)]
        } else {
            vec![seg(format!("  {mark} {name} "), C::Gray)]
        };
        if !theme::no_color() {
            row.extend(SWATCH.iter().map(|&color| styled("■", theme.style(color, None))));
        }
        lines.push(aim(row, Target::Item(at)));
    }
    if found.is_empty() {
        lines.push(vec![seg("  no matching theme", C::DarkGray)]);
    }
    while lines.len() + footer.len() < rows {
        lines.push(Vec::new());
    }
    lines.extend(footer);
    lines.truncate(rows);
    lines.into_iter().map(|line| aim(line, Target::Box)).collect()
}

/// A group's heading in the list: its name on a rule.
fn heading(label: &str, cells: usize) -> Vec<Seg> {
    let rule = "─".repeat(cells.saturating_sub(label.chars().count() + 3));
    vec![seg(format!(" {label} "), C::DarkCyan), seg(rule, C::DarkGray)]
}

/// The name in exactly `cells` characters: cut with … or padded.
fn fit_name(name: &str, cells: usize) -> String {
    if name.chars().count() > cells {
        format!("{}…", name.chars().take(cells.saturating_sub(1)).collect::<String>())
    } else {
        format!("{name:<cells$}")
    }
}

/// `text` in lines of at most `cells` characters, broken between words.
fn wrap(text: &str, cells: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for word in text.split_whitespace() {
        match lines.last_mut() {
            Some(line) if line.chars().count() + 1 + word.chars().count() <= cells => {
                line.push(' ');
                line.push_str(word);
            }
            _ => lines.push(word.to_string()),
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_cut_or_padded_and_descriptions_wrap_between_words() {
        assert_eq!(fit_name("Catppuccin Mocha", 8), "Catppuc…");
        assert_eq!(fit_name("Nord", 6), "Nord  ");
        assert_eq!(wrap("a calm dark theme with soft colours", 12), ["a calm dark", "theme with", "soft colours"]);
    }

    #[test]
    fn typing_filters_by_name_and_falls_back_to_descriptions() {
        let (themes, _) = theme::load(None);
        assert_eq!(matches(&themes, "").len(), themes.len());
        let nord = matches(&themes, "nord");
        assert_eq!(themes[nord[0]].id, "nord");
        assert!(matches(&themes, "zzzz").is_empty());
    }
}
