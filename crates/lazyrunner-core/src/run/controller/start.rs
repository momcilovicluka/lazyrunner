//! Starting: apps that need no build start at once; the ones that build go through the build queue one at a
//! time (skipped when nothing changed) and start together once all are built. Also npm ci and port conflicts.

use std::path::Path;
use std::time::Instant;

use sha2::{Digest, Sha256};

use super::{Conflict, Controller, Timeline, ToolJob};
use crate::output::Hue;
use crate::profile::{AppKind, join};
use crate::run::app::Status;
use crate::run::build::{self, ToolLine};
use crate::run::child::Child;
use crate::run::net::Listening;
use crate::run::{Tag, format_duration, tools};

/// A build cannot start (no dotnet, say): it fails like a failed build.
const MAX_BUILD_ERROR_LINES: usize = 6;
/// `y` copies at most this many errors of a failed build.
const MAX_COPIED_BUILD_ERRORS: usize = 200;
/// How long a stopped app's port may stay held.
const STOP_SETTLE: std::time::Duration = std::time::Duration::from_secs(3);

impl Controller {
    /// Which variant each npm app runs on this checkout, and so its port.
    pub(super) fn update_modes(&mut self) {
        for app in self.apps.clone().iter().filter(|app| app.traits().variants) {
            let mode = tools::mode_of(&self.root, app, &self.apps);
            self.ports.insert(app.key.clone(), mode.as_ref().and_then(|mode| mode.port));
            self.modes.insert(app.key.clone(), mode);
        }
    }

    /// A running npm app whose branch now needs another of its variants.
    pub(super) fn mode_changed(&self, key: &str) -> bool {
        let Some(state) = self.state(key) else { return false };
        let variants = self.app(key).is_some_and(|app| app.traits().variants);
        state.is_running() && variants && state.script.as_deref() != self.mode(key).map(|mode| mode.script.as_str())
    }

    fn since_start(&self) -> Option<f64> {
        self.action_started.map(|started| ((self.now)() - started).as_secs_f64())
    }

    fn timeline_entry(&mut self, key: &str) -> &mut Timeline {
        self.timeline.entry(key.to_string()).or_default()
    }

    /// U, u, and `lazyrunner start`: starts `only` (all when `None`) that isn't running, building first what needs it.
    pub fn start(&mut self, only: Option<&[String]>, force_build: bool) {
        if self.building.is_some() {
            self.start_during_build(only);
            return;
        }
        self.update_modes();
        self.test_lock_changed();
        let mut keys: Vec<String> = self
            .keys()
            .into_iter()
            .filter(|key| only.is_none_or(|only| only.contains(key)) && !self.is_running(key))
            .collect();
        for key in keys.clone() {
            let variants = self.app(&key).is_some_and(|app| app.traits().variants);
            if variants && self.mode(&key).is_none() {
                let (name, reason) = self.skip_reason(&key);
                self.add_log(format!("{name} skipped: {reason}"), Hue::Yellow);
                keys.retain(|other| *other != key);
            }
        }
        if let Some(job) = self.npm_ci.as_mut().filter(|job| keys.contains(&job.key)) {
            job.start_after = true; // its node_modules is being rewritten
            let key = job.key.clone();
            let name = self.app(&key).map(|app| app.name.clone()).unwrap_or_default();
            self.add_log(format!("{name} starts after npm ci"), Hue::DarkGray);
            keys.retain(|other| *other != key);
            if keys.is_empty() {
                return;
            }
        }
        if keys.is_empty() {
            self.add_log("Already running.", Hue::Gray);
            return;
        }
        if self.find_port_conflicts(&keys) {
            return;
        }
        self.action_started = Some((self.now)());
        self.timeline.clear();
        self.ready_pending.clear();
        self.last_build_failed = false;

        let direct: Vec<String> = keys.iter().filter(|key| !self.builds(key)).cloned().collect();
        for key in &direct {
            self.start_app(key);
        }
        if !direct.is_empty() {
            let names = self.names(&direct);
            self.add_log(format!("Starting {names}"), Hue::Cyan);
        }
        let apis: Vec<String> = keys.iter().filter(|key| self.builds(key)).cloned().collect();
        let started: Vec<String> = direct.iter().filter(|key| self.is_running(key)).cloned().collect();
        self.ready_pending.extend(started);
        if !apis.is_empty() {
            self.code_at_start = self.code.seen();
            self.fingerprint = build::fingerprint(&self.root, &self.project.source_dir);
            self.start_keys = keys;
            self.built.clear();
            self.force_build = force_build;
            self.build_queue = apis;
            self.next_build();
        }
    }

