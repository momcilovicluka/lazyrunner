//! Running the test suites (T): several at once in the background, up to `"testParallelism"` (half the cores by
//! default), so the running apps stay responsive. The .NET suites build together first, in one MSBuild run (the
//! projects they share build once), then run with --no-build side by side. An app's build stops that test build,
//! which runs again after it (they share obj and bin); npm ci stops the suites that run from its folder.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::Instant;

use super::Controller;
use crate::output::{Hue, strip_ansi};
use crate::profile::AppKind;
use crate::run::build::{self, ToolLine};
use crate::run::child::Child;
use crate::run::history::Kind;
use crate::run::suites::{self, MAX_OUTPUT, Outcome, Runner, Status, Suite};
use crate::run::{Tag, format_duration};

/// Lines of a failed test build shown when it named no errors.
const BUILD_TAIL: usize = 40;
/// Characters of test names a run of the failed tests passes at most (cmd.exe, which npm runs scripts with, takes
/// 8191); more failed tests than that and the whole suite runs.
const MAX_NAMES: usize = 6000;

/// The test build: the queued .NET suites' projects, built in one MSBuild run.
pub struct TestBuild {
    pub id: u64,
    pub keys: Vec<String>,
    pub started: Instant,
    /// Projects built so far.
    pub done: usize,
    /// The output worth showing (no warnings), the newest last.
    pub output: VecDeque<String>,
    lines: Vec<String>,
    child: Child,
}

/// What a run that left no results said went wrong: its first line naming something missing or an error, to its
/// first full stop ("Test environment jest-environment-jsdom cannot be found").
fn reason(lines: &[String]) -> Option<String> {
    const SIGNS: [&str; 6] =
        ["cannot be found", "Cannot find", "not found", "is not recognized", "error TS", "Error: "];
    let line = lines
        .iter()
        .map(|line| line.trim())
        .find(|line| !line.contains("Validation Error") && SIGNS.iter().any(|sign| line.contains(sign)))?;
    let sentence = line.split(". ").next().unwrap_or(line).trim_end_matches('.');
    Some(sentence.chars().take(160).collect())
}

/// The suites' results added up.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Totals {
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
    /// Suites that didn't run (their build failed, they left no results).
    pub errors: usize,
    /// Suites with results.
    pub ran: usize,
}

impl Controller {
    /// Where the suites' results and logs are kept.
    pub fn tests_dir(&self) -> PathBuf {
        self.state_dir.join("tests")
    }

    /// T: looks for the suites (and their last results) in the background; they arrive as `Event::Suites`. Again
    /// each time, to find new ones, unless tests are running.
    pub fn find_suites(&mut self) {
        if self.finding_suites || self.tests_active() {
            return;
        }
        self.finding_suites = true;
        let (root, apps, dir, events) = (self.root.clone(), self.apps.clone(), self.tests_dir(), self.events.clone());
        tokio::task::spawn_blocking(move || {
            let mut found = suites::discover(&root, &apps);
            for suite in &mut found {
                suite.load_last(&dir);
            }
            let _ = events.send(crate::run::Event::Suites(found));
        });
    }

    /// Whether the suites are being looked for.
    pub fn finding_suites(&self) -> bool {
        self.finding_suites
    }

    /// The suites were found: the ones already known keep what they have (results, a run going on).
    pub(super) fn suites_found(&mut self, found: Vec<Suite>) {
        self.finding_suites = false;
        let mut known = self.suites.take().unwrap_or_default();
        let mut suites: Vec<Suite> = Vec::new();
        for suite in found {
            match known.iter().position(|old| old.key == suite.key) {
                Some(at) => {
                    let mut known = known.remove(at);
                    known.app = suite.app; // the profile may have changed
                    suites.push(known);
                }
                None => suites.push(suite),
            }
        }
        suites.extend(known.into_iter().filter(Suite::is_active));
        if suites.iter().all(|suite| suite.untested) {
            self.add_log(
                "No tests found: .NET projects referencing Microsoft.NET.Test.Sdk, npm apps whose test script runs \
                 jest or vitest, Nx projects with a test target.",
                Hue::Yellow,
            );
        }
        self.suites = Some(suites);
        self.check_packages();
        self.prune_test_watch();
    }

    /// Whether each npm and Nx suite's packages are there (when found, before a run, after npm ci).
    pub(super) fn check_packages(&mut self) {
        for suite in self.suites.iter_mut().flatten() {
            suite.needs_packages = crate::run::tools::packages_problem(&suite.runner);
        }
    }

    pub fn suite(&self, key: &str) -> Option<&Suite> {
        self.suites.as_ref()?.iter().find(|suite| suite.key == key)
    }

    fn suite_mut(&mut self, key: &str) -> Option<&mut Suite> {
        self.suites.as_mut()?.iter_mut().find(|suite| suite.key == key)
    }

    fn all_suites(&self) -> impl Iterator<Item = &Suite> {
        self.suites.iter().flatten()
    }

