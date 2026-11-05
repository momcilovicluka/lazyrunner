//! The audit view's scans (#79): one app at a time, in the background, each kept with the others in the state folder
//! as it lands.

use std::time::Instant;

use serde_json::json;

use super::{Controller, parallel};
use crate::output::Hue;
use crate::run::Event;
use crate::run::audit::{self, AppAudit, Audits, Package, Severity};

/// The fixes being installed from the audit view: their app, the commands, whether the app was running (it starts
/// again after), and the packages they fix.
pub(super) struct Installing {
    key: String,
    command: String,
    restart: bool,
    names: Vec<String>,
}

impl Controller {
    /// The apps' last scans, by key.
    pub fn audits(&self) -> &Audits {
        &self.audits
    }

    /// The apps being scanned now.
    pub fn auditing(&self) -> &[String] {
        &self.auditing
    }

    /// How many apps are scanned at once: `"auditParallelism"`, 2 when it's missing (1 to 8) - a .NET app's scan
    /// keeps every core busy for a while.
    pub fn audit_parallelism(&self) -> usize {
        parallel::at_once(&self.config["auditParallelism"], 2)
    }

    /// `+` and `-` in the audit view: how many apps are scanned at once, saved; the waiting scans go so.
    pub fn set_audit_parallelism(&mut self, count: usize) {
        let count = count.clamp(1, parallel::MOST);
        self.set_setting("auditParallelism", json!(count));
        let plural = if count == 1 { "" } else { "s" };
        self.add_log(format!("Audit: {count} app{plural} scanned at once"), Hue::Gray);
        self.next_audit();
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
        self.audit_install_all(key, vec![package.clone()]);
    }

    /// `I` in the audit view, after a yes: installs the fixes of `packages` together - the app stops once, an npm
    /// app's go in one `npm install` - then the app starts again and is scanned again.
    pub fn audit_install_all(&mut self, key: &str, packages: Vec<Package>) {
        if let Some((_, command)) = self.audit_installing() {
            self.add_log(format!("Still installing ({command})"), Hue::Gray);
            return;
        }
        let Some(app) = self.app(key).cloned() else { return };
        let commands = audit::commands(&app, &packages);
        if commands.is_empty() {
            let what = match packages.as_slice() {
                [package] => package.name.clone(),
                _ => app.name.clone(),
            };
            self.add_log(format!("{what}: nothing to install"), Hue::DarkGray);
            return;
        }
        let command = commands.join(" && ");
        let restart = self.is_running(key);
        if restart {
            self.stop(Some(&[key.to_string()]), true);
        }
        let what = if packages.len() > 1 { format!("{} fixes", packages.len()) } else { "the fix".into() };
        self.add_log(format!("Installing {what} for {}: {command}…", app.name), Hue::Cyan);
        let names = packages.iter().map(|package| package.name.clone()).collect();
        self.audit_installing = Some(Installing { key: key.to_string(), command, restart, names });
        let (root, events, key) = (self.root.clone(), self.events.clone(), key.to_string());
        tokio::task::spawn_blocking(move || {
            let result = audit::install(&root, &app, &packages).map_err(|error| error.to_string());
            let _ = events.send(Event::AuditInstalled { key, result });
        });
    }

