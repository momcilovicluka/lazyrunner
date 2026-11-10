//! Tasks (#56): the profile's one-off commands - seed the database, regenerate a client - run from the palette in the
//! background, beside the apps. Their output goes to the panel's log between a rule with their name and their result:
//! how long they took against their usual time, and the first error line when they failed. A long one says when it's
//! done with a notification.

use std::sync::LazyLock;
use std::time::{Duration, Instant};

use regex::Regex;

use super::Controller;
use crate::output::Hue;
use crate::profile::Task;
use crate::run::child::Child;
use crate::run::history::Kind;
use crate::run::{Tag, format_duration, tools};

/// A task that took longer than this says when it's done with a notification.
const NOTIFY_AFTER: Duration = Duration::from_secs(10);

/// An error line in a task's output, and a warning line.
static ERROR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(error|fatal|exception|traceback)\b|^\s*✗|panicked at").expect("a valid pattern")
});
static WARNING: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\bwarn(ing)?\b").expect("a valid pattern"));

/// A task while it runs.
pub struct TaskJob {
    pub key: String,
    pub id: u64,
    pub started: Instant,
    _child: Child,
    first_error: Option<String>,
}

/// How a task's last run went.
#[derive(Clone, Debug, PartialEq)]
pub struct TaskResult {
    pub ok: bool,
    pub seconds: f64,
    pub at: Instant,
    /// Its first error line (or its exit code) when it failed.
    pub error: Option<String>,
    /// A failure put away (its chip in the header clicked).
    pub dismissed: bool,
}

impl Controller {
    /// The profile's tasks (`"tasks"`).
    pub fn tasks(&self) -> &[Task] {
        &self.project.tasks
    }

    /// When a task started, while it runs.
    pub fn task_started(&self, key: &str) -> Option<Instant> {
        self.task_jobs.iter().find(|job| job.key == key).map(|job| job.started)
    }

    /// The tasks running, with when each started.
    pub fn tasks_running(&self) -> Vec<(&Task, Instant)> {
        let task = |key: &str| self.project.tasks.iter().find(|task| task.key == key);
        self.task_jobs.iter().filter_map(|job| Some((task(&job.key)?, job.started))).collect()
    }

    /// How long a task usually takes, once it has passed.
    pub fn task_usual(&self, key: &str) -> Option<f64> {
        self.durations.usual(Kind::Task, key)
    }

    /// The tasks whose last run failed, not put away yet.
    pub fn task_failures(&self) -> Vec<(&Task, &TaskResult)> {
        let failed = |task: &&Task| {
            self.task_started(&task.key).is_none()
                && self.task_results.get(&task.key).is_some_and(|result| !result.ok && !result.dismissed)
        };
        self.project
            .tasks
            .iter()
            .filter(failed)
            .filter_map(|task| Some((task, self.task_results.get(&task.key)?)))
            .collect()
    }

    /// Puts a task's failure away.
    pub fn dismiss_task_result(&mut self, key: &str) {
        if let Some(result) = self.task_results.get_mut(key) {
            result.dismissed = true;
        }
    }

    /// How a task's last run in this session went.
    pub fn task_result(&self, key: &str) -> Option<&TaskResult> {
        self.task_results.get(key)
    }

    /// Runs task `key` in the background (once at a time each).
    pub fn run_task(&mut self, key: &str) {
        let Some(task) = self.project.tasks.iter().find(|task| task.key == key).cloned() else {
            self.add_log(format!("There is no task '{key}' in the profile"), Hue::Yellow);
            return;
        };
        if self.task_started(key).is_some() {
            self.add_log(format!("{} is already running", task.name), Hue::Gray);
            return;
        }
        let folder = self.root.join(&task.cwd);
        if !folder.is_dir() {
            self.add_log(
                format!("✗ {} couldn't start: its folder {} isn't there", task.name, folder.display()),
                Hue::Red,
            );
            return;
        }
        let spec = match tools::task_spec(&self.root, &task) {
            Ok(spec) => spec,
            Err(error) => {
                self.add_log(format!("✗ {} couldn't start: {error}", task.name), Hue::Red);
                return;
            }
        };
        self.next_job += 1;
        let id = self.next_job;
        let tag = Tag::Task { key: key.to_string(), job: id };
        match Child::spawn(&spec, tag, self.events.clone(), self.guard.as_ref()) {
            Ok(child) => {
                self.add_log(format!("╭─ ▸ {} · {}", task.name, task.run.join(" ")), Hue::Cyan);
                let started = (self.now)();
                self.task_jobs.push(TaskJob { key: key.to_string(), id, started, _child: child, first_error: None });
            }
            Err(error) => self.add_log(format!("✗ {} couldn't start: {error}", task.name), Hue::Red),
        }
    }

    /// A line a task printed: in the panel's log under its rule, an error line red (the first one kept).
    pub(super) fn task_output(&mut self, key: &str, id: u64, line: &str) {
        let Some(job) = self.task_jobs.iter_mut().find(|job| job.key == key && job.id == id) else { return };
        let text = crate::output::strip_ansi(line).replace('\t', "    ");
        let error = ERROR.is_match(&text);
        if error && job.first_error.is_none() && !text.trim().is_empty() {
            job.first_error = Some(text.trim().to_string());
        }
        let hue = match () {
            _ if error => Hue::Red,
            _ if WARNING.is_match(&text) => Hue::Yellow,
            _ => Hue::Gray,
        };
        self.add_log(format!("│ {text}"), hue);
        if let Some(entry) = self.log.back_mut() {
            entry.task = Some(id);
        }
    }

    /// A task ended: its result under its output, its time in the durations, a hook, and a notification when it was
    /// long.
    pub(super) fn task_exited(&mut self, key: &str, id: u64, code: Option<i32>) {
        let Some(at) = self.task_jobs.iter().position(|job| job.key == key && job.id == id) else { return };
        let job = self.task_jobs.remove(at);
        let now = (self.now)();
        let seconds = now.saturating_duration_since(job.started).as_secs_f64();
        let name =
            self.project.tasks.iter().find(|task| task.key == key).map_or(key.to_string(), |task| task.name.clone());
        let ok = code == Some(0);
        let note = self.record_duration_as(Kind::Task, key, &name, seconds, ok);
        let took = format_duration(seconds);
        let error = (!ok).then(|| {
            job.first_error
                .clone()
                .unwrap_or_else(|| format!("exit code {}", code.map_or("-".into(), |code| code.to_string())))
        });
        match &error {
            None => {
                let before = self.log.len();
                self.log.retain(|entry| entry.task != Some(id));
                let folded = match before - self.log.len() {
                    0 => String::new(),
                    1 => " · 1 line folded (the log file has it)".to_string(),
                    lines => format!(" · {lines} lines folded (the log file has them)"),
                };
                self.add_log(format!("╰─ ✓ {name} in {took}{note}{folded}"), Hue::Green);
                self.happened("task.passed", None, format!("{name} in {took}"));
            }
            Some(why) => {
                self.add_log(format!("╰─ ✗ {name} failed after {took}: {why}"), Hue::Red);
                self.happened("task.failed", None, format!("{name} failed: {why}"));
            }
        }
        if Duration::from_secs_f64(seconds) >= NOTIFY_AFTER {
            let title = if ok { format!("{name} is done") } else { format!("{name} failed") };
            self.notifications.push((title, error.clone().unwrap_or_else(|| format!("in {took}"))));
        }
        self.task_results.insert(key.to_string(), TaskResult { ok, seconds, at: now, error, dismissed: false });
    }
}