    /// Whether tests are queued, building or running.
    pub fn tests_active(&self) -> bool {
        self.test_build.is_some() || self.all_suites().any(Suite::is_active)
    }

    /// How many suites run at once: `"testParallelism"`, or half the cores.
    pub fn test_parallelism(&self) -> usize {
        match self.config["testParallelism"].as_u64() {
            Some(count) if count > 0 => count as usize,
            _ => std::thread::available_parallelism().map_or(1, |cores| (cores.get() / 2).max(1)),
        }
    }

    pub fn test_totals(&self) -> Totals {
        self.totals(|_| true)
    }

    fn totals(&self, counted: impl Fn(&Suite) -> bool) -> Totals {
        self.all_suites().filter(|suite| counted(suite)).fold(Totals::default(), |totals, suite| Totals {
            passed: totals.passed + suite.passed,
            failed: totals.failed + suite.failed,
            skipped: totals.skipped + suite.skipped,
            errors: totals.errors + usize::from(suite.status == Status::Error),
            ran: totals.ran + usize::from(suite.last_run.is_some()),
        })
    }

    /// The suite's log file, with what a running suite printed so far; `None` before its first run.
    pub fn suite_log(&self, key: &str) -> Option<PathBuf> {
        let suite = self.suite(key)?;
        let file = suite.log_file(&self.tests_dir());
        if suite.status == Status::Running {
            let _ = std::fs::write(&file, suite.lines.join("\n") + "\n");
        }
        file.is_file().then_some(file)
    }

    /// r, R, f, F: queues these suites (only their failed tests with `failed_only`).
    pub fn run_tests(&mut self, keys: &[String], failed_only: bool) {
        self.test_watch.ran_by_hand(keys);
        self.check_packages();
        let mut queued = Vec::new();
        // Suites that can't run for want of packages: one line per folder, not one per suite.
        let mut missing: Vec<(String, usize)> = Vec::new();
        for key in keys {
            let Some(suite) = self.suite(key) else { continue };
            if suite.untested {
                continue;
            }
            if let Some(problem) = suite.needs_packages.clone() {
                match missing.iter_mut().find(|(known, _)| *known == problem) {
                    Some((_, count)) => *count += 1,
                    None => missing.push((problem, 1)),
                }
                continue;
            }
            let failed: Vec<String> = suite
                .tests
                .iter()
                .filter(|test| test.outcome == Outcome::Failed)
                .map(|test| test.name.clone())
                .collect();
            if suite.is_active() || (failed_only && failed.is_empty()) {
                continue;
            }
            queued.extend(self.queue_suite(key, failed_only.then_some(failed)));
        }
        for (problem, count) in &missing {
            let suites = if *count == 1 { "1 suite".to_string() } else { format!("{count} suites") };
            self.add_log(format!("{suites} can't run: {problem} - i installs the packages"), Hue::Yellow);
        }
        if queued.is_empty() {
            if missing.is_empty() {
                let why =
                    if failed_only { "No failed tests to run again." } else { "Those tests are already running." };
                self.add_log(why, Hue::DarkGray);
            }
            return;
        }
        let now = (self.now)();
        self.tests_started.get_or_insert(now);
        let what = if queued.len() == 1 { queued.remove(0) } else { format!("{} test suites", queued.len()) };
        let only = if failed_only { " (failed tests only)" } else { "" };
        let at_once = self.test_parallelism();
        self.add_log(format!("Running tests: {what}{only}, up to {at_once} at once"), Hue::Cyan);
        self.pump_tests();
    }

    /// One test of a suite (picked in the tests view); its result joins the suite's.
    pub fn run_test(&mut self, key: &str, test: &str) {
        match self.suite(key) {
            None => return,
            Some(suite) if suite.is_active() => {
                let text = format!("{} is already running.", suite.name);
                self.add_log(text, Hue::DarkGray);
                return;
            }
            Some(_) => {}
        }
        self.queue_suite(key, Some(vec![test.to_string()]));
        let now = (self.now)();
        self.tests_started.get_or_insert(now);
        self.add_log(format!("Running 1 test: {test}"), Hue::Cyan);
        self.pump_tests();
    }

    /// Puts a suite in the queue (only the `rerun` tests, when there are some); its name.
    fn queue_suite(&mut self, key: &str, rerun: Option<Vec<String>>) -> Option<String> {
        let suite = self.suite_mut(key)?;
        suite.before = suite.status;
        suite.status = Status::Queued;
        suite.note.clear();
        suite.built = false;
        suite.failed_only = rerun.is_some();
        suite.rerun = rerun.unwrap_or_default();
        let name = suite.name.clone();
        self.test_queue.push(key.to_string());
        if !self.run_keys.iter().any(|known| known == key) {
            self.run_keys.push(key.to_string());
        }
        Some(name)
    }

