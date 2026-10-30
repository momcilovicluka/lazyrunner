//! The rail's motion: an ember from the bracket to the mark of a new error that lands off-screen - the mark waits, dim,
//! until the ember eases into it, then settles in (a burst sends one, at most one every 750 ms, its tail longer for
//! each error it carries) - one shimmer as `G` snaps the view back to live, down the map from where the view was to the
//! bottom, where it hands over to the bracket's `▼` - and one pulse, bright then fading, on the row of a search's match
//! when the search jumps there. In the theme's focus colour, which every theme sets apart from its background. None
//! with reduced motion.

use std::cell::RefCell;
use std::collections::HashMap;
use std::time::{Duration, Instant};

use super::Ui;
use super::seg::{Seg, on, seg};
use super::theme::C;

/// How long an ember takes to a new error's mark, and how soon another can follow.
const COMET: Duration = Duration::from_millis(450);
const COMET_EVERY: Duration = Duration::from_millis(750);
/// How long the mark it lands on stands out before it is just a mark.
const SETTLE: Duration = Duration::from_millis(300);
/// The longest tail an ember gets, carrying several errors.
const LONGEST_TAIL: usize = 4;
/// How long the shimmer takes down the map, how long the `▼` it hands over to glows, and the rows of its trail.
const SHIMMER: Duration = Duration::from_millis(350);
const SHIMMER_LAND: Duration = Duration::from_millis(200);
const SHIMMER_TRAIL: usize = 2;
const PULSE: Duration = Duration::from_millis(400);
/// How much of the pulse is bright before it fades.
const PULSE_BRIGHT: f64 = 0.5;

/// What a column's rail showed last, and what moves on it.
#[derive(Default)]
struct Column {
    errors: u64,
    scroll: usize,
    matched: Option<usize>,
    /// The first row of the bracket: where a shimmer starts from when the view snaps back to live.
    top: usize,
    comet: Option<Comet>,
    /// When the shimmer started, and from which row.
    shimmer: Option<(Instant, usize)>,
    pulse: Option<(usize, Instant)>,
}

struct Comet {
    from: usize,
    to: usize,
    started: Instant,
    carried: usize,
}

/// Each column's, by its app's key.
#[derive(Default)]
pub struct Effects(RefCell<HashMap<String, Column>>);

/// What a column's rail shows in this frame: its app's errors, how far it's scrolled back, its search's match, the
/// rows of the newest error's mark and of the match, the rows on screen, the rail's height, and whether it's the map
/// alone (no bracket).
pub struct Now {
    pub narrow: bool,
    pub errors: u64,
    pub scroll: usize,
    pub matched: Option<usize>,
    pub error_row: Option<usize>,
    pub match_row: Option<usize>,
    pub top: usize,
    pub bottom: usize,
    pub height: usize,
}

/// What moves on a rail in this frame.
#[derive(Default)]
pub struct Moving {
    /// The ember's rows, its head first.
    comet: Vec<usize>,
    /// The row the ember flies to: its mark waits, dim, until it lands.
    landing: Option<usize>,
    /// The row it just landed on: its mark settles in.
    landed: Option<usize>,
    /// The map's row the shimmer's head is on.
    shimmer: Option<usize>,
    /// The bottom row, once the shimmer hands over to the `▼` there.
    live_edge: Option<usize>,
    narrow: bool,
    /// The pulsing row, and how far it is through (0 to 1).
    pulse: Option<(usize, f64)>,
}

impl Moving {
    /// Row `at`'s bracket cell: the `▼` glows as the shimmer hands over to it.
    pub fn paint_edge(&self, at: usize, edge: Seg) -> Seg {
        if self.live_edge == Some(at) { on(edge.text, C::FocusText, C::FocusBack) } else { edge }
    }

