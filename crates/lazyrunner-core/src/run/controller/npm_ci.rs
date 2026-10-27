//! npm ci and npm install: whether an npm app's packages match its lock file, and installing them in the background.

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

    /// i, c: npm ci for an npm app (the focused one, or the one whose lock file changed), in the background.
    pub fn npm_ci(&mut self, focused: Option<&str>) {
        self.install_packages(focused, true);
    }

    /// i, n: npm install for the same app: packages added and updated to match package.json.
    pub fn npm_install(&mut self, focused: Option<&str>) {
        self.install_packages(focused, false);
    }

    /// npm ci (`clean`) or npm install, in the background.
    fn install_packages(&mut self, focused: Option<&str>, clean: bool) {
        let what = if clean { "npm ci" } else { "npm install" };
        let Some(key) = self.npm_ci_key(focused) else {
            self.add_log(
                "There is no npm app (the project has none, or lazyrunner.config.json turns them off).",
                Hue::Yellow,
            );
            return;
        };
        if let Some(job) = &self.npm_ci {
            let name = self.app(&job.key).map(|app| app.name.clone()).unwrap_or_default();
            self.add_log(format!("{} is already running ({name}).", job.what), Hue::Gray);
            return;
        }
        let app = self.app(&key).expect("a known app").clone();
        let AppKind::Npm { dir, .. } = &app.kind else { return };
        let folder = self.root.join(dir);
        if clean && !folder.join("package-lock.json").is_file() {
            let text =
                format!("No package-lock.json in {} - npm ci needs one (npm install makes it).", folder.display());
            self.add_log(text, Hue::Yellow);
            return;
        }
        let was_running = self.is_running(&key);
        if was_running {
            self.stop(Some(std::slice::from_ref(&key)), true); // a running dev server locks files in node_modules
        }
        self.pane(&key, &format!("──── ⚙ {} · {what} ────", clock()), Hue::Cyan);
        let beside = if self.building.is_some() { " (next to the build)" } else { "" };
        self.add_log(format!("Running {what} for {}…{beside}", app.name), Hue::Cyan);
        self.pause_npm_suites(&folder);
        stop_nx_daemon(&folder);
        // node_modules is about to be rewritten: until it succeeds it isn't known to be in sync.
        let _ = std::fs::remove_file(self.npm_ci_stamp(&key));
        let spec = if clean { self.commands.npm_ci(&folder) } else { self.commands.npm_install(&folder) };
        let spawned = spec.and_then(|spec| {
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
                    what,
                });
            }
            Err(error) => self.add_log(format!("✗ {what} couldn't start: {error}"), Hue::Red),
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
            self.app(key).is_some_and(|app| matches!(&app.kind, AppKind::Npm { dir, .. } if !super::super::tools::has_packages(&self.root.join(dir))))
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
        let (key, what) = (job.key.clone(), job.what);
        let (kind, timing) = if what == "npm ci" { (Kind::NpmCi, "npmci") } else { (Kind::NpmInstall, "npminstall") };
        let name = self.app(&key).map(|app| app.name.clone()).unwrap_or_default();
        let log = self.state_dir.join(format!("{}-{key}.log", what.replace(' ', "-")));
        let _ = std::fs::write(&log, job.lines.join("\n") + "\n");
        let seconds = ((self.now)() - job.started).as_secs_f64();
        let waiting =
            if job.start_after { format!(" - {name} was waiting for it and stays stopped") } else { String::new() };
        match end {
            NpmCiEnd::Cancelled => {
                let text = format!("■ {what} cancelled - node_modules may be incomplete, press i again");
                self.pane(&key, &text, Hue::Yellow);
                self.add_log(format!("{text}{waiting}"), Hue::Yellow);
            }
            NpmCiEnd::Failed => {
                self.record_duration(kind, &key, seconds, false);
                self.last_failed_log = Some(log);
                self.notifications.push((format!("{what} failed"), format!("See the {name} column in lazyrunner")));
                self.pane(&key, &format!("✗ {what} failed after {}s", seconds as u64), Hue::Red);
                let text = format!("✗ {what} failed after {}s - o opens the full log{waiting}", seconds as u64);
                self.add_log(text, Hue::Red);
            }
            NpmCiEnd::Ok => {
                self.check_packages();
                let stamp =
                    serde_json::json!({ "root": self.root.display().to_string(), "lockHash": self.lock_hash(&key) });
                let _ = crate::config::write(&self.npm_ci_stamp(&key), &stamp);
                self.lock_changed.retain(|changed| *changed != key);
                self.timings.add(timing, &key, seconds);
                let note = self.record_duration(kind, &key, seconds, true);
                let done = format!("✓ {what} done in {}{note}", format_duration(seconds));
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
    let Some(nx) = tools::nx_script(dir) else { return };
    let Some(node) = tools::find_on_path("node") else { return };
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
