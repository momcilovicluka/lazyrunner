//! The settings for this machine: the defaults, then the settings file. Its top level is for everything; the
//! settings that belong to a project ([`PROJECT_KEYS`]) live in that project's section, `"projects": { "<id>": {} }`.

use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::json;

/// The settings kept per project.
pub const PROJECT_KEYS: [&str; 4] = ["columnWidths", "apps", "frontend", "stopFrontendOnStop"];
const WRITE_ATTEMPTS: usize = 10;
const WRITE_RETRY: Duration = Duration::from_millis(40);

/// The on/off settings lazyrunner reads: their names in lazyrunner.config.json and their defaults, in one list,
/// so a name can't be mistyped where it is read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Toggle {
    AutoStartOnOpen,
    AutoRestartOnBranchChange,
    StopFrontendOnStop,
    SkipUnchangedBuilds,
    Notifications,
    RestartOnCodeChange,
    StartupChecklist,
    AutoUpdate,
    Mouse,
    ReducedMotion,
    GitBeside,
    NerdIcons,
}

impl Toggle {
    pub const ALL: [Toggle; 12] = [
        Toggle::AutoStartOnOpen,
        Toggle::AutoRestartOnBranchChange,
        Toggle::StopFrontendOnStop,
        Toggle::SkipUnchangedBuilds,
        Toggle::Notifications,
        Toggle::RestartOnCodeChange,
        Toggle::StartupChecklist,
        Toggle::AutoUpdate,
        Toggle::Mouse,
        Toggle::ReducedMotion,
        Toggle::GitBeside,
        Toggle::NerdIcons,
    ];

    /// Its name in the settings file.
    pub fn key(self) -> &'static str {
        match self {
            Toggle::AutoStartOnOpen => "autoStartOnOpen",
            Toggle::AutoRestartOnBranchChange => "autoRestartOnBranchChange",
            Toggle::StopFrontendOnStop => "stopFrontendOnStop",
            Toggle::SkipUnchangedBuilds => "skipUnchangedBuilds",
            Toggle::Notifications => "notifications",
            Toggle::RestartOnCodeChange => "restartOnCodeChange",
            Toggle::StartupChecklist => "startupChecklist",
            Toggle::AutoUpdate => "autoUpdate",
            Toggle::Mouse => "mouse",
            Toggle::ReducedMotion => "reducedMotion",
            Toggle::GitBeside => "gitBeside",
            Toggle::NerdIcons => "nerdIcons",
        }
    }

    pub fn default_on(self) -> bool {
        !matches!(
            self,
            Toggle::AutoRestartOnBranchChange | Toggle::RestartOnCodeChange | Toggle::ReducedMotion | Toggle::GitBeside
        )
    }

    /// Whether it is on in these settings (defaults included, as `ConfigFile::load` gives them).
    pub fn read(self, config: &Value) -> bool {
        json::truthy(json::field(config, self.key()))
    }
}

/// Used for any setting missing from the file.
pub fn defaults() -> Map<String, Value> {
    let Value::Object(mut defaults) = json!({
        "repoRoot": "",
        "frontend": true,
        "columnWidths": {},
        "panelLogLines": 5,
        "theme": "default",
        "gitTool": "lazygit",
        "editor": "auto",
        "layout": "auto",
        "updateRepoUrl": "",
        "updateBranch": "",
    }) else {
        unreachable!("an object literal")
    };
    for toggle in Toggle::ALL {
        defaults.insert(toggle.key().into(), Value::Bool(toggle.default_on()));
    }
    defaults
}

#[derive(Clone, Debug)]
pub struct ConfigFile {
    pub path: PathBuf,
}

impl ConfigFile {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// The settings as lazyrunner uses them, for `project` (an id) or before one is chosen.
    pub fn load(&self, project: Option<&str>) -> Value {
        let mut config = defaults();
        let Some(Value::Object(saved)) = json::read(&self.path) else { return Value::Object(config) };
        for (key, value) in &saved {
            if key != "projects" && !PROJECT_KEYS.contains(&key.as_str()) {
                config.insert(key.clone(), value.clone());
            }
        }
        for (key, value) in project_section(&saved, project) {
            config.insert(key, value);
        }
        Value::Object(config)
    }

    /// Saves one setting (a project's in its section, the rest at the top) and returns the settings as `load` reads
    /// them.
    pub fn set(&self, project: Option<&str>, name: &str, value: Value) -> Result<Value> {
        // A file that's there but can't be read stays as it is: better no change than losing the rest.
        let mut saved = match json::read_for_update(&self.path) {
            Ok(Some(Value::Object(saved))) => saved,
            Ok(None) => Map::new(),
            Ok(Some(_)) => return Err(anyhow!("lazyrunner: {} must hold one JSON object", self.path.display())),
            Err(error) => {
                return Err(anyhow!(
                    "lazyrunner: can't read {} ({error}), so it was left as it is",
                    self.path.display()
                ));
            }
        };
        match project {
            Some(id) if PROJECT_KEYS.contains(&name) => {
                let mut section = project_section(&saved, Some(id));
                section.insert(name.to_string(), value);
                let projects = saved.entry("projects").or_insert_with(|| json!({}));
                if !projects.is_object() {
                    *projects = json!({});
                }
                projects[id] = Value::Object(section);
            }
            _ => {
                saved.insert(name.to_string(), value);
            }
        }
        write(&self.path, &Value::Object(saved))?;
        Ok(self.load(project))
    }
}

