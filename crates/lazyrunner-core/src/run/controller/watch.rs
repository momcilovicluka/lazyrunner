//! Watching what runs: readiness (listening, or a watch build written), unexpected exits, "Ready in", response
//! probes, port snapshots, the branch, code changes, live CPU and memory, and the database label.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::mpsc::UnboundedSender;

use super::{Controller, Probe};
use crate::config::Toggle;
use crate::output::Hue;
use crate::profile::{self, AppKind, trust};
use crate::run::app::{Status, push_history};
use crate::run::history::Kind;
use crate::run::net::{self, Listening, ProbeResult};
use crate::run::{Event, format_duration, secrets, tools};

const LISTENING_EVERY: Duration = Duration::from_secs(1);
/// A port snapshot that hasn't come back by then never will (its task died): ask again.
const LISTENING_STALL: Duration = Duration::from_secs(10);
const SLOW_TICK_EVERY: Duration = Duration::from_secs(1);
const DB_LABEL_EVERY: Duration = Duration::from_secs(5);
const PROBE_EVERY: Duration = Duration::from_secs(5);
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
const METRICS_EVERY: Duration = Duration::from_secs(2);
/// A branch switch is acted on once the checkout has been quiet this long; a code change likewise.
const BRANCH_SETTLE: Duration = Duration::from_secs(3);
const CODE_SETTLE: Duration = Duration::from_secs(2);
const TIMELINE_WIDTH: usize = 24;
const CELEBRATE_FOR: Duration = Duration::from_secs(4);
/// A start that took this long gets a notification when it's ready.
const NOTIFY_READY_AFTER: Duration = Duration::from_secs(20);

/// Saved sources not acted on yet.
#[derive(Debug, Default)]
pub(super) struct CodeChanges {
    count: u64,
    handled: u64,
    last_at: Option<Instant>,
    last_file: Option<PathBuf>,
}

impl CodeChanges {
    pub(super) fn changed(&mut self, path: PathBuf, now: Instant) {
        self.count += 1;
        self.last_at = Some(now);
        self.last_file = Some(path);
    }

    pub(super) fn handled(&mut self) {
        self.handled = self.count;
    }

    /// How many changes have been seen so far.
    pub(super) fn seen(&self) -> u64 {
        self.count
    }

    /// Marks the changes up to `count` as handled (later ones still are not).
    pub(super) fn handled_up_to(&mut self, count: u64) {
        self.handled = self.handled.max(count);
    }
}

/// Samples CPU and memory of each app's process tree in a thread of its own.
pub(super) fn spawn_metrics(roots: Arc<Mutex<Vec<(String, u32)>>>, events: UnboundedSender<Event>) {
    std::thread::spawn(move || {
        let mut sampler = crate::metrics::TreeSampler::new();
        loop {
            std::thread::sleep(METRICS_EVERY);
            let wanted = roots.lock().map(|roots| roots.clone()).unwrap_or_default();
            let samples = if wanted.is_empty() { Vec::new() } else { sampler.sample(&wanted) };
            if events.send(Event::Metrics(samples)).is_err() {
                return;
            }
        }
    });
}

impl Controller {
    /// The process trees to sample.
    pub(super) fn update_metric_roots(&mut self) {
        let roots: Vec<(String, u32)> =
            self.states.iter().filter_map(|state| Some((state.key.clone(), state.child.as_ref()?.pid))).collect();
        if let Ok(mut shared) = self.metric_roots.lock() {
            *shared = roots;
        }
    }

    pub(super) fn metrics(&mut self, samples: Vec<(String, f32, f32)>) {
        let seconds = METRICS_EVERY.as_secs_f32();
        for state in &mut self.states {
            let Some((_, cpu, memory)) = samples.iter().find(|(key, _, _)| *key == state.key) else {
                state.cpu = None;
                continue;
            };
            state.cpu = Some(*cpu);
            state.memory_mb = Some(*memory);
            push_history(&mut state.cpu_history, *cpu);
            push_history(&mut state.memory_history, *memory);
            push_history(&mut state.request_history, state.requests as f32 / seconds);
            state.requests = 0;
        }
    }

