//! `Alt+x`: the focused app's crashes this session (#99) - a timeline of when, then each one's time, exit code and
//! the last error it printed before it. `y` copies the newest error, `u` starts a held app again.

use std::time::{Duration, Instant};

use lazyrunner_core::output::Hue;
use lazyrunner_core::run::controller::Crash;
use lazyrunner_core::run::{Controller, format_duration};

use super::actions::Run;
use super::icons::{self, Icon};
use super::overlays::boxed;
use super::seg::{Seg, pad, seg};
use super::theme::C;
use super::{Overlay, Ui};

const INNER: usize = 100;
const FOOTER: &str = "y copies the last error · u starts it again · Esc closes";
const WHEN_CELLS: usize = 20;
const CODE_CELLS: usize = 10;
/// A held app's border pulses red this long, as it's held.
pub const HELD_PULSE: Duration = Duration::from_millis(900);

pub fn open(ui: &mut Ui, controller: &mut Controller) {
    let Some(app) = controller.apps.get(ui.focus) else { return };
    if controller.crashes_of(&app.key).is_empty() {
        controller.add_log(format!("{} hasn't crashed this session.", app.name), Hue::DarkGray);
        return;
    }
    ui.overlay = Some(Overlay::Crashes { app: ui.focus });
}

pub fn key(ui: &mut Ui, controller: &mut Controller, name: &str) {
    let Some(Overlay::Crashes { app }) = ui.overlay else { return };
    let Some(key) = controller.apps.get(app).map(|app| app.key.clone()) else { return };
    match name {
        "Escape" | "q" | "Alt+x" => ui.overlay = None,
        "y" => {
            let error = controller.crashes_of(&key).iter().rev().find_map(|crash| crash.error.clone());
            match error {
                Some(error) => super::copy::put(ui.clipboard, controller, &error, "its last error"),
                None => controller.add_log("It printed no error before its crashes.", Hue::DarkGray),
            }
        }
        "u" => {
            ui.overlay = None;
            ui.focus = app;
            super::keys::run(ui, controller, &Run::StartFocused);
        }
        _ => {}
    }
}

/// When it crashed, along a line from its first crash to now.
fn timeline(crashes: &[Crash], now: Instant, cells: usize) -> Vec<Seg> {
    let Some(first) = crashes.first().map(|crash| crash.at) else { return Vec::new() };
    let span = now.saturating_duration_since(first).as_secs_f64().max(1.0);
    let mut line: Vec<char> = "─".repeat(cells).chars().collect();
    for crash in crashes {
        let at = (crash.at.saturating_duration_since(first).as_secs_f64() / span * (cells - 1) as f64) as usize;
        line[at.min(cells - 1)] = '✗';
    }
    let ago = format_duration(span);
    let mut segs = vec![seg(format!(" {ago} ago "), C::DarkGray)];
    let mut run = String::new();
    let mut crossed = false;
    for letter in line {
        if (letter == '✗') != crossed && !run.is_empty() {
            segs.push(seg(std::mem::take(&mut run), if crossed { C::Red } else { C::DarkGray }));
        }
        crossed = letter == '✗';
        run.push(letter);
    }
    segs.push(seg(run, if crossed { C::Red } else { C::DarkGray }));
    segs.push(seg(" now", C::DarkGray));
    segs
}

/// A crash's wall-clock time.
fn clock(ui: &Ui, at: Instant) -> String {
    let ago = chrono::Duration::from_std(ui.now.saturating_duration_since(at)).unwrap_or_default();
    (chrono::Local::now() - ago).format("%H:%M:%S").to_string()
}

pub fn rows(ui: &Ui, controller: &Controller, cells: usize, rows: usize) -> Vec<Vec<Seg>> {
    let Some(Overlay::Crashes { app }) = &ui.overlay else { return Vec::new() };
    let Some(found) = controller.apps.get(*app) else { return Vec::new() };
    let inner = cells.saturating_sub(8).clamp(30, INNER);
    let crashes = controller.crashes_of(&found.key);
    let state = match controller.recovery(&found.key) {
        Some(recovery) if recovery.held => (format!(" {} - u starts it", recovery.said(ui.now)), C::Red),
        Some(recovery) if recovery.next.is_some() => (format!(" {}", recovery.said(ui.now)), C::Yellow),
        _ if controller.is_running(&found.key) => (" up again".to_string(), C::Green),
        _ => (" not running".to_string(), C::DarkGray),
    };
    let mut lines = vec![vec![seg(state.0, state.1)], Vec::new()];
    lines.push(timeline(crashes, ui.now, inner.saturating_sub(16).max(8)));
    lines.push(Vec::new());
    for crash in crashes.iter().rev() {
        let ago = format_duration(ui.now.saturating_duration_since(crash.at).as_secs_f64());
        let code = crash.code.map_or("killed".to_string(), |code| format!("exit {code}"));
        let room = inner.saturating_sub(WHEN_CELLS + CODE_CELLS + 4);
        let error = crash.error.as_deref().unwrap_or("no error printed");
        lines.push(vec![
            seg(pad(&format!(" ✗ {} · {ago} ago", clock(ui, crash.at)), WHEN_CELLS), C::Gray),
            seg(pad(&code, CODE_CELLS), C::Red),
            seg(super::requests::cut(error, room), if crash.error.is_some() { C::White } else { C::DarkGray }),
        ]);
    }
    let title = icons::titled(ui.nerd, Icon::Reload, &format!("{}'s crashes", found.name));
    boxed(&title, lines, FOOTER, cells, rows, inner)
}

/// The border colour of app `index`: red for a moment as it's held after a crash loop, else its state's.
pub fn border(ui: &Ui, controller: &Controller, index: usize) -> C {
    let held_at = controller.apps.get(index).and_then(|app| controller.recovery(&app.key)).and_then(|it| it.held_at);
    let pulsing = held_at.is_some_and(|at| ui.now.saturating_duration_since(at) < HELD_PULSE);
    if pulsing && !ui.reduced_motion { C::Red } else { super::columns::border(controller, index) }
}

/// Whether a held app's border pulses.
pub fn animating(ui: &Ui, controller: &Controller) -> bool {
    let pulsing = |key: &str| {
        controller
            .recovery(key)
            .and_then(|it| it.held_at)
            .is_some_and(|at| ui.now.saturating_duration_since(at) < HELD_PULSE)
    };
    !ui.reduced_motion && controller.apps.iter().any(|app| pulsing(&app.key))
}

/// The status line's word on a held focused app: its last error, and what to press.
pub fn held_banner(ui: &Ui, controller: &Controller) -> Option<Vec<Seg>> {
    let app = controller.apps.get(ui.focus)?;
    controller.recovery(&app.key).filter(|recovery| recovery.held)?;
    let error = controller.crashes_of(&app.key).iter().rev().find_map(|crash| crash.error.clone());
    let said = error.map_or(String::new(), |error| format!(": {}", super::requests::cut(&error, 60)));
    Some(vec![
        seg(format!(" ✗ {} is held after its crashes{said} - ", app.name), C::Red),
        seg("[u]", C::Cyan),
        seg(" starts it · ", C::Red),
        seg("[Alt+x]", C::Cyan),
        seg(" its crashes", C::Red),
    ])
}
