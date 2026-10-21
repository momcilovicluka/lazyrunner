//! Builds: the queue a start builds one app at a time (then starts them), b / B's build only, and what a build's
//! output and exit mean.

use std::path::Path;

use super::start::clock;
use super::{Controller, ToolJob};
use crate::config::Toggle;
use crate::output::Hue;
use crate::profile::AppKind;
use crate::run::Tag;
use crate::run::build::{self, ToolLine};
use crate::run::child::Child;
use crate::run::history::Kind;

/// A failed build's errors shown in the panel's log (o opens the full log).
const MAX_BUILD_ERROR_LINES: usize = 6;
/// `y` copies at most this many errors of a failed build.
const MAX_COPIED_BUILD_ERRORS: usize = 200;

impl Controller {
    /// b, B: a full build (never skipped) that leaves the apps stopped; u / U then start them without building
    /// again. A running app is stopped first, since it locks its own build output. npm apps have no separate build.
    pub fn build_only(&mut self, only: Option<&[String]>) {
        if self.building.is_some() {
            self.add_log("A build is running - press b again once it's done (builds run one at a time)", Hue::Yellow);
            return;
        }
        let kept = if only.is_none() { self.kept_keys(false) } else { Vec::new() }; // B leaves what S leaves
        let keys: Vec<String> = self
            .keys()
            .into_iter()
            .filter(|key| only.is_none_or(|only| only.contains(key)) && self.builds(key) && !kept.contains(key))
            .collect();
        if keys.is_empty() {
            let text = match only.and_then(|only| only.first()).and_then(|key| self.app(key)) {
                Some(app) => format!("{} has no separate build - it compiles while it runs (r restarts it).", app.name),
                None => "Nothing to build: no app here has a separate build step.".to_string(),
            };
            self.add_log(text, Hue::DarkGray);
            return;
        }
        let running: Vec<String> = keys.iter().filter(|key| self.is_running(key)).cloned().collect();
        if !running.is_empty() {
            let names = self.names(&running);
            self.add_log(format!("Stopping {names} first - a running app locks its build output"), Hue::DarkGray);
            self.stop(Some(&running), true);
        }
        let stays = if keys.len() == 1 { "it stays" } else { "they stay" };
        self.add_log(format!("Building {} - {stays} stopped afterwards", self.names(&keys)), Hue::Cyan);
        if self.ready_pending.is_empty() {
            // Nothing else is getting ready: an app started during the build counts "Ready in" from now.
            self.action_started = Some((self.now)());
            self.timeline.clear();
        }
        self.last_build_failed = false;
        self.code_at_start = self.code.seen();
        self.fingerprint = build::fingerprint(&self.root, &self.project.source_dir);
        self.start_keys = Vec::new(); // a cancelled build has nothing to stop
        self.built.clear();
        self.force_build = true;
        self.build_only = true;
        self.build_queue = keys;
        self.next_build();
    }

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
                    self.build_failed();
                    return;
                }
            }
        }
        if std::mem::take(&mut self.build_only) {
            self.build_only_done();
            return;
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

    /// A build failed: the rest of the queue is dropped, and nothing starts.
    fn build_failed(&mut self) {
        self.build_queue.clear();
        self.built.clear();
        if std::mem::take(&mut self.build_only) {
            // What b stopped left ready_pending then; anything else in it is getting ready on its own.
            self.add_log("Fix the build and press b or B again - the apps stay stopped.", Hue::Yellow);
        } else {
            self.ready_pending.clear();
            self.add_log("APIs not started - fix the build and press U or R.", Hue::Yellow);
        }
    }

    /// b / B's builds are done: the apps stay stopped, and the next start skips their builds (nothing changed).
    fn build_only_done(&mut self) {
        let built = std::mem::take(&mut self.built);
        if !built.is_empty() {
            let starts = if built.len() == 1 { "u starts it" } else { "U starts them" };
            let skips = self.setting(Toggle::SkipUnchangedBuilds) && self.fingerprint.is_some();
            let again = if skips { " without building again" } else { "" };
            self.add_log(format!("Build done - {starts}{again}"), Hue::Green);
        }
        self.code.handled_up_to(self.code_at_start);
    }

    fn build_app(&mut self, key: &str) -> BuildStep {
        let app = self.app(key).expect("a known app").clone();
        let AppKind::Dotnet { project, .. } = &app.kind else { return BuildStep::Skipped };
        let skip = self.setting(Toggle::SkipUnchangedBuilds)
            && build::is_up_to_date(&self.state_dir, &self.root, key, project, self.fingerprint.as_deref());
        if !self.force_build && skip {
            self.add_log(
                format!("✓ {} unchanged since its last build - skipped (b / B force a full build)", app.name),
                Hue::Green,
            );
            return BuildStep::Skipped;
        }
        if self.force_build {
            build::remove_stamp(&self.state_dir, key);
        }
        let spec = self.commands.build(&self.root, project);
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
            self.record_duration(Kind::Build, key, seconds, false);
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
            self.build_failed();
            return;
        }
        if let AppKind::Dotnet { project, .. } = &app.kind {
            build::save_stamp(&self.state_dir, &self.root, key, project, self.fingerprint.as_deref());
        }
        self.timings.add("build", key, seconds);
        let note = self.record_duration(Kind::Build, key, seconds, true);
        let warnings = if job.warnings > 0 { format!(", {} warnings", job.warnings) } else { String::new() };
        let summary = format!("built in {whole}s ({} projects{warnings}){note}", job.done);
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
            self.build_only = false;
            let keys = std::mem::take(&mut self.start_keys);
            self.stop(Some(&keys), true);
        }
    }
}

enum BuildStep {
    Started,
    Skipped,
    Failed,
}
