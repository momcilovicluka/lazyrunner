//! Profiling an app (#83) in the controller: `Alt+p` samples a running .NET app's CPU with dotnet-trace in the
//! background - the process holding its port, the app itself rather than `dotnet run` - and keeps the capture; `Alt+p`
//! again stops it. The startup checklist installs dotnet-trace when it's missing.

use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Instant;

use super::Controller;
use crate::output::Hue;
use crate::run::Event;
use crate::run::profile::{self, KEPT, Profile};

/// The shortest and longest capture.
pub const LEAST_SECONDS: u64 = 3;
pub const MOST_SECONDS: u64 = 120;

/// A capture under way: the app's key, when it started and for how long, dotnet-trace's pid while it runs, and
/// whether it was stopped.
#[derive(Debug)]
pub struct Profiling {
    pub key: String,
    pub started: Instant,
    pub seconds: u64,
    running: Arc<AtomicU32>,
    stopped: Arc<AtomicBool>,
}

impl Controller {
    fn name_of(&self, key: &str) -> String {
        self.app(key).map_or_else(|| key.to_string(), |app| app.name.clone())
    }

    /// The process to sample: the one holding the app's port - the app, not `dotnet run` - else its own.
    fn sampled_pid(&self, index: usize) -> Option<u32> {
        let key = &self.apps[index].key;
        let holder = self.port(key).and_then(|port| self.listening.ports.get(&port)).map(|owner| owner.pid);
        holder.or_else(|| self.states[index].child.as_ref().map(|child| child.pid))
    }

    /// Samples app `key`'s CPU for `seconds`: what it does, or why it can't.
    pub fn profile(&mut self, key: &str, seconds: u64) -> Result<String, String> {
        if let Some(job) = &self.profiling {
            return Err(format!("{} is being profiled already - Alt+p stops it.", self.name_of(&job.key)));
        }
        let Some(index) = self.index(key) else { return Err(format!("No app {key} here.")) };
        let name = self.apps[index].name.clone();
        let node = matches!(self.apps[index].kind, crate::profile::AppKind::Npm { .. });
        if !self.apps[index].is_dotnet() && !node {
            return Err(format!("{name} isn't a .NET or an npm app: profiling is for those."));
        }
        if !self.states[index].is_up() {
            return Err(format!("{name} isn't running - start it, then profile it."));
        }
        let tool = profile::tool();
        let Some(tool) = tool.or_else(|| node.then(std::path::PathBuf::new)) else {
            return Err(format!(
                "dotnet-trace isn't installed: {} (f on its line in the startup checklist does it).",
                profile::INSTALL
            ));
        };
        let Some(pid) = self.sampled_pid(index) else { return Err(format!("{name}'s process isn't known yet.")) };
        let (running, stopped) = (Arc::new(AtomicU32::new(0)), Arc::new(AtomicBool::new(false)));
        let (events, state, owned) = (self.events.clone(), self.state_dir.clone(), key.to_string());
        let (sampling, asked) = (running.clone(), stopped.clone());
        tokio::task::spawn_blocking(move || {
            let at = || chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
            let result = if node {
                profile::ask_node(&state, pid, seconds, &asked)
                    .and_then(|text| profile::from_cpuprofile(&text, &owned, &at(), seconds))
            } else {
                profile::capture(&tool, pid, seconds, &state.join("profiles"), &sampling)
                    .and_then(|text| profile::from_speedscope(&text, &owned, &at(), seconds))
            };
            let _ = events.send(Event::Profiled { key: owned, result: result.map(Box::new) });
        });
        let started = (self.now)();
        self.profiling = Some(Profiling { key: key.to_string(), started, seconds, running, stopped });
        let text = format!("◉ Profiling {name} for {seconds} s - make the slow request meanwhile; Alt+p stops it.");
        self.add_log(text.clone(), Hue::Magenta);
        Ok(text)
    }

