//! The screens and the bar over each: every full screen - the stack, the requests, the migrations, the audit, the
//! tests, the ports, the durations, the timeline - starts with a row naming them all with their keys, the open one
//! lit. A screen's key, or a click on its name, goes there from any other, on the app the open one is looking at: the
//! Map's picked box, the requests' app, the migrations' context, the audited app, the picked suite or port.

use lazyrunner_core::run::Controller;

use super::actions::Run;
use super::icons::Icon;
use super::mouse::Target;
use super::seg::{Seg, seg, width};
use super::suites::Row;
use super::theme::C;
use super::{Overlay, Ui};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    Stack,
    Requests,
    Migrations,
    Audit,
    Tests,
    Ports,
    Durations,
    Timeline,
    Database,
}

impl Screen {
    pub const ALL: [Self; 9] = [
        Self::Stack,
        Self::Requests,
        Self::Migrations,
        Self::Audit,
        Self::Tests,
        Self::Ports,
        Self::Durations,
        Self::Timeline,
        Self::Database,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Self::Stack => "K",
            Self::Requests => "H",
            Self::Migrations => "M",
            Self::Audit => "A",
            Self::Tests => "T",
            Self::Ports => "P",
            Self::Durations => "D",
            Self::Timeline => "~",
            Self::Database => "d",
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Stack => "Stack",
            Self::Requests => "Requests",
            Self::Migrations => "Migrations",
            Self::Audit => "Audit",
            Self::Tests => "Tests",
            Self::Ports => "Ports",
            Self::Durations => "Durations",
            Self::Timeline => "Timeline",
            Self::Database => "Database",
        }
    }

    fn icon(self) -> Icon {
        match self {
            Self::Stack => Icon::Stack,
            Self::Requests => Icon::Requests,
            Self::Migrations => Icon::Migrations,
            Self::Audit => Icon::Shield,
            Self::Tests => Icon::Tests,
            Self::Ports => Icon::Plug,
            Self::Durations => Icon::Hourglass,
            Self::Timeline => Icon::Timeline,
            Self::Database => Icon::Database,
        }
    }

    fn run(self) -> Run {
        match self {
            Self::Stack => Run::Stack,
            Self::Requests => Run::Requests,
            Self::Migrations => Run::Migrations,
            Self::Audit => Run::Audit,
            Self::Tests => Run::Tests,
            Self::Ports => Run::Ports,
            Self::Durations => Run::Durations,
            Self::Timeline => Run::Timeline,
            Self::Database => Run::Database,
        }
    }

    /// The screen open, if one is: the Code tab is the stack's.
    pub fn of(overlay: Option<&Overlay>) -> Option<Self> {
        Some(match overlay? {
            Overlay::Stack { .. } | Overlay::Code(_) | Overlay::Health { .. } | Overlay::Flame(_) => Self::Stack,
            Overlay::Requests(_) => Self::Requests,
            Overlay::Migrations { .. } => Self::Migrations,
            Overlay::Audit { .. } => Self::Audit,
            Overlay::Tests { .. } => Self::Tests,
            Overlay::Ports { .. } => Self::Ports,
            Overlay::Durations { .. } => Self::Durations,
            Overlay::Timeline(_) => Self::Timeline,
            _ => return None,
        })
    }

    /// Another screen's key this one keeps for itself: the tests' `A` watches them all.
    fn keeps(self, key: &str) -> bool {
        self == Self::Tests && key == Self::Audit.key()
    }
}

/// The bar over `open`: each screen with its key - just the keys when the names don't fit - the open one lit, each a
/// click away.
pub fn bar(ui: &Ui, open: Screen, cells: usize) -> Vec<Seg> {
    let row = |names: bool| {
        let mut segs = Vec::new();
        for screen in Screen::ALL {
            let target = Target::Screen(screen);
            let leaving = ui
                .screen_change
                .filter(|(_, to, at)| *to == open && moving(ui, *at, SLIDE))
                .is_some_and(|(from, ..)| from == screen);
            if leaving {
                segs.push(
                    super::seg::on(format!("  {} {}", screen.key(), screen.name()), C::Black, C::DarkGray).at(target),
                );
                continue;
            }
            if screen == open {
                segs.push(super::viewbar::chip(ui.nerd, screen.icon(), screen.name()).at(target));
                continue;
            }
            let key = if open.keeps(screen.key()) { "" } else { screen.key() };
            segs.push(seg(format!("  {key}"), C::Cyan).at(target));
            if names || key.is_empty() {
                segs.push(seg(format!(" {}", screen.name()), C::DarkGray).at(target));
            }
        }
        segs
    };
    let named = row(true);
    if width(&named) <= cells { named } else { super::columns::fit(row(false), cells) }
}

