//! The mouse (`, m` turns it off, so the terminal selects text again): a click focuses a column (a double click on
//! its title zooms it), the wheel scrolls the output under the pointer, a click picks a list item and a double click
//! runs it, a click outside a box closes it, and a setting in the panel toggles on a click.
//!
//! What is where comes from the last frame: segments carry a `Target`, and drawing records where each one landed
//! (`Hit`). Clicks go through the keys' own paths (arrows, Enter, Esc, the actions), so they do what the keys do.

use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use lazyrunner_core::config::Toggle;
use lazyrunner_core::output::Hue;
use lazyrunner_core::run::Controller;

use super::actions::Run;
use super::{Overlay, Ui, keys};

/// Two clicks on the same cell within this long are a double click.
const DOUBLE_CLICK: Duration = Duration::from_millis(500);
/// Lines a wheel notch scrolls a column or a box of text.
const WHEEL_LINES: isize = 3;

/// What a segment on the screen is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// A column's title bar (or the line under it), by its app's index.
    Title(usize),
    /// A column's output.
    Output(usize),
    /// An item of the open list (menu, palette, database or theme picker), by its index in that list.
    Item(usize),
    /// The rest of an open box: a click there keeps it open.
    Box,
    /// A link in a column's output: the column, and the link (in `Ui::links`).
    Link { column: usize, link: usize },
    /// A test of the suite picked in the tests view, by its place in that list.
    Test(usize),
    /// The tests' summary on the panel's status line: a click opens the tests view.
    Tests,
    /// A view's chip of news on the panel's status line: a click opens the view on the first new thing.
    News(super::jumps::View),
    /// A migration of the context selected in the migrations view, by its place in that list (newest first).
    Migration(usize),
    /// A row of a column's rail: the column, the row, where a click scrolls it to (lines back from the newest), and
    /// the line it brings into view (0 = the oldest kept), which the pointer on it previews.
    Rail { column: usize, row: usize, scroll: usize, line: usize },
    /// A diagnostics zip's path in the log (in `Ui::links`): a click shows it in its folder.
    Reveal(usize),
}

/// Where a target was drawn in the last frame: one row, from `x`, `width` cells.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hit {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub target: Target,
}

impl Hit {
    fn contains(&self, x: u16, y: u16) -> bool {
        y == self.y && x >= self.x && x < self.x.saturating_add(self.width)
    }
}

/// The mouse events lazyrunner acts on: clicks, the wheel, moves (for what the pointer is over) and drags (along a
/// rail); releases are dropped before anything redraws.
pub fn wanted(event: &MouseEvent) -> bool {
    matches!(
        event.kind,
        MouseEventKind::Down(MouseButton::Left)
            | MouseEventKind::Drag(MouseButton::Left)
            | MouseEventKind::ScrollUp
            | MouseEventKind::ScrollDown
            | MouseEventKind::Moved
    )
}

/// What is at a cell in the last frame.
pub fn target_at(ui: &Ui, x: u16, y: u16) -> Option<Target> {
    ui.hits.iter().rev().find(|hit| hit.contains(x, y)).map(|hit| hit.target)
}

