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
#[cfg(test)]
mod tests;
pub mod trust;

use std::path::{MAIN_SEPARATOR, PathBuf};

use anyhow::{Result, bail};
use serde_json::Value;

pub use load::{Found, Known, find_project_profile, import, known_profiles, merge, read_profile_data, resolve};

pub const SCHEMA_VERSION: i64 = 1;
pub const REPO_PROFILE_FILE: &str = ".lazyrunner.json";

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
    Dotnet { project: String, launch_profile: String, swagger: Option<String> },
    Npm { dir: String, variants: Vec<Variant> },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct App {
    pub key: String,
    pub name: String,
    pub icon: Option<String>,
    pub kind: AppKind,
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
