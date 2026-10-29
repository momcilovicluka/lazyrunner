//! A key held down: the terminal repeats its press until it's let go (Windows' console as presses, with a release
//! at the end). Moving and scrolling repeat, and so does typing in a prompt; any other key acts once per press, so a
//! held `f` in the settings doesn't flick reduced motion on and off.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};

use super::{Ui, keys};

/// Keys that move or scroll wherever they are.
const MOVES: [&str; 16] = [
    "j",
    "k",
    "UpArrow",
    "DownArrow",
    "LeftArrow",
    "RightArrow",
    "PageUp",
    "PageDown",
    "Ctrl+D",
    "Ctrl+U",
    "Ctrl+F",
    "Ctrl+B",
    "Ctrl+N",
    "Ctrl+P",
    "Tab",
    "Shift+Tab",
];
/// Keys that move only on the columns: in a box or a menu they open, copy or toggle something.
const COLUMN_MOVES: [&str; 8] = ["h", "l", "n", "N", "[", "]", "+", "-"];

/// The key last pressed and not yet let go.
#[derive(Default)]
pub struct Held {
    key: Option<KeyCode>,
    /// The terminal tells when a key is let go; without that, a press again is a press.
    releases: bool,
}

/// Whether the loop drops this key event: a release, or a held key's repeat that doesn't move.
pub fn dropped(ui: &mut Ui, key: &KeyEvent) -> bool {
    let code = folded(key.code);
    match key.kind {
        KeyEventKind::Release => {
            ui.held.releases = true;
            if ui.held.key == Some(code) {
                ui.held.key = None;
            }
            true
        }
        KeyEventKind::Repeat => !repeats(ui, key),
        KeyEventKind::Press if ui.held.releases && ui.held.key == Some(code) => !repeats(ui, key),
        KeyEventKind::Press => {
            ui.held.key = Some(code);
            false
        }
    }
}

fn repeats(ui: &Ui, key: &KeyEvent) -> bool {
    let name = keys::key_name(key);
    let columns = ui.overlay.is_none() && ui.picker.is_none();
    keys::prompt(ui)
        || name == "Backspace"
        || name == "Delete"
        || MOVES.contains(&name.as_str())
        || (columns && COLUMN_MOVES.contains(&name.as_str()))
}

/// A letter is let go as itself whether Shift went up first or not.
fn folded(code: KeyCode) -> KeyCode {
    match code {
        KeyCode::Char(c) => KeyCode::Char(c.to_ascii_lowercase()),
        other => other,
    }
}
