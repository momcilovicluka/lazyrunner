//! The controller: everything lazyrunner does while it runs a project, as the PowerShell version's panel did it,
//! but event-driven. Commands (start, stop, restart, ...) come from the UI; what happens elsewhere comes back as
//! [`Event`]s; `tick` does the timed things. Nothing here blocks for long: builds, probes and port snapshots run
//! in tasks of their own, so keys and output keep flowing during a build.

mod handover;
mod start;
#[cfg(test)]
mod tests;
mod watch;

use std::collections::{HashMap, VecDeque};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::Result;
use serde_json::{Value, json};
use tokio::sync::mpsc::UnboundedSender;

use super::app::{AppState, Status};
use super::build::Timings;
use super::child::{Child, Guard};
use super::history::{Durations, Kind};
use super::net::{Listening, ProbeResult};
use super::watcher::CodeWatcher;
use super::{Event, Tag};
use crate::apptypes::{self, Mode};
use crate::config::ConfigFile;
use crate::output::Hue;
use crate::paths::Paths;
use crate::profile::{App, Project};

pub use handover::{HANDOVER_VERSION, Handover, Passed, Releasing, read as read_handover};

/// Entries the panel's log keeps.
const MAX_LOG_ENTRIES: usize = 200;

/// A line in the panel's log (and lazyrunner.log).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogEntry {
    pub time: String,
    pub text: String,
    pub hue: Hue,
}

/// A port an app needs that something else holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict {
    pub key: String,
    pub port: u16,
    pub pid: u32,
    pub name: String,
}

/// A running build (one at a time) or npm ci (one at a time, next to a build).
pub struct ToolJob {
    pub key: String,
    pub id: u64,
    pub child: Child,
    pub started: Instant,
    pub lines: Vec<String>,
    /// Projects built so far, and how many the build compiles (0: unknown).
    pub done: usize,
    pub total: usize,
    pub warnings: usize,
    /// npm ci: start the app once it's done.
    pub start_after: bool,
}

/// When each app of the current start built, started and got up (seconds since the start), for the timeline.
#[derive(Clone, Copy, Debug, Default)]
struct Timeline {
    build_from: Option<f64>,
    build_to: Option<f64>,
    start_from: Option<f64>,
    up_at: Option<f64>,
}

#[derive(Debug, Default)]
struct Probe {
    in_flight: bool,
    last: Option<ProbeResult>,
    next_at: Option<Instant>,
}

/// What the controller needs to run a project.
pub struct Setup {
    pub project: Project,
    pub root: PathBuf,
    pub paths: Paths,
    pub events: UnboundedSender<Event>,
    /// Kills what lazyrunner started if it dies (Linux, macOS); `None` in tests.
    pub guard: Option<Arc<Guard>>,
}