/// Acts on a mouse event; true when the screen changes.
pub fn handle(ui: &mut Ui, controller: &mut Controller, event: MouseEvent) -> bool {
    if !controller.setting(Toggle::Mouse) {
        return false; // turned off (, m): what was on its way still arrives
    }
    if ui.follow.on {
        return false;
    }
    let (x, y) = (event.column, event.row);
    let target = target_at(ui, x, y);
    ui.pending_g = None;
    match event.kind {
        // Only a move onto something else redraws.
        MouseEventKind::Moved => return std::mem::replace(&mut ui.hover, target) != target,
        MouseEventKind::ScrollUp => wheel(ui, controller, target, 1),
        MouseEventKind::ScrollDown => wheel(ui, controller, target, -1),
        MouseEventKind::Down(MouseButton::Left) => {
            let double = ui
                .last_click
                .is_some_and(|(at, cx, cy)| (cx, cy) == (x, y) && ui.now.saturating_duration_since(at) < DOUBLE_CLICK);
            // A double click is the end of a pair: a third click starts a new one.
            ui.last_click = if double { None } else { Some((ui.now, x, y)) };
            let boxes = (ui.overlay.is_some(), ui.picker.is_some());
            let modified = event.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
            click(ui, controller, target, double, modified);
            if (ui.overlay.is_some(), ui.picker.is_some()) != boxes {
                ui.last_click = None; // a box opened or closed: the next click is on what is there now
            }
        }
        // A drag along a rail scrolls the column as it goes (the line the pointer is on previews).
        MouseEventKind::Drag(MouseButton::Left) => {
            let Some(Target::Rail { column, scroll, .. }) = target else { return false };
            ui.hover = target;
            to_rail(ui, controller, column, scroll);
        }
        _ => return false,
    }
    true
}

/// A click or a drag on a column's rail: the column has the focus and scrolls there.
fn to_rail(ui: &mut Ui, controller: &mut Controller, column: usize, scroll: usize) {
    ui.focus = column;
    if let Some(state) = controller.states.get_mut(column) {
        state.scroll = scroll;
    }
}

fn click(ui: &mut Ui, controller: &mut Controller, target: Option<Target>, double: bool, modified: bool) {
    ui.reload = None; // a click ends the reload show too
    if let Some(target) = target {
        ui.motion.flash(super::motion::Spot::Clicked(target), ui.now);
    }
    match &mut ui.overlay {
        Some(Overlay::Checklist(list)) => {
            list.skip = true; // as any key
            return;
        }
        Some(Overlay::Help) => {
            ui.overlay = None;
            return;
        }
        _ => {}
    }
    let typing_in_picker = ui.picker.as_ref().is_some_and(|picker| picker.filtering || picker.new_name.is_some());
    if typing_in_picker {
        // Typing a filter or a new database's name: a click there doesn't pick (or add) anything; outside, it closes.
        if !matches!(target, Some(Target::Item(_) | Target::Box)) {
            ui.picker = None;
        }
        return;
    }
    if super::suites::click(ui, controller, target, double) || super::migrations::click(ui, controller, target, double)
    {
        return;
    }
    if ui.overlay.is_some() || ui.picker.is_some() {
        match target {
            Some(Target::Item(index)) => {
                select(ui, controller, index);
                if double {
                    press(ui, controller, KeyCode::Enter);
                }
            }
            Some(Target::Box) => {}
            Some(Target::Link { link, .. }) => open_link(ui, controller, link, None, modified),
            _ => press(ui, controller, KeyCode::Esc), // outside the box: closes it, as Esc
        }
        return;
    }
    if ui.search.as_ref().is_some_and(|search| search.typing) {
        press(ui, controller, KeyCode::Enter); // a search being typed ends as Enter ends it
    }
    match target {
        Some(Target::Title(index)) => {
            ui.focus = index;
            if double {
                keys::run(ui, controller, &Run::Zoom);
            }
        }
        Some(Target::Output(index)) => ui.focus = index,
        Some(Target::Rail { column, scroll, .. }) => to_rail(ui, controller, column, scroll),
        Some(Target::Tests) => keys::run(ui, controller, &Run::Tests),
        Some(Target::News(view)) => super::seen::open(ui, controller, view),
        Some(Target::Reveal(link)) => {
            if link_click(ui, controller, modified) {
                super::bundle::reveal(ui, controller, link);
            }
        }
        Some(Target::Link { column, link }) => {
            ui.focus = column;
            open_link(ui, controller, link, Some(column), modified);
        }
        _ => {}
    }
}

