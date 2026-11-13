//! `Alt+l` on a .NET app: its log levels (#101) - the default, EF Core's SQL, HttpClient's calls, ASP.NET Core and
//! its own namespace, each at Trace, Debug, Information or Warning, or its own settings' level. `j` `k` pick a
//! category, `h` `l` a level, `Enter` sets it (restarting the app when it runs), `x` gives the category back to the
//! app's settings, `r` all of them; `s` keeps them beyond this session. Its title says `Debug` while it differs.

use lazyrunner_core::output::Hue;
use lazyrunner_core::profile::AppKind;
use lazyrunner_core::run::Controller;
use lazyrunner_core::run::controller::{DEFAULT, LEVELS};

use super::icons::{self, Icon};
use super::keys;
use super::overlays::boxed;
use super::seg::{Seg, on, pad, seg};
use super::theme::C;
use super::{Overlay, Ui};

const INNER: usize = 100;
const CATEGORY_CELLS: usize = 51;
/// What a category is at without an override: what the app's own settings say.
const OWN: &str = "its own";
const FOOTER: &str = "j k category · h l level · Enter sets it (restarts it) · x its own · r all · s keeps · Esc";

/// Alt+l: the focused app's log levels, on the default.
pub fn open(ui: &mut Ui, controller: &mut Controller) {
    let Some(app) = controller.apps.get(ui.focus) else { return };
    if !matches!(app.kind, AppKind::Dotnet { .. }) {
        controller.add_log(format!("Log levels are for .NET apps; {} isn't one.", app.name), Hue::DarkGray);
        return;
    }
    let level = level_at(controller, &app.key, DEFAULT);
    ui.overlay = Some(Overlay::LogLevels { app: ui.focus, index: 0, level });
}

/// Where category `category`'s level is among the levels; its own counts as Information.
fn level_at(controller: &Controller, key: &str, category: &str) -> usize {
    let set =
        controller.log_levels(key).iter().find(|level| level.category == category).map(|level| level.level.clone());
    let level = set.unwrap_or_else(|| "Information".to_string());
    LEVELS.iter().position(|each| *each == level).unwrap_or(2)
}

pub fn key(ui: &mut Ui, controller: &mut Controller, name: &str) {
    let Some(Overlay::LogLevels { app, index, level }) = &mut ui.overlay else { return };
    let Some(key) = controller.apps.get(*app).map(|app| app.key.clone()) else { return };
    let categories = controller.log_categories(&key);
    let category = categories.get(*index).cloned().unwrap_or_default();
    match name {
        "Escape" | "q" | "Alt+l" => ui.overlay = None,
        "j" | "DownArrow" | "k" | "UpArrow" => {
            let by = if matches!(name, "j" | "DownArrow") { 1 } else { -1 };
            *index = keys::wrapped(*index, by, categories.len().max(1));
            *level = level_at(controller, &key, &categories[*index]);
        }
        "h" | "LeftArrow" => *level = level.saturating_sub(1),
        "l" | "RightArrow" => *level = (*level + 1).min(LEVELS.len() - 1),
        "Enter" | "Space" => controller.set_log_level(&key, &category, Some(LEVELS[*level])),
        "x" | "Backspace" => controller.set_log_level(&key, &category, None),
        "r" => controller.reset_log_levels(&key),
        "s" => controller.keep_log_levels(&key),
        _ => {}
    }
}

pub fn rows(ui: &Ui, controller: &Controller, cells: usize, rows: usize) -> Vec<Vec<Seg>> {
    let Some(Overlay::LogLevels { app, index, level }) = &ui.overlay else { return Vec::new() };
    let Some(found) = controller.apps.get(*app) else { return Vec::new() };
    let inner = cells.saturating_sub(8).clamp(20, INNER);
    let set = controller.log_levels(&found.key);
    let mut lines = Vec::new();
    for (at, category) in controller.log_categories(&found.key).iter().enumerate() {
        let current = set.iter().find(|each| each.category == *category);
        let picked = at == *index;
        let mut line = vec![seg(
            pad(&format!(" {} {category}", if picked { "▸" } else { " " }), CATEGORY_CELLS),
            if picked { C::White } else { C::Gray },
        )];
        for (place, name) in LEVELS.iter().enumerate() {
            let chosen = current.is_some_and(|each| each.level == *name);
            let text = format!(" {}{name} ", if chosen { "●" } else { " " });
            line.push(match (picked && place == *level, chosen) {
                (true, _) => on(text, C::Black, C::Cyan),
                (false, true) => seg(text, C::Green),
                _ => seg(text, C::DarkGray),
            });
        }
        let note = match current {
            Some(each) if each.kept => " kept",
            Some(_) => " this session",
            None => OWN,
        };
        line.push(seg(format!(" {note}"), if current.is_some() { C::DarkCyan } else { C::DarkGray }));
        lines.push(line);
    }
    lines.push(Vec::new());
    lines.push(vec![seg(
        " After lazyrunner's own --Logging arguments, so they win; this session only, unless kept (s).",
        C::DarkGray,
    )]);
    let title = icons::titled(ui.nerd, Icon::Filter, &format!("{} log levels", found.name));
    boxed(&title, lines, FOOTER, cells, rows, inner)
}
