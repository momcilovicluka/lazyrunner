//! One app while lazyrunner runs it: its process, its column's output (coloured and counted as it arrives, and
//! written to its log file), and its live numbers.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use regex::Regex;

use super::child::Child;
use crate::output::{Hue, Output};
use crate::profile::App;

/// Lines each column keeps.
pub const MAX_PANE_LINES: usize = 5000;
/// Samples kept for the sparklines.
pub const HISTORY: usize = 30;
const ERROR_FLASH: Duration = Duration::from_millis(1500);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Stopped,
    Running,
    Exited(Option<i32>),
    Error(String),
}

impl Status {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Stopped => "stopped",
            Self::Running => "running",
            Self::Exited(_) => "exited",
            Self::Error(_) => "error",
        }
    }
}

pub struct AppState {
    pub key: String,
    pub status: Status,
    /// Counts the starts, so output and exits of an earlier run are told apart.
    pub generation: u64,
    pub started_at: Option<Instant>,
    /// The same moment on the wall clock (a watch build is ready once its file is newer).
    pub started_wall: Option<std::time::SystemTime>,
    /// The npm script this run uses (a branch can need another).
    pub script: Option<String>,
    pub label: Option<String>,
    pub child: Option<Child>,
    pub output: Output,
    pub scroll: usize,
    pub errors: u64,
    pub warnings: u64,
    pub requests: u64,
    pub flash_until: Option<Instant>,
    /// The level of the last ASP.NET log entry: its indented lines take its colour.
    last_level: String,
    log: Option<BufWriter<File>>,
    pub log_path: Option<PathBuf>,
    pub cpu: Option<f32>,
    pub memory_mb: Option<f32>,
    pub cpu_history: VecDeque<f32>,
    pub memory_history: VecDeque<f32>,
    pub response_history: VecDeque<f32>,
    pub request_history: VecDeque<f32>,
    /// The generation that got up, and how long it took.
    pub up_generation: Option<u64>,
    pub up_after: Option<f64>,
}

impl AppState {
    pub fn new(key: &str) -> Self {
        Self {
            key: key.to_string(),
            status: Status::Stopped,
            generation: 0,
            started_at: None,
            started_wall: None,
            script: None,
            label: None,
            child: None,
            output: Output::new(MAX_PANE_LINES),
            scroll: 0,
            errors: 0,
            warnings: 0,
            requests: 0,
            flash_until: None,
            last_level: "info".into(),
            log: None,
            log_path: None,
            cpu: None,
            memory_mb: None,
            cpu_history: VecDeque::new(),
            memory_history: VecDeque::new(),
            response_history: VecDeque::new(),
            request_history: VecDeque::new(),
            up_generation: None,
            up_after: None,
        }
    }

    pub fn is_running(&self) -> bool {
        self.status == Status::Running
    }

    /// Whether this run is up (listening, or its watch build written).
    pub fn is_up(&self) -> bool {
        self.is_running() && self.up_generation == Some(self.generation)
    }

    /// Opens the column's log file (the previous run's becomes `<key>.prev.log`).
    pub fn open_log(&mut self, dir: &Path) {
        let _ = std::fs::create_dir_all(dir);
        let path = dir.join(format!("{}.log", self.key));
        if path.exists() {
            let _ = std::fs::rename(&path, dir.join(format!("{}.prev.log", self.key)));
        }
        self.log = File::create(&path).ok().map(BufWriter::new);
        self.log_path = Some(path);
    }

    /// Goes on writing the log file of a run another lazyrunner handed over (instead of starting a new one, which
    /// `open_log` did at startup: that rotated this run's file to `<key>.prev.log`).
    pub fn resume_log(&mut self, dir: &Path) {
        let path = dir.join(format!("{}.log", self.key));
        let previous = dir.join(format!("{}.prev.log", self.key));
        let fresh = std::fs::metadata(&path).map_or(true, |meta| meta.len() == 0);
        self.log = None; // closes the fresh file, so it can be replaced
        if fresh && previous.exists() {
            let _ = std::fs::rename(&previous, &path);
        }
        self.log = std::fs::OpenOptions::new().create(true).append(true).open(&path).ok().map(BufWriter::new);
        self.log_path = Some(path);
    }

    pub fn flush_log(&mut self) {
        if let Some(log) = &mut self.log {
            let _ = log.flush();
        }
    }

    /// A line in the column (and its log file).
    pub fn add_line(&mut self, text: &str, hue: Hue, now: Instant) {
        self.output.push_hued(text, Some(hue), now);
        if self.scroll > 0 {
            self.scroll = (self.scroll + 1).min(self.output.len().saturating_sub(1)); // a scrolled-back view stays put
        }
        if let Some(log) = &mut self.log {
            let _ = writeln!(log, "{text}");
        }
    }

    /// A line the app printed: coloured, counted (errors flash the title), then added.
    pub fn receive(&mut self, app: &App, raw: &str, now: Instant) {
        let text = crate::output::strip_ansi(raw).replace('\t', "    ");
        let hue = self.hue_of(app, &text);
        let colors_errors = app.traits().colors_errors;
        if LEVEL_ERROR.is_match(&text) || (colors_errors && hue == Hue::Red) {
            self.errors += 1;
            self.flash_until = Some(now + ERROR_FLASH);
        } else if text.get(..6).is_some_and(|head| head.eq_ignore_ascii_case("warn: "))
            || (colors_errors && hue == Hue::Yellow)
        {
            self.warnings += 1;
        }
        if text.contains("Request finished ") {
            self.requests += 1;
        }
        self.add_line(&text, hue, now);
    }

