//! Running a project's apps: their processes (whole trees, nothing survives a quit or a crash), builds, npm ci,
//! readiness, response probes, ports, the branch and code watchers, and the database switch. The [`Controller`]
//! owns all of it; everything that happens elsewhere (output, exits, probe results, port snapshots, file changes)
//! comes back to it as an [`Event`].

pub mod app;
pub mod build;
pub mod checks;
pub mod child;
pub mod command;
pub mod controller;
pub mod history;
pub mod net;
pub mod secrets;
pub mod suites;
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
    /// A test suite's run.
    Suite {
        key: String,
        job: u64,
    },
    /// The build of the test projects.
    TestBuild {
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
    /// The test suites found (T), with their last results.
    Suites(Vec<suites::Suite>),
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
