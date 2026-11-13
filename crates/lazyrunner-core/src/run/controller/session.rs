//! The session kept when lazyrunner closes (#85): which apps ran, kept for the next lazyrunner on this branch, which
//! starts them instead of all of them.

use serde_json::Value;

use super::Controller;
use crate::config::Toggle;
use crate::output::Hue;
use crate::run::session::{self, Snapshot};

impl Controller {
    /// The snapshot to offer at the start, when restoreSession is on.
    pub fn load_session(&mut self) {
        self.resume = None;
        if self.setting(Toggle::RestoreSession) {
            self.resume = session::load(&self.state_dir, &self.branch, chrono::Local::now().fixed_offset());
        }
    }

    /// Keeps this session for the next lazyrunner: the running apps and `view`, the screen as the UI gives it.
    /// Not while the one offered at the start is still to be resumed: it stays as it was.
    pub fn save_session(&mut self, view: Value) {
        if !self.setting(Toggle::RestoreSession) || self.resume.is_some() {
            return;
        }
        let running = self.keys().into_iter().filter(|key| self.is_running(key)).collect();
        let at = chrono::Local::now().fixed_offset().to_rfc3339();
        let snapshot = Snapshot { at, branch: self.branch.clone(), running, view };
        if let Err(error) = session::save(&self.state_dir, &snapshot) {
            self.add_log(format!("✗ Couldn't keep the session: {error}"), Hue::Red);
        }
    }

    /// The apps the offered snapshot had running that this profile still has; the others are left out, each with a
    /// line in the log.
    pub fn resumed_apps(&mut self) -> Vec<String> {
        let Some(snapshot) = &self.resume else { return Vec::new() };
        let (kept, gone): (Vec<String>, Vec<String>) =
            snapshot.running.iter().cloned().partition(|key| self.app(key).is_some());
        for key in gone {
            self.add_log(format!("Left {key} out of the session: the profile no longer has it"), Hue::Yellow);
        }
        kept
    }

    /// Starts the apps that were running when lazyrunner closed, instead of all of them; their keys - none when
    /// there were none to start (then everything starts as before).
    pub fn start_resumed(&mut self) -> Vec<String> {
        let keys = self.resumed_apps();
        if keys.is_empty() {
            return keys;
        }
        let names: Vec<String> = keys.iter().filter_map(|key| self.app(key)).map(|app| app.name.clone()).collect();
        self.add_log(
            format!("↺ Resuming the session: {} - as when lazyrunner closed (U starts everything)", names.join(", ")),
            Hue::Cyan,
        );
        self.start(Some(&keys), false);
        keys
    }
}
