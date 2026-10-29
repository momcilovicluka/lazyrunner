//! Hooks: your own commands, run when something happens - `"hooks"` in your settings (`lazyrunner.config.json`),
//! never a project's profile, as a hook runs a command on your machine:
//! `[{ "on": "tests.red", "run": "pwsh -File notify.ps1" }, { "on": "app.crashed", "app": "orders", "run": "..." }]`.
//! A hook runs in the background through the shell, in the project's folder, with what happened in environment
//! variables (`LAZYRUNNER_EVENT`, `LAZYRUNNER_PROJECT`, `LAZYRUNNER_APP`, `LAZYRUNNER_DETAILS`); one that fails or runs
//! too long says so in the log.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

/// What a hook can wait for.
pub const EVENTS: [&str; 8] = [
    "app.up",
    "app.crashed",
    "build.passed",
    "build.failed",
    "tests.green",
    "tests.red",
    "migrations.applied",
    "update.installed",
];
/// A hook still running after this long is stopped.
pub const TIMEOUT: Duration = Duration::from_secs(60);
const POLL: Duration = Duration::from_millis(50);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hook {
    pub on: String,
    /// Only this app's events (an app or build event).
    pub app: Option<String>,
    pub run: String,
}

/// What happened, for a hook: the event, its app's key, and a line about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Happening {
    pub event: &'static str,
    pub app: Option<String>,
    pub details: String,
}

/// The hooks in the settings, and why any of them can't be used.
pub fn read(config: &Value) -> (Vec<Hook>, Vec<String>) {
    let list = match &config["hooks"] {
        Value::Null => return (Vec::new(), Vec::new()),
        Value::Array(list) => list,
        _ => return (Vec::new(), vec!["\"hooks\" is a list of { \"on\", \"run\" }".into()]),
    };
    let (mut hooks, mut problems) = (Vec::new(), Vec::new());
    for (at, entry) in list.iter().enumerate() {
        match hook(entry) {
            Ok(hook) => hooks.push(hook),
            Err(problem) => problems.push(format!("hook {}: {problem}", at + 1)),
        }
    }
    (hooks, problems)
}

fn hook(entry: &Value) -> Result<Hook, String> {
    let on = entry["on"].as_str().unwrap_or_default();
    if !EVENTS.contains(&on) {
        return Err(format!("\"on\" is one of {}", EVENTS.join(", ")));
    }
    let run = entry["run"].as_str().map(str::trim).unwrap_or_default();
    if run.is_empty() {
        return Err("\"run\" is the command to run".into());
    }
    let app = match &entry["app"] {
        Value::Null => None,
        Value::String(app) if on.starts_with("app.") || on.starts_with("build.") => Some(app.clone()),
        _ => return Err("\"app\" is an app's key, for an app.* or build.* event".into()),
    };
    Ok(Hook { on: on.into(), app, run: run.into() })
}

/// The hooks that run when `happening` happens.
pub fn matching<'a>(hooks: &'a [Hook], happening: &'a Happening) -> impl Iterator<Item = &'a Hook> {
    hooks.iter().filter(|hook| {
        hook.on == happening.event && hook.app.as_ref().is_none_or(|app| happening.app.as_ref() == Some(app))
    })
}

/// Runs `hook` in `folder` and waits for it, `timeout` at most: `None` when it went fine, else what went wrong.
pub fn run(hook: &Hook, folder: &Path, project: &str, happening: &Happening, timeout: Duration) -> Option<String> {
    let mut command = shell(&hook.run);
    command
        .current_dir(folder)
        .env("LAZYRUNNER_EVENT", happening.event)
        .env("LAZYRUNNER_PROJECT", project)
        .env("LAZYRUNNER_APP", happening.app.as_deref().unwrap_or_default())
        .env("LAZYRUNNER_DETAILS", &happening.details)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => return Some(format!("couldn't start: {error}")),
    };
    let until = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return None,
            Ok(Some(status)) => return Some(status.code().map_or("stopped".into(), |code| format!("exit {code}"))),
            Ok(None) if Instant::now() >= until => {
                let _ = child.kill();
                let _ = child.wait();
                return Some(format!("still running after {}s - stopped", timeout.as_secs()));
            }
            Ok(None) => std::thread::sleep(POLL),
            Err(error) => return Some(error.to_string()),
        }
    }
}

/// The command line, through the shell: `cmd /S /C` on Windows, `sh -c` elsewhere.
#[cfg(windows)]
fn shell(run: &str) -> Command {
    use std::os::windows::process::CommandExt;
    let mut command = Command::new("cmd");
    command.raw_arg(format!("/D /S /C \"{run}\""));
    command
}

#[cfg(not(windows))]
fn shell(run: &str) -> Command {
    let mut command = Command::new("sh");
    command.arg("-c").arg(run);
    command
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn happening(event: &'static str, app: Option<&str>) -> Happening {
        Happening { event, app: app.map(str::to_string), details: "2 failed, 40 passed".into() }
    }

    #[test]
    fn hooks_are_read_and_the_ones_that_cant_be_used_say_why() {
        let config = json!({ "hooks": [
            { "on": "tests.red", "run": "notify" },
            { "on": "app.crashed", "app": "orders", "run": "say crashed" },
            { "on": "tests.blue", "run": "x" },
            { "on": "tests.red" },
            { "on": "tests.red", "app": "orders", "run": "x" },
        ] });
        let (hooks, problems) = read(&config);
        assert_eq!(hooks.len(), 2);
        assert_eq!(problems.len(), 3, "{problems:?}");
        assert!(problems[0].starts_with("hook 3: \"on\""), "{problems:?}");
        assert!(read(&json!({ "hooks": "x" })).1[0].contains("is a list"));
        assert_eq!(read(&json!({})), (Vec::new(), Vec::new()));
    }

    #[test]
    fn a_hook_with_an_app_waits_for_that_apps_events_only() {
        let (hooks, _) = read(&json!({ "hooks": [
            { "on": "app.crashed", "app": "orders", "run": "a" },
            { "on": "app.crashed", "run": "b" },
        ] }));
        let runs = |app| -> Vec<String> {
            let happening = happening("app.crashed", Some(app));
            matching(&hooks, &happening).map(|hook| hook.run.clone()).collect()
        };
        assert_eq!(runs("orders"), ["a", "b"]);
        assert_eq!(runs("billing"), ["b"]);
    }

    #[test]
    fn a_hook_gets_what_happened_and_its_exit_code_or_time_limit_is_said() {
        let folder = std::env::temp_dir();
        let hook = |run: &str| Hook { on: "tests.red".into(), app: None, run: run.into() };
        let red = happening("tests.red", None);
        let check = if cfg!(windows) {
            r#"if "%LAZYRUNNER_EVENT%|%LAZYRUNNER_DETAILS%"=="tests.red|2 failed, 40 passed" (exit 0) else (exit 1)"#
        } else {
            r#"[ "$LAZYRUNNER_EVENT|$LAZYRUNNER_DETAILS" = "tests.red|2 failed, 40 passed" ]"#
        };
        assert_eq!(run(&hook(check), &folder, "shop", &red, TIMEOUT), None);
        assert_eq!(run(&hook("exit 3"), &folder, "shop", &red, TIMEOUT).as_deref(), Some("exit 3"));
        let wait = if cfg!(windows) { "ping -n 5 127.0.0.1 >nul" } else { "sleep 5" };
        let late = run(&hook(wait), &folder, "shop", &red, Duration::from_millis(200));
        assert!(late.is_some_and(|late| late.starts_with("still running")));
    }
}