    /// `Alt+p` while a capture runs: it stops, nothing kept.
    pub fn stop_profile(&mut self) {
        let Some(job) = &self.profiling else { return };
        job.stopped.store(true, Ordering::SeqCst);
        let pid = job.running.load(Ordering::SeqCst);
        if pid != 0 {
            super::start::kill_process_tree(pid);
        }
        let name = self.name_of(&job.key);
        self.add_log(format!("Profiling {name} stopped - nothing kept."), Hue::DarkGray);
    }

    pub(super) fn profiled(&mut self, key: String, result: Result<Box<Profile>, String>) {
        let Some(job) = self.profiling.take() else { return };
        if job.stopped.load(Ordering::SeqCst) {
            return;
        }
        let name = self.name_of(&key);
        match result {
            Ok(captured) => {
                let captured = *captured;
                if let Err(why) = profile::save(&self.state_dir, &captured) {
                    self.add_log(format!("✗ The profile of {name} wasn't kept: {why}"), Hue::Red);
                }
                let total = captured.total();
                let hottest = captured.top(1).first().map_or(String::new(), |hot| {
                    format!(" · hottest {} ({:.0}%)", profile::short(&hot.name), hot.own / total * 100.0)
                });
                let text = format!("✓ {name} profiled: {total:.0} ms on the CPU in {} s{hottest}", job.seconds);
                self.add_log(text, Hue::Green);
                let kept = self.profiles.entry(key.clone()).or_default();
                kept.push(captured);
                if kept.len() > KEPT {
                    kept.remove(0);
                }
                self.last_profiled = Some((key, (self.now)()));
            }
            Err(why) => self.add_log(format!("✗ Profiling {name}: {why}"), Hue::Red),
        }
    }

    /// How long a capture samples: `"profileSeconds"`, 10 when it's missing (3 to 120).
    pub fn profile_seconds(&self) -> u64 {
        self.config["profileSeconds"].as_u64().unwrap_or(profile::SECONDS).clamp(LEAST_SECONDS, MOST_SECONDS)
    }

    /// `+` and `-` in the flame chart: how long a capture samples, saved.
    pub fn set_profile_seconds(&mut self, seconds: u64) {
        let seconds = seconds.clamp(LEAST_SECONDS, MOST_SECONDS);
        self.set_setting("profileSeconds", serde_json::json!(seconds));
        self.add_log(format!("Profiling: {seconds} s a capture"), Hue::Gray);
    }

    /// App `key`'s kept captures, the oldest first.
    pub fn profiles_of(&self, key: &str) -> &[Profile] {
        self.profiles.get(key).map_or(&[], Vec::as_slice)
    }

    /// The startup checklist's fix: installs dotnet-trace in the background.
    pub fn install_trace(&mut self) {
        if self.installing_trace {
            return;
        }
        self.installing_trace = true;
        let (events, dotnet) = (self.events.clone(), crate::run::tools::dotnet());
        tokio::task::spawn_blocking(move || {
            let result = Command::new(dotnet)
                .args(["tool", "install", "-g", "dotnet-trace"])
                .output()
                .map_err(|error| error.to_string())
                .and_then(|output| {
                    let said =
                        String::from_utf8_lossy(&output.stdout).into_owned() + &String::from_utf8_lossy(&output.stderr);
                    let last =
                        said.lines().map(str::trim).rfind(|line| !line.is_empty()).unwrap_or_default().to_string();
                    if output.status.success() { Ok(()) } else { Err(last) }
                });
            let _ = events.send(Event::TraceInstalled { result });
        });
        self.add_log(format!("Installing dotnet-trace ({})…", profile::INSTALL), Hue::Cyan);
    }

    pub(super) fn trace_installed(&mut self, result: Result<(), String>) {
        self.installing_trace = false;
        match result {
            Ok(()) => self.add_log("✓ dotnet-trace installed: Alt+p profiles a running .NET app.", Hue::Green),
            Err(why) => self.add_log(format!("✗ dotnet-trace didn't install: {why}"), Hue::Red),
        }
    }
}