    /// s in the tests view, and quitting: stops the running suites and the test build, and drops the queue (the
    /// queued suites keep their last results).
    pub fn stop_tests(&mut self, quiet: bool) {
        self.test_watch.drop_pending();
        let dir = self.tests_dir();
        if let Some(build) = self.test_build.take() {
            build.child.kill();
        }
        let mut stopped = Vec::new();
        let mut queued = false;
        for suite in self.suites.iter_mut().flatten() {
            match suite.status {
                Status::Running => {
                    if let Some(child) = suite.child.take() {
                        child.kill();
                    }
                    let _ = std::fs::write(suite.log_file(&dir), suite.lines.join("\n") + "\n");
                    suite.status = Status::Cancelled;
                    suite.failed_only = false;
                    suite.rerun.clear();
                    stopped.push(suite.name.clone());
                }
                Status::Queued => {
                    suite.status = suite.before;
                    suite.note.clear();
                    suite.failed_only = false;
                    suite.rerun.clear();
                    queued = true;
                }
                _ => {}
            }
        }
        self.test_queue.clear();
        self.run_keys.clear();
        self.tests_started = None;
        if quiet || (stopped.is_empty() && !queued) {
            return;
        }
        let running =
            if stopped.is_empty() { String::new() } else { format!(" ({} were running)", stopped.join(", ")) };
        self.add_log(format!("■ Tests stopped{running}"), Hue::Yellow);
    }

    /// The timed part: the test build, the suites that can start, and the summary once all are done.
    pub(super) fn pump_tests(&mut self) {
        self.start_test_build();
        self.start_ready_suites();
        if self.tests_started.is_some() && !self.tests_active() {
            self.tests_done();
        }
    }