    /// The packages whose fixes are being installed.
    pub fn audit_installing_names(&self) -> &[String] {
        self.audit_installing.as_ref().map_or(&[], |installing| installing.names.as_slice())
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
                let failed = (key.clone(), installing.command, installing.names, (self.now)());
                self.audit_install_failed = Some(failed);
            }
        }
        if installing.restart {
            self.start(Some(std::slice::from_ref(&key)), false);
        }
        self.audit(Some(&[key]));
    }

    /// `r` / `R` in the audit view: scans these apps (every app with none) in the background, `"auditParallelism"` at
    /// once.
    pub fn audit(&mut self, keys: Option<&[String]>) {
        let keys = keys.map_or_else(|| self.keys(), <[String]>::to_vec);
        let before = self.audits_left();
        if before == 0 {
            self.audit_retry.reset();
        }
        for key in keys {
            if !self.auditing.contains(&key) && !self.audit_queue.contains(&key) && !self.audit_retry.waits(&key) {
                self.audit_queue.push(key);
            }
        }
        let added = self.audits_left() - before;
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
        Some((total.saturating_sub(self.audits_left()), total))
    }

    /// The scans not done yet: running, waiting, or waiting to go again.
    fn audits_left(&self) -> usize {
        self.audit_queue.len() + self.auditing.len() + self.audit_retry.len()
    }

    /// The last fixes that failed to install: their app, the commands, the packages, and when.
    pub fn audit_install_failed(&self) -> Option<(&str, &str, &[String], Instant)> {
        let (key, command, names, at) = self.audit_install_failed.as_ref()?;
        Some((key.as_str(), command.as_str(), names.as_slice(), *at))
    }

    /// Starts the waiting scans, up to `"auditParallelism"` at once; once all are done, those that failed beside
    /// others go again, one at a time.
    fn next_audit(&mut self) {
        while self.auditing.len() < self.audit_parallelism() && !self.audit_queue.is_empty() {
            let key = self.audit_queue.remove(0);
            self.scan_app(key);
        }
        if self.auditing.is_empty()
            && self.audit_queue.is_empty()
            && let Some(key) = self.audit_retry.next()
        {
            self.scan_app(key);
        }
    }

    /// Scans app `key` in the background.
    fn scan_app(&mut self, key: String) {
        let Some(app) = self.app(&key).cloned() else { return };
        let (root, events) = (self.root.clone(), self.events.clone());
        self.auditing.push(key.clone());
        self.audit_retry.together(self.auditing.iter().map(String::as_str));
        tokio::task::spawn_blocking(move || {
            let result = audit::scan(&root, &app).map_err(|error| error.to_string());
            let _ = events.send(Event::Audit { key, result });
        });
    }

    /// An app's scan landed: kept (a failed one keeps the scan before, and its time, and says why), said in the log;
    /// the next goes. One that failed beside other scans goes again on its own first, once they're done.
    pub(super) fn audit_done(&mut self, key: String, result: Result<Vec<Package>, String>) {
        self.auditing.retain(|scanning| *scanning != key);
        let name = self.app(&key).map_or_else(|| key.clone(), |app| app.name.clone());
        if let Err(error) = &result
            && self.audit_retry.again(&key)
        {
            let text =
                format!("! Audit: {name} couldn't be scanned beside the others ({error}) - scanning it again alone");
            self.add_log(text, Hue::Yellow);
            self.next_audit();
            return;
        }
        self.audit_retry.done(&key);
        let at = chrono::Local::now().timestamp();
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
                let before = self.audits.get(&key);
                let packages = before.map(|before| before.packages.clone()).unwrap_or_default();
                let at = before.map_or(at, |before| before.at);
                AppAudit { at, packages, error: Some(error), ..AppAudit::default() }
            }
        };
        self.audit_landed.insert(key.clone(), (self.now)());
        self.audits.insert(key, scan);
        if let Err(error) = audit::save(&self.state_dir, &self.audits) {
            self.add_log(format!("✗ Couldn't keep the audit: {error}"), Hue::Red);
        }
        self.next_audit();
        if self.audits_left() == 0 {
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

    fn controller(temp: &TempDir) -> Controller {
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
        Controller::new(Setup { project, root, paths, events, guard: None }).unwrap()
    }

    #[tokio::test]
    async fn a_scan_that_failed_beside_others_goes_again_alone_before_it_counts() {
        let temp = TempDir::new("audit-again");
        let mut controller = controller(&temp);
        let (first, second) = ("gone-a".to_string(), "gone-b".to_string());
        controller.audit_batch = Some(((controller.now)(), 2));
        controller.auditing = vec![first.clone(), second.clone()];
        controller.audit_retry.together(["gone-a", "gone-b"]);
        controller.handle(Event::Audit { key: first.clone(), result: Err("429 Too Many Requests".into()) });
        let said = controller.log.back().map(|entry| entry.text.clone()).unwrap_or_default();
        assert!(said.contains("scanning it again alone"), "{said}");
        assert!(!controller.audits().contains_key(&first), "the failure doesn't count yet");
        assert_eq!(controller.audit_batch(), Some((0, 2)), "it's still to be done");
        controller.auditing.clear();
        controller.handle(Event::Audit { key: first.clone(), result: Err("429 Too Many Requests".into()) });
        let failed = controller.audits().get(&first).and_then(|scan| scan.error.clone());
        assert_eq!(failed.as_deref(), Some("429 Too Many Requests"), "alone, it fails for good");
        assert_eq!(controller.audit_parallelism(), 2, "two at once when it isn't set");
        controller.set_audit_parallelism(20);
        assert_eq!(controller.audit_parallelism(), 8, "eight at most");
    }

    #[tokio::test]
    async fn a_scan_is_kept_and_said_and_a_failed_one_keeps_the_one_before() {
        let temp = TempDir::new("audit-done");
        let mut controller = controller(&temp);
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
        controller.auditing = vec![key.clone()];
        assert_eq!(controller.audit_batch(), Some((1, 2)), "one of two left");
        controller.handle(Event::Audit { key: key.clone(), result: Ok(vec![package.clone()]) });
        let said = controller.log.back().map(|entry| entry.text.clone()).unwrap_or_default();
        assert!(said.starts_with("▸ Audit finished in") && said.ends_with("· 1 package · 1 high"), "{said}");
        assert_eq!(controller.audit_batch(), None);
    }
}