/// A project's section of the saved settings (empty when it has none yet, or before a project is chosen).
fn project_section(saved: &Map<String, Value>, project: Option<&str>) -> Map<String, Value> {
    project.and_then(|id| saved.get("projects")?.get(id)?.as_object().cloned()).unwrap_or_default()
}

/// Writes JSON whole or not at all (a temp file next to it, then a rename), retrying while another process has the
/// file open.
pub fn write(path: &Path, value: &Value) -> Result<()> {
    let text = serde_json::to_string_pretty(value)? + "\n";
    if let Some(dir) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    let mut temp = path.as_os_str().to_owned();
    temp.push(format!(".{}.tmp", std::process::id()));
    let temp = PathBuf::from(temp);
    for _ in 0..WRITE_ATTEMPTS {
        if std::fs::write(&temp, &text).is_ok() && std::fs::rename(&temp, path).is_ok() {
            return Ok(());
        }
        thread::sleep(WRITE_RETRY);
    }
    let _ = std::fs::remove_file(&temp);
    Err(anyhow!("lazyrunner: could not write {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_toggle_has_its_own_name_and_a_default() {
        let names: std::collections::HashSet<&str> = Toggle::ALL.iter().map(|toggle| toggle.key()).collect();
        assert_eq!(names.len(), Toggle::ALL.len());
        for toggle in Toggle::ALL {
            assert_eq!(serde_json::to_value(toggle).unwrap(), json!(toggle.key()), "commands name it as the file does");
        }
        let defaults = defaults();
        assert_eq!(defaults["autoStartOnOpen"], true);
        assert_eq!(defaults["restartOnCodeChange"], false);
        assert_eq!(defaults["autoRestartOnBranchChange"], false);
        assert_eq!(defaults["reducedMotion"], false);
    }

    fn file(name: &str) -> ConfigFile {
        let dir = std::env::temp_dir().join(format!("lazyrunner-config-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        ConfigFile::new(dir.join("config.json"))
    }

    #[test]
    fn without_a_file_the_defaults() {
        let config = file("none").load(Some("sample"));
        assert_eq!(config["theme"], "default");
        assert_eq!(config["panelLogLines"], 5);
    }

    #[test]
    fn a_projects_setting_goes_in_its_section_and_the_rest_stays_at_the_top() {
        let config = file("sections");
        write(&config.path, &json!({ "theme": "dark", "projects": { "other": { "frontend": false } } })).unwrap();
        config.set(Some("sample"), "columnWidths", json!({ "shop": 60 })).unwrap();
        let saved = json::read(&config.path).unwrap();
        assert_eq!(saved["projects"]["sample"]["columnWidths"]["shop"], 60);
        assert!(saved.get("columnWidths").is_none(), "not at the top");
        config.set(Some("sample"), "theme", json!("light")).unwrap();
        let saved = json::read(&config.path).unwrap();
        assert_eq!(saved["theme"], "light");
        assert!(saved["projects"]["sample"].get("theme").is_none());

        let other = config.load(Some("other"));
        assert_eq!(other["columnWidths"], json!({}), "another project has its own");
        assert_eq!(
            (other["frontend"].clone(), other["theme"].clone()),
            (json!(false), json!("light")),
            "and shares the rest"
        );
        assert_eq!(config.load(Some("sample"))["columnWidths"]["shop"], 60);
        let _ = std::fs::remove_dir_all(config.path.parent().unwrap());
    }

    #[test]
    fn a_file_that_cant_be_read_is_never_replaced() {
        let config = file("broken");
        std::fs::create_dir_all(config.path.parent().unwrap()).unwrap();
        std::fs::write(&config.path, "{ \"theme\": \"dark\", broken").unwrap();
        assert!(config.set(None, "theme", json!("light")).is_err());
        assert_eq!(std::fs::read_to_string(&config.path).unwrap(), "{ \"theme\": \"dark\", broken", "left as it was");
        let _ = std::fs::remove_dir_all(config.path.parent().unwrap());
    }

    #[test]
    fn a_project_setting_at_the_top_is_ignored() {
        let config = file("top");
        write(&config.path, &json!({ "frontend": false })).unwrap();
        assert_eq!(config.load(Some("x"))["frontend"], true);
        let _ = std::fs::remove_dir_all(config.path.parent().unwrap());
    }
}
