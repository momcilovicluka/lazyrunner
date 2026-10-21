//! The mouse (`, m` turns it off, so the terminal selects text again): a click focuses a column (a double click on
//! its title zooms it), the wheel scrolls the output under the pointer, a click picks a list item and a double click
//! runs it, a click outside a box closes it, and a setting in the panel toggles on a click.
//!
//! What is where comes from the last frame: segments carry a `Target`, and drawing records where each one landed
//! (`Hit`). Clicks go through the keys' own paths (arrows, Enter, Esc, the actions), so they do what the keys do.

use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use lazyrunner_core::config::Toggle;
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
    /// An on/off setting in the panel.
    Setting(Toggle),
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

/// The mouse events lazyrunner acts on (a moving pointer, drags and releases are dropped before anything redraws).
pub fn wanted(event: &MouseEvent) -> bool {
    matches!(
        event.kind,
        MouseEventKind::Down(MouseButton::Left) | MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
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
    let (x, y) = (event.column, event.row);
    let target = target_at(ui, x, y);
    ui.pending_g = None;
    match event.kind {
        MouseEventKind::ScrollUp => wheel(ui, controller, target, 1),
        MouseEventKind::ScrollDown => wheel(ui, controller, target, -1),
        MouseEventKind::Down(MouseButton::Left) => {
            let double = ui
                .last_click
                .is_some_and(|(at, cx, cy)| (cx, cy) == (x, y) && ui.now.saturating_duration_since(at) < DOUBLE_CLICK);
            // A double click is the end of a pair: a third click starts a new one.
            ui.last_click = if double { None } else { Some((ui.now, x, y)) };
            let boxes = (ui.overlay.is_some(), ui.picker.is_some());
            click(ui, controller, target, double);
            if (ui.overlay.is_some(), ui.picker.is_some()) != boxes {
                ui.last_click = None; // a box opened or closed: the next click is on what is there now
            }
        }
        _ => return false,
    }
    true
}

fn click(ui: &mut Ui, controller: &mut Controller, target: Option<Target>, double: bool) {
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
    if ui.overlay.is_some() || ui.picker.is_some() {
        match target {
            Some(Target::Item(index)) => {
                select(ui, controller, index);
                if double {
                    press(ui, controller, KeyCode::Enter);
                }
            }
            Some(Target::Box) => {}
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
        Some(Target::Setting(_)) if double => {} // the first click toggled it
        Some(Target::Setting(toggle)) => {
            let label = super::actions::menu_toggles(controller)
                .find(|item| item.toggle == toggle)
                .map_or(toggle.key(), |item| item.label);
            keys::run(ui, controller, &Run::Setting { toggle, label });
        }
        _ => {}
    }
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
        Some(_) => return press(ui, controller, arrow),
        None if ui.picker.is_some() => return press(ui, controller, arrow),
        None => {}
    }
    if let Some(Target::Output(index) | Target::Title(index)) = target {
        keys::scroll_column(controller, index, WHEEL_LINES * direction);
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
            | Overlay::Themes { index, .. },
        ) => *index,
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
