//! An app that crashed starts again by itself (#99), when its profile says `"restart": "onCrash"`: after a wait that
//! doubles each time - 1 s, 2 s, 4 s, at most 30 s - which starts over once it has stayed up a minute. A crash loop
//! is held: after 3 crashes within 2 minutes it waits for `u`. A stop you asked for (`s` too, while it waits), a
//! restart or a branch change never counts, nor a clean exit (code 0).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use super::Controller;
use crate::output::{Hue, Level};
use crate::run::format_duration;

const FIRST_WAIT: Duration = Duration::from_secs(1);
const MOST_WAIT: Duration = Duration::from_secs(30);
/// Up this long, its crashes are forgotten.
const STAYS_UP: Duration = Duration::from_secs(60);
/// This many crashes within `LOOP_WINDOW` are a crash loop.
pub const LOOP_CRASHES: usize = 3;
const LOOP_WINDOW: Duration = Duration::from_secs(120);

/// An app's crashes and its restarts after them.
#[derive(Clone, Debug, Default)]
pub struct Recovery {
    /// Its crashes within `LOOP_WINDOW`, oldest first.
    pub crashes: Vec<Instant>,
    /// How many times it started again since it last stayed up.
    pub restarts: usize,
    /// When it starts again, and how long it waits in all.
    pub next: Option<(Instant, Duration)>,
    /// Held after a crash loop, until `u`, and since when (its border pulses).
    pub held: bool,
    pub held_at: Option<Instant>,
}

impl Recovery {
    /// While it waits to start again: how long is left, and the share of the wait left (1 to 0).
    pub fn left(&self, now: Instant) -> Option<(Duration, f64)> {
        let (at, wait) = self.next?;
        let left = at.saturating_duration_since(now);
        Some((left, left.as_secs_f64() / wait.as_secs_f64().max(f64::EPSILON)))
    }

    /// As its title and get_status say it: `restarting in 4s (2 of 3)`, `held after 3 crashes`, `restarted 2×`.
    pub fn said(&self, now: Instant) -> String {
        if self.held {
            return format!("held after {} crashes", self.crashes.len());
        }
        match self.left(now) {
            Some((left, _)) => format!(
                "restarting in {} ({} of {LOOP_CRASHES})",
                format_duration(left.as_secs_f64().ceil()),
                self.crashes.len()
            ),
            None => format!("restarted {}×", self.restarts),
        }
    }
}

pub(super) type Recoveries = HashMap<String, Recovery>;

/// A crash this session: when, its exit code, and the last error the app printed before it.
#[derive(Clone, Debug)]
pub struct Crash {
    pub at: Instant,
    pub code: Option<i32>,
    pub error: Option<String>,
}

/// Each app's crashes this session, oldest first, at most `MOST_KEPT`.
pub(super) type History = HashMap<String, Vec<Crash>>;
const MOST_KEPT: usize = 20;

impl Controller {
    /// An app's crashes and restarts, while it has any.
    pub fn recovery(&self, key: &str) -> Option<&Recovery> {
        self.recoveries.get(key)
    }

    /// Whether an app waits to start again after a crash.
    pub fn restarting(&self) -> bool {
        self.recoveries.values().any(|recovery| recovery.next.is_some())
    }

    /// App `key`'s crashes this session, oldest first.
    pub fn crashes_of(&self, key: &str) -> &[Crash] {
        self.crash_history.get(key).map_or(&[], Vec::as_slice)
    }

    /// Keeps a crash of app `key`, with the last error it printed.
    fn note_crash(&mut self, key: &str, code: Option<i32>, now: Instant) {
        let error = self.states.iter().find(|state| state.key == key).and_then(|state| {
            state
                .output
                .newest_first(0)
                .find(|line| line.level == Level::Error)
                .map(|line| line.text.trim().to_string())
        });
        let history = self.crash_history.entry(key.to_string()).or_default();
        history.push(Crash { at: now, code, error });
        if history.len() > MOST_KEPT {
            history.remove(0);
        }
    }

    /// After app `key` crashed (exited by itself, not with 0): it's kept in its crashes; it starts again after a wait,
    /// or is held after a loop.
    pub(super) fn after_crash(&mut self, key: &str, code: Option<i32>, now: Instant) {
        let building = self.building.as_ref().is_some_and(|job| job.key == key);
        if code == Some(0) || building {
            return;
        }
        self.note_crash(key, code, now);
        let Some(app) = self.app(key).filter(|app| app.restart_on_crash) else { return };
        let name = app.name.clone();
        let recovery = self.recoveries.entry(key.to_string()).or_default();
        recovery.crashes.retain(|at| now.saturating_duration_since(*at) < LOOP_WINDOW);
        recovery.crashes.push(now);
        let crashes = recovery.crashes.len();
        if crashes >= LOOP_CRASHES {
            let span = now.saturating_duration_since(recovery.crashes[0]).as_secs_f64();
            (recovery.held, recovery.next, recovery.held_at) = (true, None, Some(now));
            let held = format!("{name} crashed {crashes} times in {} - held (u starts it)", format_duration(span));
            self.add_log(format!("✗ {held}"), Hue::Red);
            self.notifications.push((format!("{name} is crash-looping"), held));
            return;
        }
        let wait = FIRST_WAIT.saturating_mul(1 << recovery.restarts.min(5)).min(MOST_WAIT);
        recovery.restarts += 1;
        recovery.next = Some((now + wait, wait));
        let wait = format_duration(wait.as_secs_f64());
        self.add_log(format!("↻ {name} starts again in {wait} ({crashes} of {LOOP_CRASHES} crashes)"), Hue::Yellow);
    }

