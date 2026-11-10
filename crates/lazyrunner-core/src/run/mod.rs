//! Running a project's apps: their processes (whole trees, nothing survives a quit or a crash), builds, npm ci,
//! readiness, response probes, ports, the branch and code watchers, and the database switch. The [`Controller`]
//! owns all of it; everything that happens elsewhere (output, exits, probe results, port snapshots, file changes)
//! comes back to it as an [`Event`].

pub mod app;
pub mod audit;
pub mod build;
pub mod build_map;
pub mod calls;
pub mod cause;
pub mod checks;
pub mod child;
pub mod code;
mod code_ts;
pub mod command;
pub mod compose;
pub mod controller;
pub mod diff;
pub mod history;
pub mod hooks;
pub mod impact;
pub mod migrations;
pub mod net;
pub mod ports;
pub mod requests;
pub mod secrets;
pub mod suites;
pub mod test_history;
pub mod tools;
pub mod watcher;

use std::path::PathBuf;

pub use command::{Command, Origin};
pub use controller::Controller;

/// What a child process is, for the controller.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Tag {
    /// An app's server; `generation` tells this run from the previous ones.
    App {
        key: String,
        generation: u64,
    },
    /// A build or an npm ci; `job` tells this run from a cancelled one whose exit arrives late.
    Build {
        key: String,
        job: u64,
    },
    NpmCi {
        key: String,
        job: u64,
    },
    /// A task's run (`"tasks"`).
    Task {
        key: String,
        job: u64,
    },
    /// A test suite's run.
    Suite {
        key: String,
        job: u64,
    },
    /// The build of the test projects.
    TestBuild {
        job: u64,
    },
    /// dotnet ef, for the migrations view.
    Migration {
        job: u64,
    },
}

#[derive(Debug)]
pub enum Event {
    Output {
        tag: Tag,
        line: String,
    },
    Exited {
        tag: Tag,
        code: Option<i32>,
    },
    /// The listening TCP ports and who holds them.
    Listening(net::Listening),
    Probe {
        key: String,
        generation: u64,
        result: net::ProbeResult,
    },
    /// CPU (% of the machine) and memory (MB) per app, summed over its process tree.
    Metrics(Vec<(String, f32, f32)>),
    FileChanged(PathBuf),
    /// A file saved in a watched npm or Nx test suite's folder (test watch).
    TestFileChanged(PathBuf),
    /// A command app's code (its `watch`) was saved.
    CommandFileChanged {
        key: String,
        path: PathBuf,
    },
    /// The test suites found (T), with their last results.
    Suites(Vec<suites::Suite>),
    /// What changes affect, each suite's, looked at in the background (#78).
    Impact(std::collections::HashMap<String, impact::Impact>),
    /// The code map, read in the background (#89).
    Code {
        map: Box<code::CodeMap>,
        cache: Option<Box<code::FileCache>>,
        /// Nx has been asked for its implicit dependencies too.
        done: bool,
    },
    /// A suite's failures' likely causes, worked out in the background for its history then (#34).
    Causes {
        key: String,
        stamp: String,
        found: std::collections::HashMap<String, controller::LikelyCause>,
    },
    /// An app's packages scanned for the audit view (#79), or why they couldn't be.
    Audit {
        key: String,
        result: Result<Vec<audit::Package>, String>,
    },
    /// A fix from the audit view installed, or why it couldn't be.
    AuditInstalled {
        key: String,
        result: Result<(), String>,
    },
    /// A compose app's services, polled for its run `generation`, or why they couldn't be.
    ComposeServices {
        key: String,
        generation: u64,
        services: Result<Vec<compose::Service>, String>,
    },
    /// The ports a compose app's services publish, as its compose file says, or why they couldn't be read.
    ComposePorts {
        key: String,
        ports: Result<Vec<u16>, String>,
    },
    /// A hook that didn't go well: its event, its command, and what went wrong.
    HookFailed {
        event: String,
        run: String,
        problem: String,
    },
}

/// "4s", "1m05s", as the panel says durations.
pub fn format_duration(seconds: f64) -> String {
    let seconds = seconds.max(0.0);
    if seconds < 60.0 {
        format!("{}s", seconds as u64)
    } else {
        format!("{}m{:02}s", (seconds / 60.0).floor() as u64, seconds as u64 % 60)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn durations_read_like_the_panel_says_them() {
        assert_eq!(super::format_duration(4.7), "4s");
        assert_eq!(super::format_duration(65.2), "1m05s");
    }
}