    pub fn builds(&self, key: &str) -> bool {
        self.app(key).is_some_and(|app| app.traits().builds)
    }

    fn skip_reason(&self, key: &str) -> (String, String) {
        let app = self.app(key).expect("a known app");
        (app.name.clone(), crate::apptypes::no_mode_reason(&self.root, app, &self.apps))
    }

    /// During a build only apps that need no build can start: builds run one at a time.
    fn start_during_build(&mut self, only: Option<&[String]>) {
        let keys: Vec<String> =
            self.keys().into_iter().filter(|key| only.is_none_or(|only| only.contains(key))).collect();
        for key in keys {
            let name = self.app(&key).map(|app| app.name.clone()).unwrap_or_default();
            if self.is_running(&key) {
                self.add_log(format!("{name} is already running."), Hue::Gray);
            } else if !self.builds(&key) {
                self.start_without_build(&key);
            } else if self.building.as_ref().is_some_and(|job| job.key == key) {
                self.add_log(format!("{name} is building right now."), Hue::Gray);
            } else if self.build_queue.contains(&key) {
                self.add_log(format!("{name} builds and starts after the current build."), Hue::Gray);
            } else {
                self.add_log(
                    format!("{name} can't build while another build runs (one at a time) - press u again afterwards"),
                    Hue::Yellow,
                );
            }
        }
    }

    fn start_without_build(&mut self, key: &str) {
        self.update_modes();
        let variants = self.app(key).is_some_and(|app| app.traits().variants);
        if variants && self.mode(key).is_none() {
            let (name, reason) = self.skip_reason(key);
            self.add_log(format!("{name} skipped: {reason}"), Hue::Yellow);
            return;
        }
        let name = self.app(key).map(|app| app.name.clone()).unwrap_or_default();
        if let Some(job) = self.npm_ci.as_mut().filter(|job| job.key == key) {
            job.start_after = true;
            self.add_log(format!("{name} starts after npm ci"), Hue::DarkGray);
            return;
        }
        if self.find_port_conflicts(&[key.to_string()]) {
            return;
        }
        self.start_app(key);
        self.add_log(format!("Starting {name}"), Hue::Cyan);
        if !self.ready_pending.iter().any(|pending| pending == key) {
            self.ready_pending.push(key.to_string());
        }
    }

    /// Starts one app's server.
    pub(super) fn start_app(&mut self, key: &str) {
        let Some(index) = self.index(key) else { return };
        let since = self.since_start();
        if let Some(since) = since {
            self.timeline_entry(key).start_from = Some(since);
        }
        let now = (self.now)();
        self.states[index].reset_for_start();
        let app = self.apps[index].clone();
        let command = tools::server_command(&self.root, &app, &self.apps);
        let generation = self.states[index].generation;
        let started = command.and_then(|command| {
            let header = format!("──── ▶ {} · {} · {} ────", clock(), self.branch, command.label);
            self.states[index].add_line(&header, Hue::Cyan, now);
            let tag = Tag::App { key: key.to_string(), generation };
            let child = Child::spawn(&command.spec, tag, self.events.clone(), self.guard.as_ref())?;
            Ok((command, child))
        });
        let state = &mut self.states[index];
        match started {
            Ok((command, child)) => {
                state.child = Some(child);
                state.status = Status::Running;
                state.started_at = Some(now);
                state.started_wall = Some(std::time::SystemTime::now());
                state.label = Some(command.label);
                state.script = command.script;
                self.update_metric_roots();
            }
            Err(error) => {
                state.status = Status::Error(error.to_string());
                state.add_line(&format!("✗ {error}"), Hue::Red, now);
                self.add_log(format!("✗ {}: {error}", app.name), Hue::Red);
            }
        }
    }