/// How long a new screen slides in, and how far (a share of the width).
const SLIDE: std::time::Duration = std::time::Duration::from_millis(140);
const SLIDE_SHARE: f64 = 0.25;
/// How long a key that did nothing shakes the hints.
pub const SHAKE: std::time::Duration = std::time::Duration::from_millis(160);

/// Whether a motion that began `at` is still under way, `length` long (never with reduced motion).
fn moving(ui: &Ui, at: std::time::Instant, length: std::time::Duration) -> bool {
    super::stack::moving(ui) && ui.now.saturating_duration_since(at) < length
}

/// The place of a screen in the bar.
fn place(screen: Screen) -> usize {
    Screen::ALL.iter().position(|each| *each == screen).unwrap_or(0)
}

/// A screen just switched to slides in from the side of its place in the bar, easing out.
pub fn slide(ui: &Ui, body: Vec<Vec<Seg>>, cells: usize) -> Vec<Vec<Seg>> {
    let Some((from, to, at)) = ui.screen_change.filter(|(.., at)| moving(ui, *at, SLIDE)) else { return body };
    let t = ui.now.saturating_duration_since(at).as_secs_f64() / SLIDE.as_secs_f64();
    let offset = ((1.0 - t) * (1.0 - t) * cells as f64 * SLIDE_SHARE).round() as usize;
    if offset == 0 {
        return body;
    }
    let from_right = place(to) > place(from);
    body.into_iter()
        .map(|line| {
            if from_right {
                let mut shifted = vec![seg(" ".repeat(offset), C::Gray)];
                shifted.extend(line);
                super::columns::fit(shifted, cells)
            } else {
                let mut kept = Vec::new();
                let mut skipped = 0;
                for part in line {
                    let wide = unicode_width::UnicodeWidthStr::width(part.text.as_str());
                    if skipped + wide <= offset {
                        skipped += wide;
                        continue;
                    }
                    kept.push(part);
                }
                super::columns::fit(kept, cells)
            }
        })
        .collect()
}

/// Whether a screen is sliding in or the hints shake.
pub fn animating(ui: &Ui) -> bool {
    ui.screen_change.is_some_and(|(.., at)| moving(ui, at, SLIDE))
        || ui.shake_at.is_some_and(|at| moving(ui, at, SHAKE))
}

/// The hints shifted a cell back and forth while a key that did nothing shakes them.
pub fn shaken(ui: &Ui, line: Vec<Seg>) -> Vec<Seg> {
    let Some(at) = ui.shake_at.filter(|at| moving(ui, *at, SHAKE)) else { return line };
    let step = (ui.now.saturating_duration_since(at).as_millis() / 40) % 2;
    if step == 0 {
        let mut shifted = vec![seg(" ", C::Gray)];
        shifted.extend(line);
        shifted
    } else {
        line
    }
}

/// The screen open: the database picker's, else the overlay's.
pub fn current(ui: &Ui) -> Option<Screen> {
    if ui.picker.is_some() { Some(Screen::Database) } else { Screen::of(ui.overlay.as_ref()) }
}

/// Whether the open screen takes typed text now: its keys are letters then.
fn typing(ui: &Ui) -> bool {
    if let Some(picker) = &ui.picker {
        return picker.filtering || picker.new_name.is_some();
    }
    match &ui.overlay {
        Some(Overlay::Requests(view)) => view.typing || view.url_edit.is_some(),
        Some(Overlay::Timeline(view)) => view.typing,
        Some(Overlay::Tests { .. }) => ui.tests_typing,
        Some(Overlay::Code(_)) => super::code::typing(ui),
        Some(Overlay::Flame(_)) => super::flame::typing(ui),
        Some(Overlay::Health { .. }) => ui.health_edit.is_some(),
        _ => false,
    }
}