    fn hue_of(&mut self, app: &App, text: &str) -> Hue {
        // ASP.NET's console logger: "info: Category[id]", then indented lines of the same entry.
        if let Some(found) = LEVEL.captures(text) {
            self.last_level = found[1].to_lowercase();
            return level_hue(&self.last_level, false);
        }
        if GOOD.is_match(text) {
            return Hue::Green;
        }
        if app.traits().indented_logs && INDENTED.is_match(text) {
            return level_hue(&self.last_level, true);
        }
        if BAD.is_match(text) {
            return Hue::Red;
        }
        if WARNING.is_match(text) {
            return Hue::Yellow;
        }
        if NPM_NOISE.is_match(text) {
            return Hue::DarkGray;
        }
        Hue::Gray
    }

    /// Clears the counters and histories for a new run.
    pub fn reset_for_start(&mut self) {
        self.generation += 1;
        self.errors = 0;
        self.warnings = 0;
        self.requests = 0;
        self.cpu = None;
        for history in
            [&mut self.cpu_history, &mut self.memory_history, &mut self.response_history, &mut self.request_history]
        {
            history.clear();
        }
    }
}

/// Adds a sample to a sparkline history.
pub fn push_history(history: &mut VecDeque<f32>, value: f32) {
    if history.len() == HISTORY {
        history.pop_front();
    }
    history.push_back(value);
}

fn level_hue(level: &str, continuation: bool) -> Hue {
    match (level, continuation) {
        ("info", false) => Hue::Cyan,
        ("info", true) => Hue::Gray,
        ("warn", false) => Hue::Yellow,
        ("warn", true) => Hue::DarkYellow,
        ("fail" | "crit", _) => Hue::Red,
        _ => Hue::DarkGray,
    }
}

fn pattern(regex: &str) -> Regex {
    Regex::new(regex).expect("a valid pattern")
}

static LEVEL: LazyLock<Regex> = LazyLock::new(|| pattern(r"(?i)^(info|warn|fail|crit|dbug|trce): "));
static LEVEL_ERROR: LazyLock<Regex> = LazyLock::new(|| pattern(r"(?i)^(fail|crit): "));
static GOOD: LazyLock<Regex> = LazyLock::new(|| {
    pattern(r"Now listening on|Application started|Application bundle generation complete|[Cc]ompiled successfully|✔")
});
static INDENTED: LazyLock<Regex> = LazyLock::new(|| pattern(r"^\s{2,}\S"));
static BAD: LazyLock<Regex> =
    LazyLock::new(|| pattern(r"(?i)\b(ERROR|Error:|error (TS|NG|CS)\d+|Unhandled exception|Exception:)|✘|✖"));
static WARNING: LazyLock<Regex> = LazyLock::new(|| pattern(r"(?i)\b(WARNING|Warning:)|▲"));
static NPM_NOISE: LazyLock<Regex> = LazyLock::new(|| pattern(r"(?i)^npm (verbose|info|timing|http|sill)\b"));

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::{AppKind, Variant};

    fn app(dotnet: bool) -> App {
        let kind = if dotnet {
            AppKind::Dotnet { project: "a.csproj".into(), launch_profile: "A".into(), swagger: None }
        } else {
            AppKind::Npm { dir: "web".into(), variants: Vec::<Variant>::new() }
        };
        App { key: "a".into(), name: "A".into(), icon: None, kind }
    }

    fn hues(app: &App, lines: &[&str]) -> (Vec<Hue>, AppState) {
        let mut state = AppState::new("a");
        let now = Instant::now();
        for line in lines {
            state.receive(app, line, now);
        }
        let hues =
            state.output.newest_first(0).map(|line| line.hue.unwrap()).collect::<Vec<_>>().into_iter().rev().collect();
        (hues, state)
    }

    #[test]
    fn aspnet_entries_and_their_indented_lines_share_a_colour() {
        let (hues, state) = hues(
            &app(true),
            &[
                "info: Microsoft.Hosting.Lifetime[14]",
                "      Now listening on: http://localhost:5000",
                "warn: Something[1]",
                "      details",
                "fail: Boom[2]",
                "      at Foo()",
            ],
        );
        assert_eq!(hues, [Hue::Cyan, Hue::Green, Hue::Yellow, Hue::DarkYellow, Hue::Red, Hue::Red]);
        assert_eq!((state.errors, state.warnings), (1, 1));
        assert!(state.flash_until.is_some(), "an error flashes the title");
    }

    #[test]
    fn npm_output_counts_red_and_yellow_lines() {
        let (hues, state) = hues(
            &app(false),
            &["npm verbose cli x", "ERROR in src/app.ts", "WARNING: budget", "\x1b[32mcompiled successfully\x1b[0m"],
        );
        assert_eq!(hues, [Hue::DarkGray, Hue::Red, Hue::Yellow, Hue::Green]);
        assert_eq!((state.errors, state.warnings), (1, 1));
    }

    #[test]
    fn the_log_file_gets_every_line_and_the_last_run_is_kept() {
        let dir = crate::testing::TempDir::new("pane-log");
        let mut state = AppState::new("shop");
        state.open_log(dir.path());
        state.add_line("first run", Hue::Gray, Instant::now());
        state.flush_log();
        state.open_log(dir.path());
        state.add_line("second run", Hue::Gray, Instant::now());
        state.flush_log();
        assert_eq!(std::fs::read_to_string(dir.join("shop.log")).unwrap(), "second run\n");
        assert!(std::fs::read_to_string(dir.join("shop.prev.log")).unwrap().contains("first run"));
    }
}