    /// A fresh snapshot of the listening ports, in the background (`now`: don't wait for the next second).
    pub(super) fn request_listening(&mut self, now: bool) {
        let at = (self.now)();
        let stalled = self.listening_in_flight
            && self.listening_asked_at.is_some_and(|asked| at.saturating_duration_since(asked) >= LISTENING_STALL);
        if stalled {
            self.listening_in_flight = false;
            self.add_log("The port snapshot didn't come back - asking again", Hue::DarkGray);
        }
        if self.listening_in_flight || (!now && self.listening_due.is_some_and(|due| at < due)) {
            return;
        }
        self.listening_in_flight = true;
        self.listening_asked_at = Some(at);
        self.listening_due = Some(at + LISTENING_EVERY);
        let events = self.events.clone();
        tokio::task::spawn_blocking(move || {
            let _ = events.send(Event::Listening(Listening::now()));
        });
    }

    /// The timed work: call it a few times a second.
    pub fn tick(&mut self) {
        let now = (self.now)();
        if let Some((only, force_build)) = self.waiting_start.take() {
            self.retrying_start = true;
            self.start(only.as_deref(), force_build); // parks again while the ports are still held
            self.retrying_start = false;
        }
        self.request_listening(false);
        self.watch_states();
        self.schedule_probes(now);
        if !self.test_queue.is_empty() {
            self.pump_tests(); // what waited for a build or npm ci
        }
        self.pump_migrations();
        if self.slow_tick_due.is_none_or(|due| now >= due) {
            self.slow_tick_due = Some(now + SLOW_TICK_EVERY);
            if self.building.is_none() {
                self.update_branch(now);
                self.test_code_change(now);
            }
            for state in &mut self.states {
                state.flush_log();
            }
        }
        if self.db_due.is_none_or(|due| now >= due) {
            self.db_due = Some(now + DB_LABEL_EVERY);
            self.refresh_db_label();
        }
        if self.celebrate.as_ref().is_some_and(|(_, until)| now >= *until) {
            self.celebrate = None;
        }
    }

    pub(super) fn app_exited(&mut self, key: &str, generation: u64, code: Option<i32>) {
        let Some(index) = self.index(key) else { return };
        let state = &mut self.states[index];
        if state.generation != generation || !state.is_running() {
            return;
        }
        let now = (self.now)();
        state.child = None;
        state.status = Status::Exited(code);
        let code_text = code.map_or_else(|| "?".to_string(), |code| code.to_string());
        state.add_line(&format!("■ exited with code {code_text}"), Hue::Red, now);
        state.flash_until = Some(now + Duration::from_secs(3));
        let name = self.apps[index].name.clone();
        self.add_log(format!("✗ {name} exited unexpectedly (code {code_text}) - see its column"), Hue::Red);
        self.notifications.push((format!("{name} stopped unexpectedly"), format!("Exit code {code_text}")));
        self.ready_pending.clear();
        self.update_metric_roots();
    }

    /// Up: listening on its port; a watch build has no port, so it is up once it has written its ready file.
    fn is_ready(&self, index: usize) -> bool {
        let key = &self.apps[index].key;
        if let Some(port) = self.port(key) {
            // Only a snapshot asked for after the start counts: an earlier one can still show the old server.
            let fresh = self
                .listening_at
                .zip(self.states[index].started_at)
                .is_some_and(|(snapshot, started)| snapshot >= started);
            return fresh && self.listening.contains(port);
        }
        let (Some(mode), AppKind::Npm { dir, .. }) = (self.mode(key), &self.apps[index].kind) else { return false };
        let Some(ready) = &mode.ready_file else { return false };
        let written = std::fs::metadata(self.root.join(dir).join(ready)).and_then(|meta| meta.modified()).ok();
        written.zip(self.states[index].started_wall).is_some_and(|(written, started)| written > started)
    }

