//! Test history (T): each test's outcome over its suite's last runs, with the code each run ran on (the source
//! fingerprint builds use), kept in test-history.json in the tests' folder. A failure is new when the test didn't
//! fail the run before (or never ran), known when it did; a test that failed and passed on the same code is flaky;
//! one that passes after failing the run before is fixed.

use std::collections::{BTreeMap, HashMap};
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::suites::{Outcome, TestResult};

pub const FILE: &str = "test-history.json";
/// The project's test runs, all suites together (`TestRuns`).
pub const RUNS_FILE: &str = "test-runs.json";
/// Runs kept per suite.
pub const RUNS_KEPT: usize = 20;
const PASSED: char = 'p';
const FAILED: char = 'f';
const SKIPPED: char = 's';
/// A run the test wasn't in (a run of other tests only).
pub const ABSENT: char = '-';

/// A suite's last runs.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SuiteHistory {
    /// The code each run ran on, oldest first; empty outside git (never the same code).
    pub runs: Vec<String>,
    /// Each test's outcome in each run, a character a run: `p` passed, `f` failed, `s` skipped, `-` not in it.
    pub tests: BTreeMap<String, String>,
    /// When each run finished (`2026-11-02 09:41:07`), oldest first; runs kept from before this was are missing at
    /// the front.
    #[serde(default)]
    pub at: Vec<String>,
    /// What its last whole run ran on: what changed since is what affects it (#78).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<super::impact::Baseline>,
}

/// What a test's history says about it now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Story {
    /// It failed in its last run and didn't in the one before (or had none).
    pub new: bool,
    /// It passed in its last run after failing in the one before.
    pub fixed: bool,
    /// It failed and passed on the same code: how many of its runs failed, of how many.
    pub flaky: Option<(usize, usize)>,
}

/// A suite's tests as their histories sum them up: its failures new and known, the flaky tests, the fixed ones.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tally {
    pub new: usize,
    pub known: usize,
    pub flaky: usize,
    pub fixed: usize,
}

impl SuiteHistory {
    /// A run on `code` (none outside git) and its `results`; the tests it didn't run weren't in it.
    pub fn record(&mut self, code: Option<&str>, results: &[TestResult]) {
        let column = self.runs.len();
        while self.at.len() < column {
            self.at.insert(0, String::new());
        }
        self.at.push(chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string());
        self.runs.push(code.unwrap_or_default().to_string());
        for test in results {
            let mark = match test.outcome {
                Outcome::Passed => PASSED,
                Outcome::Failed => FAILED,
                Outcome::Skipped => SKIPPED,
            };
            let line = self.tests.entry(test.name.clone()).or_default();
            pad(line, column);
            line.push(mark);
        }
        for line in self.tests.values_mut() {
            pad(line, column + 1);
        }
        if self.runs.len() > RUNS_KEPT {
            let drop = self.runs.len() - RUNS_KEPT;
            self.runs.drain(..drop);
            self.at.drain(..drop.min(self.at.len()));
            for line in self.tests.values_mut() {
                *line = line.chars().skip(drop).collect();
            }
            self.tests.retain(|_, line| line.chars().any(|mark| mark != ABSENT));
        }
    }

    /// When run `run` (0 = the oldest kept) finished, when that's known.
    pub fn when(&self, run: usize) -> Option<&str> {
        let missing = self.runs.len().saturating_sub(self.at.len());
        self.at.get(run.checked_sub(missing)?).map(String::as_str).filter(|at| !at.is_empty())
    }

    /// How many tests passed and failed in run `run` (0 = the oldest kept).
    pub fn counts(&self, run: usize) -> (usize, usize) {
        let marks = self.tests.values().filter_map(|line| line.chars().nth(run));
        marks.fold((0, 0), |(passed, failed), mark| match mark {
            PASSED => (passed + 1, failed),
            FAILED => (passed, failed + 1),
            _ => (passed, failed),
        })
    }

    /// The tests that failed in run `run` (0 = the oldest kept), by name.
    pub fn failed_in(&self, run: usize) -> Vec<&str> {
        let failed = self.tests.iter().filter(|(_, line)| line.chars().nth(run) == Some(FAILED));
        failed.map(|(name, _)| name.as_str()).collect()
    }

