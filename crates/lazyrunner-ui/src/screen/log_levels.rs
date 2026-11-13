//! `Alt+l` on a .NET app: its log levels (#101) - the default, EF Core's SQL, HttpClient's calls, ASP.NET Core and
//! its own namespace, each at Trace, Debug, Information or Warning, or its own settings' level. `j` `k` pick a
//! category, `h` `l` a level, `Enter` sets it (restarting the app when it runs), `x` gives the category back to the
//! app's settings, `r` all of them; `s` keeps them beyond this session. Its title says `Debug` while it differs. Each
//! category says how many lines it logged the last minute, and the loudest ones show under them.

use std::collections::HashMap;
use std::time::Duration;

use lazyrunner_core::output::Hue;
use lazyrunner_core::profile::AppKind;
use lazyrunner_core::run::Controller;
use lazyrunner_core::run::controller::{DEFAULT, LEVELS};

use super::icons::{self, Icon};
use super::keys;
use super::motion::Spot;
use super::overlays::boxed;
use super::seg::{Seg, on, pad, seg};
use super::theme::C;
use super::{Overlay, Ui};

const INNER: usize = 116;
const CATEGORY_CELLS: usize = 50;
/// What a category is at without an override: what the app's own settings say.
const OWN: &str = "its own";
/// The console logger's level prefixes, the window a rate counts over, from how many lines a minute a category is
/// loud, how many loud ones show and their bars' cells.
const HEADERS: [&str; 6] = ["trce", "dbug", "info", "warn", "fail", "crit"];
const RATE_WINDOW: Duration = Duration::from_secs(60);
const LOUD: usize = 30;
const MOST_LOUD: usize = 3;
const LOUD_BAR: usize = 12;
/// The slider's track and knob, and the flash its knob snaps with.
const TRACK_FILLED: &str = "━";
const TRACK_EMPTY: &str = "─";
const KNOB: &str = "●";
const SNAP: &str = "log-level-set";
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
        "Enter" | "Space" => {
            controller.set_log_level(&key, &category, Some(LEVELS[*level]));
            let app = *app;
            ui.motion.flash(Spot::Hint(SNAP.into()), ui.now);
            ui.motion.flash(Spot::Title(app), ui.now);
        }
        "x" | "Backspace" => controller.set_log_level(&key, &category, None),
        "r" => controller.reset_log_levels(&key),
        "s" => controller.keep_log_levels(&key),
        _ => {}
    }
}

/// Lines each logged category wrote in the app's output over the last `RATE_WINDOW`, from its .NET console headers
/// (`info: Microsoft.AspNetCore.Hosting.Diagnostics[1]`).
fn rates(ui: &Ui, controller: &Controller, app: usize) -> HashMap<String, usize> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    let Some(state) = controller.states.get(app) else { return counts };
    for line in state.output.newest_first(0) {
        if ui.now.saturating_duration_since(line.born) > RATE_WINDOW {
            break;
        }
        let Some((level, rest)) = line.text.split_once(": ") else { continue };
        if !HEADERS.contains(&level) {
            continue;
        }
        let Some((category, _)) = rest.split_once('[') else { continue };
        *counts.entry(category.trim().to_string()).or_default() += 1;
    }
    counts
}

/// A configured category's lines a minute: every category's for the default, else those under its namespace.
fn rate_of(rates: &HashMap<String, usize>, category: &str) -> usize {
    let under = |each: &String| category == DEFAULT || each == category || each.starts_with(&format!("{category}."));
    rates.iter().filter(|(each, _)| under(each)).map(|(_, count)| count).sum()
}

/// A category's levels as a slider: the track filled up to its knob - the level picked on the picked row (green as it
/// snaps there on Enter), else the level set - and none for its own settings' level.
fn slider(knob: Option<usize>, picked: bool, snapped: bool) -> Vec<Seg> {
    let fill = if picked && !snapped { C::Cyan } else { C::Green };
    let mut segs = Vec::new();
    for (place, name) in LEVELS.iter().enumerate() {
        if place > 0 {
            let filled = knob.is_some_and(|knob| place <= knob);
            segs.push(if filled { seg(TRACK_FILLED, fill) } else { seg(TRACK_EMPTY, C::DarkGray) });
        }
        let at_knob = knob == Some(place);
        let text = format!("{}{name}", if at_knob { KNOB } else { " " });
        segs.push(match (at_knob, picked) {
            (true, true) => on(text, C::Black, fill),
            (true, false) => seg(text, C::Green),
            _ if knob.is_some_and(|knob| place < knob) => seg(text, C::Gray),
            _ => seg(text, C::DarkGray),
        });
    }
    segs
}

pub fn rows(ui: &Ui, controller: &Controller, cells: usize, rows: usize) -> Vec<Vec<Seg>> {
    let Some(Overlay::LogLevels { app, index, level }) = &ui.overlay else { return Vec::new() };
    let Some(found) = controller.apps.get(*app) else { return Vec::new() };
    let inner = cells.saturating_sub(8).clamp(20, INNER);
    let set = controller.log_levels(&found.key);
    let rates = rates(ui, controller, *app);
    let snapped = ui.motion.lit(&Spot::Hint(SNAP.into()), ui.now).is_some();
    let categories = controller.log_categories(&found.key);
    let mut lines = Vec::new();
    for (at, category) in categories.iter().enumerate() {
        let current = set.iter().find(|each| each.category == *category);
        let picked = at == *index;
        let chosen = current.and_then(|each| LEVELS.iter().position(|name| *name == each.level));
        let knob = if picked { Some(*level) } else { chosen };
        let mut line = vec![seg(
            pad(&format!(" {} {category}", if picked { "▸" } else { " " }), CATEGORY_CELLS),
            if picked { C::White } else { C::Gray },
        )];
        line.extend(slider(knob, picked, snapped));
        let rate = rate_of(&rates, category);
        let rate_text = if rate > 0 { format!("{rate:>4}/min") } else { format!("{:>8}", "·") };
        line.push(seg(rate_text, if rate >= LOUD { C::Yellow } else { C::DarkGray }));
        let note = match current {
            Some(each) if each.kept => " kept",
            Some(_) => " session",
            None => OWN,
        };
        line.push(seg(format!("  {note}"), if current.is_some() { C::DarkCyan } else { C::DarkGray }));
        lines.push(line);
    }
    let mut loud: Vec<(&String, &usize)> = rates.iter().filter(|(_, count)| **count >= LOUD).collect();
    loud.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
    if !loud.is_empty() {
        lines.push(Vec::new());
        lines.push(vec![seg(" LOUD CATEGORIES, THE LAST MINUTE", C::DarkCyan)]);
        let most = *loud[0].1;
        for (category, count) in loud.into_iter().take(MOST_LOUD) {
            let bar = (count * LOUD_BAR).div_ceil(most.max(1));
            lines.push(vec![
                seg(pad(&format!("   {category}"), CATEGORY_CELLS), C::Gray),
                seg(pad(&"▬".repeat(bar), LOUD_BAR + 1), C::Yellow),
                seg(format!("{count:>4}/min"), C::Yellow),
            ]);
        }
    }
    lines.push(Vec::new());
    lines.push(vec![seg(
        " After lazyrunner's own --Logging arguments, so they win; this session only, unless kept (s).",
        C::DarkGray,
    )]);
    let title = icons::titled(ui.nerd, Icon::Filter, &format!("{} log levels", found.name));
    boxed(&title, lines, FOOTER, cells, rows, inner)
}