    /// Marks the apps that got up, and says "Ready" once everything this start launched is up.
    pub(super) fn watch_states(&mut self) {
        let now = (self.now)();
        for index in 0..self.apps.len() {
            let state = &self.states[index];
            if !state.is_running() || state.up_generation == Some(state.generation) || !self.is_ready(index) {
                continue;
            }
            let timed = state.started_at.is_some(); // without a start time there is nothing to record
            let up_after = state.started_at.map_or(0.0, |started| (now - started).as_secs_f64());
            let key = self.apps[index].key.clone();
            let generation = state.generation;
            let state = &mut self.states[index];
            state.up_generation = Some(generation);
            state.up_after = Some(up_after);
            if let Some(started) = self.action_started {
                self.timeline.entry(key.clone()).or_default().up_at = Some((now - started).as_secs_f64());
            }
            self.timings.add("start", &key, up_after);
            let note = if timed { self.record_duration(Kind::Start, &key, up_after, true) } else { String::new() };
            let name = self.apps[index].name.clone();
            let what = match self.port(&key) {
                Some(port) => format!("is listening on :{port}"),
                None => {
                    let server = self.mode(&key).and_then(|mode| mode.served_by.clone()).unwrap_or_default();
                    let server_name = self.app(&server).map(|app| app.name.clone()).unwrap_or(server.clone());
                    let url =
                        format!("http://localhost:{}", self.port(&server).map_or(String::new(), |p| p.to_string()));
                    format!("is built (served by {server_name} at {url})")
                }
            };
            self.add_log(format!("● {name} {what} - up in {}{note}", format_duration(up_after)), Hue::Green);
        }
        // Ready once every app this start launched is up in its current run, and nothing is still building.
        if self.building.is_some() || !self.build_queue.is_empty() || self.ready_pending.is_empty() {
            return;
        }
        if self.ready_pending.iter().any(|key| !self.state(key).is_some_and(|state| state.is_up())) {
            return;
        }
        let seconds = self.action_started.map_or(0.0, |started| (now - started).as_secs_f64());
        let pending = std::mem::take(&mut self.ready_pending);
        self.add_log(format!("Ready in {}", format_duration(seconds)), Hue::Green);
        self.write_timeline(&pending, seconds);
        self.celebrate = Some((format!("ready in {}", format_duration(seconds)), now + CELEBRATE_FOR));
        if seconds >= NOTIFY_READY_AFTER.as_secs_f64() {
            let names = self.names(&pending);
            let title = format!("{} is ready", self.project.name);
            self.notifications.push((title, format!("{names} up in {}", format_duration(seconds))));
        }
    }

    /// After "Ready": one line per app, █ building, ▒ starting, · up, over the whole start.
    fn write_timeline(&mut self, keys: &[String], total: f64) {
        for key in keys {
            let Some(entry) = self.timeline.get(key).copied() else { continue };
            let (Some(start_from), Some(up_at)) = (entry.start_from, entry.up_at) else { continue };
            let cells: String = (0..TIMELINE_WIDTH)
                .map(|cell| {
                    let at = (cell as f64 + 0.5) / TIMELINE_WIDTH as f64 * total;
                    let building = entry.build_from.zip(entry.build_to).is_some_and(|(from, to)| at >= from && at < to);
                    if building {
                        '█'
                    } else if at >= start_from && at < up_at {
                        '▒'
                    } else if at >= up_at {
                        '·'
                    } else {
                        ' '
                    }
                })
                .collect();
            let build = entry
                .build_from
                .zip(entry.build_to)
                .map_or(String::new(), |(from, to)| format!("build {} · ", format_duration(to - from)));
            let name = self.app(key).map(|app| app.name.clone()).unwrap_or_default();
            let line = format!("  {name:<9}▕{cells}▏ {build}start {}", format_duration(up_at - start_from));
            self.add_log(line, Hue::DarkCyan);
        }
    }

    // ---------------------------------------------------------------- response probes

    fn schedule_probes(&mut self, now: Instant) {
        for index in 0..self.apps.len() {
            let key = self.apps[index].key.clone();
            let port = self.port(&key);
            let live = self.states[index].is_running() && port.is_some_and(|port| self.listening.contains(port));
            if !live {
                self.probes.remove(&key);
                continue;
            }
            let probe = self.probes.entry(key.clone()).or_default();
            if probe.in_flight || probe.next_at.is_some_and(|at| now < at) {
                continue;
            }
            probe.in_flight = true;
            let path = probe_path(&self.apps[index]);
            let (events, generation, port) =
                (self.events.clone(), self.states[index].generation, port.unwrap_or_default());
            tokio::spawn(async move {
                let result = net::probe(port, &path, PROBE_TIMEOUT).await;
                let _ = events.send(Event::Probe { key, generation, result });
            });
        }
    }