    // ---------------------------------------------------------------- port conflicts

    /// Whether anything else holds a port these apps need (each said in the log).
    fn find_port_conflicts(&mut self, keys: &[String]) -> bool {
        let mut listening = Listening::now();
        let recently_stopped = self.last_stop.is_some_and(|at| (self.now)() - at < STOP_SETTLE);
        let held =
            |listening: &Listening| keys.iter().any(|key| self.port(key).is_some_and(|port| listening.contains(port)));
        if recently_stopped && held(&listening) {
            // Killing a tree is asynchronous: give the old server a moment to let go of its port.
            let deadline = Instant::now() + STOP_SETTLE;
            while held(&listening) && Instant::now() < deadline {
                std::thread::sleep(std::time::Duration::from_millis(50));
                listening = Listening::now();
            }
        }
        self.conflicts = keys
            .iter()
            .filter_map(|key| {
                let port = self.port(key)?;
                let owner = listening.ports.get(&port)?;
                Some(Conflict { key: key.clone(), port, pid: owner.pid, name: owner.name.clone() })
            })
            .collect();
        for conflict in self.conflicts.clone() {
            let text = format!(
                "Port {} is already used by {} (PID {}) - press ! to kill it and start",
                conflict.port, conflict.name, conflict.pid
            );
            self.add_log(text, Hue::Red);
        }
        !self.conflicts.is_empty()
    }

