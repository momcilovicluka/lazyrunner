//! `t`: the theme picker. A narrow panel on the right, so the rest of the screen shows the theme on the real columns.
//! Moving through the list applies each theme at once (a preview, terminal background included; nothing is saved);
//! typing filters by name (by description only when no name matches); Enter keeps and saves the selected theme for
//! every project, Tab for this project alone; Esc puts back the one the picker opened with. Themes are read again
//! every time it opens.

use crossterm::event::KeyEvent;
use lazyrunner_core::output::Hue;
use lazyrunner_core::run::Controller;
use serde_json::json;

use super::actions::fuzzy_score;
use super::mouse::Target;
use super::seg::{Seg, aim, on, seg, styled};
use super::theme::{self, C, Theme};
use super::{Overlay, Ui, keys};

const PAGE: usize = 10;
/// The colours each row's swatch shows, in the theme's own colours.
const SWATCH: [C; 6] = [C::Red, C::Yellow, C::Green, C::Cyan, C::Blue, C::Magenta];
/// Lines of the selected theme's description under the list.
const DESCRIPTION_LINES: usize = 3;

/// `t`: reads the themes again and opens the picker on the theme in use.
pub fn open(ui: &mut Ui, controller: &mut Controller) {
    let (themes, warnings) = theme::load(ui.user_themes.as_deref());
    for warning in warnings {
        controller.add_log(warning, Hue::Yellow);
    }
    ui.themes = themes;
    let index = ui.themes.iter().position(|theme| theme.id == ui.theme.id).unwrap_or(0);
    let own = controller.own_setting("theme").and_then(|id| id.as_str().map(str::to_string));
    ui.overlay = Some(Overlay::Themes { query: String::new(), index, original: ui.theme.id.clone(), own });
}

/// The themes the picker lists (indexes into `themes`): all of them, or the fuzzy matches for `query` - by name and
/// id, and by description only when no name matches (a few letters match some description far too easily).
pub fn matches(themes: &[Theme], query: &str) -> Vec<usize> {
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
    let Some(Overlay::Themes { query, index, original, .. }) = &mut ui.overlay else { return };
    match name {
        "Escape" => {
            let original = original.clone();
            ui.overlay = None;
            ui.set_theme(&original);
            return;
        }
        "Enter" | "Tab" => {
            let picked = matches(&ui.themes, query).get(*index).map(|&at| ui.themes[at].id.clone());
            let original = original.clone();
            ui.overlay = None;
            match picked {
                Some(id) => save(ui, controller, &id, name == "Tab"),
                None => ui.set_theme(&original),
            }
            return;
        }
        "UpArrow" | "Ctrl+P" => *index = index.saturating_sub(1),
        "DownArrow" | "Ctrl+N" => *index += 1,
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
    let saved = if own { format!("saved for {}", controller.project.name) } else { "saved".to_string() };
    controller.add_log(format!("Theme: {}{description}{no_color} ({saved})", ui.theme.name), Hue::Cyan);
}

/// The panel: the themes found, each with a swatch of its own colours; the selected one's description below.
pub fn rows(ui: &Ui, saved: &str, cells: usize, rows: usize) -> Vec<Vec<Seg>> {
    let Some(Overlay::Themes { query, index, own, .. }) = &ui.overlay else { return Vec::new() };
    let found = matches(&ui.themes, query);
    let index = (*index).min(found.len().saturating_sub(1));
    let mut lines = vec![
        vec![
            on(" Themes ", C::FocusText, C::FocusBack),
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
    if own.is_some() {
        footer.push(vec![seg(" ◆ this project's own theme", C::DarkGray)]);
    }
    footer.push(vec![seg(" ↑↓ preview · Esc undo", C::DarkGray)]);
    footer.push(vec![seg(" Enter keep · Tab this project", C::DarkGray)]);
    let visible = rows.saturating_sub(lines.len() + footer.len()).max(1);
    let first = index.saturating_sub(visible / 2).min(found.len().saturating_sub(visible));
    let name_cells = cells.saturating_sub(15).max(8);
    for (at, &theme_at) in found.iter().enumerate().skip(first).take(visible) {
        let theme = &ui.themes[theme_at];
        let mark = match own {
            Some(own) if *own == theme.id => '◆',
            _ if theme.id == saved => '•',
            _ => ' ',
        };
        let name = fit_name(&theme.name, name_cells);
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
