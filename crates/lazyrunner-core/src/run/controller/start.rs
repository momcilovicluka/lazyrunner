//! Starting: apps that need no build start at once; the ones that build go through the build queue one at a
//! time (skipped when nothing changed) and start together once all are built. Also npm ci and port conflicts.

use super::{Conflict, Controller, Timeline};
use crate::output::Hue;
use crate::run::app::Status;
use crate::run::build;
use crate::run::child::Child;
use crate::run::net::{self, Listening};
use crate::run::{Tag, tools};

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

    pub(super) fn since_start(&self) -> Option<f64> {
        self.action_started.map(|started| ((self.now)() - started).as_secs_f64())
    }

    pub(super) fn timeline_entry(&mut self, key: &str) -> &mut Timeline {
        self.timeline.entry(key.to_string()).or_default()
    }

    /// U, u, and `lazyrunner start`: starts `only` (all when `None`) that isn't running, building first what needs it.
    pub fn start(&mut self, only: Option<&[String]>, force_build: bool) {
        debug_assert!(self.building.is_some() || !self.build_only, "b's flag outlives its builds");
        if self.wait_for_ports(only, force_build) {
            return;
        }
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
        let keys = self.without_reserved_ports(keys);
        if keys.is_empty() {
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
            self.build_only = false;
            self.build_queue = apis;
            self.next_build();
        }
    }

    /// Right after a stop the old servers can hold their ports a moment longer (killing a tree is asynchronous).
    /// Then the start waits for them on the tick instead of blocking, for up to `STOP_SETTLE` after the stop; past
    /// that a port still held is a conflict.
    fn wait_for_ports(&mut self, only: Option<&[String]>, force_build: bool) -> bool {
        let settling = self.last_stop.is_some_and(|at| (self.now)().saturating_duration_since(at) < STOP_SETTLE);
        if !settling {
            return false;
        }
        self.update_modes(); // this checkout's ports (a branch can change an npm app's variant)
        let ports: Vec<u16> = self
            .keys()
            .into_iter()
            .filter(|key| only.is_none_or(|only| only.contains(key)) && !self.is_running(key))
            .filter_map(|key| self.port(&key))
            .collect();
        if ports.is_empty() {
            return false;
        }
        let listening = Listening::now();
        let held: Vec<String> =
            ports.iter().filter(|port| listening.contains(**port)).map(|port| format!(":{port}")).collect();
        if held.is_empty() {
            return false;
        }
        if !self.retrying_start {
            self.add_log(format!("Waiting for {} to be let go…", held.join(" ")), Hue::DarkGray);
        }
        self.park_start(only, force_build);
        true
    }

    /// Parks a start until the ports are free; one parked already takes its apps in too.
    pub(super) fn park_start(&mut self, only: Option<&[String]>, force_build: bool) {
        self.waiting_start = Some(match (self.waiting_start.take(), only) {
            (Some((Some(mut parked), parked_force)), Some(only)) => {
                for key in only {
                    if !parked.contains(key) {
                        parked.push(key.clone());
                    }
                }
                (Some(parked), parked_force || force_build)
            }
            (Some((_, parked_force)), _) => (None, parked_force || force_build),
            (None, only) => (only.map(<[String]>::to_vec), force_build),
        });
    }

    pub fn builds(&self, key: &str) -> bool {
        self.app(key).is_some_and(|app| app.traits().builds)
    }

    pub(super) fn skip_reason(&self, key: &str) -> (String, String) {
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
            } else if self.build_only && self.building.as_ref().is_some_and(|job| job.key == key) {
                self.add_log(format!("{name} is being built only (b) - press u once the build is done"), Hue::Gray);
            } else if self.building.as_ref().is_some_and(|job| job.key == key) {
                self.add_log(format!("{name} is building right now."), Hue::Gray);
            } else if self.build_queue.contains(&key) && self.build_only {
                self.add_log(format!("{name} is being built only (b) - press u once the build is done"), Hue::Gray);
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

    pub(super) fn start_without_build(&mut self, key: &str) {
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
        if self.without_reserved_ports(vec![key.to_string()]).is_empty() {
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
        let command = self.commands.server(&self.root, &app, &self.apps);
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

    /// The apps but those whose port Windows reserved: they couldn't listen on it, so they don't start, and the log
    /// says why and what frees it. The others start.
    fn without_reserved_ports(&mut self, keys: Vec<String>) -> Vec<String> {
        let ranges = (self.reserved)();
        if ranges.is_empty() {
            return keys;
        }
        let mut starting = Vec::new();
        for key in keys {
            let port = self.port(&key);
            let Some((port, range)) = port.and_then(|port| net::reserved(port, &ranges).map(|range| (port, range)))
            else {
                starting.push(key);
                continue;
            };
            let name = self.app(&key).map_or_else(|| key.clone(), |app| app.name.clone());
            self.add_log(format!("✗ {name} can't start: {}", net::reserved_advice(port, range)), Hue::Red);
        }
        starting
    }

    /// Whether anything else holds a port these apps need (each said in the log).
    fn find_port_conflicts(&mut self, keys: &[String]) -> bool {
        let listening = Listening::now();
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

    /// !: kills what holds the ports (if it still does), then starts once they are free.
    pub fn resolve_port_conflicts(&mut self) {
        let conflicts = std::mem::take(&mut self.conflicts);
        if conflicts.is_empty() {
            return;
        }
        let listening = Listening::now();
        for conflict in conflicts {
            // The conflict may be minutes old: kill only what still holds the port, never a reused PID.
            match listening.ports.get(&conflict.port) {
                Some(owner) if owner.pid == conflict.pid => {}
                Some(owner) => {
                    let text = format!(
                        "Port {} is held by {} (PID {}) now - checking again",
                        conflict.port, owner.name, owner.pid
                    );
                    self.add_log(text, Hue::DarkGray);
                    continue;
                }
                None => {
                    self.add_log(format!("Port {} is free now", conflict.port), Hue::DarkGray);
                    continue;
                }
            }
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
        self.last_stop = Some((self.now)()); // what was killed lets go of its ports in a moment: the start waits
        self.start(None, false);
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
}

/// The time of day, as pane headers show it.
pub(super) fn clock() -> String {
    chrono::Local::now().format("%H:%M:%S").to_string()
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