    /// An app answering in `ms`, as if a probe had come back (screenshots and tests have no server to probe).
    #[cfg(any(test, feature = "test-support"))]
    pub fn answered(&mut self, key: &str, ms: u64) {
        let now = (self.now)();
        let last = Some(ProbeResult { ok: true, code: 200, ms });
        self.probes.insert(key.to_string(), Probe { in_flight: false, last, next_at: Some(now + PROBE_EVERY) });
    }

    pub(super) fn probe_result(&mut self, key: &str, generation: u64, result: ProbeResult) {
        let now = (self.now)();
        let Some(index) = self.index(key) else { return };
        if self.states[index].generation != generation || !self.probes.contains_key(key) {
            return;
        }
        let previous = self.probes.get(key).and_then(|probe| probe.last);
        self.probes
            .insert(key.to_string(), Probe { in_flight: false, last: Some(result), next_at: Some(now + PROBE_EVERY) });
        let ms = if result.ok { result.ms as f32 } else { PROBE_TIMEOUT.as_millis() as f32 };
        push_history(&mut self.states[index].response_history, ms);
        // Only transitions, so lazyrunner.log records when an app hung and when it came back.
        let name = self.apps[index].name.clone();
        match previous {
            None if result.ok => self.add_log(format!("● {name} responding ({} ms)", result.ms), Hue::Green),
            Some(previous) if previous.ok != result.ok && result.ok => {
                self.add_log(format!("● {name} responding again ({} ms)", result.ms), Hue::Green);
            }
            Some(previous) if previous.ok != result.ok => {
                let text = format!("✗ {name} is not responding (no answer within {}s)", PROBE_TIMEOUT.as_secs());
                self.add_log(text, Hue::Red);
            }
            _ => {}
        }
    }

    /// The last probe of an app, for its title.
    pub fn last_probe(&self, key: &str) -> Option<ProbeResult> {
        self.probes.get(key).and_then(|probe| probe.last)
    }

    // ---------------------------------------------------------------- the branch

    fn update_branch(&mut self, now: Instant) {
        let Some(git_dir) = self.git_dir.clone() else { return };
        let branch = tools::branch(&git_dir).unwrap_or_default();
        if branch.is_empty() || branch == self.branch {
            self.pending_branch = None;
            return;
        }
        match &self.pending_branch {
            Some((pending, since)) if *pending == branch => {
                // Wait until the checkout has settled before acting on it.
                if now - *since < BRANCH_SETTLE || git_dir.join("index.lock").exists() {
                    return;
                }
            }
            _ => {
                self.pending_branch = Some((branch, now));
                return;
            }
        }
        let previous = std::mem::replace(&mut self.branch, branch.clone());
        self.pending_branch = None;
        self.test_profile_change();
        self.update_modes();
        self.test_lock_changed();
        self.migrations_changed();
        let running: Vec<String> = self.keys().into_iter().filter(|key| self.is_running(key)).collect();
        if !self.setting(Toggle::AutoRestartOnBranchChange) {
            self.add_log(format!("Branch changed {previous} → {branch} (auto-restart is off, press R)"), Hue::Yellow);
            return;
        }
        if running.is_empty() {
            self.add_log(
                format!("Branch changed {previous} → {branch} (nothing running, not restarting)"),
                Hue::Yellow,
            );
            return;
        }
        // Only the running apps that build restart: npm's own watcher picks up the others' changes, unless the
        // branch needs another variant.
        let (restart, keep): (Vec<String>, Vec<String>) = running.into_iter().partition(|key| {
            !self.app(key).is_some_and(|app| app.traits().watches_own_files) || self.mode_changed(key)
        });
        for key in keep {
            let name = self.app(&key).map(|app| app.name.clone()).unwrap_or_default();
            let script = self.mode(&key).map(|mode| mode.script.clone()).unwrap_or_default();
            self.add_log(format!("{name} keeps running ({script} watches the files itself)"), Hue::DarkGray);
        }
        if !restart.is_empty() {
            self.restart(&format!("branch changed {previous} → {branch}"), Some(&restart), false);
        }
    }