/// A link opens on Alt+click (`linksOnAltClick`, the default; Ctrl+click too, where the terminal doesn't take it for
/// its own links - Windows Terminal does, on URLs) or on any click; a plain click on one says so, once.
fn open_link(ui: &mut Ui, controller: &mut Controller, link: usize, app: Option<usize>, modified: bool) {
    if link_click(ui, controller, modified) {
        super::links::open(ui, controller, link, app);
    }
}

/// Whether a click opens a link; a plain one that doesn't says so, once.
fn link_click(ui: &mut Ui, controller: &mut Controller, modified: bool) -> bool {
    if modified || !controller.setting(Toggle::LinksOnAltClick) {
        return true;
    }
    if !std::mem::replace(&mut ui.link_hint_shown, true) {
        controller.add_log("Alt+click opens a link (, then l: a click does)", Hue::DarkGray);
    }
    false
}

/// The wheel: a box of text or a list moves as its arrow keys move it; elsewhere the column under the pointer
/// scrolls (`direction` 1 = back, to older lines), whichever column has the focus.
fn wheel(ui: &mut Ui, controller: &mut Controller, target: Option<Target>, direction: isize) {
    let arrow = if direction > 0 { KeyCode::Up } else { KeyCode::Down };
    match &ui.overlay {
        Some(Overlay::Changelog { .. } | Overlay::Durations { .. }) => {
            for _ in 0..WHEEL_LINES {
                press(ui, controller, arrow);
            }
            return;
        }
        Some(Overlay::Checklist(_) | Overlay::Help) => return,
        Some(Overlay::Tests { .. }) => return super::suites::wheel(ui, controller, target, direction > 0),
        Some(Overlay::Migrations { .. }) => return super::migrations::wheel(ui, controller, target, direction > 0),
        Some(_) => return press(ui, controller, arrow),
        None if ui.picker.is_some() => return press(ui, controller, arrow),
        None => {}
    }
    if let Some(
        Target::Output(index)
        | Target::Title(index)
        | Target::Link { column: index, .. }
        | Target::Rail { column: index, .. },
    ) = target
    {
        keys::scroll_column(controller, index, WHEEL_LINES * direction, ui.search.as_ref());
    }
}

/// Moves the open list's selection to `index` with its arrow keys, so it does what they do (the theme picker
/// previews, the lists scroll).
fn select(ui: &mut Ui, controller: &mut Controller, index: usize) {
    let current = match &ui.overlay {
        Some(
            Overlay::Menu { index }
            | Overlay::Settings { index }
            | Overlay::Palette { index, .. }
            | Overlay::Themes { index, .. }
            | Overlay::Tests { index, .. },
        ) => *index,
        Some(Overlay::Requests(view)) => view.index,
        Some(Overlay::Install { index } | Overlay::Copy { index } | Overlay::About { index }) => *index,
        _ => ui.picker.as_ref().map_or(index, |picker| picker.index),
    };
    let arrow = if index > current { KeyCode::Down } else { KeyCode::Up };
    for _ in 0..index.abs_diff(current) {
        press(ui, controller, arrow);
    }
}

fn press(ui: &mut Ui, controller: &mut Controller, code: KeyCode) {
    keys::press(ui, controller, KeyEvent::new(code, KeyModifiers::NONE));
}

/// A click at a cell, as the control channel and the tests give it (a second one there soon after is a double).
pub fn click_at(ui: &mut Ui, controller: &mut Controller, x: u16, y: u16, now: Instant) {
    ui.now = now;
    let event =
        MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: x, row: y, modifiers: KeyModifiers::NONE };
    handle(ui, controller, event);
}

/// A wheel notch at a cell (`back`: up, to older lines).
pub fn wheel_at(ui: &mut Ui, controller: &mut Controller, x: u16, y: u16, back: bool) {
    let kind = if back { MouseEventKind::ScrollUp } else { MouseEventKind::ScrollDown };
    handle(ui, controller, MouseEvent { kind, column: x, row: y, modifiers: KeyModifiers::NONE });
}
