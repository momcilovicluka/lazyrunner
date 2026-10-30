//! Test watch (`a` / `A` in the tests view): a watched suite runs again when its code is saved - a quiet moment after
//! the last save, so a formatter pass or a checkout is one run. A green suite runs whole; a red one its failed tests,
//! then, if they pass, whole, so the green is real. A running suite is never cancelled: a save during its run runs it
//! again after. .NET suites follow the profile's watch folder; npm and Nx ones their own folder, watched while they
//! are. For this session only: opening lazyrunner never runs tests by itself.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::Controller;
use crate::output::Hue;
use crate::run::suites::Runner;
use crate::run::watcher::{self, CodeWatcher};

/// How long saving has to be quiet before the watched suites run.
pub const QUIET: Duration = Duration::from_millis(400);
/// The least time between two runs of a suite the watch starts.
const BETWEEN: Duration = Duration::from_millis(1500);

#[derive(Default)]
pub struct TestWatch {
    /// The suites watched, by key.
    pub suites: BTreeSet<String>,
    /// Saved, waiting for the quiet: the suites, and the last save (when, which file).
    due: BTreeSet<String>,
    last_save: Option<(Instant, PathBuf)>,
    /// Saved while they ran: they run again once their run is over.
    again: BTreeSet<String>,
    /// Red, their failed tests running: whole next, if those pass.
    then_whole: BTreeSet<String>,
    ran: HashMap<String, Instant>,
    /// The npm and Nx suites' folders, watched.
    watchers: Vec<(PathBuf, CodeWatcher)>,
}

impl TestWatch {
    /// How long until the saved suites run, while they wait for the quiet.
    pub fn runs_in(&self, now: Instant) -> Option<Duration> {
        let (at, _) = self.last_save.as_ref().filter(|_| !self.due.is_empty())?;
        Some(QUIET.saturating_sub(now.saturating_duration_since(*at)))
    }

    /// `r` / `f` and the like reset what a save was about to run of these.
    pub(super) fn ran_by_hand(&mut self, keys: &[String]) {
        for key in keys {
            self.due.remove(key);
        }
    }

    /// `s`: what was waiting to run doesn't; the watch stays.
    pub(super) fn drop_pending(&mut self) {
        self.due.clear();
        self.again.clear();
        self.then_whole.clear();
        self.last_save = None;
    }
}

impl Controller {
    /// `a` / `A`: watches these suites, or, when all of them are watched, stops.
    pub fn toggle_watch(&mut self, keys: &[String]) {
        let keys: Vec<String> =
            keys.iter().filter(|key| self.suite(key).is_some_and(|suite| !suite.untested)).cloned().collect();
        if keys.is_empty() {
            self.add_log("Nothing to watch here: these projects have no test files.", Hue::DarkGray);
            return;
        }
        let stop = keys.iter().all(|key| self.test_watch.suites.contains(key));
        for key in &keys {
            if stop {
                self.test_watch.suites.remove(key);
                self.test_watch.due.remove(key);
            } else {
                self.test_watch.suites.insert(key.clone());
            }
        }
        let names = self.suite_names(&keys);
        if stop {
            self.add_log(format!("Stopped watching {names}"), Hue::DarkGray);
        } else {
            self.add_log(
                format!("◉ Watching {names}: a save runs them again (s stops a run, a again unwatches)"),
                Hue::Cyan,
            );
        }
        self.update_test_watchers();
    }

    /// A file was saved: the watched suites it belongs to run once saving is quiet. `dotnet`: a .NET source in the
    /// profile's watch folder; otherwise one in a watched npm or Nx suite's folder.
    pub(super) fn test_file_saved(&mut self, path: &Path, dotnet: bool, now: Instant) {
        let hit: Vec<String> = self
            .suites
            .iter()
            .flatten()
            .filter(|suite| self.test_watch.suites.contains(&suite.key))
            .filter(|suite| match &suite.runner {
                Runner::Dotnet { .. } => dotnet,
                Runner::Npm { dir, .. } => !dotnet && path.starts_with(dir),
                Runner::Nx { workspace, .. } => !dotnet && path.starts_with(workspace),
            })
            .map(|suite| suite.key.clone())
            .collect();
        if hit.is_empty() {
            return;
        }
        self.test_watch.due.extend(hit);
        self.test_watch.last_save = Some((now, path.to_path_buf()));
    }

