//! What a change affects (#78), kept fresh while someone looks: the tests view and the agent ask, a save or a whole
//! run makes it due again, and git is asked in the background.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use super::Controller;
use crate::output::Hue;
use crate::run::impact::{self, Baseline, Impact};
use crate::run::suites::Runner;

/// How long after the last save it's looked at again: a formatter pass or a checkout is one look.
const SETTLE: Duration = Duration::from_millis(700);
/// How long one ask keeps it fresh: the tests view asks each frame it's open.
const WANTED_FOR: Duration = Duration::from_secs(3);
/// Looked at again this often while wanted, for changes no watcher sees (a commit, a checkout, a web app's save).
const REFRESH: Duration = Duration::from_secs(10);

#[derive(Default)]
pub struct Impacts {
    /// Each suite's, by key, once looked at.
    found: Option<HashMap<String, Impact>>,
    running: bool,
    due: Option<Instant>,
    wanted_until: Option<Instant>,
    at: Option<Instant>,
}

/// What [`impact::assess`] needs: the root, the .NET sources' folder, and each suite with its last whole run's base.
type Inputs = (PathBuf, String, Vec<(String, Runner, Option<Baseline>)>);

impl Controller {
    /// The tests view is open (or the agent asks): keep what changes affect fresh for a while.
    pub fn want_impact(&mut self) {
        let now = (self.now)();
        self.impacts.wanted_until = Some(now + WANTED_FOR);
        if self.impacts.found.is_none() && self.impacts.due.is_none() {
            self.impacts.due = Some(now);
        }
    }

    /// What changes since its last whole run mean for a suite, once looked at.
    pub fn impact(&self, key: &str) -> Option<&Impact> {
        self.impacts.found.as_ref()?.get(key)
    }

    /// Whether what changes affect is known yet.
    pub fn impacts_known(&self) -> bool {
        self.impacts.found.is_some()
    }

    /// The suites that run for what changed (and those it can't be told for), in the tests view's order.
    pub fn affected(&self) -> Vec<String> {
        let Some(found) = &self.impacts.found else { return Vec::new() };
        self.suites
            .iter()
            .flatten()
            .filter(|suite| !suite.untested && found.get(&suite.key).is_some_and(Impact::runs))
            .map(|suite| suite.key.clone())
            .collect()
    }

    /// Alt+r in the tests view, and the agent's `affected`: runs the suites what changed affects, looked at now.
    pub fn run_affected(&mut self) {
        self.assess_now();
        let keys = self.affected();
        if keys.is_empty() {
            self.add_log("Nothing changed since the last runs.", Hue::DarkGray);
            return;
        }
        self.run_tests(&keys, false);
    }

    /// Looks at what changes affect now, waiting for git.
    pub fn assess_now(&mut self) {
        let (root, source_dir, suites) = self.impact_inputs();
        let found = impact::assess(&root, &source_dir, &suites);
        self.impact_done(found);
    }

    fn impact_inputs(&self) -> Inputs {
        let suites = self
            .suites
            .iter()
            .flatten()
            .map(|suite| {
                let base = self.test_history.suite(&suite.key).and_then(|history| history.base.clone());
                (suite.key.clone(), suite.runner.clone(), base)
            })
            .collect();
        (self.root.clone(), self.project.source_dir.clone(), suites)
    }

    /// A save, or a whole run (its base moved): looked at again in a quiet moment.
    pub(super) fn impact_soon(&mut self, now: Instant) {
        self.impacts.due = Some(now + SETTLE);
    }

    /// Looks again in the background when it's due or stale, while it's wanted.
    pub(super) fn tick_impact(&mut self, now: Instant) {
        let wanted = self.impacts.wanted_until.is_some_and(|until| now < until);
        if self.impacts.running || !wanted || self.suites.is_none() {
            return;
        }
        let due = self.impacts.due.is_some_and(|due| now >= due);
        let stale = self.impacts.at.is_none_or(|at| now.saturating_duration_since(at) >= REFRESH);
        if !due && !stale {
            return;
        }
        self.impacts.running = true;
        self.impacts.due = None;
        let (root, source_dir, suites) = self.impact_inputs();
        let events = self.events.clone();
        tokio::task::spawn_blocking(move || {
            let _ = events.send(crate::run::Event::Impact(impact::assess(&root, &source_dir, &suites)));
        });
    }

    /// For tests: what changes affect, as if looked at.
    #[cfg(any(test, feature = "test-support"))]
    pub fn set_impacts(&mut self, found: HashMap<String, Impact>) {
        self.impact_done(found);
    }

    pub(super) fn impact_done(&mut self, found: HashMap<String, Impact>) {
        self.impacts.running = false;
        self.impacts.at = Some((self.now)());
        self.impacts.found = Some(found);
    }
}
