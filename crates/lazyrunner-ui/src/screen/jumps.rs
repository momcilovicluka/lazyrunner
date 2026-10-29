//! The jump list, as an editor's: `Ctrl+O` / `Alt+←` back to where you were before a jump, `Alt+→` forward again. A
//! jump is a view opened (`H` `T` `M`), the focused column scrolled more than half a page at once (`gg` `G`, a search
//! match, a mark, a page), or another column scrolled as it gets the focus (a mark's jump). A place is the view, the
//! column and its bottom line - by number, so it stays put as new lines arrive; at most 50 are kept.

use crossterm::event::{KeyEvent, KeyModifiers};
use lazyrunner_core::output::Hue;
use lazyrunner_core::run::Controller;

use super::columns;
use super::motion::Spot;
use super::{Overlay, Ui};

const MOST: usize = 50;
const GONE: &str = "Skipped a place: its line left the buffer, or the filter or search hides it.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    Requests,
    Tests,
    Migrations,
}

/// Where you are: the view open (none: the columns), the focused column, and its bottom line (none: following).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place {
    view: Option<View>,
    app: usize,
    line: Option<u64>,
}

#[derive(Debug, Default)]
pub struct Jumps {
    list: Vec<Place>,
    /// Where in the list you are; its length when you haven't gone back.
    at: usize,
    /// The key went back or forward: not a jump of its own.
    travelled: bool,
}

impl Jumps {
    fn record(&mut self, place: Place) {
        self.list.truncate(self.at);
        if self.list.last() != Some(&place) {
            self.list.push(place);
        }
        if self.list.len() > MOST {
            self.list.remove(0);
        }
        self.at = self.list.len();
    }

    /// The place before; the first step back keeps `current` for the way forward.
    fn back(&mut self, current: Place) -> Option<Place> {
        if self.at == 0 {
            return None;
        }
        if self.at == self.list.len() {
            self.list.push(current);
        }
        self.at -= 1;
        self.list.get(self.at).cloned()
    }

    fn forward(&mut self) -> Option<Place> {
        if self.at + 1 >= self.list.len() {
            return None;
        }
        self.at += 1;
        self.list.get(self.at).cloned()
    }

    /// Drops the place just stepped onto (it's gone), so the next step goes on past it.
    fn forget(&mut self, back: bool) {
        self.list.remove(self.at);
        if !back {
            self.at -= 1;
        }
    }

    /// How many places back there are, of how many: the panel's `↶ 3/12`.
    pub fn count(&self) -> Option<(usize, usize)> {
        (self.at > 0).then_some((self.at, self.list.len()))
    }
}

fn view(overlay: Option<&Overlay>) -> Option<View> {
    match overlay {
        Some(Overlay::Requests(_)) => Some(View::Requests),
        Some(Overlay::Tests { .. }) => Some(View::Tests),
        Some(Overlay::Migrations { .. }) => Some(View::Migrations),
        _ => None,
    }
}

pub(super) fn here(ui: &Ui, controller: &Controller) -> Place {
    let app = ui.focus;
    let line = controller.states.get(app).filter(|state| state.scroll > 0).and_then(|state| {
        let shown = columns::shown_lines(state, ui.search.as_ref());
        let bottom = shown.len().checked_sub(1 + state.scroll)?;
        Some(state.output.number_at(shown[bottom].position))
    });
    // A box opened over a view (settings, the key reference) keeps the view in `behind`.
    Place { view: view(ui.overlay.as_ref()).or(view(ui.behind.as_ref())), app, line }
}

/// Where you were before a key, and how far back each column was scrolled.
pub struct Before {
    place: Place,
    scrolls: Vec<usize>,
}

pub fn before(ui: &Ui, controller: &Controller) -> Before {
    Before { place: here(ui, controller), scrolls: controller.states.iter().map(|state| state.scroll).collect() }
}

/// After a key: when it jumped, keeps where you were.
pub fn noticed(ui: &mut Ui, controller: &Controller, before: Before) {
    if std::mem::take(&mut ui.jumps.travelled) {
        return;
    }
    let to = here(ui, controller);
    if to == before.place {
        return;
    }
    let opened = to.view.is_some() && to.view != before.place.view;
    let was = before.scrolls.get(to.app).copied().unwrap_or_default();
    let now = controller.states.get(to.app).map_or(was, |state| state.scroll);
    let far = if to.app == before.place.app { now.abs_diff(was) > (ui.body_rows / 2).max(1) } else { now != was };
    if opened || (to.view.is_none() && far) {
        ui.jumps.record(before.place);
    }
}

