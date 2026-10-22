//! The screen of a running project, as the PowerShell version draws it: the columns (or an overlay over them),
//! the panel below, the key reference on the whole screen. `Ui` is what the screen shows besides the controller's
//! state; `draw` puts both on a frame.

pub mod actions;
pub mod checklist;
pub mod columns;
pub mod copy;
pub mod durations;
pub mod keys;
pub mod mouse;
pub mod overlays;
pub mod panel;
pub mod seg;
pub mod status;
#[cfg(test)]
mod tests;
pub mod theme;
pub mod theme_picker;

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use lazyrunner_core::changelog::{self, Line};
use lazyrunner_core::config::Toggle;
use lazyrunner_core::run::Controller;
use lazyrunner_core::run::secrets::Entry;
use ratatui::Frame;
use ratatui::style::Style;

use seg::{Seg, seg as s};
use theme::{C, Theme};

const MIN_SIZE: (u16, u16) = (60, 15);
/// The theme picker's panel: a third of the width, within these.
const THEME_PANEL: (usize, usize) = (30, 40);
/// The PowerShell version redrew every 100 ms; its frame-counted animations (spinners, glints, blinking cursors)
/// keep that pace here at any frame rate.
const ANIMATION_TICK: Duration = Duration::from_millis(100);

/// / in a column: the query, and the match it is on.
#[derive(Clone, Debug, Default)]
pub struct Search {
    pub typing: bool,
    pub query: String,
    pub key: String,
    /// The matching line (0 = the oldest line kept).
    pub index: Option<usize>,
    pub note: String,
    pub position: usize,
    pub total: usize,
}

pub enum Overlay {
    Palette {
        query: String,
        index: usize,
    },
    Help,
    Menu {
        index: usize,
    },
    Settings {
        index: usize,
    },
    Checklist(Box<checklist::Checklist>),
    Changelog {
        lines: Vec<Line>,
        top: usize,
    },
    /// `D`: how long builds, starts and npm ci took over their last runs.
    Durations {
        top: usize,
    },
    /// `t`: the theme picker; `original` is the theme Esc puts back.
    Themes {
        query: String,
        index: usize,
        original: String,
    },
}

impl Overlay {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Palette { .. } => "palette",
            Self::Help => "help",
            Self::Menu { .. } => "menu",
            Self::Settings { .. } => "settings",
            Self::Checklist(_) => "checklist",
            Self::Changelog { .. } => "changelog",
            Self::Durations { .. } => "durations",
            Self::Themes { .. } => "themes",
        }
    }
}

/// d: the database picker.
pub struct Picker {
    pub path: PathBuf,
    pub groups: Vec<(String, Vec<Entry>)>,
    pub group: usize,
    pub index: usize,
    pub top: usize,
    pub filter: String,
    pub filtering: bool,
    /// n: the name of a new database being typed.
    pub new_name: Option<String>,
}

impl Picker {
    /// The current key's entries, narrowed by the / filter.
    pub fn entries(&self) -> Vec<Entry> {
        let entries = self.groups.get(self.group).map(|(_, entries)| entries.clone()).unwrap_or_default();
        if self.filter.is_empty() {
            return entries;
        }
        entries.into_iter().filter(|entry| columns::contains_ignore_case(&entry.label, &self.filter)).collect()
    }

    pub fn active_index(&self) -> usize {
        self.groups.get(self.group).and_then(|(_, entries)| entries.iter().position(|entry| entry.active)).unwrap_or(0)
    }
}

/// How the screen ends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Exit {
    Quit,
    /// Open lazyrunner again on `root` (another project, or this one with its profile reloaded).
    Reopen {
        root: PathBuf,
        start: bool,
    },
}