    /// Test `name`'s outcomes in the suite's runs, oldest first: `p` `f` `s`, `-` where it wasn't in a run.
    pub fn line(&self, name: &str) -> &str {
        self.tests.get(name).map_or("", String::as_str)
    }

    pub fn story(&self, name: &str) -> Story {
        let ran: Vec<(char, &str)> = self
            .line(name)
            .chars()
            .zip(&self.runs)
            .filter(|(mark, _)| matches!(*mark, PASSED | FAILED))
            .map(|(mark, code)| (mark, code.as_str()))
            .collect();
        let last = ran.last().map(|(mark, _)| *mark);
        let before = ran.len().checked_sub(2).map(|at| ran[at].0);
        let failed = ran.iter().filter(|(mark, _)| *mark == FAILED).count();
        let same_code_both = ran.iter().any(|(mark, code)| {
            *mark == FAILED && !code.is_empty() && ran.iter().any(|(other, same)| *other == PASSED && same == code)
        });
        Story {
            new: last == Some(FAILED) && before != Some(FAILED),
            fixed: last == Some(PASSED) && before == Some(FAILED),
            flaky: same_code_both.then_some((failed, ran.len())),
        }
    }

    /// The suite's `tests` (its last results) summed up.
    pub fn tally(&self, tests: &[TestResult]) -> Tally {
        let mut tally = Tally::default();
        for test in tests {
            let story = self.story(&test.name);
            match test.outcome {
                Outcome::Failed if story.new => tally.new += 1,
                Outcome::Failed => tally.known += 1,
                Outcome::Passed if story.fixed => tally.fixed += 1,
                _ => {}
            }
            tally.flaky += usize::from(story.flaky.is_some());
        }
        tally
    }
}

/// `line` filled with absent runs up to `runs` long.
fn pad(line: &mut String, runs: usize) {
    let missing = runs.saturating_sub(line.chars().count());
    line.extend(std::iter::repeat_n(ABSENT, missing));
}

/// Every suite's history, by its key.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TestHistory(pub HashMap<String, SuiteHistory>);

impl TestHistory {
    /// The history kept in the tests' folder `dir`; none when there's no file or it doesn't read.
    pub fn load(dir: &Path) -> Self {
        crate::json::read(&dir.join(FILE)).and_then(|value| serde_json::from_value(value).ok()).unwrap_or_default()
    }

    pub fn save(&self, dir: &Path) -> io::Result<()> {
        let text = serde_json::to_string(self).map_err(io::Error::other)?;
        std::fs::create_dir_all(dir)?;
        std::fs::write(dir.join(FILE), text)
    }

    pub fn suite(&self, key: &str) -> Option<&SuiteHistory> {
        self.0.get(key)
    }

    pub fn record(&mut self, key: &str, code: Option<&str>, results: &[TestResult]) {
        self.0.entry(key.to_string()).or_default().record(code, results);
    }

    /// What a suite's whole run ran on (none outside git).
    pub fn set_base(&mut self, key: &str, base: Option<super::impact::Baseline>) {
        self.0.entry(key.to_string()).or_default().base = base;
    }
}

/// A test run of the project (one or more suites): when it finished, how its tests did, how long it took.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TestRun {
    pub at: String,
    pub passed: usize,
    pub failed: usize,
    /// Suites that didn't run (their build failed, they left no results).
    pub errors: usize,
    pub seconds: f64,
}

/// The project's last test runs, oldest first (test-runs.json in the tests' folder).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TestRuns(pub Vec<TestRun>);

impl TestRuns {
    pub fn load(dir: &Path) -> Self {
        crate::json::read(&dir.join(RUNS_FILE)).and_then(|value| serde_json::from_value(value).ok()).unwrap_or_default()
    }

    /// A run that just finished, now; the oldest go past `RUNS_KEPT`.
    pub fn push(&mut self, passed: usize, failed: usize, errors: usize, seconds: f64) {
        let at = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        self.0.push(TestRun { at, passed, failed, errors, seconds });
        let extra = self.0.len().saturating_sub(RUNS_KEPT);
        self.0.drain(..extra);
    }