/// `Ctrl+O` or `Alt+←` go back (true), `Alt+→` forward.
pub fn travel_key(name: &str, key: &KeyEvent) -> Option<bool> {
    let alt = key.modifiers == KeyModifiers::ALT;
    match name {
        "Ctrl+O" => Some(true),
        "LeftArrow" if alt => Some(true),
        "RightArrow" if alt => Some(false),
        _ => None,
    }
}

/// Back and forth work over the columns and the views, not while a box or a prompt is open.
pub fn can_travel(ui: &Ui) -> bool {
    let typing = ui.search.as_ref().is_some_and(|search| search.typing);
    ui.picker.is_none() && !typing && (ui.overlay.is_none() || view(ui.overlay.as_ref()).is_some())
}

pub fn travel(ui: &mut Ui, controller: &mut Controller, back: bool) {
    ui.jumps.travelled = true;
    let current = here(ui, controller);
    loop {
        let next = if back { ui.jumps.back(current.clone()) } else { ui.jumps.forward() };
        let Some(place) = next else { break };
        if place == current {
            continue;
        }
        if go(ui, controller, &place) {
            return;
        }
        ui.jumps.forget(back);
        controller.add_log(GONE, Hue::DarkGray);
    }
    let why = if back {
        "Nowhere to jump back to - a view opened, gg / G, a search match, a mark or a long scroll is a jump."
    } else {
        "Nowhere to jump forward to - Ctrl+O or Alt+← goes back first."
    };
    controller.add_log(why, Hue::DarkGray);
}

/// Goes to a place: its column with its line at the bottom again (flashed), its view; false when the line is gone.
pub(super) fn go(ui: &mut Ui, controller: &mut Controller, place: &Place) -> bool {
    let Some(state) = controller.states.get(place.app) else { return false };
    let (scroll, position) = match place.line {
        None => (0, None),
        Some(number) => {
            let shown = columns::shown_lines(state, ui.search.as_ref());
            let Some(at) = shown.iter().rposition(|item| state.output.number_at(item.position) == number) else {
                return false;
            };
            (shown.len() - 1 - at, Some(shown[at].position))
        }
    };
    ui.focus = place.app;
    controller.states[place.app].scroll = scroll;
    if let Some(position) = position {
        ui.motion.flash(Spot::Match { app: place.app, line: position }, ui.now);
    }
    if view(ui.overlay.as_ref()) != place.view {
        ui.overlay = None;
        match place.view {
            Some(View::Requests) => super::requests::open(ui),
            Some(View::Tests) => super::suites::open(ui, controller),
            Some(View::Migrations) => super::migrations::open(ui, controller),
            None => {}
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(app: usize) -> Place {
        Place { view: None, app, line: Some(app as u64) }
    }

    #[test]
    fn back_keeps_where_you_are_for_the_way_forward_and_a_new_jump_drops_what_was_ahead() {
        let mut jumps = Jumps::default();
        jumps.record(at(1));
        jumps.record(at(2));
        assert_eq!(jumps.count(), Some((2, 2)));
        assert_eq!(jumps.back(at(3)), Some(at(2)));
        assert_eq!(jumps.back(at(2)), Some(at(1)));
        assert_eq!(jumps.back(at(1)), None, "nothing older");
        assert_eq!(jumps.forward(), Some(at(2)));
        assert_eq!(jumps.forward(), Some(at(3)), "back where the first step back left");
        assert_eq!(jumps.forward(), None);
        jumps.back(at(3));
        jumps.record(at(2));
        assert_eq!(jumps.list, [at(1), at(2)], "a jump from the middle drops the places ahead");
    }

    #[test]
    fn a_gone_place_is_dropped_and_at_most_fifty_are_kept() {
        let mut jumps = Jumps::default();
        (1..=3).for_each(|app| jumps.record(at(app)));
        assert_eq!(jumps.back(at(4)), Some(at(3)));
        jumps.forget(true);
        assert_eq!(jumps.back(at(4)), Some(at(2)), "on past the gone one");
        (0..60).for_each(|app| jumps.record(at(app)));
        assert_eq!(jumps.list.len(), MOST);
    }
}