pub struct Ui {
    pub focus: usize,
    pub zoom: bool,
    pub search: Option<Search>,
    pub overlay: Option<Overlay>,
    pub picker: Option<Picker>,
    /// `L` was pressed: the loop opens the git tool.
    pub git_tool_wanted: bool,
    /// The git tool has the terminal (its name): nothing is drawn, and quitting or switching versions waits.
    pub away: Option<String>,
    pub pending_g: Option<Instant>,
    /// Where each clickable thing was in the last frame, and the last click (a second one soon after is a double).
    pub hits: Vec<mouse::Hit>,
    pub last_click: Option<(Instant, u16, u16)>,
    pub started: Instant,
    /// The time of the frame being drawn (fixed in tests).
    pub now: Instant,
    pub body_rows: usize,
    /// The focused column's width (search centres a match by its wrapped rows).
    pub focus_cells: usize,
    /// "Checking for source changes…": shown for a frame before an action that fingerprints the source runs.
    pub busy: Option<&'static str>,
    /// The action waiting for that frame.
    pub deferred: Option<actions::Run>,
    pub height: usize,
    pub themes: Vec<Theme>,
    pub theme: Theme,
    pub user_themes: Option<PathBuf>,
    pub exit: Option<Exit>,
    /// An installed lazyrunner version to switch to once nothing is in the way (the apps are handed over to it).
    pub switch_to: Option<String>,
    /// What's new since the last version this machine ran (shown once, in the checklist).
    pub whats_new: Vec<changelog::Entry>,
    /// Opens a link or a file (the default app); tests swap it for one that only records.
    pub opener: fn(&str) -> bool,
    /// Copies to the clipboard (`y`); tests swap it for one that only records.
    pub clipboard: copy::Clipboard,
    /// The reducedMotion setting (`, f`), read at every frame: nothing blinks, spins, glides or fades.
    pub reduced_motion: bool,
    bars: HashMap<String, (f64, Instant)>,
}

impl Ui {
    pub fn new(controller: &Controller, user_themes: Option<PathBuf>, now: Instant) -> (Self, Vec<String>) {
        let (themes, warnings) = theme::load(user_themes.as_deref());
        let mut ui = Self {
            focus: 0,
            zoom: false,
            search: None,
            overlay: None,
            picker: None,
            git_tool_wanted: false,
            away: None,
            pending_g: None,
            hits: Vec::new(),
            last_click: None,
            started: now,
            now,
            body_rows: 20,
            focus_cells: 40,
            busy: None,
            deferred: None,
            height: 40,
            theme: themes[0].clone(),
            themes,
            user_themes,
            exit: None,
            switch_to: None,
            whats_new: Vec::new(),
            opener: status::open,
            clipboard: copy::to_clipboard,
            reduced_motion: controller.setting(Toggle::ReducedMotion),
            bars: HashMap::new(),
        };
        let wanted = controller.config["theme"].as_str().unwrap_or("default").to_string();
        ui.set_theme(&wanted);
        (ui, warnings)
    }

    /// The PowerShell version's frame count: one every 100 ms.
    pub fn tick(&self) -> u64 {
        (self.now.saturating_duration_since(self.started).as_millis() / ANIMATION_TICK.as_millis()) as u64
    }

    /// Uses theme `id` (the first theme when there is none by that id; none at all with NO_COLOR).
    pub fn set_theme(&mut self, id: &str) {
        let found = self.themes.iter().find(|theme| theme.id == id).unwrap_or(&self.themes[0]).clone();
        self.theme = if theme::no_color() {
            let mut plain = Theme::no_color();
            plain.id = found.id; // the theme still counts as chosen, it just has no colours
            plain
        } else {
            found
        };
    }

    /// Glides a progress bar towards `target` (35% of the way per 100 ms, never backwards); `id` keeps its place.
    pub fn glide(&mut self, id: &str, target: f64) -> f64 {
        let now = self.now;
        let target = if target.is_finite() { target } else { 0.0 };
        let (shown, at) = self.bars.get(id).copied().unwrap_or((target, now));
        let mut shown = if target < shown { target } else { shown };
        let steps = now.saturating_duration_since(at).as_secs_f64() / ANIMATION_TICK.as_secs_f64();
        shown += (target - shown) * (1.0 - 0.65f64.powf(steps.max(0.0)));
        if (target - shown).abs() < 0.004 {
            shown = target;
        }
        self.bars.insert(id.to_string(), (shown, now));
        shown
    }

