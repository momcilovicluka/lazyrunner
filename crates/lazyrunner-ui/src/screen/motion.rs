//! Motion: the short effects that answer what the user does - a flash where something happened, a box that opens
//! from its middle, column widths and scrolling that glide - and their timings, in one place. They are only drawn:
//! input never waits for one, a new one replaces the same one still running, and with reduced motion (or `enabled`
//! off, as the screen tests have it) there are none.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use lazyrunner_core::config::Toggle;

use super::mouse::Target;

/// A flash where something happened: long enough to be seen, short enough to never be waited for.
pub const FLASH: Duration = Duration::from_millis(220);
/// Copied lines stay lit a little longer: they show what was taken.
pub const COPY_FLASH: Duration = Duration::from_millis(450);
/// What an agent read stays outlined long enough to follow (follow mode).
pub const LOOK: Duration = Duration::from_secs(4);
/// A box opening from its middle.
pub const REVEAL: Duration = Duration::from_millis(120);
/// Column widths and scrolling moving to where they're going.
pub const GLIDE: Duration = Duration::from_millis(120);
/// The palette's selection bar slides a row at a time, this long a row; a longer jump than `BAR_ROWS` is cut.
pub const BAR_STEP: Duration = Duration::from_millis(40);
pub const BAR_ROWS: usize = 3;
/// After Enter the chosen keycap shows pressed for a frame, then the palette wipes closed.
pub const PRESS: Duration = Duration::from_millis(16);
pub const WIPE: Duration = Duration::from_millis(60);

/// The palette closing after Enter: shown pressed, then wiping (how much of it is left, 1 to 0).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Closing {
    Pressed,
    Wiping(f64),
}

/// Where a flash shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Spot {
    /// A key's hint in the panel, by the key pressed ("u", "U", "Space").
    Hint(String),
    /// A column's title (an action on that app, or the focus moving to it).
    Title(usize),
    /// Every column's title (an action on all the apps).
    Titles,
    /// An on/off setting, in the , menu and the panel.
    Setting(Toggle),
    /// Lines of an app's output (by their text) that were just copied.
    Copied { app: usize, lines: HashSet<String> },
    /// Lines of an app's output (by their text) an agent just read.
    Looked { app: usize, lines: HashSet<String> },
    /// Requests (by number) an agent just read.
    LookedRequests(HashSet<u64>),
    /// The search match just jumped to (the line's index, 0 = the oldest kept).
    Match { app: usize, line: usize },
    /// What was clicked.
    Clicked(Target),
}

impl Spot {
    fn lasts(&self) -> Duration {
        match self {
            Spot::Copied { .. } => COPY_FLASH,
            Spot::Looked { .. } | Spot::LookedRequests(_) => LOOK,
            _ => FLASH,
        }
    }
}

/// Where a glide started, where it goes, and when it set off.
#[derive(Clone, Debug)]
struct Glide<T> {
    from: T,
    to: T,
    at: Instant,
}

pub struct Motion {
    /// Off in the screen tests (their frames are taken at one frozen instant).
    pub enabled: bool,
    /// The reducedMotion setting, read at every frame: off too.
    pub reduced: bool,
    flashes: Vec<(Spot, Instant)>,
    /// The box shown last frame, and when the one showing now opened.
    shown: Option<&'static str>,
    opened: Option<Instant>,
    /// The column focused last frame: a new one pulses.
    focus: Option<usize>,
    widths: Option<(Vec<usize>, Glide<Vec<f64>>)>,
    scrolls: HashMap<usize, Glide<f64>>,
    bar: Option<Glide<usize>>,
    /// The palette Enter closed: its search, the chosen item, and when.
    closing: Option<(String, usize, Instant)>,
}

impl Default for Motion {
    fn default() -> Self {
        Self {
            enabled: true,
            reduced: false,
            flashes: Vec::new(),
            shown: None,
            opened: None,
            focus: None,
            widths: None,
            scrolls: HashMap::new(),
            bar: None,
            closing: None,
        }
    }
}

/// Fast at first, gentle at the end.
fn ease(progress: f64) -> f64 {
    let left = 1.0 - progress.clamp(0.0, 1.0);
    1.0 - left * left * left
}

fn progress(since: Instant, now: Instant, lasts: Duration) -> f64 {
    now.saturating_duration_since(since).as_secs_f64() / lasts.as_secs_f64()
}

impl Motion {
    fn on(&self) -> bool {
        self.enabled && !self.reduced
    }

