//! The startup doctor's coverage fix (#33): adds what collecting code coverage needs - coverlet's collector to the
//! .NET test projects without it, a coverage provider to the vitest folders without one - one command after another,
//! in the background.

use std::process::Command;

use super::Controller;
use crate::output::Hue;
use crate::run::Event;
use crate::run::coverage_needs;

impl Controller {
    /// `f` on the doctor's coverage row: runs what adds each missing package; `Event::CoverageInstalled` when done.
    pub fn install_coverage(&mut self) {
        if self.installing_coverage {
            return;
        }
        let (missing, _) = coverage_needs::missing(&self.root, &self.apps);
        if missing.is_empty() {
            return;
        }
        self.installing_coverage = true;
        let shown: Vec<String> = missing.iter().map(|missing| missing.command(&self.root)).collect();
        let events = self.events.clone();
        tokio::task::spawn_blocking(move || {
            let result = missing.iter().try_for_each(add);
            let _ = events.send(Event::CoverageInstalled { result });
        });
        self.add_log(format!("Adding what coverage needs: {}…", shown.join("; ")), Hue::Cyan);
    }

    pub(super) fn coverage_installed(&mut self, result: Result<(), String>) {
        self.installing_coverage = false;
        match result {
            Ok(()) => self.add_log("✓ Coverage can be collected: what it needs is added.", Hue::Green),
            Err(why) => self.add_log(format!("✗ What coverage needs didn't add: {why}"), Hue::Red),
        }
        self.find_suites();
    }
}

/// Runs what adds one missing package; its last line when it fails.
fn add(missing: &coverage_needs::Missing) -> Result<(), String> {
    let spec = missing.spec().map_err(|error| error.to_string())?;
    let mut command = Command::new(&spec.program);
    command.args(&spec.args).current_dir(&spec.dir);
    let output =
        crate::run::tools::own_console(&mut command).output().map_err(|error| format!("{}: {error}", missing.suite))?;
    if output.status.success() {
        return Ok(());
    }
    let said = String::from_utf8_lossy(&output.stdout).into_owned() + &String::from_utf8_lossy(&output.stderr);
    let last = said.lines().map(str::trim).rfind(|line| !line.is_empty()).unwrap_or_default();
    Err(format!("{}: {last}", missing.suite))
}
