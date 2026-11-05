//! A column's title as its app changes state (not with reduced motion): coming up fills its chip green from the left,
//! stopping drains the colour it had from the right, and a crash pulses it twice - so a glance across the titles says
//! what just happened where.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use lazyrunner_core::run::app::{AppState, Status};
use unicode_width::UnicodeWidthStr;

use super::Ui;
use super::seg::{Seg, on, take_cells};
use super::theme::C;

/// How long a change shows: the fill and the drain, and the crash's two pulses.
const CHANGE: Duration = Duration::from_millis(400);
const PULSE_STEP: u128 = 100;

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

/// Each app's phase, and its last change: from what, and when.
#[derive(Debug, Default)]
pub struct Phases {
    apps: HashMap<String, (Phase, Option<(Phase, Instant)>)>,
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

/// The first `cells` cells of `text`, and the rest.
fn split(text: &str, cells: usize) -> (String, String) {
    let first = take_cells(text, cells);
    let rest = text[first.len()..].to_string();
    (first, rest)
}

/// App `key`'s title chip, `text` in `fg` on `bg` as it is - or changing: filling green from the left as it comes up,
/// the colour it had draining from the right as it stops, pulsing as it crashes.
pub fn chip(ui: &Ui, key: &str, text: String, fg: C, bg: C) -> Vec<Seg> {
    let Some((was, now, progress, since)) = change(ui, key) else { return vec![on(text, fg, bg)] };
    let cells = text.width();
    match now {
        Phase::Up => {
            let (filled, rest) = split(&text, (cells as f64 * progress).ceil() as usize);
            vec![on(filled, C::Black, C::Green), on(rest, C::Black, was.color())]
        }
        Phase::Stopped => {
            let (kept, rest) = split(&text, (cells as f64 * (1.0 - progress)).round() as usize);
            vec![on(kept, C::Black, was.color()), on(rest, fg, bg)]
        }
        Phase::Crashed if (since.as_millis() / PULSE_STEP).is_multiple_of(2) => vec![on(text, C::Black, C::White)],
        _ => vec![on(text, fg, bg)],
    }
}