    /// What was clicked a moment ago, while it shows pressed.
    pub fn pressed(&self, now: Instant) -> Option<Target> {
        self.flashes.iter().find_map(|(spot, at)| match spot {
            Spot::Clicked(target) if progress(*at, now, FLASH) < 1.0 => Some(*target),
            _ => None,
        })
    }

    /// Lights `spot` from `now` (again, if it still is).
    pub fn flash(&mut self, spot: Spot, now: Instant) {
        if !self.on() {
            return;
        }
        self.flashes.retain(|(other, _)| *other != spot);
        self.flashes.push((spot, now));
    }

    /// How lit `spot` is: 1 just after it flashed, falling to 0; none once it's over.
    pub fn lit(&self, spot: &Spot, now: Instant) -> Option<f64> {
        let (spot, at) = self.flashes.iter().find(|(other, _)| other == spot)?;
        let done = progress(*at, now, spot.lasts());
        (done < 1.0).then_some(1.0 - done)
    }

    /// The lines of app `app` that were just copied, while they're lit.
    pub fn copied(&self, app: usize, now: Instant) -> Option<&HashSet<String>> {
        self.flashes.iter().find_map(|(spot, at)| match spot {
            Spot::Copied { app: of, lines } if *of == app && progress(*at, now, COPY_FLASH) < 1.0 => Some(lines),
            _ => None,
        })
    }

    /// Outlines what an agent read - with reduced motion too: it shows where the agent looks, it isn't a flourish.
    pub fn look(&mut self, spot: Spot, now: Instant) {
        self.flashes.retain(|(other, _)| std::mem::discriminant(other) != std::mem::discriminant(&spot));
        self.flashes.push((spot, now));
    }

    /// The lines of `app` an agent read, while they're outlined.
    pub fn looked(&self, app: usize, now: Instant) -> Option<&HashSet<String>> {
        self.flashes.iter().find_map(|(spot, at)| match spot {
            Spot::Looked { app: of, lines } if *of == app && progress(*at, now, LOOK) < 1.0 => Some(lines),
            _ => None,
        })
    }

    /// The requests an agent read, while they're outlined.
    pub fn looked_requests(&self, now: Instant) -> Option<&HashSet<u64>> {
        self.flashes.iter().find_map(|(spot, at)| match spot {
            Spot::LookedRequests(read) if progress(*at, now, LOOK) < 1.0 => Some(read),
            _ => None,
        })
    }

    /// Whether a title is lit: its own flash, or every title's.
    pub fn title_lit(&self, index: usize, now: Instant) -> bool {
        self.lit(&Spot::Title(index), now).or_else(|| self.lit(&Spot::Titles, now)).is_some()
    }

    /// Notes the box shown this frame (none: the columns); a box that just appeared opens from now. The focused
    /// column too: one that just got the focus pulses.
    pub fn frame(&mut self, shown: Option<&'static str>, focus: usize, now: Instant) {
        if shown != self.shown {
            self.opened = shown.map(|_| now);
            self.shown = shown;
            self.bar = None; // a list that opens starts with its bar where it is
        }
        if self.focus.is_some_and(|before| before != focus) {
            self.flash(Spot::Title(focus), now);
        }
        self.focus = Some(focus);
        self.flashes.retain(|(spot, at)| progress(*at, now, spot.lasts()) < 1.0);
    }

    /// How far the box that's showing has opened: 0 just opened, 1 open.
    pub fn revealed(&self, now: Instant) -> f64 {
        match self.opened {
            Some(at) if self.on() => ease(progress(at, now, REVEAL)),
            _ => 1.0,
        }
    }

    /// The column widths to draw for these columns (`keys`) going to `target`: from where they were towards it when
    /// it changed, the same total each frame. Another set of columns (zoom, more or fewer apps) starts where it is.
    pub fn widths(&mut self, keys: &[usize], target: Vec<usize>, now: Instant) -> Vec<usize> {
        let as_floats = |widths: &[usize]| widths.iter().map(|width| *width as f64).collect::<Vec<_>>();
        let same_columns = self.widths.as_ref().is_some_and(|(shown_for, _)| shown_for == keys);
        let moved = self.widths.as_ref().is_some_and(|(_, glide)| glide.to != as_floats(&target));
        if !self.on() || !same_columns {
            self.widths = Some((keys.to_vec(), Glide { from: as_floats(&target), to: as_floats(&target), at: now }));
            return target;
        }
        if moved {
            let current = self.width_floats(now);
            self.widths = Some((keys.to_vec(), Glide { from: current, to: as_floats(&target), at: now }));
        }
        let shown = self.width_floats(now);
        let mut widths: Vec<usize> = shown.iter().map(|width| (width.round() as usize).max(1)).collect();
        // Rounding mustn't change the total: the last column takes the difference.
        let total: usize = target.iter().sum();
        let others: usize = widths.iter().rev().skip(1).sum();
        if let Some(last) = widths.last_mut() {
            *last = total.saturating_sub(others).max(1);
        }
        widths
    }