/// Another screen's key on a screen goes there; false for any other key.
pub fn switch_key(ui: &mut Ui, controller: &mut Controller, name: &str) -> bool {
    let Some(open) = current(ui) else { return false };
    let Some(next) = Screen::ALL.into_iter().find(|screen| screen.key() == name) else { return false };
    if next == open || open.keeps(name) || typing(ui) {
        return false;
    }
    go(ui, controller, next);
    true
}

/// Goes to `next`, on the app the open screen is looking at (its column focused, too); stays when `next` has nothing
/// to show (it says why).
pub fn go(ui: &mut Ui, controller: &mut Controller, next: Screen) {
    if current(ui) == Some(next) {
        return;
    }
    let app = looked_at(ui, controller);
    if let Some(at) = app {
        ui.focus = at;
    }
    let open = ui.overlay.take();
    (ui.behind, ui.picker) = (None, None);
    super::keys::run(ui, controller, &next.run());
    if ui.overlay.is_none() && ui.picker.is_none() {
        ui.overlay = open;
        return;
    }
    if let Some(at) = app {
        on_app(ui, controller, next, at);
    }
}

/// The app the open screen is looking at, by its place in the apps.
fn looked_at(ui: &Ui, controller: &Controller) -> Option<usize> {
    let place = |key: &str| controller.apps.iter().position(|app| app.key == key);
    let key = match &ui.overlay {
        Some(Overlay::Stack { index, .. } | Overlay::Health { index, .. }) => {
            return (*index < controller.apps.len()).then_some(*index);
        }
        Some(Overlay::Audit { app, .. }) => return Some(*app),
        Some(Overlay::Code(view)) => {
            let picked = view.picked.as_deref().and_then(|id| {
                let map = controller.code_map()?;
                map.parts[map.find(id)?].app.clone()
            });
            view.app.clone().or(picked)
        }
        Some(Overlay::Requests(view)) => view.app.clone().or_else(|| {
            let selected = controller.requests.items.iter().find(|request| request.seq == view.selected);
            selected.filter(|_| !view.follow).map(|request| request.app.clone())
        }),
        Some(Overlay::Migrations { index, .. }) => {
            controller.contexts.as_deref().unwrap_or_default().get(*index).map(|context| context.app.clone())
        }
        Some(Overlay::Tests { index, .. }) => match super::suites::list_rows(controller).get(*index) {
            Some(Row::Suite(at)) => controller.suites.as_deref().unwrap_or_default().get(*at)?.app.clone(),
            _ => None,
        },
        Some(Overlay::Ports { .. }) => super::ports::picked_app(ui, controller),
        Some(Overlay::Flame(view)) => Some(view.app.clone()),
        _ => None,
    };
    key.as_deref().and_then(place)
}

/// The screen just opened, on app `at`: its requests alone, its first context, its audit, its first suite, its port.
fn on_app(ui: &mut Ui, controller: &mut Controller, next: Screen, at: usize) {
    let Some(key) = controller.apps.get(at).map(|app| app.key.clone()) else { return };
    match (next, &mut ui.overlay) {
        (Screen::Requests, Some(Overlay::Requests(view))) => view.app = Some(key),
        (Screen::Migrations, Some(Overlay::Migrations { index, .. })) => {
            let contexts = controller.contexts.as_deref().unwrap_or_default();
            if let Some(found) = contexts.iter().position(|context| context.app == key) {
                *index = found;
            }
        }
        (Screen::Audit, Some(Overlay::Audit { app, .. })) => *app = at,
        (Screen::Tests, Some(Overlay::Tests { index, .. })) => {
            let suites = controller.suites.as_deref().unwrap_or_default();
            let mine = |row: &Row| matches!(row, Row::Suite(suite) if suites[*suite].app.as_deref() == Some(&key));
            if let Some(row) = super::suites::list_rows(controller).iter().position(mine) {
                *index = row;
            }
        }
        (Screen::Ports, _) => {
            if let Some(port) = controller.port(&key) {
                super::ports::open(ui, controller, Some(port));
            }
        }
        _ => {}
    }
}