    /// One MSBuild run over every queued .NET suite's project not built yet; it waits for an app's build.
    fn start_test_build(&mut self) {
        if self.test_build.is_some() {
            return;
        }
        let waiting: Vec<(String, PathBuf)> = self
            .test_queue
            .iter()
            .filter_map(|key| match self.suite(key) {
                Some(Suite { runner: Runner::Dotnet { project }, built: false, .. }) => {
                    Some((key.clone(), project.clone()))
                }
                _ => None,
            })
            .collect();
        if waiting.is_empty() {
            return;
        }
        let keys: Vec<String> = waiting.iter().map(|(key, _)| key.clone()).collect();
        if self.building.is_some() {
            self.note_suites(&keys, "waits for the build");
            return;
        }
        let dir = self.tests_dir();
        let file = dir.join("lazyrunner-tests.proj");
        let projects: Vec<&Path> = waiting.iter().map(|(_, project)| project.as_path()).collect();
        let written =
            std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&file, suites::build_project(&projects)));
        if let Err(error) = written {
            let why = vec![format!("Can't write {} ({error})", file.display())];
            self.fail_suites(&keys, "the test build didn't start", why);
            return;
        }
        let spec = self.commands.test_build(&file);
        self.next_job += 1;
        let id = self.next_job;
        match Child::spawn(&spec, Tag::TestBuild { job: id }, self.events.clone(), self.guard.as_ref()) {
            Ok(child) => {
                self.note_suites(&keys, "building");
                let started = (self.now)();
                self.test_build =
                    Some(TestBuild { id, keys, started, done: 0, output: VecDeque::new(), lines: Vec::new(), child });
            }
            Err(error) => self.fail_suites(&keys, "the test build didn't start", vec![format!("dotnet: {error}")]),
        }
    }

    /// Starts queued suites that are ready (.NET: built; npm: no npm ci in their folder), up to the limit.
    fn start_ready_suites(&mut self) {
        let limit = self.test_parallelism();
        let npm_ci_dir = self.npm_ci.as_ref().and_then(|job| match &self.app(&job.key)?.kind {
            AppKind::Npm { dir, .. } => Some(self.root.join(dir)),
            AppKind::Dotnet { .. } => None,
        });
        for key in self.test_queue.clone() {
            let running = self.all_suites().filter(|suite| suite.status == Status::Running).count();
            if running >= limit {
                return;
            }
            let Some(suite) = self.suite_mut(&key) else { continue };
            if matches!(suite.runner, Runner::Dotnet { .. }) && !suite.built {
                continue;
            }
            if suite.runner.npm_dir().is_some_and(|dir| Some(dir) == npm_ci_dir.as_deref()) {
                suite.note = "waits for npm ci".into();
                continue;
            }
            self.test_queue.retain(|queued| *queued != key);
            self.start_suite(&key);
        }
    }

    fn start_suite(&mut self, key: &str) {
        let dir = self.tests_dir();
        let now = (self.now)();
        self.next_job += 1;
        let job = self.next_job;
        let Some(suite) = self.suite_mut(key) else { return };
        // Never this run's results from the run before.
        let file = if suite.failed_only { suite.rerun_file(&dir) } else { suite.results_file(&dir) };
        let _ = std::fs::remove_file(&file);
        let mut names: Vec<String> = if suite.failed_only { suite.rerun.clone() } else { Vec::new() };
        let too_many = names.iter().map(|name| name.len() * 2 + 24).sum::<usize>() > MAX_NAMES;
        if too_many {
            names.clear();
            suite.failed_only = false;
            suite.rerun.clear();
            let file = suite.results_file(&dir);
            let _ = std::fs::remove_file(file);
        }
        let file = if suite.failed_only { file } else { suite.results_file(&dir) };
        suite.rerun = names.clone();
        let runner = suite.runner.clone();
        let name = suite.name.clone();
        if too_many {
            self.add_log(
                format!("{name}: too many failed tests to name on a command line - all of it runs"),
                Hue::DarkGray,
            );
        }
        let Some(suite) = self.suite_mut(key) else { return };
        suite.output.clear();
        suite.lines.clear();
        suite.done = 0;
        suite.note.clear();
        suite.started = Some(now);
        suite.job = job;
        suite.status = Status::Running;
        let spawned = std::fs::create_dir_all(&dir)
            .map_err(anyhow::Error::from)
            .and_then(|()| self.commands.test(&runner, &file, &names))
            .and_then(|spec| {
                let tag = Tag::Suite { key: key.to_string(), job };
                Ok(Child::spawn(&spec, tag, self.events.clone(), self.guard.as_ref())?)
            });
        match spawned {
            Ok(child) => {
                if let Some(suite) = self.suite_mut(key) {
                    suite.child = Some(child);
                }
            }
            Err(error) => {
                self.fail_suites(&[key.to_string()], "didn't start", vec![error.to_string()]);
                let name = self.suite(key).map(|suite| suite.name.clone()).unwrap_or_default();
                self.add_log(format!("✗ {name} didn't start: {error}"), Hue::Red);
            }
        }
    }

    pub(super) fn suite_output(&mut self, key: &str, job: u64, line: &str) {
        if let Some(suite) = self.suite_mut(key).filter(|suite| suite.job == job && suite.status == Status::Running) {
            suite.receive(line);
        }
    }

    pub(super) fn suite_exited(&mut self, key: &str, job: u64, code: Option<i32>) {
        if self.suite(key).is_some_and(|suite| suite.job == job && suite.status == Status::Running) {
            self.complete_suite(key, code);
            self.watched_run_over(key);
            self.pump_tests();
        }
    }

    /// A suite's run is over: its results (merged into the last ones for a run of the failed tests), its log file,
    /// its duration, and a line in the log.
    fn complete_suite(&mut self, key: &str, code: Option<i32>) {
        let dir = self.tests_dir();
        let now = (self.now)();
        let Some(suite) = self.suite_mut(key) else { return };
        suite.child = None;
        let _ = std::fs::write(suite.log_file(&dir), suite.lines.join("\n") + "\n");
        suite.seconds = suite.started.map_or(0.0, |started| now.saturating_duration_since(started).as_secs_f64());
        suite.last_run = Some(chrono::Local::now());
        let failed_only = std::mem::take(&mut suite.failed_only);
        let requested = std::mem::take(&mut suite.rerun);
        let file = if failed_only { suite.rerun_file(&dir) } else { suite.results_file(&dir) };
        let (name, seconds) = (suite.name.clone(), suite.seconds);
        let tests = match suites::read_results(&suite.runner, &file) {
            Ok((tests, _)) => tests,
            Err(error) => {
                suite.status = Status::Error;
                let exit = code.map_or_else(|| "?".to_string(), |code| code.to_string());
                let readable = file.is_file();
                suite.note = match (readable, reason(&suite.lines)) {
                    (true, _) => "unreadable results".into(),
                    (false, Some(reason)) => reason,
                    (false, None) => format!("no results (exit code {exit})"),
                };
                let why = if readable { error } else { suite.note.clone() };
                self.add_log(format!("✗ {name}: {why} - o in the tests view opens its log"), Hue::Red);
                return;
            }
        };
        let ran = if failed_only {
            suite.merge(tests, &requested)
        } else {
            suite.set_tests(tests);
            0
        };
        suite.status = if suite.failed > 0 { Status::Failed } else { Status::Passed };
        let (passed, failed, skipped) = (suite.passed, suite.failed, suite.skipped);
        let note = if failed_only {
            String::new()
        } else {
            self.record_duration_as(Kind::Test, key, &name, seconds, failed == 0)
        };
        let again =
            if failed_only { format!(" ({ran} test{} again)", if ran == 1 { "" } else { "s" }) } else { String::new() };
        let skipped = if skipped > 0 { format!(", {skipped} skipped") } else { String::new() };
        let took = format_duration(seconds);
        if failed > 0 {
            let text = format!("✗ {name}{again} - {failed} failed, {passed} passed{skipped} ({took}){note}");
            self.add_log(text, Hue::Red);
        } else {
            self.add_log(format!("✓ {name}{again} - {passed} passed{skipped} ({took}){note}"), Hue::Green);
        }
    }

    /// Every suite of the run is done: one line, and a notification when something failed.
    fn tests_done(&mut self) {
        let Some(started) = self.tests_started.take() else { return };
        let took = format_duration((self.now)().saturating_duration_since(started).as_secs_f64());
        let ran = std::mem::take(&mut self.run_keys);
        let totals = self.totals(|suite| ran.contains(&suite.key));
        if totals.failed == 0 && totals.errors == 0 {
            self.add_log(format!("Tests done in {took}: all {} passed", totals.passed), Hue::Green);
            self.happened("tests.green", None, format!("all {} passed", totals.passed));
            return;
        }
        let didnt = if totals.errors > 0 { format!(", {} suites didn't run", totals.errors) } else { String::new() };
        let counts = format!("{} failed, {} passed", totals.failed, totals.passed);
        self.add_log(format!("Tests done in {took}: {counts}{didnt} - T shows them"), Hue::Red);
        self.happened("tests.red", None, format!("{counts}{didnt}"));
        self.notifications.push(("Tests failed".into(), counts));
    }

    pub(super) fn test_build_output(&mut self, job: u64, line: &str) {
        let Some(build) = self.test_build.as_mut().filter(|build| build.id == job) else { return };
        let text = strip_ansi(line);
        build.lines.push(text.clone());
        match build::tool_line(&text, true) {
            ToolLine::Built(project) => {
                build.done += 1;
                let keys = build.keys.clone();
                let output = text.split_once(" -> ").map(|(_, path)| path.trim().to_lowercase()).unwrap_or_default();
                // Its output under its project's folder; by name only when no folder matches (an artifacts folder).
                let under = |suite: &Suite| match &suite.runner {
                    Runner::Dotnet { project } => {
                        project.parent().is_some_and(|dir| output.starts_with(&dir.to_string_lossy().to_lowercase()))
                    }
                    _ => false,
                };
                let by_folder = keys.iter().any(|key| self.suite(key).is_some_and(under));
                for key in keys {
                    let built = |suite: &Suite| if by_folder { under(suite) } else { suite.name == project };
                    if let Some(suite) = self.suite_mut(&key).filter(|suite| built(suite)) {
                        suite.built = true;
                        suite.note.clear();
                    }
                }
                self.start_ready_suites();
            }
            ToolLine::Warning => {}
            ToolLine::Shown(text, _) => {
                build.output.push_back(text);
                while build.output.len() > MAX_OUTPUT {
                    build.output.pop_front();
                }
            }
        }
    }

    pub(super) fn test_build_exited(&mut self, job: u64, code: Option<i32>) {
        let Some(build) = self.test_build.take_if(|build| build.id == job) else { return };
        let _ = std::fs::write(self.tests_dir().join("build.log"), build.lines.join("\n") + "\n");
        let unbuilt: Vec<String> = build
            .keys
            .iter()
            .filter(|key| self.suite(key).is_some_and(|suite| !suite.built && suite.status == Status::Queued))
            .cloned()
            .collect();
        if code == Some(0) {
            // Built, though MSBuild didn't name them (another AssemblyName, say).
            for key in &unbuilt {
                if let Some(suite) = self.suite_mut(key) {
                    suite.built = true;
                    suite.note.clear();
                }
            }
        } else if !unbuilt.is_empty() {
            let mut errors: Vec<String> = Vec::new();
            for line in build.lines.iter().filter(|line| line.contains(": error ")) {
                let error = build::format_error(line);
                if !errors.contains(&error) {
                    errors.push(error);
                }
            }
            let count = errors.len();
            if errors.is_empty() {
                errors = build.output.iter().rev().take(BUILD_TAIL).rev().cloned().collect();
            }
            self.fail_suites(&unbuilt, "build failed", errors);
            let names: Vec<String> =
                unbuilt.iter().filter_map(|key| self.suite(key)).map(|suite| suite.name.clone()).collect();
            let names = names.join(", ");
            let text = format!("✗ Test build failed ({count} errors) - {names} can't run; T shows the errors");
            self.add_log(text, Hue::Red);
        }
        self.pump_tests();
    }

    /// An app's build is about to start: the test build stops (they share obj and bin) and runs again after it.
    pub(super) fn pause_test_build(&mut self, reason: &str) {
        let Some(build) = self.test_build.take() else { return };
        build.child.kill();
        let unbuilt: Vec<String> =
            build.keys.iter().filter(|key| self.suite(key).is_some_and(|suite| !suite.built)).cloned().collect();
        self.note_suites(&unbuilt, "waits for the build");
        self.add_log(
            format!("Tests paused for {reason} - the test build runs again after it (running suites carry on)"),
            Hue::DarkGray,
        );
    }

    /// npm ci is about to rewrite `dir`'s node_modules: the suites running from it stop and run again after it.
    pub(super) fn pause_npm_suites(&mut self, dir: &Path) {
        let mut paused = Vec::new();
        for suite in self.suites.iter_mut().flatten() {
            if suite.status == Status::Running && suite.runner.npm_dir() == Some(dir) {
                if let Some(child) = suite.child.take() {
                    child.kill();
                }
                suite.status = Status::Queued;
                suite.note = "waits for npm ci".into();
                paused.push((suite.key.clone(), suite.name.clone()));
            }
        }
        if paused.is_empty() {
            return;
        }
        for (key, _) in paused.iter().rev() {
            self.test_queue.insert(0, key.clone());
        }
        let names: Vec<&str> = paused.iter().map(|(_, name)| name.as_str()).collect();
        self.add_log(format!("Tests paused for npm ci - {} run again after it", names.join(", ")), Hue::DarkGray);
    }

    fn note_suites(&mut self, keys: &[String], note: &str) {
        for key in keys {
            if let Some(suite) = self.suite_mut(key) {
                suite.note = note.to_string();
            }
        }
    }

    /// These suites can't run: out of the queue, with what went wrong as their output.
    fn fail_suites(&mut self, keys: &[String], note: &str, output: Vec<String>) {
        self.test_queue.retain(|queued| !keys.contains(queued));
        for key in keys {
            if let Some(suite) = self.suite_mut(key) {
                suite.status = Status::Error;
                suite.note = note.to_string();
                suite.output = output.iter().cloned().collect();
                suite.last_run = Some(chrono::Local::now());
                suite.failed_only = false;
                suite.rerun.clear();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;
    use std::time::Duration;

    use serde_json::json;
    use tokio::sync::mpsc::UnboundedReceiver;

    use crate::paths::Paths;
    use crate::profile::{App, ProfileDirs, import};
    use crate::run::child::Spec;
    use crate::run::history::Kind;
    use crate::run::suites::{Runner, Status, Suite};
    use crate::run::tools::{Commands, ServerCommand};
    use crate::run::{Controller, Event};
    use crate::testing::{TempDir, sample_profile, sample_repo};

    /// A suite's run prints a line and writes jest results: its first test passes, the second fails unless it runs
    /// on its own (the failed tests again); the test build does nothing.
    struct Scripts;

    fn shell(script: String, dir: &Path) -> Spec {
        let (program, flag) = if cfg!(windows) { ("cmd", "/C") } else { ("sh", "-c") };
        Spec {
            program: program.into(),
            args: vec![flag.into(), script.into()],
            dir: dir.to_path_buf(),
            env: Vec::new(),
        }
    }

    impl Commands for Scripts {
        fn server(&self, root: &Path, app: &App, _: &[App]) -> anyhow::Result<ServerCommand> {
            Ok(ServerCommand { spec: shell("exit 0".into(), root), label: app.name.clone(), script: None })
        }

        fn build(&self, root: &Path, _: &str) -> Spec {
            shell("exit 0".into(), root)
        }

        fn npm_ci(&self, dir: &Path) -> anyhow::Result<Spec> {
            Ok(shell("exit 0".into(), dir))
        }

        fn test(&self, _: &Runner, results: &Path, names: &[String]) -> anyhow::Result<Spec> {
            let second = if names.is_empty() { "failed" } else { "passed" };
            let json = format!(
                r#"{{"startTime":0,"testResults":[{{"name":"a.spec.ts","endTime":5,"assertionResults":[{{"fullName":"adds","status":"passed"}},{{"fullName":"rounds","status":"{second}","failureMessages":["Expected 2"]}}]}}]}}"#
            );
            let dir = results.parent().expect("the tests folder").to_path_buf();
            let file = results.file_name().expect("a file").to_string_lossy().into_owned();
            let next = format!("{file}.next");
            std::fs::write(dir.join(&next), json)?;
            let copy = if cfg!(windows) { "copy /y" } else { "cp" };
            Ok(shell(format!("echo PASS a.spec.ts&& {copy} {next} {file}"), &dir))
        }

        fn test_build(&self, project: &Path) -> Spec {
            shell("exit 0".into(), project.parent().expect("the tests folder"))
        }
    }

    fn controller(temp: &TempDir) -> (Controller, UnboundedReceiver<Event>) {
        let repo = sample_repo(&temp.join("repo"));
        let paths = Paths {
            config_file: temp.join("config.json"),
            state_dir: temp.join("state"),
            profiles: ProfileDirs {
                user: temp.join("profiles"),
                bundled: temp.join("bundled"),
                only: Some(sample_profile()),
            },
        };
        crate::config::write(&paths.config_file, &json!({ "testParallelism": 1 })).unwrap();
        let project = import(&sample_profile(), &paths.profiles).unwrap();
        let (events, rx) = tokio::sync::mpsc::unbounded_channel();
        let setup = crate::run::controller::Setup { project, root: repo, paths, events, guard: None };
        let mut controller = Controller::new(setup).unwrap();
        controller.commands = Arc::new(Scripts);
        let npm = |name: &str| {
            std::fs::create_dir_all(temp.join(name).join("node_modules")).unwrap(); // its packages are there
            Suite::new(format!("npm:{name}"), name, Runner::Npm { dir: temp.join(name), vitest: false })
        };
        controller.suites = Some(vec![npm("cart"), npm("checkout")]);
        (controller, rx)
    }

    async fn run_until(
        controller: &mut Controller,
        events: &mut UnboundedReceiver<Event>,
        done: impl Fn(&Controller) -> bool,
    ) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        while !done(controller) {
            match tokio::time::timeout_at(deadline, events.recv()).await {
                Ok(Some(event)) => controller.handle(event),
                _ => panic!("timed out; the log:\n{}", log(controller)),
            }
        }
    }

    fn log(controller: &Controller) -> String {
        controller.log.iter().map(|entry| entry.text.as_str()).collect::<Vec<_>>().join("\n")
    }

    fn keys(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| format!("npm:{name}")).collect()
    }

    #[tokio::test]
    async fn a_pinned_test_is_all_a_save_runs_and_pinning_it_again_stops() {
        let temp = TempDir::new("suites-pin");
        let (mut sample, mut events) = controller(&temp);
        sample.run_tests(&keys(&["cart"]), false);
        run_until(&mut sample, &mut events, |sample| !sample.tests_active()).await;
        sample.pin_watch("npm:cart", "nothing like it");
        assert!(log(&sample).ends_with("r runs it once"), "{}", log(&sample));
        sample.pin_watch("npm:cart", "rounds");
        assert_eq!(sample.test_watch.pinned.get("npm:cart").map(String::as_str), Some("rounds"));
        let saved = std::time::Instant::now();
        sample.test_file_saved(&temp.join("cart").join("cart.test.js"), false, saved);
        sample.tick_test_watch(saved + super::super::test_watch::QUIET);
        assert_eq!(log(&sample).lines().last(), Some("Running 1 test: rounds"), "{}", log(&sample));
        assert_eq!(sample.suite("npm:cart").unwrap().rerun, ["rounds"]);
        run_until(&mut sample, &mut events, |sample| !sample.tests_active()).await;
        sample.toggle_watch(&keys(&["cart"]));
        assert!(
            sample.test_watch.pinned.is_empty() && sample.test_watch.suites.contains("npm:cart"),
            "a on the suite: all of it"
        );
        sample.pin_watch("npm:cart", "rounds");
        sample.pin_watch("npm:cart", "rounds");
        assert!(sample.test_watch.suites.is_empty(), "pinning it again stops watching");
    }

    #[tokio::test]
    async fn a_watched_red_suite_runs_its_failed_tests_on_a_save_then_all_of_it() {
        let temp = TempDir::new("suites-watch");
        let (mut sample, mut events) = controller(&temp);
        sample.run_tests(&keys(&["cart"]), false);
        run_until(&mut sample, &mut events, |sample| !sample.tests_active()).await;
        sample.toggle_watch(&keys(&["cart"]));
        assert!(log(&sample).ends_with("(s stops a run, a again unwatches)"), "{}", log(&sample));
        let saved = std::time::Instant::now();
        let quiet = super::super::test_watch::QUIET;
        sample.test_file_saved(&temp.join("checkout").join("pay.js"), false, saved);
        sample.tick_test_watch(saved + quiet);
        assert!(!sample.tests_active(), "checkout is not watched");
        sample.test_file_saved(&temp.join("cart").join("cart.test.js"), false, saved);
        sample.tick_test_watch(saved + Duration::from_millis(100));
        assert!(!sample.tests_active(), "not before saving is quiet");
        sample.tick_test_watch(saved + quiet);
        let said = log(&sample);
        assert!(said.contains("cart.test.js changed - running the watched tests: cart"), "{said}");
        assert!(sample.suite("npm:cart").unwrap().failed_only, "red: its failed tests first");
        run_until(&mut sample, &mut events, |sample| {
            log(sample).contains("checking the rest") && !sample.tests_active()
        })
        .await;
        sample.toggle_watch(&keys(&["cart"]));
        assert!(log(&sample).ends_with("Stopped watching cart"));
    }

    #[test]
    fn a_run_without_results_says_what_went_wrong() {
        let lines = [
            "> nx run @focus/core:test --ci --json --outputFile=x.json",
            "  NX   ● Validation Error:",
            "  Test environment jest-environment-jsdom cannot be found. Make sure the testEnvironment option points to an existing node module.",
            "  NX   Running target test for project @focus/core failed",
        ]
        .map(String::from);
        assert_eq!(super::reason(&lines).as_deref(), Some("Test environment jest-environment-jsdom cannot be found"));
        assert_eq!(super::reason(&["all good".to_string()]), None);
    }

    #[tokio::test]
    async fn suites_run_up_to_the_limit_and_their_results_are_read() {
        let temp = TempDir::new("suites-run");
        let (mut sample, mut events) = controller(&temp);
        sample.run_tests(&keys(&["cart", "checkout"]), false);
        let status = |sample: &Controller, key: &str| sample.suite(key).unwrap().status;
        assert_eq!(status(&sample, "npm:cart"), Status::Running);
        assert_eq!(status(&sample, "npm:checkout"), Status::Queued, "one at a time");
        run_until(&mut sample, &mut events, |sample| !sample.tests_active()).await;
        let cart = sample.suite("npm:cart").unwrap();
        assert_eq!((cart.status, cart.passed, cart.failed, cart.done), (Status::Failed, 1, 1, 1));
        let log = log(&sample);
        assert!(log.contains("✗ cart - 1 failed, 1 passed"), "{log}");
        assert!(log.contains(": 2 failed, 2 passed - T shows them"), "{log}");
        assert_eq!(sample.notifications.last().map(|(title, _)| title.as_str()), Some("Tests failed"));
        let saved = sample.suite_log("npm:cart").map(|file| std::fs::read_to_string(file).unwrap());
        assert!(saved.is_some_and(|text| text.contains("PASS a.spec.ts")));
        assert_eq!(sample.durations.item(Kind::Test, "npm:cart").map(|item| item.runs.len()), Some(1));
    }

    #[tokio::test]
    async fn the_failed_tests_run_again_and_their_results_merge() {
        let temp = TempDir::new("suites-failed-only");
        let (mut sample, mut events) = controller(&temp);
        sample.run_tests(&keys(&["cart"]), false);
        run_until(&mut sample, &mut events, |sample| !sample.tests_active()).await;
        sample.run_tests(&keys(&["checkout"]), true);
        assert!(log(&sample).ends_with("No failed tests to run again."), "checkout hasn't run");
        sample.run_tests(&keys(&["cart"]), true);
        run_until(&mut sample, &mut events, |sample| !sample.tests_active()).await;
        let cart = sample.suite("npm:cart").unwrap();
        assert_eq!((cart.status, cart.passed, cart.failed), (Status::Passed, 2, 0));
        assert!(log(&sample).contains("✓ cart (1 test again) - 2 passed"), "{}", log(&sample));
        let mut reloaded = Suite::new("npm:cart", "cart", cart.runner.clone());
        reloaded.load_last(&sample.tests_dir());
        assert_eq!(reloaded.failed, 1, "a restart shows the last full run");
    }

    #[tokio::test]
    async fn one_test_runs_on_its_own_and_its_result_joins_the_suites() {
        let temp = TempDir::new("suites-one");
        let (mut sample, mut events) = controller(&temp);
        sample.run_tests(&keys(&["cart"]), false);
        run_until(&mut sample, &mut events, |sample| !sample.tests_active()).await;
        sample.run_test("npm:cart", "rounds");
        assert_eq!(sample.suite("npm:cart").unwrap().rerun, ["rounds"]);
        assert_eq!(log(&sample).lines().last(), Some("Running 1 test: rounds"));
        sample.run_test("npm:cart", "adds");
        assert_eq!(log(&sample).lines().last(), Some("cart is already running."));
        run_until(&mut sample, &mut events, |sample| !sample.tests_active()).await;
        let cart = sample.suite("npm:cart").unwrap();
        assert_eq!((cart.passed, cart.failed, cart.rerun.len()), (2, 0, 0));
        assert!(log(&sample).contains("✓ cart (1 test again) - 2 passed"), "{}", log(&sample));
    }

    #[tokio::test]
    async fn suites_without_packages_are_said_once_and_untested_ones_never_run() {
        let temp = TempDir::new("suites-packages");
        let (mut sample, _events) = controller(&temp);
        let nx = |name: &str| {
            Suite::new(format!("nx:{name}"), name, Runner::Nx { workspace: temp.join("spa"), project: name.into() })
        };
        let mut empty = nx("models");
        empty.untested = true;
        sample.suites = Some(vec![nx("core"), nx("settings"), empty]);
        sample.run_tests(&["nx:core".into(), "nx:settings".into(), "nx:models".into()], false);
        assert!(!sample.tests_active(), "nothing queued");
        let log = log(&sample);
        let lines: Vec<&str> = log.lines().collect();
        assert_eq!(lines.len(), 1, "one line for the folder: {lines:?}");
        assert!(lines[0].starts_with("2 suites can't run: "), "{}", lines[0]);
        assert!(lines[0].ends_with("has no node_modules - i installs the packages"), "{}", lines[0]);
        assert!(sample.suite("nx:core").unwrap().needs_packages.is_some());
        assert_eq!(sample.suite("nx:core").unwrap().status, Status::Idle, "not red");
    }

    #[tokio::test]
    async fn npm_ci_pauses_its_folders_suites_and_a_stop_puts_the_queued_back() {
        let temp = TempDir::new("suites-stop");
        let (mut sample, _events) = controller(&temp);
        sample.run_tests(&keys(&["cart", "checkout"]), false);
        sample.pause_npm_suites(&temp.join("cart"));
        assert_eq!(sample.suite("npm:cart").unwrap().note, "waits for npm ci");
        assert_eq!(sample.test_queue, keys(&["cart", "checkout"]), "first again");
        sample.stop_tests(false);
        assert!(!sample.tests_active());
        assert_eq!(sample.suite("npm:cart").unwrap().status, Status::Idle, "as before");
        assert_eq!(log(&sample).lines().last(), Some("■ Tests stopped"));
    }
}