    fn width_floats(&self, now: Instant) -> Vec<f64> {
        let Some((_, glide)) = &self.widths else { return Vec::new() };
        let done = ease(progress(glide.at, now, GLIDE));
        glide.from.iter().zip(&glide.to).map(|(from, to)| from + (to - from) * done).collect()
    }

    /// The scroll to draw for column `app` going to `target` (lines back from the newest).
    pub fn scroll(&mut self, app: usize, target: usize, now: Instant) -> usize {
        if !self.on() {
            return target;
        }
        let target = target as f64;
        let shown = |glide: &Glide<f64>| glide.from + (glide.to - glide.from) * ease(progress(glide.at, now, GLIDE));
        let glide = self.scrolls.entry(app).or_insert(Glide { from: target, to: target, at: now });
        if glide.to != target {
            *glide = Glide { from: shown(glide), to: target, at: now };
        }
        shown(glide).round().max(0.0) as usize
    }

    /// The row to draw the palette's selection bar on, on its way to `target`: a row every `BAR_STEP`, a jump of more
    /// than `BAR_ROWS` cut.
    pub fn bar(&mut self, target: usize, now: Instant) -> usize {
        if !self.on() {
            return target;
        }
        match &self.bar {
            Some(glide) if glide.to == target => {}
            Some(_) => {
                let current = self.bar_at(now);
                let from = if current.abs_diff(target) > BAR_ROWS { target } else { current };
                self.bar = Some(Glide { from, to: target, at: now });
            }
            None => self.bar = Some(Glide { from: target, to: target, at: now }),
        }
        self.bar_at(now)
    }

    fn bar_at(&self, now: Instant) -> usize {
        let Some(glide) = &self.bar else { return 0 };
        let rows = (now.saturating_duration_since(glide.at).as_secs_f64() / BAR_STEP.as_secs_f64()) as usize;
        if glide.from < glide.to {
            (glide.from + rows).min(glide.to)
        } else {
            glide.from.saturating_sub(rows).max(glide.to)
        }
    }

    /// Enter closed the palette on item `index` of what `query` found: it shows that, pressed, then wipes away.
    pub fn close_palette(&mut self, query: &str, index: usize, now: Instant) {
        if self.on() {
            self.closing = Some((query.to_string(), index, now));
        }
    }

    /// The palette closing, while it does: its search, the chosen item, and how far along.
    pub fn palette_closing(&self, now: Instant) -> Option<(String, usize, Closing)> {
        let (query, index, at) = self.closing.as_ref().filter(|_| self.on())?;
        let elapsed = now.saturating_duration_since(*at);
        let phase = if elapsed < PRESS {
            Closing::Pressed
        } else if elapsed < PRESS + WIPE {
            Closing::Wiping(1.0 - (elapsed - PRESS).as_secs_f64() / WIPE.as_secs_f64())
        } else {
            return None;
        };
        Some((query.clone(), *index, phase))
    }