pub struct Controller {
    pub project: Project,
    /// The apps on on this machine, in column order; `states` matches it.
    pub apps: Vec<App>,
    pub states: Vec<AppState>,
    pub root: PathBuf,
    pub branch: String,
    pub config: Value,
    pub log: VecDeque<LogEntry>,
    pub listening: Listening,
    pub conflicts: Vec<Conflict>,
    pub building: Option<ToolJob>,
    pub build_queue: Vec<String>,
    pub npm_ci: Option<ToolJob>,
    pub db_label: Option<String>,
    /// "ready in 12s", shown for a moment after a start.
    pub celebrate: Option<(String, Instant)>,
    pub profile_changed: bool,
    pub last_failed_log: Option<PathBuf>,
    /// Whether the last build of the current start failed (the tab turns red).
    pub last_build_failed: bool,
    /// The errors of each app's last failed build, with their full paths (`y` copies them; the columns show them
    /// shortened). Gone once that app builds again.
    pub failed_build_errors: HashMap<String, Vec<String>>,
    /// Desktop notifications to show (title, message); the UI takes them and shows them when the setting is on.
    pub notifications: Vec<(String, String)>,
    git_dir: Option<PathBuf>,
    pending_branch: Option<(String, Instant)>,
    config_file: ConfigFile,
    paths: Paths,
    state_dir: PathBuf,
    log_path: PathBuf,
    events: UnboundedSender<Event>,
    guard: Option<Arc<Guard>>,
    ports: HashMap<String, Option<u16>>,
    modes: HashMap<String, Option<Mode>>,
    ready_pending: Vec<String>,
    action_started: Option<Instant>,
    timeline: HashMap<String, Timeline>,
    /// The current start: all its keys (a cancelled build stops them), the APIs built so far, the build fingerprint.
    start_keys: Vec<String>,
    built: Vec<String>,
    force_build: bool,
    fingerprint: Option<String>,
    lock_changed: Vec<String>,
    lock_warned: HashMap<String, String>,
    timings: Timings,
    /// How long builds, starts and npm ci took over their last runs (D).
    pub durations: Durations,
    probes: HashMap<String, Probe>,
    listening_due: Option<Instant>,
    listening_in_flight: bool,
    /// When the snapshot in `listening` was asked for (an app started after that isn't in it yet).
    listening_at: Option<Instant>,
    listening_asked_at: Option<Instant>,
    /// When apps were last stopped: their ports can take a moment to free.
    last_stop: Option<Instant>,
    next_job: u64,
    slow_tick_due: Option<Instant>,
    db_due: Option<Instant>,
    code: watch::CodeChanges,
    /// The code changes seen when the current start's builds began.
    code_at_start: u64,
    _watcher: Option<CodeWatcher>,
    metric_roots: Arc<Mutex<Vec<(String, u32)>>>,
    pub(crate) now: fn() -> Instant,
}

impl Controller {
    /// Ready to run `setup.project`'s apps in `setup.root`: its state folder, its apps' log files, the watchers.
    pub fn new(setup: Setup) -> Result<Self> {
        let Setup { project, root, paths, events, guard } = setup;
        let config_file = ConfigFile::new(&paths.config_file);
        let config = config_file.load(Some(&project.id));
        let apps = project.active_apps(&config, &config_file.path)?;
        let state_dir = paths.project_state(&project.id)?;
        let log_path = paths.state_dir.join("lazyrunner.log");
        let mut states: Vec<AppState> = apps.iter().map(|app| AppState::new(&app.key)).collect();
        for state in &mut states {
            state.open_log(&state_dir.join("logs"));
        }
        let watcher = super::watcher::watch(&root.join(&project.source_dir), events.clone());
        let metric_roots = Arc::new(Mutex::new(Vec::new()));
        watch::spawn_metrics(Arc::clone(&metric_roots), events.clone());
        let mut controller = Self {
            timings: Timings::load(&state_dir),
            durations: Durations::load(&state_dir),
            project,
            apps,
            states,
            root: PathBuf::new(),
            branch: String::new(),
            config,
            log: VecDeque::new(),
            listening: Listening::default(),
            conflicts: Vec::new(),
            building: None,
            build_queue: Vec::new(),
            npm_ci: None,
            db_label: None,
            celebrate: None,
            profile_changed: false,
            last_failed_log: None,
            last_build_failed: false,
            failed_build_errors: HashMap::new(),
            notifications: Vec::new(),
            git_dir: None,
            pending_branch: None,
            config_file,
            paths,
            state_dir,
            log_path,
            events,
            guard,
            ports: HashMap::new(),
            modes: HashMap::new(),
            ready_pending: Vec::new(),
            action_started: None,
            timeline: HashMap::new(),
            start_keys: Vec::new(),
            built: Vec::new(),
            force_build: false,
            fingerprint: None,
            lock_changed: Vec::new(),
            lock_warned: HashMap::new(),
            probes: HashMap::new(),
            listening_due: None,
            listening_in_flight: false,
            listening_at: None,
            listening_asked_at: None,
            last_stop: None,
            next_job: 0,
            slow_tick_due: None,
            db_due: None,
            code: watch::CodeChanges::default(),
            code_at_start: 0,
            _watcher: watcher,
            metric_roots,
            now: Instant::now,
        };
        if let Some(problem) = controller.durations.problem.take() {
            controller.add_log(problem, Hue::Yellow);
        }
        if controller._watcher.is_none() && root.join(&controller.project.source_dir).is_dir() {
            let dir = root.join(&controller.project.source_dir);
            controller.add_log(
                format!("Can't watch {} for code changes (restart on code change is off)", dir.display()),
                Hue::Yellow,
            );
        }
        controller.set_root(&root);
        Ok(controller)
    }

