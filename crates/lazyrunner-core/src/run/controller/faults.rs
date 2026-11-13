//! The faults set on the calls between apps (#105): kept in faults.json for the hooks, answered by the fault server.
//! An npm app's call hook reads them as they change. A .NET app needs its startup hook - built with dotnet for its
//! target framework the first time a fault is set on one of its calls - so one that started before the hook was there
//! starts again once to load it; every .NET app started after gets it (with no fault it does nothing).

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::Instant;

use super::Controller;
use crate::output::Hue;
use crate::profile::AppKind;
use crate::run::faults::{self, Fault, Kind};
use crate::run::tools::ServerCommand;

/// The faults, the fault server once one is set, and the .NET hooks: built (and when), and the one being built - for
/// which framework, and the apps that wait for it.
#[derive(Default)]
pub(super) struct Faults {
    list: Vec<Fault>,
    next_id: u64,
    server: Option<faults::Server>,
    built: HashMap<String, (PathBuf, Instant)>,
    building: Option<Building>,
    /// The kept ones were read in; they're kept as they change (`Ctrl+S` in the box).
    loaded: bool,
    keeping: bool,
}

struct Building {
    framework: String,
    waiting: Vec<String>,
    result: Receiver<Result<PathBuf, String>>,
}

impl Controller {
    /// The faults set, the newest last.
    pub fn faults(&self) -> &[Fault] {
        &self.faults.list
    }

    /// How many calls fault `id` answered (failed or mocked ones; a delay or a drop happens in the app).
    pub fn fault_hits(&self, id: u64) -> usize {
        self.faults.server.as_ref().map_or(0, |server| server.hits(id))
    }

    /// Sets a fault on `caller`'s calls to `target` (`host:port` or a host; `to_app` when it's an app) on routes
    /// starting with `path`, for `share` percent of them.
    pub fn set_fault(
        &mut self,
        caller: &str,
        (target, to_app): (String, Option<String>),
        path: &str,
        (kind, share): (Kind, u8),
        lasts: Option<std::time::Duration>,
    ) {
        let Some(app) = self.app(caller).cloned() else { return };
        if matches!(app.kind, AppKind::Command(_)) {
            self.add_log(format!("Faults are for .NET and npm apps' calls; {} is neither.", app.name), Hue::DarkGray);
            return;
        }
        if self.faults.server.is_none() {
            match faults::Server::start() {
                Ok(server) => self.faults.server = Some(server),
                Err(error) => {
                    self.add_log(format!("✗ The fault server didn't start: {error}"), Hue::Red);
                    return;
                }
            }
        }
        self.faults.next_id += 1;
        let share = share.clamp(1, 100);
        let until = lasts.map(|lasts| (self.now)() + lasts);
        let id = self.faults.next_id;
        let fault = Fault { id, caller: caller.into(), target, to_app, path: path.into(), kind, share, until };
        self.add_log(format!("⚡ {} → {}: {}", app.name, fault.target, fault.said()), Hue::Yellow);
        self.faults.list.push(fault);
        self.publish_faults();
        if let AppKind::Dotnet { project, .. } = &app.kind {
            let framework = faults::target_framework(&self.root.join(project));
            self.hook_for(caller, &framework);
        }
    }

    /// Takes faults away: `caller`'s to `to` (every target when none), or every one with no caller.
    pub fn clear_faults(&mut self, caller: Option<&str>, to: Option<&str>) {
        let before = self.faults.list.len();
        self.faults.list.retain(|fault| {
            let mine = caller.is_none_or(|caller| fault.caller == caller);
            let there = to.is_none_or(|to| fault.target == to || fault.to_app.as_deref() == Some(to));
            !(mine && there)
        });
        let gone = before - self.faults.list.len();
        if gone > 0 {
            self.publish_faults();
            self.add_log(format!("⚡ {gone} fault{} taken away", if gone == 1 { "" } else { "s" }), Hue::Cyan);
        }
    }

    fn faults_file(&self) -> PathBuf {
        self.state_dir.join(faults::FILE)
    }

    /// Writes the faults for the hooks and hands them to the fault server.
    fn publish_faults(&mut self) {
        let port = self.faults.server.as_ref().map_or(0, |server| server.port);
        if let Some(server) = &self.faults.server {
            server.set(&self.faults.list);
        }
        if let Err(error) = faults::write(&self.faults_file(), port, &self.faults.list) {
            self.add_log(format!("✗ Couldn't write the faults for the apps: {error}"), Hue::Red);
        }
        if self.faults.keeping
            && let Err(error) = faults::keep(&self.state_dir.join(faults::KEPT_FILE), &self.faults.list)
        {
            self.add_log(format!("✗ Couldn't keep the faults: {error}"), Hue::Red);
        }
    }

    /// `Ctrl+S` in the faults box: the faults (and those set after) are set again when lazyrunner opens.
    pub fn keep_faults(&mut self) {
        self.faults.keeping = true;
        self.publish_faults();
        let count = self.faults.list.iter().filter(|fault| fault.until.is_none()).count();
        let plural = if count == 1 { "" } else { "s" };
        self.add_log(format!("⚡ {count} fault{plural} kept: set again when lazyrunner opens"), Hue::Cyan);
    }

    /// Whether the faults are kept beyond this session.
    pub fn faults_kept(&self) -> bool {
        self.faults.keeping
    }

    /// The faults kept from before, set again once, as lazyrunner opens.
    fn load_kept_faults(&mut self) {
        self.faults.loaded = true;
        let kept = faults::kept(&self.state_dir.join(faults::KEPT_FILE));
        if kept.is_empty() {
            return;
        }
        self.faults.keeping = true;
        for fault in kept {
            let target = (fault.target, fault.to_app);
            self.set_fault(&fault.caller, target, &fault.path, (fault.kind, fault.share), None);
        }
    }

