//! The audit view's scans (#79): one app at a time, in the background, each kept with the others in the state folder
//! as it lands.

use std::time::Instant;

use super::Controller;
use crate::output::Hue;
use crate::run::Event;
use crate::run::audit::{self, AppAudit, Audits, Package, Severity};

/// A fix being installed from the audit view: its app, the command, and whether the app was running (it starts
/// again after).
pub(super) struct Installing {
    key: String,
    command: String,
    restart: bool,
}

impl Controller {
    /// The apps' last scans, by key.
    pub fn audits(&self) -> &Audits {
        &self.audits
    }

    /// The app being scanned.
    pub fn auditing(&self) -> Option<&str> {
        self.auditing.as_deref()
    }

    /// The apps waiting to be scanned.
    pub fn audit_queue(&self) -> &[String] {
        &self.audit_queue
    }

    /// When app `key`'s last scan landed (this session), for the view's motion.
    pub fn audit_landed(&self, key: &str) -> Option<Instant> {
        self.audit_landed.get(key).copied()
    }

    /// The fix being installed: its app and its command.
    pub fn audit_installing(&self) -> Option<(&str, &str)> {
        self.audit_installing.as_ref().map(|installing| (installing.key.as_str(), installing.command.as_str()))
    }

    /// `i` in the audit view, after a yes: installs `package`'s fix in its app's folder, in the background - the app
    /// stops first and starts again after - then scans the app again.
    pub fn audit_install(&mut self, key: &str, package: &Package) {
        if let Some((_, command)) = self.audit_installing() {
            self.add_log(format!("Still installing ({command})"), Hue::Gray);
            return;
        }
        let Some(app) = self.app(key).cloned() else { return };
        let Some(command) = audit::command(&app, package) else {
            self.add_log(format!("{}: nothing to install", package.name), Hue::DarkGray);
            return;
        };
        let restart = self.is_running(key);
        if restart {
            self.stop(Some(&[key.to_string()]), true);
        }
        self.add_log(format!("Installing for {}: {command}…", app.name), Hue::Cyan);
        self.audit_installing = Some(Installing { key: key.to_string(), command, restart });
        let (root, events, package, key) = (self.root.clone(), self.events.clone(), package.clone(), key.to_string());
        tokio::task::spawn_blocking(move || {
            let result = audit::install(&root, &app, &package).map_err(|error| error.to_string());
            let _ = events.send(Event::AuditInstalled { key, result });
        });
    }

    /// A fix was installed (or wasn't): said in the log, the app started again if it ran, and scanned again.
    pub(super) fn audit_installed(&mut self, key: String, result: Result<(), String>) {
        let Some(installing) = self.audit_installing.take().filter(|installing| installing.key == key) else { return };
        let name = self.app(&key).map_or_else(|| key.clone(), |app| app.name.clone());
        match result {
            Ok(()) => {
                self.audit_install_failed = None;
                self.add_log(format!("✓ {name}: {} - scanning it again", installing.command), Hue::Green);
            }
            Err(error) => {
                self.add_log(format!("✗ {name}: {} failed - {error}", installing.command), Hue::Red);
                self.audit_install_failed = Some((key.clone(), installing.command, (self.now)()));
            }
        }
        if installing.restart {
            self.start(Some(std::slice::from_ref(&key)), false);
        }
        self.audit(Some(&[key]));
    }

    /// `r` / `R` in the audit view: scans these apps (every app with none), one after another in the background.
    pub fn audit(&mut self, keys: Option<&[String]>) {
        let keys = keys.map_or_else(|| self.keys(), <[String]>::to_vec);
        let before = self.audit_queue.len() + usize::from(self.auditing.is_some());
        for key in keys {
            if self.auditing.as_ref() != Some(&key) && !self.audit_queue.contains(&key) {
                self.audit_queue.push(key);
            }
        }
        let added = self.audit_queue.len() + usize::from(self.auditing.is_some()) - before;
        match &mut self.audit_batch {
            Some((_, total)) if before > 0 => *total += added,
            _ if added > 0 => self.audit_batch = Some(((self.now)(), added)),
            _ => {}
        }
        self.next_audit();
    }

    /// The scans asked for together: how many are done, of how many.
    pub fn audit_batch(&self) -> Option<(usize, usize)> {
        let (_, total) = self.audit_batch?;
        let left = self.audit_queue.len() + usize::from(self.auditing.is_some());
        Some((total.saturating_sub(left), total))
    }

    /// The last fix that failed to install: its app, its command, and when.
    pub fn audit_install_failed(&self) -> Option<(&str, &str, Instant)> {
        let (key, command, at) = self.audit_install_failed.as_ref()?;
        Some((key.as_str(), command.as_str(), *at))
    }

    fn next_audit(&mut self) {
        while self.auditing.is_none() && !self.audit_queue.is_empty() {
            let key = self.audit_queue.remove(0);
            let Some(app) = self.app(&key).cloned() else { continue };
            let (root, events) = (self.root.clone(), self.events.clone());
            self.auditing = Some(key.clone());
            tokio::task::spawn_blocking(move || {
                let result = audit::scan(&root, &app).map_err(|error| error.to_string());
                let _ = events.send(Event::Audit { key, result });
            });
        }
    }