    /// Runs the project in `root` (another worktree of it, say).
    pub fn set_root(&mut self, root: &Path) {
        self.root = root.to_path_buf();
        self.git_dir = super::tools::git_dir(root);
        self.branch = self.git_dir.as_deref().and_then(super::tools::branch).unwrap_or_default();
        self.pending_branch = None;
        self.update_modes();
        for app in &self.apps {
            if !app.traits().variants {
                self.ports.insert(app.key.clone(), apptypes::port(root, app, &self.apps));
            }
        }
        self.refresh_db_label();
    }

    // ---------------------------------------------------------------- lookups

    pub fn paths(&self) -> &Paths {
        &self.paths
    }

    pub fn index(&self, key: &str) -> Option<usize> {
        self.apps.iter().position(|app| app.key == key)
    }

    pub fn app(&self, key: &str) -> Option<&App> {
        self.apps.iter().find(|app| app.key == key)
    }

    pub fn state(&self, key: &str) -> Option<&AppState> {
        self.index(key).map(|index| &self.states[index])
    }

    fn state_mut(&mut self, key: &str) -> Option<&mut AppState> {
        self.index(key).map(|index| &mut self.states[index])
    }

    pub fn keys(&self) -> Vec<String> {
        self.apps.iter().map(|app| app.key.clone()).collect()
    }

    pub fn is_running(&self, key: &str) -> bool {
        self.state(key).is_some_and(AppState::is_running)
    }

    /// The port an app listens on in this checkout (its variant's, for an npm app).
    pub fn port(&self, key: &str) -> Option<u16> {
        self.ports.get(key).copied().flatten()
    }

    pub fn mode(&self, key: &str) -> Option<&Mode> {
        self.modes.get(key).and_then(Option::as_ref)
    }

    /// The names of these apps, as the log says them: "Orders + Billing".
    pub fn names(&self, keys: &[String]) -> String {
        keys.iter().filter_map(|key| self.app(key)).map(|app| app.name.as_str()).collect::<Vec<_>>().join(" + ")
    }

    pub fn timing_estimate(&self, kind: &str, key: &str) -> Option<f64> {
        self.timings.estimate(kind, key)
    }

    /// Records a finished run in the durations history. Returns what a successful run's log line adds when it was
    /// notably off its usual (" · 18% slower than usual (36s)").
    fn record_duration(&mut self, kind: Kind, key: &str, seconds: f64, ok: bool) -> String {
        let label = self.app(key).map_or_else(|| key.to_string(), |app| app.name.clone());
        if let Some(why) = self.durations.add(kind, key, &label, seconds, ok) {
            self.add_log(format!("Couldn't save the durations history - {why}"), Hue::Yellow);
        }
        if ok { self.durations.note(kind, key) } else { String::new() }
    }

    pub fn is_busy(&self) -> bool {
        self.building.is_some()
    }

    // ---------------------------------------------------------------- the log