    /// Row `at`'s map cell as what moves on it shows it.
    pub fn paint(&self, at: usize, cell: Seg) -> Seg {
        match self.comet.iter().position(|ember| *ember == at) {
            Some(0) => return seg("◉", C::Red),
            Some(_) => return seg("·", C::Red),
            None => {}
        }
        if self.landing == Some(at) {
            return seg("╎", C::DarkGray);
        }
        if self.landed == Some(at) {
            return on("◆", C::Red, C::FocusBack);
        }
        match self.pulse {
            Some((row, through)) if row == at && through < PULSE_BRIGHT => {
                Seg { fg: C::FocusText, bg: Some(C::FocusBack), ..cell }
            }
            Some((row, _)) if row == at => Seg { fg: C::FocusBack, ..cell },
            _ if self.shimmer == Some(at) || (self.narrow && self.live_edge == Some(at)) => {
                Seg { fg: C::FocusText, bg: Some(C::FocusBack), ..cell }
            }
            _ if self.shimmer.is_some_and(|head| at < head && head - at <= SHIMMER_TRAIL) => {
                Seg { fg: C::FocusBack, ..cell }
            }
            _ => cell,
        }
    }
}

/// What moves on the rail of app `key` now, given what changed since its last frame.
pub fn moving(ui: &Ui, key: &str, now: Now) -> Moving {
    let mut all = ui.rail_motion.0.borrow_mut();
    let column = all.entry(key.to_string()).or_default();
    let new_errors = now.errors.saturating_sub(column.errors);
    let back_to_live = column.scroll > 0 && now.scroll == 0;
    let jumped = now.matched.is_some() && now.matched != column.matched;
    let was_at = column.top;
    (column.errors, column.scroll, column.matched, column.top) = (now.errors, now.scroll, now.matched, now.top);
    if ui.reduced_motion {
        (column.comet, column.shimmer, column.pulse) = (None, None, None);
        return Moving::default();
    }
    let at = ui.now;
    let off_screen = now.error_row.filter(|row| !(now.top..now.bottom).contains(row));
    if let Some(to) = off_screen.filter(|_| new_errors > 0 && now.scroll > 0) {
        let carried = usize::try_from(new_errors).unwrap_or(usize::MAX);
        match &mut column.comet {
            Some(comet) if at.saturating_duration_since(comet.started) < COMET_EVERY => {
                comet.carried = comet.carried.saturating_add(carried);
            }
            _ => {
                let from = if to >= now.bottom { now.bottom.saturating_sub(1) } else { now.top };
                column.comet = Some(Comet { from, to, started: at, carried });
            }
        }
    }
    if back_to_live {
        column.shimmer = Some((at, was_at));
    }
    if let Some(row) = now.match_row.filter(|_| jumped) {
        column.pulse = Some((row, at));
    }
    let mut moving = Moving { narrow: now.narrow, ..Moving::default() };
    if let Some(comet) = &column.comet {
        let elapsed = at.saturating_duration_since(comet.started);
        if elapsed < COMET {
            moving.comet = embers(comet, through(elapsed, COMET));
            moving.landing = Some(comet.to);
        } else if elapsed < COMET + SETTLE {
            moving.landed = Some(comet.to);
        }
    }
    if let Some((started, from)) = column.shimmer {
        let elapsed = at.saturating_duration_since(started);
        let last = now.height.saturating_sub(1);
        if elapsed < SHIMMER {
            let rows = last.saturating_sub(from) as f64;
            moving.shimmer = Some((from + (rows * ease(through(elapsed, SHIMMER))).round() as usize).min(last));
        } else if elapsed < SHIMMER + SHIMMER_LAND {
            moving.live_edge = Some(last);
        } else {
            column.shimmer = None;
        }
    }
    if let Some((row, started)) = column.pulse {
        let elapsed = at.saturating_duration_since(started);
        if elapsed < PULSE {
            moving.pulse = Some((row, through(elapsed, PULSE)));
        } else {
            column.pulse = None;
        }
    }
    moving
}

