//! What an app gets on this machine on top of what it's started with (the start sheet, `e`): environment variables
//! that win over its launch profile's and the profile's own - on every branch, or kept to one. Kept in the project's
//! state folder (`env-overrides.json`), never in a shared file. A .NET app gets them as `dotnet run -e`, which beats
//! its launch profile's variables; an npm app in its environment. They apply from the app's next start.

use std::collections::HashMap;
use std::ffi::OsString;

use anyhow::{Result, anyhow};
use serde_json::{Map, Value, json};

use super::Controller;
use crate::output::Hue;
use crate::profile::AppKind;
use crate::run::secrets;
use crate::run::tools::ServerCommand;

const FILE: &str = "env-overrides.json";
/// Where the file keeps the overrides for every branch, and those kept to one.
const EVERY_BRANCH: &str = "machine";
const BRANCHES: &str = "branches";

/// A variable an app gets on this machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Override {
    pub name: String,
    pub value: String,
    /// Kept to the branch it was set on.
    pub branch: bool,
}

impl Controller {
    /// App `key`'s overrides on this branch, by name.
    pub fn env_overrides(&self, key: &str) -> &[Override] {
        self.env_overrides.get(key).map_or(&[], Vec::as_slice)
    }

    /// Gives app `key` `name` = `value` from its next start: on this branch only, or on every one.
    pub fn set_env_override(&mut self, key: &str, name: &str, value: &str, branch: bool) {
        self.edit_overrides(key, |every, here| {
            let (to, from) = if branch { (here, every) } else { (every, here) };
            from.remove(name);
            to.insert(name.to_string(), json!(value));
        });
        let on = if branch { format!(" on {}", self.branch) } else { String::new() };
        self.add_log(format!("{}: {name} overridden{on} from its next start", self.app_title(key)), Hue::Cyan);
    }

    /// Drops app `key`'s override of `name`: from its next start it gets what it's started with again.
    pub fn remove_env_override(&mut self, key: &str, name: &str) {
        self.edit_overrides(key, |every, here| {
            every.remove(name);
            here.remove(name);
        });
        self.add_log(format!("{}: {name} no longer overridden from its next start", self.app_title(key)), Hue::Cyan);
    }

    /// Moves app `key`'s override of `name` into its user secrets (`Section__Key` as `Section:Key`), out of
    /// lazyrunner's state.
    pub fn override_to_secrets(&mut self, key: &str, name: &str) -> Result<()> {
        let found = self.env_overrides(key).iter().find(|found| found.name == name).cloned();
        let found = found.ok_or_else(|| anyhow!("{name} isn't overridden"))?;
        let app = self.app(key).ok_or_else(|| anyhow!("no app '{key}'"))?;
        let path = secrets::app_secrets_path(&self.root, app)
            .ok_or_else(|| anyhow!("it has no user secrets (dotnet user-secrets init gives it some)"))?;
        secrets::set(&path, &name.replace("__", ":"), &found.value)?;
        self.edit_overrides(key, |every, here| {
            every.remove(name);
            here.remove(name);
        });
        self.add_log(format!("{}: {name} moved into its user secrets", self.app_title(key)), Hue::Green);
        Ok(())
    }

    /// `command` with app `key`'s overrides: `-e NAME=VALUE` before a .NET app's own arguments, an npm app's in its
    /// environment after the profile's, so they win. Its label names them, not their values.
    pub(super) fn with_overrides(&self, key: &str, mut command: ServerCommand) -> ServerCommand {
        let overrides = self.env_overrides(key);
        if overrides.is_empty() {
            return command;
        }
        if matches!(self.app(key).map(|app| &app.kind), Some(AppKind::Dotnet { .. })) {
            let at = command.spec.args.iter().position(|arg| arg == "--").unwrap_or(command.spec.args.len());
            let flags = overrides
                .iter()
                .flat_map(|found| [OsString::from("-e"), format!("{}={}", found.name, found.value).into()]);
            command.spec.args.splice(at..at, flags);
        } else {
            let pairs = overrides.iter().map(|found| (OsString::from(&found.name), OsString::from(&found.value)));
            command.spec.env.extend(pairs);
        }
        let names: Vec<&str> = overrides.iter().map(|found| found.name.as_str()).collect();
        command.label = format!("{} · env {}", command.label, names.join(" "));
        command
    }

    /// Reads the overrides for this branch: those for every branch, and this branch's over them.
    pub(super) fn load_env_overrides(&mut self) {
        let all = self.saved_overrides();
        let mut loaded = HashMap::new();
        for app in &self.apps {
            let mut found: Vec<Override> = Vec::new();
            let scopes = [(false, vec![EVERY_BRANCH, &app.key]), (true, vec![BRANCHES, &self.branch, &app.key])];
            for (branch, path) in scopes {
                for (name, value) in scope(&all, &path) {
                    let Some(value) = value.as_str() else { continue };
                    found.retain(|other| other.name != name);
                    found.push(Override { name, value: value.to_string(), branch });
                }
            }
            found.sort_by(|one, other| one.name.cmp(&other.name));
            if !found.is_empty() {
                loaded.insert(app.key.clone(), found);
            }
        }
        self.env_overrides = loaded;
    }

    /// Changes app `key`'s overrides for every branch and for this one, and keeps them.
    fn edit_overrides(&mut self, key: &str, edit: impl FnOnce(&mut Map<String, Value>, &mut Map<String, Value>)) {
        let mut all = self.saved_overrides();
        let (every_path, here_path) = ([EVERY_BRANCH, key], [BRANCHES, self.branch.as_str(), key]);
        let (mut every, mut here) = (scope(&all, &every_path), scope(&all, &here_path));
        edit(&mut every, &mut here);
        put(&mut all, &every_path, every);
        put(&mut all, &here_path, here);
        if let Err(error) = crate::config::write(&self.state_dir.join(FILE), &Value::Object(all)) {
            self.add_log(format!("✗ Couldn't keep the override: {error}"), Hue::Red);
        }
        self.load_env_overrides();
    }

