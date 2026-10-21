//! npm ci: whether an npm app's packages match its lock file, and reinstalling them in the background.

use std::path::Path;
use std::time::Instant;

use sha2::{Digest, Sha256};

use super::start::clock;
use super::{Controller, ToolJob};
use crate::output::Hue;
use crate::profile::{AppKind, join};
use crate::run::build::{self, ToolLine};
use crate::run::child::Child;
use crate::run::history::Kind;
use crate::run::{Tag, format_duration, tools};

impl Controller {
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
        // node_modules is about to be rewritten: until npm ci succeeds it isn't known to be in sync.
        let _ = std::fs::remove_file(self.npm_ci_stamp(&key));
        let spawned = self.commands.npm_ci(&folder).and_then(|spec| {
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
                self.record_duration(Kind::NpmCi, &key, seconds, false);
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
                let note = self.record_duration(Kind::NpmCi, &key, seconds, true);
                let done = format!("✓ npm ci done in {}{note}", format_duration(seconds));
                self.pane(&key, &done, Hue::Green);
                self.add_log(done, Hue::Green);
                if job.start_after {
                    self.start(Some(&[key]), false);
                }
            }
        }
    }
}

enum NpmCiEnd {
    Ok,
    Failed,
    Cancelled,
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
