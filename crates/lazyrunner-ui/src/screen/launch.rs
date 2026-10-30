//! The launch picker (`x` then `p`, or the palette): what the focused app runs on - a .NET app's launch profiles with
//! their URLs, an npm app's variants - the one it runs now marked `●`, the profile's own said so. Enter runs it on the
//! one picked (restarting it when it runs), remembered for this branch; picking the profile's own forgets the choice.

use lazyrunner_core::output::Hue;
use lazyrunner_core::run::Controller;

use super::icons::{self, Icon};
use super::keys;
use super::overlays::boxed;
use super::seg::{Seg, on, pad, seg};
use super::theme::C;
use super::{Overlay, Ui};

const INNER: usize = 84;
const NAME_CELLS: usize = 30;

/// x then p: the focused app's choices, the one it runs now selected.
pub fn open(ui: &mut Ui, controller: &mut Controller) {
    let Some(app) = controller.apps.get(ui.focus) else { return };
    let choices = controller.launch_choices(&app.key);
    if choices.len() < 2 {
        let what = format!("{} has nothing else to run on: one launch profile or variant", app.name);
        controller.add_log(what, Hue::DarkGray);
        return;
    }
    let index = choices.iter().position(|choice| choice.current).unwrap_or(0);
    ui.overlay = Some(Overlay::Launch { app: ui.focus, index });
}

pub fn key(ui: &mut Ui, controller: &mut Controller, name: &str) {
    let Some(Overlay::Launch { app, index }) = &mut ui.overlay else { return };
    let Some(key) = controller.apps.get(*app).map(|app| app.key.clone()) else { return };
    let choices = controller.launch_choices(&key);
    match name {
        "Escape" | "q" | "p" => ui.overlay = None,
        "j" | "DownArrow" => *index = keys::wrapped(*index, 1, choices.len().max(1)),
        "k" | "UpArrow" => *index = keys::wrapped(*index, -1, choices.len().max(1)),
        "Enter" | "Space" => {
            let picked = choices.get(*index).map(|choice| choice.name.clone());
            ui.overlay = None;
            if let Some(picked) = picked {
                controller.choose_launch(&key, &picked);
            }
        }
        _ => {}
    }
}

pub fn rows(ui: &Ui, controller: &Controller, cells: usize, rows: usize) -> Vec<Vec<Seg>> {
    let Some(Overlay::Launch { app, index }) = &ui.overlay else { return Vec::new() };
    let Some(found) = controller.apps.get(*app) else { return Vec::new() };
    let choices = controller.launch_choices(&found.key);
    let selected = (*index).min(choices.len().saturating_sub(1));
    let inner = cells.saturating_sub(8).clamp(20, INNER);
    let mut lines: Vec<Vec<Seg>> = choices
        .iter()
        .enumerate()
        .map(|(at, choice)| {
            let mark = if choice.current { "●" } else { " " };
            let own = if choice.default { " (its own)" } else { "" };
            let name = pad(&format!("{}{own}", choice.name), NAME_CELLS);
            if at == selected {
                vec![on(pad(&format!(" {mark} {name} {}", choice.detail), inner), C::Black, C::Cyan)]
            } else {
                vec![
                    seg(format!(" {mark} "), C::Green),
                    seg(format!("{name} "), C::White),
                    seg(&choice.detail, C::DarkGray),
                ]
            }
        })
        .collect();
    let branch = if controller.branch.is_empty() { "this checkout".to_string() } else { controller.branch.clone() };
    lines.push(Vec::new());
    lines.push(vec![seg(format!(" Remembered for {branch}; its own forgets the choice."), C::DarkGray)]);
    let title = icons::titled(ui.nerd, Icon::Terminal, &format!("{} runs on", found.name));
    boxed(&title, lines, "j k pick · Enter runs it on that (restarting it) · Esc closes", cells, rows, inner)
}