    /// A branch can change the repo's .lazyrunner.json, or add or remove one: say what changed (lazyrunner keeps
    /// running with the profile it has).
    fn test_profile_change(&mut self) {
        let dirs = self.paths.profiles.clone();
        let now = profile::find_project_profile(&self.root, &dirs)
            .and_then(|found| found.map(|found| profile::resolve(&found, &dirs)).transpose())
            .map_err(|error| error.to_string());
        let trusted = now
            .as_ref()
            .ok()
            .and_then(Option::as_ref)
            .is_none_or(|now| trust::is_trusted(now, &self.paths.trust_store()));
        let active = self.keys();
        let active: Vec<&str> = active.iter().map(String::as_str).collect();
        if let Some(change) = profile::change::describe(
            &self.project,
            &active,
            now.as_ref().map(Option::as_ref).map_err(Clone::clone),
            trusted,
        ) {
            self.profile_changed = true;
            let hue = if change.tone == profile::change::Tone::Error { Hue::Red } else { Hue::Yellow };
            self.add_log(change.text, hue);
        }
    }

    // ---------------------------------------------------------------- code changes

    /// restartOnCodeChange: once saving has been quiet for a moment, rebuild and restart the running apps that build.
    fn test_code_change(&mut self, now: Instant) {
        if self.code.count == self.code.handled {
            return;
        }
        if !self.setting(Toggle::RestartOnCodeChange) {
            self.code.handled();
            return;
        }
        if self.code.last_at.is_some_and(|at| now - at < CODE_SETTLE) {
            return;
        }
        self.code.handled();
        let apis: Vec<String> = self
            .apps
            .iter()
            .filter(|app| app.traits().builds && self.is_running(&app.key))
            .map(|app| app.key.clone())
            .collect();
        if !apis.is_empty() {
            let file = self
                .code
                .last_file
                .as_ref()
                .and_then(|file| file.file_name())
                .map_or(String::new(), |name| name.to_string_lossy().into_owned());
            self.restart(&format!("code changed: {file}"), Some(&apis), false);
        }
    }

    // ---------------------------------------------------------------- the database

    pub(super) fn refresh_db_label(&mut self) {
        self.db_label =
            secrets::secrets_path(&self.root, &self.apps).and_then(|path| match std::fs::read_to_string(path) {
                Ok(text) => secrets::active_label(&text),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(_) => Some("secrets.json unreadable".into()),
            });
    }

    /// d: makes `entry` the active connection string, then restarts the APIs that read it.
    pub fn switch_database(&mut self, entry: &secrets::Entry) {
        let Some(path) = secrets::secrets_path(&self.root, &self.apps) else {
            self.add_log("No secrets.json: no API has a UserSecretsId.", Hue::Yellow);
            return;
        };
        if let Err(error) = secrets::switch(&path, entry) {
            self.add_log(format!("✗ {error}"), Hue::Red);
            return;
        }
        self.refresh_db_label();
        self.migrations_changed();
        self.add_log(
            format!(
                "Database → {}  ({}; previous file saved as secrets.json.lazyrunner-backup)",
                entry.label, entry.key
            ),
            Hue::Cyan,
        );
        let running: Vec<String> = self
            .apps
            .iter()
            .filter(|app| app.traits().uses_secrets && self.is_running(&app.key))
            .map(|app| app.key.clone())
            .collect();
        if !running.is_empty() {
            self.restart_api_processes(&running, "database switched");
        }
    }
}

/// What the response-time probe asks an app for: its Swagger page, else its root.
pub(super) fn probe_path(app: &crate::profile::App) -> String {
    match &app.kind {
        AppKind::Dotnet { swagger: Some(swagger), .. } => format!("{swagger}/index.html"),
        _ => "/".into(),
    }
}