    /// A .NET caller has its hook: built already (it starts again when it started before), or built now.
    fn hook_for(&mut self, caller: &str, framework: &str) {
        if let Some((_, ready)) = self.faults.built.get(framework) {
            let before = self.state(caller).and_then(|state| state.started_at).is_some_and(|started| started < *ready);
            if before && self.is_running(caller) {
                self.restart("to load lazyrunner's fault hook", Some(&[caller.to_string()]), false);
            }
            return;
        }
        if let Some(building) = self.faults.building.as_mut().filter(|building| building.framework == framework) {
            if !building.waiting.iter().any(|key| key == caller) {
                building.waiting.push(caller.to_string());
            }
            return;
        }
        let (sender, result) = channel();
        let (state_dir, wanted) = (self.state_dir.clone(), framework.to_string());
        std::thread::spawn(move || {
            let _ = sender.send(faults::build_hook(&state_dir, &wanted));
        });
        self.add_log(format!("⚡ Building lazyrunner's fault hook for {framework} (once)…"), Hue::DarkGray);
        let waiting = vec![caller.to_string()];
        self.faults.building = Some(Building { framework: framework.to_string(), waiting, result });
    }

    /// A hook that's built: the .NET apps that wait for it start again with it.
    pub(super) fn tick_faults(&mut self) {
        if !self.faults.loaded {
            self.load_kept_faults();
        }
        let now = (self.now)();
        let before = self.faults.list.len();
        self.faults.list.retain(|fault| fault.until.is_none_or(|until| now < until));
        if self.faults.list.len() < before {
            self.publish_faults();
            self.add_log("⚡ The chaos is over: those faults are gone", Hue::Cyan);
        }
        let Some(building) = &self.faults.building else { return };
        let outcome = match building.result.try_recv() {
            Ok(outcome) => outcome,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err("the build stopped".to_string()),
        };
        let Some(building) = self.faults.building.take() else { return };
        match outcome {
            Ok(path) => {
                self.faults.built.insert(building.framework.clone(), (path, (self.now)()));
                let running: Vec<String> = building.waiting.into_iter().filter(|key| self.is_running(key)).collect();
                if running.is_empty() {
                    self.add_log("⚡ The fault hook is ready: the app gets it from its next start", Hue::Cyan);
                } else {
                    self.restart("to load lazyrunner's fault hook", Some(&running), false);
                }
            }
            Err(why) => {
                self.add_log(format!("✗ The fault hook didn't build ({why}): .NET apps' faults won't apply"), Hue::Red)
            }
        }
    }

    /// `command` with what the hooks need: an npm app its key (its call hook reads the faults beside it); a .NET app,
    /// once its framework's hook is built, the hook, the faults file and its key.
    pub(super) fn with_fault_hook(&self, key: &str, mut command: ServerCommand) -> ServerCommand {
        let Some(app) = self.app(key) else { return command };
        let hook = match &app.kind {
            AppKind::Npm { .. } => None,
            AppKind::Dotnet { project, .. } => {
                let framework = faults::target_framework(&self.root.join(project));
                let Some((path, _)) = self.faults.built.get(&framework) else { return command };
                Some(path.clone())
            }
            AppKind::Command(_) => return command,
        };
        command.spec.env.push((OsString::from(faults::ENV_APP), OsString::from(key)));
        if let Some(hook) = hook {
            let name = OsString::from(faults::ENV_HOOKS);
            let joined = match std::env::var_os(faults::ENV_HOOKS).filter(|before| !before.is_empty()) {
                Some(before) => {
                    let mut joined = before;
                    joined.push(if cfg!(windows) { ";" } else { ":" });
                    joined.push(hook.as_os_str());
                    joined
                }
                None => hook.into_os_string(),
            };
            command.spec.env.push((name, joined));
            command.spec.env.push((OsString::from(faults::ENV_FILE), self.faults_file().into_os_string()));
        }
        command
    }
}

#[cfg(test)]
mod tests {
    use crate::run::faults::Kind;
    use crate::testing::TempDir;

    #[tokio::test]
    async fn a_fault_is_written_for_the_hooks_and_taken_away_again() {
        let temp = TempDir::new("controller-faults");
        let (mut controller, _rx) = super::super::tests::controller(&temp, serde_json::json!({}));
        let npm = controller.apps.iter().find(|app| matches!(app.kind, crate::profile::AppKind::Npm { .. })).unwrap();
        let (caller, target) = (npm.key.clone(), controller.apps[0].key.clone());
        let to = (format!("localhost:{}", controller.port(&target).unwrap_or(0)), Some(target.clone()));
        controller.set_fault(&caller, to, "/api", (Kind::Fail(503), 30), None);
        let written = crate::json::read(&controller.state_folder().join(crate::run::faults::FILE)).unwrap();
        assert_eq!(written["faults"][0]["caller"], caller.as_str());
        assert_eq!(written["faults"][0]["status"], 503);
        assert!(written["port"].as_u64().unwrap_or(0) > 0, "the fault server listens");
        assert!(
            controller.log.back().unwrap().text.ends_with("503 · 30% /api"),
            "{}",
            controller.log.back().unwrap().text
        );
        controller.clear_faults(Some(&caller), Some(&target));
        assert!(controller.faults().is_empty());
        let written = crate::json::read(&controller.state_folder().join(crate::run::faults::FILE)).unwrap();
        assert_eq!(written["faults"].as_array().map(Vec::len), Some(0));
    }
}
