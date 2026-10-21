//! What can be asked of the controller, from any side: a key, the command line (`lazyrunner restart`), the control
//! channel, and later an agent. One list, so those can't drift apart; serde, so the control channel (and an agent)
//! take it as JSON: `{"command": "startAll"}`, `{"command": {"restart": "orders"}}`,
//! `{"command": {"setToggle": {"toggle": "autoUpdate", "on": false}}}`.

use serde::{Deserialize, Serialize};

use super::Controller;
use crate::config::Toggle;

/// One thing to do; an app is named by its key in the profile.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Command {
    /// Start the app, or stop it (or cancel its build): whichever it isn't.
    ToggleApp(String),
    Start(String),
    StartAll,
    /// Stop the app, cancel its build, take it out of the build queue, or cancel its npm ci.
    Stop(String),
    /// Everything but the apps Stop all leaves running (includeInAll, stopFrontendOnStop).
    StopAll,
    Restart(String),
    RestartAll,
    /// A full build that leaves the app stopped.
    Build(String),
    BuildAll,
    NpmCi(String),
    /// Kill what holds a port an app needs, then start.
    KillPortUser,
    SetToggle {
        toggle: Toggle,
        on: bool,
    },
}

/// Who asked: what a restart says it was "requested from" (the panel's keys, the command line or control channel,
/// an agent).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    Panel,
    CommandLine,
    Agent,
}

impl Origin {
    fn says(self) -> &'static str {
        match self {
            Origin::Panel => "requested from the panel",
            Origin::CommandLine => "requested from the command line",
            Origin::Agent => "requested by an agent",
        }
    }
}

impl Command {
    /// The app it is about, if it is about one.
    fn app(&self) -> Option<&str> {
        match self {
            Command::ToggleApp(key)
            | Command::Start(key)
            | Command::Stop(key)
            | Command::Restart(key)
            | Command::Build(key)
            | Command::NpmCi(key) => Some(key),
            _ => None,
        }
    }
}

impl Controller {
    /// Does what was asked; an app this project doesn't have is refused (what happens goes to the log).
    pub fn execute(&mut self, command: Command, origin: Origin) -> Result<(), String> {
        if let Some(key) = command.app()
            && self.app(key).is_none()
        {
            return Err(format!("no app '{key}' here (the apps: {})", self.keys().join(", ")));
        }
        let reason = origin.says();
        match command {
            Command::ToggleApp(key) => self.toggle(&key),
            Command::Start(key) => self.start(Some(&[key]), false),
            Command::StartAll => self.start(None, false),
            Command::Stop(key) => self.stop_focused(&key),
            Command::StopAll => self.stop_all(),
            Command::Restart(key) => self.restart(reason, Some(&[key]), false),
            Command::RestartAll => self.restart(reason, None, false),
            Command::Build(key) => self.build_only(Some(&[key])),
            Command::BuildAll => self.build_only(None),
            Command::NpmCi(key) => self.npm_ci(Some(&key)),
            Command::KillPortUser => self.resolve_port_conflicts(),
            Command::SetToggle { toggle, on } => self.set_toggle(toggle, on),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn commands_read_as_json() {
        let read = |value| serde_json::from_value::<Command>(value).unwrap();
        assert_eq!(read(json!("startAll")), Command::StartAll);
        assert_eq!(read(json!({ "restart": "orders" })), Command::Restart("orders".into()));
        assert_eq!(
            read(json!({ "setToggle": { "toggle": "autoUpdate", "on": false } })),
            Command::SetToggle { toggle: Toggle::AutoUpdate, on: false }
        );
        assert!(serde_json::from_value::<Command>(json!({ "launch": "orders" })).is_err());
    }
}
