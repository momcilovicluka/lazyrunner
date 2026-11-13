//! Project profiles: what a project is (the file that marks its repo, the folder whose changes need a rebuild, its
//! apps) as data, apart from lazyrunner's code. `projects/README.md` describes the format; `profile.schema.json`
//! checks it in editors.
//!
//! Where profiles come from, checked in this order in each folder from the current one up:
//! - repo: `.lazyrunner.json` at the repo root, a whole profile or `"extends": "<id>"` with changes to a known one.
//!   Versioned with the code, so used only once you trust it ([`trust`]).
//! - user: your own folder of profiles (`paths`), for projects whose repo has none.
//! - bundled: `projects/` next to lazyrunner.

pub mod change;
pub mod check;
pub mod load;
pub mod preset;
#[cfg(test)]
mod tests;
pub mod trust;

use std::path::{MAIN_SEPARATOR, PathBuf};

use anyhow::{Result, bail};
use serde_json::Value;

pub use load::{Found, Known, find_project_profile, import, known_profiles, merge, read_profile_data, resolve};

pub const SCHEMA_VERSION: i64 = 1;
pub const REPO_PROFILE_FILE: &str = ".lazyrunner.json";
/// An app's `"type"`.
pub const TYPE_DOTNET: &str = "dotnet";
pub const TYPE_NPM: &str = "npm";
pub const TYPE_COMMAND: &str = "command";
/// An app's `"restart"`: started again after a crash, or not (#99).
pub const RESTART_ON_CRASH: &str = "onCrash";
pub const RESTARTS: [&str; 2] = ["never", RESTART_ON_CRASH];
pub const TYPE_COMPOSE: &str = "compose";
/// In a command app's commands and env, its port; and its folder.
pub const PORT_PLACEHOLDER: &str = "{port}";
pub const CWD_PLACEHOLDER: &str = "{cwd}";
/// A `path` folder from the home folder (`~/.cargo/bin`) instead of the app's.
pub const HOME_PREFIX: &str = "~/";

/// Where a profile came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Repo,
    User,
    Bundled,
    /// Named directly: `LAZYRUNNER_PROFILE`, a file given to `profile check`, tests.
    Given,
}

impl Source {
    pub fn name(self) -> &'static str {
        match self {
            Self::Repo => "repo",
            Self::User => "user",
            Self::Bundled => "bundled",
            Self::Given => "given",
        }
    }
}

/// Where to look for profiles besides a repo's own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProfileDirs {
    pub user: PathBuf,
    pub bundled: PathBuf,
    /// When set, the only profile (and repos' own files are ignored).
    pub only: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Variant {
    pub label: String,
    /// The package.json script that selects it; `None` applies always.
    pub when: Option<String>,
    pub script: String,
    /// A dev server's port; `None` for a watch build.
    pub port: Option<u16>,
    pub served_by: Option<String>,
    pub ready_file: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppKind {
    /// `args`: what `dotnet run` passes the app besides its own (`"args"`): configuration, say.
    Dotnet {
        project: String,
        launch_profile: String,
        swagger: Option<String>,
        args: Vec<String>,
    },
    Npm {
        dir: String,
        variants: Vec<Variant>,
    },
    /// Any long-running command (`"type": "command"`, #56).
    Command(Box<Command>),
}

/// How a command app counts as up.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ready {
    /// Once it has started: it has no `ready` and no port.
    Started,
    /// Once this port listens.
    Port(u16),
    /// Once a line of its output matches this pattern.
    Line(String),
    /// Once a line of its output matches this pattern, or its port listens (`"orPort": true`).
    LineOrPort(String),
    /// Once its port answers this path with a 2xx.
    Http(String),
    /// Once its compose services are running, and healthy when they have a healthcheck.
    Services,
}

/// What runs Docker Compose, and its commands.
pub const DOCKER: &str = "docker";
pub const COMPOSE_UP: &str = "up";
pub const COMPOSE_STOP: &str = "stop";
pub const COMPOSE_PS: &str = "ps";
pub const COMPOSE_CONFIG: &str = "config";
/// The files Docker Compose reads when it's given none.
pub const COMPOSE_FILES: [&str; 4] = ["compose.yaml", "compose.yml", "docker-compose.yaml", "docker-compose.yml"];

/// A compose app (`"type": "compose"`, #56): its compose file (Docker's own when `None`) and the services it runs
/// (all when empty).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Compose {
    pub file: Option<String>,
    pub services: Vec<String>,
}

impl Compose {
    /// `docker` arguments for compose's `verb`, with `options`, for its services.
    pub fn args(&self, verb: &str, options: &[&str]) -> Vec<String> {
        let mut args = vec!["compose".to_string(), "--ansi".into(), "never".into()];
        if let Some(file) = &self.file {
            args.extend(["-f".to_string(), file.clone()]);
        }
        args.push(verb.to_string());
        args.extend(options.iter().map(|option| option.to_string()));
        args.extend(self.services.iter().cloned());
        args
    }
}