    /// Starts again what waited long enough; forgets the crashes of an app that has stayed up a while.
    pub(super) fn tick_recoveries(&mut self, now: Instant) {
        let due: Vec<String> = self
            .recoveries
            .iter()
            .filter(|(_, recovery)| recovery.next.is_some_and(|(at, _)| now >= at))
            .map(|(key, _)| key.clone())
            .collect();
        for key in due {
            let Some(mut recovery) = self.recoveries.remove(&key) else { continue };
            recovery.next = None;
            if !self.is_running(&key) {
                let name = self.app(&key).map(|app| app.name.clone()).unwrap_or_default();
                self.add_log(format!("↻ Starting {name} again after its crash"), Hue::Cyan);
                self.start(Some(std::slice::from_ref(&key)), false);
            }
            self.recoveries.insert(key, recovery);
        }
        let states = &self.states;
        self.recoveries.retain(|key, recovery| {
            let stayed_up = states.iter().find(|state| state.key == *key).is_some_and(|state| {
                state.is_up() && state.started_at.is_some_and(|at| now.saturating_duration_since(at) >= STAYS_UP)
            });
            recovery.held || recovery.next.is_some() || !stayed_up
        });
    }

    /// A stop asked for (or a start by hand) ends a wait and forgets the crashes.
    pub(super) fn forget_crashes(&mut self, keys: &[String]) {
        self.recoveries.retain(|key, _| !keys.contains(key));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run::app::Status;
    use crate::testing::TempDir;

    #[tokio::test]
    async fn a_crash_waits_longer_each_time_and_a_loop_is_held_until_a_stop_forgets_it() {
        let temp = TempDir::new("crashes");
        let (mut controller, _rx) = super::super::tests::controller(&temp, serde_json::json!({}));
        controller.apps[0].restart_on_crash = true;
        let key = controller.apps[0].key.clone();
        let other = controller.apps[1].key.clone();
        let start = Instant::now();
        controller.after_crash(&other, Some(1), start);
        assert!(controller.recovery(&other).is_none(), "its profile doesn't ask for it");
        controller.after_crash(&key, Some(0), start);
        assert!(controller.recovery(&key).is_none(), "a clean exit isn't a crash");
        assert_eq!(controller.crashes_of(&other).len(), 1, "a crash is kept even without a restart");
        controller.after_crash(&key, Some(1), start);
        assert_eq!(controller.recovery(&key).unwrap().said(start), "restarting in 1s (1 of 3)");
        let later = start + Duration::from_secs(5);
        controller.after_crash(&key, None, later);
        assert_eq!(controller.recovery(&key).unwrap().said(later), "restarting in 2s (2 of 3)", "twice as long");
        controller.after_crash(&key, Some(1), start + Duration::from_secs(9));
        let held = controller.recovery(&key).unwrap();
        assert!(held.held && held.next.is_none(), "the third within two minutes holds it");
        assert!(controller.log.back().unwrap().text.ends_with("crashed 3 times in 9s - held (u starts it)"));
        assert!(controller.notifications.last().is_some_and(|(title, _)| title.ends_with("is crash-looping")));
        controller.stop(Some(std::slice::from_ref(&key)), true);
        assert!(controller.recovery(&key).is_none(), "a stop forgets it");
    }

    #[tokio::test]
    async fn an_app_that_stays_up_a_minute_after_its_restart_has_its_crashes_forgotten() {
        let temp = TempDir::new("crashes-up");
        let (mut controller, _rx) = super::super::tests::controller(&temp, serde_json::json!({}));
        controller.apps[0].restart_on_crash = true;
        let key = controller.apps[0].key.clone();
        let start = Instant::now();
        controller.after_crash(&key, Some(1), start);
        controller.recoveries.get_mut(&key).unwrap().next = None; // it started again
        let state = &mut controller.states[0];
        (state.status, state.generation, state.up_generation, state.started_at) =
            (Status::Running, 1, Some(1), Some(start));
        controller.tick_recoveries(start + Duration::from_secs(30));
        assert!(controller.recovery(&key).is_some(), "not a minute yet");
        controller.tick_recoveries(start + STAYS_UP);
        assert!(controller.recovery(&key).is_none());
    }
}
