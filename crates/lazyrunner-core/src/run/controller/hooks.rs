//! Hooks (`"hooks"` in the settings) run when something happens: an app up or crashed, a build, a test run, migrations
//! applied, an update installed.

use super::Controller;
use crate::output::Hue;
use crate::run::Event;
use crate::run::hooks::{self, Happening};

impl Controller {
    /// `event` happened (to app `app`): each hook waiting for it runs in the background. Hooks that can't be used
    /// are said once, until the setting changes.
    pub(super) fn happened(&mut self, event: &'static str, app: Option<&str>, details: String) {
        let (all, problems) = hooks::read(&self.config);
        if !problems.is_empty() && self.hooks_said != self.config["hooks"] {
            self.hooks_said = self.config["hooks"].clone();
            for problem in problems {
                self.add_log(format!("▸ {problem}"), Hue::Yellow);
            }
        }
        let happening = Happening { event, app: app.map(str::to_string), details };
        for hook in hooks::matching(&all, &happening) {
            let (hook, happening) = (hook.clone(), happening.clone());
            let (folder, project, events) = (self.root.clone(), self.project.name.clone(), self.events.clone());
            std::thread::spawn(move || {
                if let Some(problem) = hooks::run(&hook, &folder, &project, &happening, hooks::TIMEOUT) {
                    let _ = events.send(Event::HookFailed { event: happening.event.into(), run: hook.run, problem });
                }
            });
        }
    }

    /// A hook went wrong: the log says which, and how.
    pub(super) fn hook_failed(&mut self, event: &str, run: &str, problem: &str) {
        self.add_log(format!("▸ hook {event} ({run}): {problem}"), Hue::Red);
    }

    /// An update this process installed: the hooks for it run once.
    pub(super) fn installed_update(&mut self) {
        let Some(version) = crate::update::installed() else { return };
        if self.hooked_update.as_deref() != Some(version.as_str()) {
            self.hooked_update = Some(version.clone());
            self.happened("update.installed", None, format!("v{version}"));
        }
    }
}
