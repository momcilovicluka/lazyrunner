//! A failure's likely cause (#34), worked out in the background for each suite's failed tests once its history has
//! a run they passed in, and again after each run of it.

use std::collections::{BTreeSet, HashMap};

use super::Controller;
use crate::run::cause::{self, Cause};
use crate::run::impact::Baseline;
use crate::run::suites::{Outcome, Runner};

/// What changed between the run a test last passed in and the one it failed in, most likely first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LikelyCause {
    /// When the run it passed in finished (`2026-11-02 09:41:07`), when that's known.
    pub passed_at: Option<String>,
    pub causes: Vec<Cause>,
}

#[derive(Default)]
pub struct Causes {
    /// Each suite's failed tests', by name, for its history as it was then, by the suite's key.
    found: HashMap<String, (String, HashMap<String, LikelyCause>)>,
    running: BTreeSet<String>,
}

/// A failed test with its stack and the two runs it turned red between.
struct Failure {
    name: String,
    stack: String,
    passed: Baseline,
    passed_at: Option<String>,
    failed: Baseline,
}

/// What working a suite's causes out needs.
struct Job {
    root: std::path::PathBuf,
    source_dir: String,
    runner: Runner,
    failures: Vec<Failure>,
}

impl Job {
    fn run(self) -> HashMap<String, LikelyCause> {
        self.failures
            .into_iter()
            .filter_map(|failure| {
                let causes = cause::likely_cause(
                    &self.root,
                    &self.source_dir,
                    &self.runner,
                    &failure.passed,
                    &failure.failed,
                    &failure.stack,
                )?;
                Some((failure.name, LikelyCause { passed_at: failure.passed_at, causes }))
            })
            .collect()
    }
}

impl Controller {
    /// The tests view is open: the suites whose failures' causes aren't known for their last run are looked at.
    pub fn want_causes(&mut self) {
        let keys: Vec<String> = self.suites.iter().flatten().map(|suite| suite.key.clone()).collect();
        for key in keys {
            if self.causes.running.contains(&key) {
                continue;
            }
            let Some((stamp, job)) = self.cause_job(&key) else { continue };
            self.causes.running.insert(key.clone());
            let events = self.events.clone();
            tokio::task::spawn_blocking(move || {
                let _ = events.send(crate::run::Event::Causes { key, stamp, found: job.run() });
            });
        }
    }

    /// A suite's failures' causes now, waiting for git (the agent asks).
    pub fn causes_now(&mut self, key: &str) {
        if let Some((stamp, job)) = self.cause_job(key) {
            self.causes_done(key.to_string(), stamp, job.run());
        }
    }

    /// A failed test's likely cause, once worked out; none when it never passed in the runs kept, or git can't say.
    pub fn likely_cause(&self, key: &str, test: &str) -> Option<&LikelyCause> {
        self.causes.found.get(key)?.1.get(test)
    }

    /// The work for a suite whose causes aren't known for its last run, with that run's stamp.
    fn cause_job(&self, key: &str) -> Option<(String, Job)> {
        let suite = self.suite(key)?;
        let history = self.test_history.suite(key)?;
        let stamp = format!("{} {}", history.runs.len(), history.at.last().map_or("", String::as_str));
        if self.causes.found.get(key).is_some_and(|(known, _)| *known == stamp) {
            return None;
        }
        let failures: Vec<Failure> = suite
            .tests
            .iter()
            .filter(|test| test.outcome == Outcome::Failed)
            .filter_map(|test| {
                let turned = history.turned(&test.name)?;
                Some(Failure {
                    name: test.name.clone(),
                    stack: test.stack.clone(),
                    passed: turned.passed.clone(),
                    passed_at: turned.passed_at.map(str::to_string),
                    failed: turned.failed.clone(),
                })
            })
            .collect();
        let job = Job {
            root: self.root.clone(),
            source_dir: self.project.source_dir.clone(),
            runner: suite.runner.clone(),
            failures,
        };
        Some((stamp, job))
    }

    /// For tests: a suite's failures' likely causes, as if worked out.
    #[cfg(any(test, feature = "test-support"))]
    pub fn set_likely_causes(&mut self, key: &str, found: HashMap<String, LikelyCause>) {
        self.causes_done(key.to_string(), String::new(), found);
    }

    pub(super) fn causes_done(&mut self, key: String, stamp: String, found: HashMap<String, LikelyCause>) {
        self.causes.running.remove(&key);
        self.causes.found.insert(key, (stamp, found));
    }
}