    /// !: kills what holds the ports, then starts.
    pub fn resolve_port_conflicts(&mut self) {
        let conflicts = std::mem::take(&mut self.conflicts);
        if conflicts.is_empty() {
            return;
        }
        for conflict in conflicts {
            if conflict.pid <= 4 {
                let text =
                    format!("Port {} belongs to the system (PID {}); free it manually.", conflict.port, conflict.pid);
                self.add_log(text, Hue::Red);
                continue;
            }
            if kill_process_tree(conflict.pid) {
                let text = format!("Killed {} (PID {}) to free port {}", conflict.name, conflict.pid, conflict.port);
                self.add_log(text, Hue::Yellow);
            } else {
                let text = format!(
                    "Couldn't kill {} (PID {}) on port {} - free it manually",
                    conflict.name, conflict.pid, conflict.port
                );
                self.add_log(text, Hue::Red);
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
        self.start(None, false);
    }

    // ---------------------------------------------------------------- builds

    /// Builds the next app in the queue; once all are built, starts them.
    pub(super) fn next_build(&mut self) {
        while let Some(key) = self.build_queue.first().cloned() {
            match self.build_app(&key) {
                BuildStep::Started => return,
                BuildStep::Skipped => {
                    self.build_queue.remove(0);
                    self.built.push(key);
                }
                BuildStep::Failed => {
                    self.build_queue.clear();
                    self.built.clear();
                    self.ready_pending.clear();
                    self.add_log("APIs not started - fix the build and press U or R.", Hue::Yellow);
                    return;
                }
            }
        }
        let apis = std::mem::take(&mut self.built);
        for key in &apis {
            self.start_app(key);
        }
        if !apis.is_empty() {
            let names = self.names(&apis);
            self.add_log(format!("Starting {names}"), Hue::Cyan);
        }
        let started: Vec<String> = apis.into_iter().filter(|key| self.is_running(key)).collect();
        for key in started {
            if !self.ready_pending.contains(&key) {
                self.ready_pending.push(key);
            }
        }
        self.code.handled_up_to(self.code_at_start); // what the build picked up; edits saved since still count
    }

    fn build_app(&mut self, key: &str) -> BuildStep {
        let app = self.app(key).expect("a known app").clone();
        let AppKind::Dotnet { project, .. } = &app.kind else { return BuildStep::Skipped };
        let skip = self.setting("skipUnchangedBuilds")
            && build::is_up_to_date(&self.state_dir, &self.root, key, project, self.fingerprint.as_deref());
        if !self.force_build && skip {
            self.add_log(
                format!("✓ {} unchanged since its last build - skipped (b / B force a rebuild)", app.name),
                Hue::Green,
            );
            return BuildStep::Skipped;
        }
        let spec = tools::build_command(&self.root, project);
        let file = Path::new(project).file_name().map_or(String::new(), |name| name.to_string_lossy().into_owned());
        self.pane(key, &format!("──── ⚙ {} · dotnet build {file} ────", clock()), Hue::Cyan);
        self.failed_build_errors.remove(key);
        let total = build::project_count(&self.root.join(project));
        self.next_job += 1;
        let id = self.next_job;
        match Child::spawn(
            &spec,
            Tag::Build { key: key.to_string(), job: id },
            self.events.clone(),
            self.guard.as_ref(),
        ) {
            Ok(child) => {
                let started = (self.now)();
                if let Some(since) = self.since_start() {
                    self.timeline_entry(key).build_from = Some(since);
                }
                self.building = Some(ToolJob {
                    key: key.to_string(),
                    id,
                    child,
                    started,
                    lines: Vec::new(),
                    done: 0,
                    total,
                    warnings: 0,
                    start_after: false,
                });
                BuildStep::Started
            }
            Err(error) => {
                self.pane(key, &format!("✗ {error}"), Hue::Red);
                self.add_log(format!("✗ {} build failed: dotnet couldn't start ({error})", app.name), Hue::Red);
                BuildStep::Failed
            }
        }
    }

    pub(super) fn build_output(&mut self, key: &str, id: u64, line: &str) {
        let Some(job) = self.building.as_mut().filter(|job| job.key == key && job.id == id) else { return };
        job.lines.push(line.to_string());
        let text = crate::output::strip_ansi(line);
        match build::tool_line(&text, true) {
            ToolLine::Warning => job.warnings += 1,
            ToolLine::Built(project) => {
                job.done += 1;
                self.pane(key, &format!("  ✓ {project}"), Hue::DarkGreen);
            }
            ToolLine::Shown(text, hue) => self.pane(key, &text, hue),
        }
    }

    pub(super) fn build_exited(&mut self, key: &str, id: u64, code: Option<i32>) {
        if !self.building.as_ref().is_some_and(|job| job.key == key && job.id == id) {
            return; // a cancelled build
        }
        let job = self.building.take().expect("checked above");
        let seconds = ((self.now)() - job.started).as_secs_f64();
        let log = self.state_dir.join(format!("build-{key}.log"));
        let _ = std::fs::write(&log, job.lines.join("\n") + "\n");
        if let Some(since) = self.since_start() {
            self.timeline_entry(key).build_to = Some(since);
        }
        let app = self.app(key).expect("a known app").clone();
        let whole = seconds as u64;
        if code != Some(0) {
            self.pane(key, &format!("✗ build failed after {whole}s"), Hue::Red);
            let errors = build::errors(&job.lines);
            let full: Vec<String> = errors
                .iter()
                .map(|line| crate::output::strip_ansi(line).trim().to_string())
                .take(MAX_COPIED_BUILD_ERRORS)
                .collect();
            self.failed_build_errors.insert(key.to_string(), full);
            self.last_failed_log = Some(log);
            self.last_build_failed = true;
            self.notifications
                .push((format!("{} build failed", app.name), format!("{} errors - see lazyrunner", errors.len())));
            self.add_log(
                format!("✗ {} build failed after {whole}s ({} errors) - o opens the full log", app.name, errors.len()),
                Hue::Red,
            );
            for error in errors.iter().take(MAX_BUILD_ERROR_LINES) {
                self.add_log(format!("    {}", build::format_error(error)), Hue::Red);
            }
            self.build_queue.clear();
            self.built.clear();
            self.ready_pending.clear();
            self.add_log("APIs not started - fix the build and press U or R.", Hue::Yellow);
            return;
        }
        if let AppKind::Dotnet { project, .. } = &app.kind {
            build::save_stamp(&self.state_dir, &self.root, key, project, self.fingerprint.as_deref());
        }
        self.timings.add("build", key, seconds);
        let warnings = if job.warnings > 0 { format!(", {} warnings", job.warnings) } else { String::new() };
        let summary = format!("built in {whole}s ({} projects{warnings})", job.done);
        self.pane(key, &format!("✓ {summary}"), Hue::Green);
        self.add_log(format!("✓ {} {summary}", app.name), Hue::Green);
        self.build_queue.retain(|queued| queued != key);
        self.built.push(key.to_string());
        self.next_build();
    }

    /// Cancels the running build: `drop_only` (s on that app) takes just it out of the start; otherwise the whole
    /// start stops.
    /// S while building: cancels the build and quietly stops what this start already started.
    pub fn cancel_start(&mut self) {
        self.cancel_build(false);
    }

    pub(super) fn cancel_build(&mut self, drop_only: bool) {
        let Some(job) = self.building.take() else { return };
        job.child.kill();
        let key = job.key.clone();
        let name = self.app(&key).map(|app| app.name.clone()).unwrap_or_default();
        let _ = std::fs::write(self.state_dir.join(format!("build-{key}.log")), job.lines.join("\n") + "\n");
        self.pane(&key, "■ build cancelled", Hue::Yellow);
        if drop_only {
            self.add_log(format!("■ {name} build cancelled (s) - it stays stopped"), Hue::Yellow);
            self.build_queue.retain(|queued| *queued != key);
            self.next_build();
        } else {
            self.add_log(format!("■ {name} build cancelled"), Hue::Yellow);
            self.build_queue.clear();
            self.built.clear();
            let keys = std::mem::take(&mut self.start_keys);
            self.stop(Some(&keys), true);
        }
    }

    // ---------------------------------------------------------------- after a database switch

    /// The APIs read secrets.json only when they start, and run with --no-build: restarting the processes is enough.
    pub fn restart_api_processes(&mut self, keys: &[String], reason: &str) {
        let names = self.names(keys);
        self.add_log(format!("Restarting {names} ({reason} - no build needed)"), Hue::Cyan);
        self.stop(Some(keys), true);
        if self.building.is_none() {
            self.action_started = Some((self.now)());
            self.timeline.clear();
            self.ready_pending.clear();
        }
        for key in keys {
            self.start_app(key);
            if !self.ready_pending.contains(key) {
                self.ready_pending.push(key.clone());
            }
        }
    }

    // ---------------------------------------------------------------- npm ci

    fn npm_ci_stamp(&self, key: &str) -> std::path::PathBuf {
        self.state_dir.join(format!("npm-ci.{key}.stamp.json"))
    }

    fn lock_hash(&self, key: &str) -> Option<String> {
        let AppKind::Npm { dir, .. } = &self.app(key)?.kind else { return None };
        let text = std::fs::read_to_string(self.root.join(dir).join("package-lock.json")).ok()?;
        Some(Sha256::digest(text.replace("\r\n", "\n").as_bytes()).iter().map(|b| format!("{b:02X}")).collect())
    }

    /// The npm apps whose package-lock.json changed since their last npm ci (the panel offers i).
    pub fn lock_changed_keys(&self) -> &[String] {
        &self.lock_changed
    }

    /// Whether an npm app's packages match its package-lock.json as lazyrunner's last npm ci installed it (`None`:
    /// no npm ci from lazyrunner in this checkout to compare with).
    pub fn lock_in_sync(&self, key: &str) -> Option<bool> {
        let stamp = crate::json::read(&self.npm_ci_stamp(key))?;
        if crate::json::text(&stamp["root"]) != self.root.display().to_string() {
            return None;
        }
        Some(self.lock_hash(key).is_some_and(|hash| crate::json::text(&stamp["lockHash"]) == hash))
    }

    /// The npm apps whose package-lock.json differs from the one their last npm ci installed (said once per change).
    pub fn test_lock_changed(&mut self) {
        let mut changed = Vec::new();
        for app in self.apps.clone().iter().filter(|app| app.traits().installs) {
            let stamp = crate::json::read(&self.npm_ci_stamp(&app.key));
            let (Some(stamp), Some(hash)) = (stamp, self.lock_hash(&app.key)) else { continue };
            let same_root = crate::json::text(&stamp["root"]) == self.root.display().to_string();
            if !same_root || crate::json::text(&stamp["lockHash"]) == hash {
                continue;
            }
            changed.push(app.key.clone());
            if self.lock_warned.get(&app.key) == Some(&hash) {
                continue;
            }
            self.lock_warned.insert(app.key.clone(), hash);
            if let AppKind::Npm { dir, .. } = &app.kind {
                let lock = join(dir, "package-lock.json");
                self.add_log(
                    format!("{lock} changed since the last npm ci - press i to reinstall its packages"),
                    Hue::Yellow,
                );
            }
        }
        self.lock_changed = changed;
    }

    /// i: npm ci for an npm app (the focused one, or the one whose lock file changed), in the background.
    pub fn npm_ci(&mut self, focused: Option<&str>) {
        let Some(key) = self.npm_ci_key(focused) else {
            self.add_log(
                "There is no npm app (the project has none, or lazyrunner.config.json turns them off).",
                Hue::Yellow,
            );
            return;
        };
        if let Some(job) = &self.npm_ci {
            let name = self.app(&job.key).map(|app| app.name.clone()).unwrap_or_default();
            self.add_log(format!("npm ci is already running ({name})."), Hue::Gray);
            return;
        }
        let app = self.app(&key).expect("a known app").clone();
        let AppKind::Npm { dir, .. } = &app.kind else { return };
        let folder = self.root.join(dir);
        if !folder.join("package-lock.json").is_file() {
            self.add_log(format!("No package-lock.json in {} - npm ci needs one.", folder.display()), Hue::Yellow);
            return;
        }
        let was_running = self.is_running(&key);
        if was_running {
            self.stop(Some(std::slice::from_ref(&key)), true); // a running dev server locks files in node_modules
        }
        self.pane(&key, &format!("──── ⚙ {} · npm ci ────", clock()), Hue::Cyan);
        let beside = if self.building.is_some() { " (next to the build)" } else { "" };
        self.add_log(format!("Running npm ci for {}…{beside}", app.name), Hue::Cyan);
        stop_nx_daemon(&folder);
        let spawned = tools::npm_ci_command(&folder).and_then(|spec| {
            Ok(Child::spawn(
                &spec,
                Tag::NpmCi { key: key.clone(), job: self.next_job + 1 },
                self.events.clone(),
                self.guard.as_ref(),
            )?)
        });
        match spawned {
            Ok(child) => {
                let started = (self.now)();
                self.next_job += 1;
                self.npm_ci = Some(ToolJob {
                    key,
                    id: self.next_job,
                    child,
                    started,
                    lines: Vec::new(),
                    done: 0,
                    total: 0,
                    warnings: 0,
                    start_after: was_running,
                });
            }
            Err(error) => self.add_log(format!("✗ npm ci couldn't start: {error}"), Hue::Red),
        }
    }

    /// Which app i installs: one whose lock file changed (the focused one if it is among them), else the focused
    /// one, else one whose node_modules is missing, else the first.
    /// The app i reinstalls: one whose package-lock.json changed (the focused one first), else the focused npm app,
    /// else one without node_modules, else the first.
    pub fn npm_ci_key(&self, focused: Option<&str>) -> Option<String> {
        let keys: Vec<String> =
            self.apps.iter().filter(|app| app.traits().installs).map(|app| app.key.clone()).collect();
        if !self.lock_changed.is_empty() {
            return Some(
                focused
                    .filter(|key| self.lock_changed.iter().any(|k| k == key))
                    .map_or_else(|| self.lock_changed[0].clone(), str::to_string),
            );
        }
        if let Some(focused) = focused.filter(|key| keys.iter().any(|k| k == key)) {
            return Some(focused.to_string());
        }
        let missing = keys.iter().find(|key| {
            self.app(key).is_some_and(|app| matches!(&app.kind, AppKind::Npm { dir, .. } if !self.root.join(dir).join("node_modules").exists()))
        });
        missing.or(keys.first()).cloned()
    }

    pub(super) fn npm_ci_output(&mut self, key: &str, id: u64, line: &str) {
        let Some(job) = self.npm_ci.as_mut().filter(|job| job.key == key && job.id == id) else { return };
        job.lines.push(line.to_string());
        match build::tool_line(&crate::output::strip_ansi(line), false) {
            ToolLine::Shown(text, hue) => {
                if hue == Hue::Yellow {
                    job.warnings += 1;
                }
                self.pane(key, &text, hue);
            }
            ToolLine::Warning | ToolLine::Built(_) => {}
        }
    }

    pub(super) fn npm_ci_exited(&mut self, key: &str, id: u64, code: Option<i32>) {
        if self.npm_ci.as_ref().is_some_and(|job| job.key == key && job.id == id) {
            self.complete_npm_ci(if code == Some(0) { NpmCiEnd::Ok } else { NpmCiEnd::Failed });
        }
    }

    /// S (when Stop all includes its app), s on its app and q cancel it.
    pub(super) fn stop_npm_ci(&mut self) {
        if let Some(job) = &self.npm_ci {
            job.child.kill();
            self.complete_npm_ci(NpmCiEnd::Cancelled);
        }
    }

    fn complete_npm_ci(&mut self, end: NpmCiEnd) {
        let Some(job) = self.npm_ci.take() else { return };
        let key = job.key.clone();
        let name = self.app(&key).map(|app| app.name.clone()).unwrap_or_default();
        let log = self.state_dir.join(format!("npm-ci-{key}.log"));
        let _ = std::fs::write(&log, job.lines.join("\n") + "\n");
        let seconds = ((self.now)() - job.started).as_secs_f64();
        let waiting =
            if job.start_after { format!(" - {name} was waiting for it and stays stopped") } else { String::new() };
        match end {
            NpmCiEnd::Cancelled => {
                self.pane(&key, "■ npm ci cancelled - node_modules may be incomplete, press i again", Hue::Yellow);
                self.add_log(
                    format!("■ npm ci cancelled - node_modules may be incomplete, press i again{waiting}"),
                    Hue::Yellow,
                );
            }
            NpmCiEnd::Failed => {
                self.last_failed_log = Some(log);
                self.notifications.push(("npm ci failed".into(), format!("See the {name} column in lazyrunner")));
                self.pane(&key, &format!("✗ npm ci failed after {}s", seconds as u64), Hue::Red);
                self.add_log(
                    format!("✗ npm ci failed after {}s - o opens the full log{waiting}", seconds as u64),
                    Hue::Red,
                );
            }
            NpmCiEnd::Ok => {
                let stamp =
                    serde_json::json!({ "root": self.root.display().to_string(), "lockHash": self.lock_hash(&key) });
                let _ = crate::config::write(&self.npm_ci_stamp(&key), &stamp);
                self.lock_changed.retain(|changed| *changed != key);
                self.timings.add("npmci", &key, seconds);
                self.pane(&key, &format!("✓ npm ci done in {}", format_duration(seconds)), Hue::Green);
                self.add_log(format!("✓ npm ci done in {}", format_duration(seconds)), Hue::Green);
                if job.start_after {
                    self.start(Some(&[key]), false);
                }
            }
        }
    }
}

enum BuildStep {
    Started,
    Skipped,
    Failed,
}

enum NpmCiEnd {
    Ok,
    Failed,
    Cancelled,
}

/// The time of day, as pane headers show it.
fn clock() -> String {
    chrono::Local::now().format("%H:%M:%S").to_string()
}

/// Workspace tools can leave a daemon running from node_modules (Nx does), which keeps files locked while npm ci
/// replaces them; stop it first when there is one.
fn stop_nx_daemon(dir: &Path) {
    let nx = dir.join("node_modules").join("nx").join("bin").join("nx.js");
    let Some(node) = tools::find_on_path("node").filter(|_| nx.is_file()) else { return };
    let mut command = std::process::Command::new(node);
    command.arg(nx).args(["daemon", "--stop"]).current_dir(dir);
    command.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    let Ok(mut process) = command.spawn() else {
        return;
    };
    let deadline = Instant::now() + std::time::Duration::from_secs(10);
    while Instant::now() < deadline {
        if !matches!(process.try_wait(), Ok(None)) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let _ = process.kill();
}

/// Kills another program's process tree (what holds a port lazyrunner needs); true when it is gone.
fn kill_process_tree(pid: u32) -> bool {
    #[cfg(windows)]
    {
        std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .output()
            .is_ok_and(|output| output.status.success())
    }
    #[cfg(unix)]
    {
        // SAFETY: sends a signal to the process the user chose to kill.
        unsafe { libc::kill(pid as i32, libc::SIGKILL) == 0 }
    }
}