    /// A line in the panel's log and in lazyrunner.log.
    pub fn add_log(&mut self, text: impl Into<String>, hue: Hue) {
        let text = text.into();
        let now = chrono::Local::now();
        let time = now.format("%H:%M:%S").to_string();
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&self.log_path) {
            let _ = writeln!(file, "{} {time} {text}", now.format("%Y-%m-%d"));
        }
        self.log.push_back(LogEntry { time, text, hue });
        while self.log.len() > MAX_LOG_ENTRIES {
            self.log.pop_front();
        }
    }

    fn pane(&mut self, key: &str, text: &str, hue: Hue) {
        let now = (self.now)();
        if let Some(state) = self.state_mut(key) {
            state.add_line(text, hue, now);
        }
    }

    // ---------------------------------------------------------------- events

    pub fn handle(&mut self, event: Event) {
        let now = (self.now)();
        match event {
            Event::Output { tag: Tag::App { key, generation }, line } => {
                let Some(index) = self.index(&key) else { return };
                if self.states[index].generation == generation {
                    let app = self.apps[index].clone();
                    self.states[index].receive(&app, &line, now);
                }
            }
            Event::Output { tag: Tag::Build { key, job }, line } => self.build_output(&key, job, &line),
            Event::Output { tag: Tag::NpmCi { key, job }, line } => self.npm_ci_output(&key, job, &line),
            Event::Exited { tag: Tag::App { key, generation }, code } => self.app_exited(&key, generation, code),
            Event::Exited { tag: Tag::Build { key, job }, code } => self.build_exited(&key, job, code),
            Event::Exited { tag: Tag::NpmCi { key, job }, code } => self.npm_ci_exited(&key, job, code),
            Event::Listening(listening) => {
                self.listening = listening;
                self.listening_in_flight = false;
                self.listening_at = self.listening_asked_at;
                self.watch_states();
            }
            Event::Probe { key, generation, result } => self.probe_result(&key, generation, result),
            Event::Metrics(samples) => self.metrics(samples),
            Event::FileChanged(path) => self.code.changed(path, now),
        }
    }

    // ---------------------------------------------------------------- stopping

    /// Stops one app's process tree; false when it wasn't running.
    fn stop_app(&mut self, key: &str, reason: &str) -> bool {
        let Some(index) = self.index(key) else { return false };
        let Some(child) = self.states[index].child.take() else { return false };
        child.kill();
        drop(child);
        self.states[index].status = Status::Stopped;
        if let Some(port) = self.port(key) {
            self.listening.ports.remove(&port);
        }
        self.last_stop = Some((self.now)());
        self.pane(key, &format!("■ {reason}"), Hue::DarkGray);
        self.update_metric_roots();
        true
    }

    /// Stops `only` (all when `None`), and says what it stopped unless `quiet`.
    pub fn stop(&mut self, only: Option<&[String]>, quiet: bool) {
        let keys: Vec<String> =
            self.keys().into_iter().filter(|key| only.is_none_or(|only| only.contains(key))).collect();
        if only.is_none() {
            self.conflicts.clear();
        }
        self.ready_pending.retain(|key| !keys.contains(key));
        for key in &keys {
            self.probes.remove(key); // results in flight belong to the old servers
        }
        let stopped: Vec<String> = keys.iter().filter(|key| self.stop_app(key, "stopped")).cloned().collect();
        self.request_listening(true);
        if quiet {
            return;
        }
        if stopped.is_empty() {
            self.add_log("Nothing was running.", Hue::Gray);
        } else {
            let names = self.names(&stopped);
            self.add_log(format!("Stopped {names}."), Hue::Gray);
        }
    }

    /// The apps Stop all and Restart all leave running (includeInAll, stopFrontendOnStop). A restart still restarts
    /// one whose branch needs another variant.
    fn kept_keys(&mut self, for_restart: bool) -> Vec<String> {
        let mut kept: Vec<String> = self
            .apps
            .iter()
            .filter(|app| !apptypes::in_stop_all(&self.config, app))
            .map(|app| app.key.clone())
            .collect();
        if for_restart && !kept.is_empty() {
            self.update_modes();
            kept.retain(|key| !self.mode_changed(key));
        }
        kept
    }

    /// S, and `lazyrunner stop`.
    pub fn stop_all(&mut self) {
        let kept = self.kept_keys(false);
        if self.building.is_some() {
            self.cancel_build(false);
        }
        if self.npm_ci.as_ref().is_some_and(|job| !kept.contains(&job.key)) {
            self.stop_npm_ci();
        }
        if kept.is_empty() {
            self.stop(None, false);
            return;
        }
        let keys: Vec<String> = self.keys().into_iter().filter(|key| !kept.contains(key)).collect();
        if !keys.is_empty() {
            self.stop(Some(&keys), false);
        }
        let running: Vec<String> = kept.iter().filter(|key| self.is_running(key)).cloned().collect();
        if !running.is_empty() {
            let names = self.names(&running);
            self.add_log(format!("{names} kept running (, then s: make Stop all include it)."), Hue::DarkGray);
        } else if keys.is_empty() {
            self.add_log("Nothing was running.", Hue::Gray);
        }
    }

    pub fn restart(&mut self, reason: &str, only: Option<&[String]>, force_build: bool) {
        let kept = if only.is_some() { Vec::new() } else { self.kept_keys(true) };
        let stop_keys: Option<Vec<String>> = match only {
            Some(only) => Some(only.to_vec()),
            None if !kept.is_empty() => Some(self.keys().into_iter().filter(|key| !kept.contains(key)).collect()),
            None => None,
        };
        let names = stop_keys.as_ref().map_or(String::new(), |keys| format!(" {}", self.names(keys)));
        self.add_log(format!("Restarting{names} ({reason})"), Hue::Cyan);
        if stop_keys.is_some() || kept.is_empty() {
            self.stop(stop_keys.as_deref(), true);
        }
        self.start(only, force_build);
    }

    /// Space: start the focused app, or stop it.
    pub fn toggle(&mut self, key: &str) {
        if self.is_running(key)
            || self.building.as_ref().is_some_and(|job| job.key == key)
            || self.build_queue.iter().any(|k| k == key)
            || self.npm_ci.as_ref().is_some_and(|job| job.key == key)
        {
            self.stop_focused(key);
        } else {
            self.start(Some(&[key.to_string()]), false);
        }
    }

    /// s: stop the focused app, or cancel its build, take it out of the build queue, or cancel its npm ci.
    pub fn stop_focused(&mut self, key: &str) {
        let name = self.app(key).map(|app| app.name.clone()).unwrap_or_default();
        if self.building.as_ref().is_some_and(|job| job.key == key) {
            self.cancel_build(true);
            return;
        }
        if self.build_queue.iter().any(|queued| queued == key) {
            self.build_queue.retain(|queued| queued != key);
            self.add_log(format!("{name} won't be built or started this time"), Hue::Yellow);
            return;
        }
        if self.npm_ci.as_ref().is_some_and(|job| job.key == key) {
            self.stop_npm_ci();
            return;
        }
        self.stop(Some(&[key.to_string()]), false);
    }

    /// Before quitting: cancel the build and npm ci and stop every app, quietly.
    pub fn shutdown(&mut self) {
        if self.building.is_some() {
            self.cancel_build(false);
        }
        self.stop_npm_ci();
        self.stop(None, true);
        for state in &mut self.states {
            state.flush_log();
        }
    }

    // ---------------------------------------------------------------- settings

    /// Saves one setting and uses it from now on.
    pub fn set_setting(&mut self, name: &str, value: Value) {
        match self.config_file.set(Some(&self.project.id), name, value) {
            Ok(config) => self.config = config,
            Err(error) => self.add_log(format!("✗ {error}"), Hue::Red),
        }
    }

    /// A setting as true or false.
    pub fn setting(&self, name: &str) -> bool {
        crate::json::truthy(crate::json::field(&self.config, name))
    }

    // ---------------------------------------------------------------- for tests and the control channel

    /// What runs, as JSON.
    pub fn state_json(&self) -> Value {
        json!({
            "project": self.project.id,
            "root": self.root.display().to_string(),
            "branch": self.branch,
            "building": self.building.as_ref().map(|job| job.key.clone()),
            "apps": self.apps.iter().zip(&self.states).map(|(app, state)| json!({
                "key": app.key,
                "name": app.name,
                "status": state.status.name(),
                "up": state.is_up(),
                "port": self.port(&app.key),
                "pid": state.child.as_ref().map(|child| child.pid),
                "lines": state.output.total(),
                "errors": state.errors,
            })).collect::<Vec<_>>(),
            "log": self.log.iter().rev().take(20).rev().map(|entry| entry.text.clone()).collect::<Vec<_>>(),
        })
    }
}