/// How far through `length` `elapsed` is, 0 to 1.
fn through(elapsed: Duration, length: Duration) -> f64 {
    (elapsed.as_secs_f64() / length.as_secs_f64()).clamp(0.0, 1.0)
}

/// `progress` eased out: quick at first, slowing into the end, as lazyrunner's glides do.
fn ease(progress: f64) -> f64 {
    1.0 - (1.0 - progress).powi(3)
}

/// The ember's rows `progress` into its flight, its head first.
fn embers(comet: &Comet, progress: f64) -> Vec<usize> {
    let (from, to) = (comet.from as f64, comet.to as f64);
    let head = (from + (to - from) * ease(progress)).round() as usize;
    (0..=comet.carried.min(LONGEST_TAIL))
        .map(|back| {
            if comet.to >= comet.from {
                head.saturating_sub(back).max(comet.from)
            } else {
                (head + back).min(comet.from)
            }
        })
        .collect()
}

/// Whether something moves on a rail: frames keep coming until it's done.
pub fn animating(ui: &Ui) -> bool {
    let now = ui.now;
    ui.rail_motion.0.borrow().values().any(|column| {
        column.comet.as_ref().is_some_and(|comet| now.saturating_duration_since(comet.started) < COMET + SETTLE)
            || column.shimmer.is_some_and(|(at, _)| now.saturating_duration_since(at) < SHIMMER + SHIMMER_LAND)
            || column.pulse.is_some_and(|(_, at)| now.saturating_duration_since(at) < PULSE)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_ember_eases_into_its_mark_with_a_tail_for_what_it_carries() {
        let comet = Comet { from: 2, to: 12, started: Instant::now(), carried: 2 };
        assert_eq!(embers(&comet, 0.0), [2, 2, 2]);
        assert_eq!(embers(&comet, 0.5)[0], 11, "most of the way at half time: it slows into the mark");
        assert_eq!(embers(&comet, 1.0), [12, 11, 10]);
    }

    #[test]
    fn the_mark_waits_for_the_ember_then_settles_in_and_a_pulse_fades() {
        let mark = seg("█", C::Red);
        let flying = Moving { comet: vec![5, 4], landing: Some(9), ..Moving::default() };
        assert_eq!(flying.paint(5, mark.clone()).text, "◉");
        assert_eq!(flying.paint(4, mark.clone()).text, "·");
        assert_eq!(flying.paint(9, mark.clone()).text, "╎", "dim until the ember lands");
        let landed = Moving { landed: Some(9), ..Moving::default() };
        assert_eq!(landed.paint(9, mark.clone()).bg, Some(C::FocusBack));
        let pulse = |through| Moving { pulse: Some((3, through)), ..Moving::default() }.paint(3, mark.clone());
        assert_eq!(pulse(0.2).bg, Some(C::FocusBack), "bright first");
        assert_eq!((pulse(0.8).bg, pulse(0.8).fg), (None, C::FocusBack), "then fading");
    }

    #[test]
    fn the_shimmer_runs_down_the_map_with_a_trail_then_lights_the_live_edge() {
        let tick = seg("╎", C::DarkGray);
        let running = Moving { shimmer: Some(6), ..Moving::default() };
        assert_eq!(running.paint(6, tick.clone()).bg, Some(C::FocusBack), "its head");
        assert_eq!(running.paint(5, tick.clone()).fg, C::FocusBack, "its trail");
        assert_eq!(running.paint(3, tick.clone()).fg, C::DarkGray, "behind the trail, as it was");
        let landed = Moving { live_edge: Some(9), ..Moving::default() };
        assert_eq!(landed.paint_edge(9, seg("▼", C::DarkGray)).bg, Some(C::FocusBack), "the ▼ glows");
        assert_eq!(landed.paint(9, tick.clone()).bg, None, "beside it, the map stays");
        let narrow = Moving { live_edge: Some(9), narrow: true, ..Moving::default() };
        assert_eq!(narrow.paint(9, tick).bg, Some(C::FocusBack), "with no bracket, the map's last row glows");
    }
}