    /// Whether something is still moving (the next frame differs).
    pub fn active(&self, now: Instant) -> bool {
        if !self.on() {
            return false;
        }
        let gliding = |at: Instant| progress(at, now, GLIDE) < 1.0;
        !self.flashes.is_empty()
            || self.revealed(now) < 1.0
            || self.widths.as_ref().is_some_and(|(_, glide)| gliding(glide.at))
            || self.scrolls.values().any(|glide| gliding(glide.at))
            || self.bar.as_ref().is_some_and(|glide| self.bar_at(now) != glide.to)
            || self.palette_closing(now).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn later(start: Instant, ms: u64) -> Instant {
        start + Duration::from_millis(ms)
    }

    #[test]
    fn a_flash_fades_and_ends_and_flashing_again_starts_over() {
        let (mut motion, start) = (Motion::default(), Instant::now());
        motion.flash(Spot::Hint("u".into()), start);
        assert_eq!(motion.lit(&Spot::Hint("u".into()), start), Some(1.0));
        let half = motion.lit(&Spot::Hint("u".into()), later(start, 110)).unwrap();
        assert!((half - 0.5).abs() < 0.01, "{half}");
        assert_eq!(motion.lit(&Spot::Hint("u".into()), later(start, 220)), None);
        motion.flash(Spot::Hint("u".into()), later(start, 200));
        assert!(motion.lit(&Spot::Hint("u".into()), later(start, 300)).is_some(), "from the new press");
        assert!(motion.active(later(start, 300)));
        motion.frame(None, 0, later(start, 500));
        assert!(!motion.active(later(start, 500)), "over, and forgotten");
    }

    #[test]
    fn a_box_opens_from_now_and_the_focus_moving_pulses_the_new_title() {
        let (mut motion, start) = (Motion::default(), Instant::now());
        motion.frame(None, 0, start);
        assert!(motion.lit(&Spot::Title(0), start).is_none(), "the first frame's focus doesn't pulse");
        motion.frame(Some("menu"), 1, later(start, 10));
        assert_eq!(motion.revealed(later(start, 10)), 0.0);
        assert!(motion.revealed(later(start, 70)) > 0.5, "fast at first");
        assert_eq!(motion.revealed(later(start, 130)), 1.0);
        assert!(motion.title_lit(1, later(start, 20)));
        motion.flash(Spot::Titles, later(start, 20));
        assert!(motion.title_lit(3, later(start, 30)), "every title");
    }

    #[test]
    fn widths_glide_keeping_their_total_and_another_set_of_columns_starts_where_it_is() {
        let (mut motion, start) = (Motion::default(), Instant::now());
        assert_eq!(motion.widths(&[0, 1], vec![50, 50], start), [50, 50]);
        assert_eq!(motion.widths(&[0, 1], vec![70, 30], later(start, 1)), [50, 50]);
        let middle = motion.widths(&[0, 1], vec![70, 30], later(start, 40));
        assert!(middle[0] > 50 && middle[0] < 70 && middle.iter().sum::<usize>() == 100, "{middle:?}");
        assert_eq!(motion.widths(&[0, 1], vec![70, 30], later(start, 200)), [70, 30]);
        assert_eq!(motion.widths(&[1], vec![100], later(start, 210)), [100], "zoomed: no glide");
        motion.enabled = false;
        assert_eq!(motion.widths(&[0, 1], vec![10, 90], later(start, 220)), [10, 90]);
    }

    #[test]
    fn the_selection_bar_slides_a_row_at_a_time_and_a_long_jump_is_cut() {
        let (mut motion, start) = (Motion::default(), Instant::now());
        assert_eq!(motion.bar(0, start), 0);
        assert_eq!(motion.bar(2, later(start, 1)), 0);
        assert_eq!(motion.bar(2, later(start, 41)), 1, "40 ms a row");
        assert_eq!(motion.bar(2, later(start, 81)), 2);
        assert!(!motion.active(later(start, 81)));
        assert_eq!(motion.bar(9, later(start, 100)), 9, "more than 3 rows: cut");
        assert_eq!(motion.bar(7, later(start, 110)), 9);
        assert_eq!(motion.bar(7, later(start, 151)), 8, "back up a row at a time");
    }

    #[test]
    fn the_palette_closes_pressed_for_a_frame_then_wipes() {
        let (mut motion, start) = (Motion::default(), Instant::now());
        motion.close_palette("sta", 2, start);
        assert_eq!(motion.palette_closing(start), Some(("sta".into(), 2, Closing::Pressed)));
        let Some((_, _, Closing::Wiping(left))) = motion.palette_closing(later(start, 46)) else { panic!() };
        assert!((left - 0.5).abs() < 0.01, "{left}");
        assert_eq!(motion.palette_closing(later(start, 80)), None);
        assert!(!motion.active(later(start, 80)));
    }

    #[test]
    fn scrolling_glides_to_its_line() {
        let (mut motion, start) = (Motion::default(), Instant::now());
        assert_eq!(motion.scroll(0, 0, start), 0);
        assert_eq!(motion.scroll(0, 30, later(start, 1)), 0);
        let middle = motion.scroll(0, 30, later(start, 50));
        assert!(middle > 0 && middle < 30, "{middle}");
        assert_eq!(motion.scroll(0, 30, later(start, 200)), 30);
        assert!(!motion.active(later(start, 200)));
    }
}