    /// The text cursor of a prompt, blinking.
    pub fn cursor(&self) -> &'static str {
        if self.reduced_motion || self.tick() % 10 < 5 { "▏" } else { " " }
    }

    /// Whether the next frame differs even if nothing happens.
    pub fn is_animating(&self, controller: &Controller) -> bool {
        if self.reduced_motion {
            // Nothing moves: keys, output and the tick redraw what changed (the busy frame still has to show).
            return self.busy.is_some();
        }
        let typing = self.search.as_ref().is_some_and(|search| search.typing)
            || matches!(self.overlay, Some(Overlay::Palette { .. } | Overlay::Checklist(_) | Overlay::Themes { .. }))
            || self.picker.as_ref().is_some_and(|picker| picker.filtering || picker.new_name.is_some());
        typing
            || self.busy.is_some()
            || controller.building.is_some()
            || controller.npm_ci.is_some()
            || controller.celebrate.is_some()
            || controller.states.iter().any(|state| {
                let fresh = state
                    .output
                    .last_line_at()
                    .is_some_and(|at| self.now.saturating_duration_since(at) < Duration::from_millis(1200));
                (state.is_running() && !state.is_up())
                    || state.flash_until.is_some_and(|until| self.now < until)
                    || (fresh && state.scroll == 0)
            })
    }
}

/// Draws the whole screen.
pub fn draw(frame: &mut Frame, ui: &mut Ui, controller: &Controller) {
    ui.reduced_motion = controller.setting(Toggle::ReducedMotion);
    let area = frame.area();
    let buf = frame.buffer_mut();
    buf.set_style(area, Style::reset());
    let (width, height) = (usize::from(area.width), usize::from(area.height));
    ui.height = height;
    ui.hits.clear();
    if area.width < MIN_SIZE.0 || area.height < MIN_SIZE.1 {
        let hint = format!(" lazyrunner: make the terminal a bit bigger ({}x{} at least)", MIN_SIZE.0, MIN_SIZE.1);
        seg::draw(buf, area.x, area.y, area.width, &[s(hint, C::Yellow)], &ui.theme, &mut ui.hits);
        return;
    }
    let rows: Vec<Vec<Seg>> = if matches!(ui.overlay, Some(Overlay::Help)) {
        overlays::help(ui, controller, width, height) // the whole screen
    } else {
        let panel = panel::rows(ui, controller, width);
        let body_height = height.saturating_sub(panel.len());
        let mut rows = body(ui, controller, width, body_height);
        rows.extend(panel);
        rows
    };
    for (y, row) in rows.iter().enumerate().take(height) {
        seg::draw(buf, area.x, area.y + y as u16, area.width, row, &ui.theme, &mut ui.hits);
    }
}

fn body(ui: &mut Ui, controller: &Controller, width: usize, rows: usize) -> Vec<Vec<Seg>> {
    match &ui.overlay {
        Some(Overlay::Checklist(_)) => return overlays::checklist(ui, width, rows),
        _ if ui.picker.is_some() => return overlays::picker(ui, width, rows),
        Some(Overlay::Palette { .. }) => return overlays::palette(ui, controller, width, rows),
        Some(Overlay::Changelog { .. }) => return overlays::changelog(ui, width, rows),
        Some(Overlay::Durations { .. }) => return durations::rows(ui, controller, width, rows),
        Some(Overlay::Menu { .. } | Overlay::Settings { .. }) => return overlays::menu(ui, controller, width, rows),
        Some(Overlay::Themes { .. }) => return with_theme_picker(ui, controller, width, rows),
        _ => {}
    }
    columns::rows(ui, controller, width, rows)
}

/// The theme picker in a narrow panel on the right; the columns keep the rest, so they show the theme.
fn with_theme_picker(ui: &mut Ui, controller: &Controller, width: usize, rows: usize) -> Vec<Vec<Seg>> {
    let panel = (width / 3).clamp(THEME_PANEL.0, THEME_PANEL.1);
    let main = width.saturating_sub(panel + 1);
    let mut columns = columns::rows(ui, controller, main, rows);
    let saved = controller.config["theme"].as_str().unwrap_or("default").to_string();
    let mut picker = theme_picker::rows(ui, &saved, panel, rows);
    columns.resize(rows, Vec::new());
    picker.resize(rows, Vec::new());
    columns
        .into_iter()
        .zip(picker)
        .map(|(left, right)| {
            let mut row = columns::fit(left, main);
            // The whole panel is the picker's: a click on its blank space or border keeps it open.
            row.push(s("┃", C::FocusBorder).at(mouse::Target::Box));
            row.extend(seg::aim(columns::fit(right, panel), mouse::Target::Box));
            row
        })
        .collect()
}

/// The frame as plain text (tests, the frame dump).
pub fn text(buffer: &ratatui::buffer::Buffer) -> String {
    crate::dump::text(buffer)
}