/// A command app (#56): what it runs and where, its port, what builds and installs it, when it's up, what counts as
/// its code, and what its errors, warnings and stack frames look like. Its commands are lists of arguments (a
/// profile's text is split into them); `{port}` in them and in its env is its port.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Command {
    pub run: Vec<String>,
    /// From the repo root; empty for the root.
    pub cwd: String,
    pub port: Option<u16>,
    /// Empty: nothing to build, nothing to install.
    pub build: Vec<String>,
    pub install: Vec<String>,
    /// Runs before an install while none of its `path` folders is there (`python -m venv .venv`).
    pub setup: Vec<String>,
    /// The files its install installs from, from its `cwd`: when they change, its install is due.
    pub install_from: Vec<String>,
    pub ready: Ready,
    /// Globs of its code, from its `cwd`.
    pub watch: Vec<String>,
    /// Patterns of an error line, and of a warning line.
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
    /// A pattern of a stack frame, with `file` and `line` (and `col`) groups.
    pub frames: Option<String>,
    /// The preset it was filled in from (#61).
    pub preset: Option<String>,
    /// Folders from its `cwd` put first on PATH when they exist, and where its programs are looked for first.
    pub path: Vec<String>,
    /// Programs the startup checklist shows with their versions.
    pub tools: Vec<String>,
    /// A compose app's file and services: its run is `docker compose up`.
    pub compose: Option<Compose>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct App {
    pub key: String,
    pub name: String,
    pub icon: Option<String>,
    /// Its icon when Nerd Font icons are on (`"nerdIcon"`); without it one for its kind or framework.
    pub nerd_icon: Option<String>,
    pub kind: AppKind,
    /// Test suites that belong to it whatever lazyrunner works out (`"tests"`): .NET test projects by path or
    /// name, Nx projects by name.
    pub tests: Vec<String>,
    /// A .NET app's EF Core migrations (`"migrations"`), for the migrations view (M).
    pub migrations: Option<Migrations>,
    /// Variables the app gets on top of lazyrunner's own (`"env"`).
    pub env: Vec<(String, String)>,
    /// The apps it calls (`"dependsOn"`), by key: the stack map draws them before a call is seen.
    pub depends_on: Vec<String>,
    /// What the Health tab holds it to and what its probe asks for (`"health"`), the defaults when it says nothing.
    pub health: Health,
    /// It starts again by itself after a crash (`"restart": "onCrash"`, #99).
    pub restart_on_crash: bool,
}

/// An app's targets (#35) - p95 latency, the 5xx rate (in basis points: 200 is 2%), memory and CPU - and its own
/// health endpoint for the probe, with the status it should answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Health {
    pub path: Option<String>,
    pub expected_status: Option<u16>,
    pub p95_ms: u32,
    pub error_bp: u32,
    pub memory_mb: u32,
    pub cpu_percent: u32,
}

impl Default for Health {
    fn default() -> Self {
        Self { path: None, expected_status: None, p95_ms: 1000, error_bp: 500, memory_mb: 2048, cpu_percent: 90 }
    }
}

impl Health {
    /// The 5xx rate it's held to, as a share: 0.05 is 5%.
    pub fn error_rate(&self) -> f64 {
        f64::from(self.error_bp) / 10_000.0
    }
}

/// Where a .NET app's EF Core migrations are: the project with them (the app's own when `None`) and its DbContexts
/// (found with `dotnet ef dbcontext list` when there are none).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Migrations {
    pub project: Option<String>,
    pub contexts: Vec<String>,
}

/// A rule for the code map (`"layers"`): the projects whose name `parts` matches (`*` for anything) must not
/// reference those `must_not_reference` names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layer {
    pub parts: String,
    pub must_not_reference: Vec<String>,
}

/// A one-off command run from the palette (`"tasks"`, #56): seed the database, regenerate a client. `run` as a command
/// app's (text split into arguments, or a list of them), in `cwd` from the repo root, with `env` on top of
/// lazyrunner's own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Task {
    pub key: String,
    pub name: String,
    pub run: Vec<String>,
    pub cwd: String,
    pub env: Vec<(String, String)>,
}

/// A loaded profile: checked and turned into what lazyrunner runs. Paths use this OS's separators.
#[derive(Clone, Debug)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
    pub repo_marker: String,
    pub source_dir: String,
    /// In column order.
    pub apps: Vec<App>,
    /// The code map's rules (`"layers"`).
    pub layers: Vec<Layer>,
    /// One-off commands (`"tasks"`).
    pub tasks: Vec<Task>,
    pub root: Option<PathBuf>,
    pub source: Source,
    /// What trust is given to: the file as written.
    pub file_hash: String,
    /// What lazyrunner runs (after extends): a branch that changes it is noticed.
    pub hash: String,
}

impl Project {
    pub fn app(&self, key: &str) -> Option<&App> {
        self.apps.iter().find(|app| app.key == key)
    }

    /// The apps not turned off on this machine (`"apps": { "<key>": { "enabled": false } }`, `"frontend": false`).
    pub fn active_apps(&self, config: &Value, config_path: &std::path::Path) -> Result<Vec<App>> {
        let active: Vec<App> =
            self.apps.iter().filter(|app| crate::apptypes::is_enabled(config, app)).cloned().collect();
        if active.is_empty() {
            bail!(
                "lazyrunner: every app of {} is turned off in {} (apps.<key>.enabled, frontend)",
                self.name,
                config_path.display()
            );
        }
        Ok(active)
    }
}

/// A profile path with this OS's separators and no trailing one ("web/shop/" -> "web\shop" on Windows).
pub fn native(path: &str) -> String {
    let converted: String = path.chars().map(|c| if c == '/' || c == '\\' { MAIN_SEPARATOR } else { c }).collect();
    converted.trim_end_matches(MAIN_SEPARATOR).to_string()
}

/// `a` + separator + `b`, for paths shown in messages.
pub fn join(a: &str, b: &str) -> String {
    if a.is_empty() { b.to_string() } else { format!("{a}{MAIN_SEPARATOR}{b}") }
}