    fn saved_overrides(&self) -> Map<String, Value> {
        crate::json::read(&self.state_dir.join(FILE)).and_then(|value| value.as_object().cloned()).unwrap_or_default()
    }

    pub(super) fn app_title(&self, key: &str) -> String {
        self.app(key).map_or_else(|| key.to_string(), |app| app.name.clone())
    }
}

/// The object at `path` in `all`, or an empty one.
fn scope(all: &Map<String, Value>, path: &[&str]) -> Map<String, Value> {
    path.iter().try_fold(all, |map, step| map.get(*step)?.as_object()).cloned().unwrap_or_default()
}

/// Puts `map` at `path` in `all`, dropping the objects left empty.
fn put(all: &mut Map<String, Value>, path: &[&str], map: Map<String, Value>) {
    let [first, rest @ ..] = path else { return };
    if rest.is_empty() {
        if map.is_empty() {
            all.remove(*first);
        } else {
            all.insert((*first).to_string(), Value::Object(map));
        }
        return;
    }
    let entry = all.entry((*first).to_string()).or_insert_with(|| json!({}));
    if !entry.is_object() {
        *entry = json!({});
    }
    let empty = entry.as_object_mut().is_none_or(|inner| {
        put(inner, rest, map);
        inner.is_empty()
    });
    if empty {
        all.remove(*first);
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use serde_json::json;
    use tokio::sync::mpsc;

    use super::super::{Controller, Setup};
    use crate::paths::Paths;
    use crate::profile::{ProfileDirs, import};
    use crate::run::child::Spec;
    use crate::run::tools::ServerCommand;
    use crate::testing::{TempDir, sample_profile, sample_repo};

    fn controller(temp: &TempDir) -> Controller {
        let repo = sample_repo(&temp.join("repo"));
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
        Controller::new(Setup { project, root: repo, paths, events, guard: None }).unwrap()
    }

    #[tokio::test]
    async fn an_override_goes_before_a_dotnet_apps_own_arguments_and_one_kept_to_a_branch_stays_there() {
        let temp = TempDir::new("env-overrides");
        let mut controller = controller(&temp);
        let key = controller.apps[0].key.clone();
        controller.set_env_override(&key, "ASPNETCORE_ENVIRONMENT", "Staging", false);
        controller.set_env_override(&key, "FEATURE_X", "on", true);
        let names: Vec<(&str, bool)> =
            controller.env_overrides(&key).iter().map(|found| (found.name.as_str(), found.branch)).collect();
        assert_eq!(names, [("ASPNETCORE_ENVIRONMENT", false), ("FEATURE_X", true)]);
        let command = controller.server_command(&key).unwrap();
        let args: Vec<String> = command.spec.args.iter().map(|arg| arg.to_string_lossy().into_owned()).collect();
        let at = args.iter().position(|arg| arg == "--").unwrap();
        assert_eq!(args[at - 4..at], ["-e", "ASPNETCORE_ENVIRONMENT=Staging", "-e", "FEATURE_X=on"], "{args:?}");
        assert!(
            command.label.ends_with("· env ASPNETCORE_ENVIRONMENT FEATURE_X"),
            "names, no values: {}",
            command.label
        );

        let main = std::mem::replace(&mut controller.branch, "feature".into());
        controller.load_env_overrides();
        let names: Vec<&str> = controller.env_overrides(&key).iter().map(|found| found.name.as_str()).collect();
        assert_eq!(names, ["ASPNETCORE_ENVIRONMENT"], "another branch keeps only those for every branch");
        controller.branch = main;
        controller.load_env_overrides();
        controller.set_env_override(&key, "ASPNETCORE_ENVIRONMENT", "Staging", true);
        controller.remove_env_override(&key, "FEATURE_X");
        let kept: Vec<(&str, bool)> =
            controller.env_overrides(&key).iter().map(|found| (found.name.as_str(), found.branch)).collect();
        assert_eq!(kept, [("ASPNETCORE_ENVIRONMENT", true)], "moved to the branch, the other gone");
        controller.remove_env_override(&key, "ASPNETCORE_ENVIRONMENT");
        let saved = crate::json::read(&controller.state_dir.join(super::FILE)).unwrap();
        assert_eq!(saved, json!({}), "nothing left behind");
    }

    #[tokio::test]
    async fn an_npm_app_gets_its_overrides_in_its_environment_after_the_profiles() {
        let temp = TempDir::new("env-overrides-npm");
        let mut controller = controller(&temp);
        let key = controller.apps.iter().find(|app| !app.traits().builds).map(|app| app.key.clone()).unwrap();
        controller.set_env_override(&key, "VITE_API", "http://localhost:6001", false);
        let spec = Spec {
            program: "npm".into(),
            args: vec!["run".into(), "dev".into()],
            dir: temp.join("web"),
            env: vec![("VITE_API".into(), "http://localhost:5001".into())],
        };
        let command =
            controller.with_overrides(&key, ServerCommand { spec, label: "npm run dev".into(), script: None });
        assert_eq!(command.spec.args, [OsString::from("run"), "dev".into()], "its arguments untouched");
        assert_eq!(command.spec.env.last(), Some(&("VITE_API".into(), "http://localhost:6001".into())), "last wins");
    }
}
