//! A column's title as its app changes state (not with reduced motion): starting fills its chip yellow from the left
//! and coming up green, stopping drains the colour it had from the right - each edge moving an eighth of a cell at a
//! time - and a crash pulses it twice, so a glance across the titles says what just happened where. An error lights
//! it red, draining back to its state's colour.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use lazyrunner_core::run::app::{AppState, Status};

use super::Ui;
use super::motion::Spot;
use super::seg::{Seg, fill, on};
use super::theme::C;

/// How long a change shows: the fill and the drain, and the crash's two pulses.
const CHANGE: Duration = Duration::from_millis(400);
const PULSE_STEP: u128 = 100;
/// The share of an error's light that holds red before it drains.
const ERROR_HOLD: f64 = 0.5;

/// Where an app is, as its title shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Stopped,
    Starting,
    Up,
    Crashed,
}

impl Phase {
    pub fn of(state: &AppState) -> Self {
        match state.status {
            Status::Running if state.is_up() => Self::Up,
            Status::Running => Self::Starting,
            Status::Exited(_) | Status::Error(_) => Self::Crashed,
            Status::Stopped => Self::Stopped,
        }
    }

    fn color(self) -> C {
        match self {
            Self::Stopped => C::DarkGray,
            Self::Starting => C::Yellow,
            Self::Up => C::Green,
            Self::Crashed => C::Red,
        }
    }
}

/// Each app's phase, and its last change: from what, and when; and the end of the last error flash its title showed.
#[derive(Debug, Default)]
pub struct Phases {
    apps: HashMap<String, (Phase, Option<(Phase, Instant)>)>,
    errors: HashMap<String, Instant>,
}

/// Notes app `key`'s phase this frame: a change from the last frame is kept, to show.
pub fn note(ui: &mut Ui, key: &str, phase: Phase) {
    let now = ui.now;
    match ui.phases.apps.get_mut(key) {
        Some((was, change)) if *was != phase => {
            *change = Some((*was, now));
            *was = phase;
        }
        Some(_) => {}
        None => {
            ui.phases.apps.insert(key.to_string(), (phase, None));
        }
    }
}

/// Notes app `key`'s error flash this frame (`until`, column `index`): a new one - an error line, an alert, a crash -
/// lights its title, again while it still is, so a burst is one flash.
pub fn note_error(ui: &mut Ui, index: usize, key: &str, until: Option<Instant>) {
    let Some(until) = until.filter(|until| ui.now < *until) else { return };
    if ui.phases.errors.insert(key.to_string(), until) != Some(until) {
        ui.motion.flash(Spot::Errored(index), ui.now);
    }
}

fn moving(ui: &Ui) -> bool {
    ui.motion.enabled && !ui.reduced_motion
}

/// App `key`'s last change, while it shows: from what, to what, how far along (0 to 1), and how long ago.
fn change(ui: &Ui, key: &str) -> Option<(Phase, Phase, f64, Duration)> {
    let (now, Some((was, at))) = ui.phases.apps.get(key)? else { return None };
    let since = ui.now.saturating_duration_since(*at);
    (moving(ui) && since < CHANGE).then(|| (*was, *now, since.as_secs_f64() / CHANGE.as_secs_f64(), since))
}

/// Whether a title is changing.
pub fn animating(ui: &Ui) -> bool {
    ui.phases.apps.keys().any(|key| change(ui, key).is_some())
}

/// App `key`'s title chip, `text` in `fg` on `bg` as it is - or changing: filling yellow from the left as it starts
/// and green as it comes up, the colour it had draining from the right as it stops, pulsing as it crashes.
pub fn chip(ui: &Ui, key: &str, text: String, fg: C, bg: C) -> Vec<Seg> {
    let Some((was, now, progress, since)) = change(ui, key) else { return vec![on(text, fg, bg)] };
    match now {
        Phase::Up | Phase::Starting => fill(&text, progress, (C::Black, now.color()), (C::Black, was.color())),
        Phase::Stopped => fill(&text, 1.0 - progress, (C::Black, was.color()), (fg, bg)),
        Phase::Crashed if (since.as_millis() / PULSE_STEP).is_multiple_of(2) => vec![on(text, C::Black, C::White)],
        _ => vec![on(text, fg, bg)],
    }
}

/// A title chip an error just lit (`lit`, 1 to 0): red, white text, holding a moment, then draining from the right back
/// to `fg` on `bg`, its state's colours.
pub fn errored(text: &str, lit: f64, fg: C, bg: C) -> Vec<Seg> {
    fill(text, (lit / (1.0 - ERROR_HOLD)).min(1.0), (C::White, C::Red), (fg, bg))
}
