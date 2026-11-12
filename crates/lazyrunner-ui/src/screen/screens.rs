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
}

impl Screen {
    pub const ALL: [Self; 8] = [
        Self::Stack,
        Self::Requests,
        Self::Migrations,
        Self::Audit,
        Self::Tests,
        Self::Ports,
        Self::Durations,
        Self::Timeline,
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
        }
    }

    /// The screen open, if one is: the Code tab is the stack's.
    pub fn of(overlay: Option<&Overlay>) -> Option<Self> {
        Some(match overlay? {
            Overlay::Stack { .. } | Overlay::Code(_) => Self::Stack,
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

/// Whether the open screen takes typed text now: its keys are letters then.
fn typing(ui: &Ui) -> bool {
    match &ui.overlay {
        Some(Overlay::Requests(view)) => view.typing,
        Some(Overlay::Timeline(view)) => view.typing,
        Some(Overlay::Tests { .. }) => ui.tests_typing,
        Some(Overlay::Code(_)) => super::code::typing(ui),
        _ => false,
    }
}

/// Another screen's key on a screen goes there; false for any other key.
pub fn switch_key(ui: &mut Ui, controller: &mut Controller, name: &str) -> bool {
    let Some(open) = Screen::of(ui.overlay.as_ref()) else { return false };
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
    if Screen::of(ui.overlay.as_ref()) == Some(next) {
        return;
    }
    let app = looked_at(ui, controller);
    if let Some(at) = app {
        ui.focus = at;
    }
    let open = ui.overlay.take();
    ui.behind = None;
    super::keys::run(ui, controller, &next.run());
    if ui.overlay.is_none() {
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
        Some(Overlay::Stack { index, .. }) => return (*index < controller.apps.len()).then_some(*index),
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