    /// Once saving has been quiet: the saved suites run - or again after their run, or after the least time between.
    pub(super) fn tick_test_watch(&mut self, now: Instant) {
        let finished: Vec<String> = self
            .test_watch
            .again
            .iter()
            .filter(|key| !self.suite(key).is_some_and(|suite| suite.is_active()))
            .cloned()
            .collect();
        for key in finished {
            self.test_watch.again.remove(&key);
            self.test_watch.due.insert(key);
            self.test_watch.last_save.get_or_insert_with(|| (now - QUIET, PathBuf::new()));
        }
        let Some((at, file)) = self.test_watch.last_save.clone() else { return };
        if self.test_watch.due.is_empty() || now.saturating_duration_since(at) < QUIET {
            return;
        }
        let (mut whole, mut failed) = (Vec::new(), Vec::new());
        for key in std::mem::take(&mut self.test_watch.due) {
            let Some(suite) = self.suite(&key) else { continue };
            if suite.is_active() {
                self.test_watch.again.insert(key);
            } else if self.test_watch.ran.get(&key).is_some_and(|ran| now.saturating_duration_since(*ran) < BETWEEN) {
                self.test_watch.due.insert(key);
            } else if suite.failed > 0 {
                failed.push(key);
            } else {
                whole.push(key);
            }
        }
        if self.test_watch.due.is_empty() {
            self.test_watch.last_save = None;
        }
        if whole.is_empty() && failed.is_empty() {
            return;
        }
        let name = file.file_name().map_or("a file".into(), |name| name.to_string_lossy().into_owned());
        let all: Vec<String> = whole.iter().chain(&failed).cloned().collect();
        self.add_log(format!("↻ {name} changed - running the watched tests: {}", self.suite_names(&all)), Hue::Cyan);
        for key in &all {
            self.test_watch.ran.insert(key.clone(), now);
        }
        self.test_watch.then_whole.extend(failed.iter().cloned());
        if !whole.is_empty() {
            self.run_tests(&whole, false);
        }
        if !failed.is_empty() {
            self.run_tests(&failed, true);
        }
    }

    /// A suite's run is over: a red watched suite whose failed tests now pass runs whole.
    pub(super) fn watched_run_over(&mut self, key: &str) {
        if !self.test_watch.then_whole.remove(key) {
            return;
        }
        let Some(suite) = self.suite(key) else { return };
        if suite.failed == 0 {
            let name = suite.name.clone();
            self.add_log(format!("{name}: the failed tests pass - checking the rest"), Hue::Cyan);
            self.run_tests(&[key.to_string()], false);
        }
    }

    /// After the suites are found again (a branch change): the watched ones that are gone stop being watched.
    pub(super) fn prune_test_watch(&mut self) {
        let gone: Vec<String> =
            self.test_watch.suites.iter().filter(|key| self.suite(key).is_none()).cloned().collect();
        for key in &gone {
            self.test_watch.suites.remove(key);
            self.add_log(format!("Stopped watching {key}: it isn't here any more"), Hue::Yellow);
        }
        if !gone.is_empty() {
            self.update_test_watchers();
        }
    }

    /// The watched npm and Nx suites' folders are watched; no others.
    fn update_test_watchers(&mut self) {
        let wanted: BTreeSet<PathBuf> = self
            .suites
            .iter()
            .flatten()
            .filter(|suite| self.test_watch.suites.contains(&suite.key))
            .filter_map(|suite| match &suite.runner {
                Runner::Npm { dir, .. } => Some(dir.clone()),
                Runner::Nx { workspace, .. } => Some(workspace.clone()),
                Runner::Dotnet { .. } => None,
            })
            .collect();
        self.test_watch.watchers.retain(|(dir, _)| wanted.contains(dir));
        for dir in wanted {
            if self.test_watch.watchers.iter().any(|(known, _)| *known == dir) {
                continue;
            }
            match watcher::watch_tests(&dir, self.events.clone()) {
                Some(watching) => self.test_watch.watchers.push((dir, watching)),
                None => self.add_log(format!("Can't watch {} for test changes", dir.display()), Hue::Yellow),
            }
        }
    }

    /// "Orders.Tests", "Orders.Tests + Billing.Tests", "5 test suites".
    fn suite_names(&self, keys: &[String]) -> String {
        if keys.len() > 2 {
            return format!("{} test suites", keys.len());
        }
        keys.iter().filter_map(|key| self.suite(key).map(|suite| suite.name.clone())).collect::<Vec<_>>().join(" + ")
    }
}