    /// An app's scan landed: kept (a failed one keeps the scan before, and says why), said in the log; the next goes.
    pub(super) fn audit_done(&mut self, key: String, result: Result<Vec<Package>, String>) {
        if self.auditing.as_ref() == Some(&key) {
            self.auditing = None;
        }
        let at = chrono::Local::now().timestamp();
        let name = self.app(&key).map_or_else(|| key.clone(), |app| app.name.clone());
        let scan = match result {
            Ok(packages) => {
                let worst = packages.iter().filter_map(|package| package.severity).max();
                let hue = match worst {
                    Some(severity) if severity >= Severity::High => Hue::Red,
                    Some(_) => Hue::Yellow,
                    None => Hue::Green,
                };
                let mut scan = AppAudit { at, packages, ..AppAudit::default() };
                if let Some(before) = self.audits.get(&key) {
                    audit::compare(&before.packages, &mut scan);
                }
                let fixed = match scan.fixed.len() {
                    0 => String::new(),
                    count => format!(" · {count} fixed since the last scan"),
                };
                self.add_log(format!("Audit: {name} - {}{fixed}", audit::summary(&scan.packages)), hue);
                scan
            }
            Err(error) => {
                self.add_log(format!("✗ Audit: {name} couldn't be scanned - {error}"), Hue::Red);
                let packages = self.audits.get(&key).map(|before| before.packages.clone()).unwrap_or_default();
                AppAudit { at, packages, error: Some(error), ..AppAudit::default() }
            }
        };
        self.audit_landed.insert(key.clone(), (self.now)());
        self.audits.insert(key, scan);
        if let Err(error) = audit::save(&self.state_dir, &self.audits) {
            self.add_log(format!("✗ Couldn't keep the audit: {error}"), Hue::Red);
        }
        self.next_audit();
        if self.auditing.is_none() && self.audit_queue.is_empty() {
            self.audit_finished();
        }
    }

    /// The scans asked for together are all done: one line for them all (`▸ Audit finished in 3.8s · 12 packages · 3
    /// high · 1 critical`), when there were several.
    fn audit_finished(&mut self) {
        let Some((started, total)) = self.audit_batch.take() else { return };
        if total < 2 {
            return;
        }
        let took = (self.now)().saturating_duration_since(started).as_secs_f64();
        let all: Vec<Package> = self.audits.values().flat_map(|scan| scan.packages.iter().cloned()).collect();
        let worst = all.iter().filter_map(|package| package.severity).max();
        let hue = match worst {
            Some(severity) if severity >= Severity::High => Hue::Red,
            Some(_) => Hue::Yellow,
            None => Hue::Green,
        };
        let plural = if all.len() == 1 { "" } else { "s" };
        let found = audit::summary(&all);
        self.add_log(format!("▸ Audit finished in {took:.1}s · {} package{plural} · {found}", all.len()), hue);
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tokio::sync::mpsc;

    use super::super::{Controller, Setup};
    use crate::paths::Paths;
    use crate::profile::{ProfileDirs, import};
    use crate::run::Event;
    use crate::run::audit::{self, Package, Severity};
    use crate::testing::{TempDir, sample_profile, sample_repo};

    #[tokio::test]
    async fn a_scan_is_kept_and_said_and_a_failed_one_keeps_the_one_before() {
        let temp = TempDir::new("audit-done");
        let paths = Paths {
            config_file: temp.join("config.json"),
            state_dir: temp.join("state"),
            profiles: ProfileDirs {
                user: temp.join("profiles"),
                bundled: temp.join("bundled"),
                only: Some(sample_profile()),
            },
        };
        crate::config::write(&paths.config_file, &json!({})).unwrap();
        let project = import(&sample_profile(), &paths.profiles).unwrap();
        let (events, _rx) = mpsc::unbounded_channel();
        let root = sample_repo(&temp.join("repo"));
        let mut controller = Controller::new(Setup { project, root, paths, events, guard: None }).unwrap();
        let key = controller.apps[0].key.clone();
        let package = Package {
            name: "Example.Json".into(),
            current: "12.0.1".into(),
            wanted: None,
            latest: Some("13.0.4".into()),
            direct: true,
            severity: Some(Severity::High),
            advisories: Vec::new(),
            fix: None,
            via: Vec::new(),
        };
        controller.handle(Event::Audit { key: key.clone(), result: Ok(vec![package.clone()]) });
        let said = controller.log.back().map(|entry| entry.text.clone()).unwrap_or_default();
        assert!(said.ends_with("- 1 high"), "{said}");
        assert_eq!(audit::load(&controller.state_dir)[&key].packages, std::slice::from_ref(&package), "kept");
        controller.handle(Event::Audit { key: key.clone(), result: Err("No assets file was found".into()) });
        let scan = &controller.audits()[&key];
        assert_eq!((scan.packages.len(), scan.error.as_deref()), (1, Some("No assets file was found")));
        controller.handle(Event::Audit { key: key.clone(), result: Ok(Vec::new()) });
        let said = controller.log.back().map(|entry| entry.text.clone()).unwrap_or_default();
        assert!(said.ends_with("up to date · 1 fixed since the last scan"), "{said}");
        assert!(controller.audit_landed(&key).is_some());

        controller.audit_batch = Some(((controller.now)(), 2));
        controller.auditing = Some(key.clone());
        assert_eq!(controller.audit_batch(), Some((1, 2)), "one of two left");
        controller.handle(Event::Audit { key: key.clone(), result: Ok(vec![package.clone()]) });
        let said = controller.log.back().map(|entry| entry.text.clone()).unwrap_or_default();
        assert!(said.starts_with("▸ Audit finished in") && said.ends_with("· 1 package · 1 high"), "{said}");
        assert_eq!(controller.audit_batch(), None);
    }
}
