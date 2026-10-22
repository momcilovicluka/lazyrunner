//! A handover between two lazyrunners on the same project (Windows): the running one lets go of its apps without
//! stopping them and writes what the next one needs into a file; the next one, started with the apps' handles,
//! takes them over - same processes, same ports, their output and state carried on. After an update the new version
//! takes over this way, and so does reloading the project profile (only apps whose command changed start again).
//!
//! In two steps on the running side: `release_running` stops this lazyrunner's readers (the output they had read
//! still arrives as events, which the caller handles), then `write_handover` writes the file with everything shown.

use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{Controller, LogEntry};
use crate::changelog;
use crate::output::Hue;
use crate::run::Tag;
use crate::run::app::Status;
use crate::run::child::{self, Child, Handles, Released};

/// The handover file's format; a lazyrunner reads this version and older ones (fields it doesn't find are empty).
pub const HANDOVER_VERSION: u32 = 1;
/// Lines of each column the next lazyrunner shows again.
const LINES_KEPT: usize = 500;
/// Entries of the panel's log it shows again.
const LOG_KEPT: usize = 50;
/// Carried-over lines count as old (no fade-in).
const AGED: Duration = Duration::from_secs(10);

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Handover {
    pub version: u32,
    /// The version of lazyrunner that handed over.
    pub from: String,
    pub project: String,
    pub root: String,
    pub apps: Vec<HandedApp>,
    /// Apps that were running but couldn't be handed over (they stopped): the next lazyrunner starts them again.
    pub restart: Vec<String>,
    pub log: Vec<(String, String, Hue)>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct HandedApp {
    pub key: String,
    /// What the app ran (a fingerprint of its command): another command is started again, not taken over.
    pub command: String,
    pub handles: Handles,
    pub generation: u64,
    pub running_for_ms: u64,
    pub started_wall_ms: Option<u64>,
    pub up: bool,
    pub up_after: Option<f64>,
    pub script: Option<String>,
    pub label: Option<String>,
    pub errors: u64,
    pub warnings: u64,
    pub requests: u64,
    pub lines: Vec<(String, Option<Hue>)>,
}

/// The running apps released by `release_running`, for `write_handover`.
pub struct Releasing {
    apps: Vec<(usize, Released)>,
    restart: Vec<String>,
}

/// A handover on its way: the file for the next lazyrunner, every handle value it inherits (also given on its
/// command line, so it can close them if the file is unreadable), and the released apps, whose handles stay open in
/// this process until it has started.
pub struct Passed {
    pub file: PathBuf,
    pub handles: Vec<u64>,
    _released: Vec<Released>,
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// Reads a handover file and deletes it (it holds handles meant for one lazyrunner, once).
pub fn read(path: &Path) -> Result<Handover, String> {
    let text = std::fs::read_to_string(path).map_err(|error| format!("couldn't read {} - {error}", path.display()));
    let _ = std::fs::remove_file(path);
    let handover: Handover =
        serde_json::from_str(&text?).map_err(|error| format!("the handover file is broken - {error}"))?;
    if handover.version > HANDOVER_VERSION {
        return Err(format!("it comes from a newer lazyrunner (v{}), which this one can't read", handover.from));
    }
    Ok(handover)
}

impl Controller {
    /// Why the apps can't be handed over right now, if they can't: a build, npm ci or tests would be cut off.
    pub fn handover_blocked(&self) -> Option<String> {
        if self.building.is_some() || !self.build_queue.is_empty() {
            return Some("a build is running".into());
        }
        if self.npm_ci.is_some() {
            return Some("npm ci is running".into());
        }
        self.tests_active().then(|| "tests are running".into())
    }

    /// A fingerprint of what an app runs here (its command, folder and label): another one means it changed.
    fn command_of(&self, index: usize) -> String {
        let command = self
            .commands
            .server(&self.root, &self.apps[index], &self.apps)
            .map(|command| {
                format!("{:?}|{:?}|{:?}|{}", command.spec.program, command.spec.args, command.spec.dir, command.label)
            })
            .unwrap_or_default();
        Sha256::digest(command.as_bytes()).iter().map(|byte| format!("{byte:02x}")).collect()
    }

    /// Step one: lets go of the running apps without stopping them (this lazyrunner stops reading their output).
    /// An app whose process has just exited is left as it is: its exit is on its way.
    pub fn release_running(&mut self) -> Result<Releasing, String> {
        if let Some(why) = self.handover_blocked() {
            return Err(why);
        }
        let mut apps = Vec::new();
        let mut restart = Vec::new();
        for index in 0..self.states.len() {
            let state = &mut self.states[index];
            if !state.is_running() || state.child.as_ref().is_none_or(Child::has_exited) {
                continue;
            }
            match state.child.take().map(Child::release) {
                Some(Ok(released)) => apps.push((index, released)),
                _ => restart.push(state.key.clone()), // couldn't be released: it stopped with its Child
            }
        }
        Ok(Releasing { apps, restart })
    }

    /// Step two, once the output read before step one is in: writes the handover file.
    pub fn write_handover(&mut self, releasing: Releasing) -> Result<Passed, String> {
        let now = (self.now)();
        let mut apps = Vec::new();
        let mut released = Vec::new();
        for (index, done) in releasing.apps {
            let command = self.command_of(index);
            let state = &mut self.states[index];
            state.flush_log();
            let mut lines: Vec<(String, Option<Hue>)> =
                state.output.newest_first(0).take(LINES_KEPT).map(|line| (line.text.clone(), line.hue)).collect();
            lines.reverse();
            apps.push(HandedApp {
                key: state.key.clone(),
                command,
                handles: done.handles,
                generation: state.generation,
                running_for_ms: state.started_at.map_or(0, |started| millis(now.saturating_duration_since(started))),
                started_wall_ms: state.started_wall.and_then(|wall| wall.duration_since(UNIX_EPOCH).ok()).map(millis),
                up: state.is_up(),
                up_after: state.up_after,
                script: state.script.clone(),
                label: state.label.clone(),
                errors: state.errors,
                warnings: state.warnings,
                requests: state.requests,
                lines,
            });
            state.status = Status::Stopped;
            released.push(done);
        }
        let log = self
            .log
            .iter()
            .rev()
            .take(LOG_KEPT)
            .rev()
            .map(|entry| (entry.time.clone(), entry.text.clone(), entry.hue))
            .collect();
        let handles = apps.iter().flat_map(|app| app.handles.values()).collect();
        let handover = Handover {
            version: HANDOVER_VERSION,
            from: changelog::VERSION.to_string(),
            project: self.project.id.clone(),
            root: self.root.display().to_string(),
            apps,
            restart: releasing.restart,
            log,
        };
        let file = self.state_dir.join(format!("handover-{}.json", std::process::id()));
        let text = serde_json::to_string(&handover).map_err(|error| error.to_string())?;
        std::fs::write(&file, text).map_err(|error| format!("couldn't write {} - {error}", file.display()))?;
        Ok(Passed { file, handles, _released: released })
    }

    /// Takes over what the previous lazyrunner handed over. Returns the apps to start again: the ones whose command
    /// changed (or that couldn't be taken over), which were running.
    ///
    /// # Safety
    /// The handover's handles must be ones this process inherited for it, owned by nothing else in it.
    pub unsafe fn take_over(&mut self, handover: Handover) -> Vec<String> {
        let now = (self.now)();
        let mut restart = handover.restart.clone();
        let mut taken = Vec::new();
        let logs = self.state_dir.join("logs");
        for app in handover.apps {
            let Some(index) = self.index(&app.key) else {
                // SAFETY: the caller's contract. Gone from the profile: it stops.
                unsafe { child::discard(&app.handles, true) };
                continue;
            };
            let name = self.apps[index].name.clone();
            if !child::handles_match(&app.handles) {
                self.add_log(
                    format!("✗ Couldn't take over {name}: its handles don't match - it stops when lazyrunner quits"),
                    Hue::Red,
                );
                continue;
            }
            if self.command_of(index) != app.command {
                // SAFETY: the caller's contract. It ends without events, before it starts again.
                unsafe { child::discard(&app.handles, true) };
                self.states[index].generation = app.generation; // the new run's events can't be mistaken for its
                self.add_log(format!("{name} changed - starting it again"), Hue::Cyan);
                restart.push(app.key);
                continue;
            }
            let tag = Tag::App { key: app.key.clone(), generation: app.generation };
            // SAFETY: the caller's contract; checked above.
            let child = match unsafe { Child::adopt(app.handles, tag, self.events.clone()) } {
                Ok(child) => child,
                Err(error) => {
                    // Past its checks adopt owns the handles: it closed them, so the app stopped.
                    self.states[index].generation = app.generation;
                    self.add_log(format!("✗ Couldn't take over {name}: {error} - starting it again"), Hue::Yellow);
                    restart.push(app.key);
                    continue;
                }
            };
            let state = &mut self.states[index];
            let aged = now.checked_sub(AGED).unwrap_or(now);
            for (text, hue) in &app.lines {
                state.output.push_hued(text, *hue, aged);
            }
            state.resume_log(&logs);
            state.generation = app.generation;
            state.child = Some(child);
            state.status = Status::Running;
            state.started_at = Some(now.checked_sub(Duration::from_millis(app.running_for_ms)).unwrap_or(now));
            state.started_wall = app.started_wall_ms.map(|ms| UNIX_EPOCH + Duration::from_millis(ms));
            state.up_generation = app.up.then_some(app.generation);
            state.up_after = app.up_after;
            state.script = app.script;
            state.label = app.label;
            state.errors = app.errors;
            state.warnings = app.warnings;
            state.requests = app.requests;
            if !app.up {
                self.ready_pending.push(app.key.clone());
                self.action_started.get_or_insert(now);
            }
            taken.push(app.key);
        }
        for (time, text, hue) in handover.log.into_iter().rev() {
            self.log.push_front(LogEntry { time, text, hue });
        }
        self.update_metric_roots();
        if !taken.is_empty() {
            let names = self.names(&taken);
            let from = if handover.from == changelog::VERSION {
                "the previous lazyrunner".to_string()
            } else {
                format!("v{}", handover.from)
            };
            self.add_log(format!("Took over {names} from {from} - they kept running"), Hue::Green);
        }
        restart
    }
}