    pub fn save(&self, dir: &Path) -> io::Result<()> {
        let text = serde_json::to_string(self).map_err(io::Error::other)?;
        std::fs::create_dir_all(dir)?;
        std::fs::write(dir.join(RUNS_FILE), text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TempDir;

    fn result(name: &str, outcome: Outcome) -> TestResult {
        TestResult { name: name.into(), outcome, ms: 1.0, message: String::new(), stack: String::new() }
    }

    #[test]
    fn a_failure_is_new_then_known_and_a_pass_after_one_is_fixed() {
        let mut history = SuiteHistory::default();
        history.record(Some("a"), &[result("Totals", Outcome::Passed)]);
        history.record(Some("b"), &[result("Totals", Outcome::Failed)]);
        assert_eq!(history.story("Totals"), Story { new: true, fixed: false, flaky: None });
        history.record(Some("c"), &[result("Totals", Outcome::Failed)]);
        assert!(!history.story("Totals").new, "it failed the run before too: known");
        history.record(Some("d"), &[result("Totals", Outcome::Passed)]);
        assert!(history.story("Totals").fixed);
        assert_eq!(history.line("Totals"), "pffp");
    }

    #[test]
    fn a_test_that_fails_and_passes_on_the_same_code_is_flaky() {
        let mut history = SuiteHistory::default();
        for outcome in [Outcome::Passed, Outcome::Failed, Outcome::Passed] {
            history.record(Some("same"), &[result("Retries", outcome), result("Steady", Outcome::Passed)]);
        }
        assert_eq!(history.story("Retries").flaky, Some((1, 3)));
        assert_eq!((history.counts(1), history.failed_in(1)), ((1, 1), vec!["Retries"]), "the second run");
        assert!(history.when(1).is_some_and(|at| at.len() == 19), "when it finished: {:?}", history.when(1));
        assert_eq!(history.story("Steady").flaky, None);
        let mut outside_git = SuiteHistory::default();
        for outcome in [Outcome::Passed, Outcome::Failed] {
            outside_git.record(None, &[result("Retries", outcome)]);
        }
        assert_eq!(outside_git.story("Retries").flaky, None, "no code to tell the same from another");
    }

    #[test]
    fn a_run_of_some_tests_leaves_the_others_out_and_old_runs_go() {
        let mut history = SuiteHistory::default();
        history.record(Some("a"), &[result("One", Outcome::Failed), result("Two", Outcome::Passed)]);
        history.record(Some("a"), &[result("One", Outcome::Passed)]);
        assert_eq!((history.line("One"), history.line("Two")), ("fp", "p-"));
        for _ in 0..RUNS_KEPT {
            history.record(Some("a"), &[result("One", Outcome::Passed)]);
        }
        assert_eq!(history.runs.len(), RUNS_KEPT);
        assert_eq!(history.line("One").len(), RUNS_KEPT);
        assert_eq!(history.line("Two"), "", "a test in none of the runs kept is forgotten");
    }

    #[test]
    fn the_projects_runs_keep_the_newest() {
        let temp = TempDir::new("test-runs");
        let mut runs = TestRuns::default();
        for failed in 0..RUNS_KEPT + 2 {
            runs.push(10, failed, 0, 1.5);
        }
        assert_eq!((runs.0.len(), runs.0[0].failed), (RUNS_KEPT, 2), "the two oldest went");
        runs.save(temp.path()).unwrap();
        assert_eq!(TestRuns::load(temp.path()), runs);
    }

    #[test]
    fn the_history_is_kept_in_the_tests_folder() {
        let temp = TempDir::new("test-history");
        let mut history = TestHistory::default();
        history.record("shop", Some("a"), &[result("One", Outcome::Failed)]);
        history.save(temp.path()).unwrap();
        assert_eq!(TestHistory::load(temp.path()), history);
        let tally = history.suite("shop").unwrap().tally(&[result("One", Outcome::Failed)]);
        assert_eq!(tally, Tally { new: 1, known: 0, flaky: 0, fixed: 0 });
    }
}
