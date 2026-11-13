//! A .NET app's log levels, changed from lazyrunner (#101): the default level, a few categories worth having at hand
//! (EF Core's SQL, HttpClient's calls, ASP.NET Core) and the app's own namespace, each at Trace, Debug, Information
//! or Warning. They go on its command line after lazyrunner's own `--Logging` arguments, so they win; for this
//! session, unless kept - then in `log-levels.json` in the project's state folder, on this machine.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::Path;

use serde_json::{Map, Value, json};

use super::Controller;
use crate::output::Hue;
use crate::profile::AppKind;
use crate::run::tools::ServerCommand;

const FILE: &str = "log-levels.json";
/// The categories every .NET app has to hand, before its own namespace.
pub const CATEGORIES: [&str; 4] =
    ["Default", "Microsoft.EntityFrameworkCore.Database.Command", "System.Net.Http.HttpClient", "Microsoft.AspNetCore"];
pub const LEVELS: [&str; 4] = ["Trace", "Debug", "Information", "Warning"];
/// A category's level is this argument, `--Logging:LogLevel:<category>=<level>`.
const ARGUMENT: &str = "--Logging:LogLevel:";
pub const DEFAULT: &str = "Default";

/// A category an app logs at another level than its own settings say.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogLevel {
    pub category: String,
    pub level: String,
    /// Kept beyond this session.
    pub kept: bool,
}

pub(super) type LogLevels = HashMap<String, Vec<LogLevel>>;

/// The levels kept in the state folder, by app.
pub(super) fn load(state_dir: &Path) -> LogLevels {
    let Some(Value::Object(apps)) = crate::json::read(&state_dir.join(FILE)) else { return LogLevels::new() };
    apps.into_iter()
        .map(|(key, levels)| {
            let levels = levels.as_object().into_iter().flatten().filter_map(|(category, level)| {
                Some(LogLevel { category: category.clone(), level: level.as_str()?.to_string(), kept: true })
            });
            (key, levels.collect())
        })
        .collect()
}

impl Controller {
    /// The categories app `key`'s levels can be set for: everyone's, then its project's own namespace.
    pub fn log_categories(&self, key: &str) -> Vec<String> {
        let mut categories: Vec<String> = CATEGORIES.iter().map(|category| category.to_string()).collect();
        if let Some(AppKind::Dotnet { project, .. }) = self.app(key).map(|app| &app.kind)
            && let Some(own) = Path::new(project).file_stem().and_then(|stem| stem.to_str())
        {
            categories.push(own.to_string());
        }
        for level in self.log_levels(key) {
            if !categories.contains(&level.category) {
                categories.push(level.category.clone());
            }
        }
        categories
    }

    /// App `key`'s levels that differ from its own settings.
    pub fn log_levels(&self, key: &str) -> &[LogLevel] {
        self.log_levels.get(key).map_or(&[], Vec::as_slice)
    }

    /// Sets `category`'s level for app `key` (none: back to its own), restarting it when it runs.
    pub fn set_log_level(&mut self, key: &str, category: &str, level: Option<&str>) {
        let levels = self.log_levels.entry(key.to_string()).or_default();
        let kept = levels.iter().find(|found| found.category == category).is_some_and(|found| found.kept);
        levels.retain(|found| found.category != category);
        if let Some(level) = level {
            levels.push(LogLevel { category: category.to_string(), level: level.to_string(), kept: false });
        }
        if kept {
            self.save_log_levels();
        }
        let said = match level {
            Some(level) => format!("{category} at {level}"),
            None => format!("{category} at its own level"),
        };
        self.apply_log_levels(key, &said);
    }

    /// Keeps app `key`'s levels beyond this session, on this machine.
    pub fn keep_log_levels(&mut self, key: &str) {
        for level in self.log_levels.entry(key.to_string()).or_default() {
            level.kept = true;
        }
        self.save_log_levels();
        self.add_log(format!("{}: its log levels are kept on this machine", self.app_title(key)), Hue::Cyan);
    }

    /// Back to app `key`'s own levels, kept ones too.
    pub fn reset_log_levels(&mut self, key: &str) {
        let had_kept = self.log_levels(key).iter().any(|level| level.kept);
        self.log_levels.remove(key);
        if had_kept {
            self.save_log_levels();
        }
        self.apply_log_levels(key, "its own log levels");
    }

    /// A running app starts again with its levels; a stopped one has them from its next start.
    fn apply_log_levels(&mut self, key: &str, said: &str) {
        if self.is_running(key) {
            self.restart(said, Some(&[key.to_string()]), false);
        } else {
            self.add_log(format!("{}: {said} from its next start", self.app_title(key)), Hue::Cyan);
        }
    }

    fn save_log_levels(&mut self) {
        let mut apps = Map::new();
        for (key, levels) in &self.log_levels {
            let kept: Map<String, Value> = levels
                .iter()
                .filter(|level| level.kept)
                .map(|level| (level.category.clone(), json!(level.level)))
                .collect();
            if !kept.is_empty() {
                apps.insert(key.clone(), Value::Object(kept));
            }
        }
        if let Err(error) = crate::config::write(&self.state_dir.join(FILE), &Value::Object(apps)) {
            self.add_log(format!("✗ Couldn't keep the log levels: {error}"), Hue::Red);
        }
    }

    /// `command` with app `key`'s levels at the end of a .NET app's arguments, after lazyrunner's own; its label
    /// names them.
    pub(super) fn with_log_levels(&self, key: &str, mut command: ServerCommand) -> ServerCommand {
        let levels = self.log_levels(key);
        let dotnet = matches!(self.app(key).map(|app| &app.kind), Some(AppKind::Dotnet { .. }));
        if levels.is_empty() || !dotnet {
            return command;
        }
        let arguments =
            levels.iter().map(|level| OsString::from(format!("{ARGUMENT}{}={}", level.category, level.level)));
        command.spec.args.extend(arguments);
        let said: Vec<String> = levels.iter().map(|level| format!("{}={}", level.category, level.level)).collect();
        command.label = format!("{} · log {}", command.label, said.join(" "));
        command
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run::child::Spec;
    use crate::testing::TempDir;

    #[tokio::test]
    async fn a_level_goes_last_on_a_dotnet_apps_command_line_and_only_kept_ones_outlast_the_session() {
        let temp = TempDir::new("log-levels");
        let (mut controller, _rx) = super::super::tests::controller(&temp, json!({}));
        let key = controller.apps[0].key.clone();
        assert!(controller.log_categories(&key).len() > CATEGORIES.len(), "its own namespace too");
        controller.set_log_level(&key, DEFAULT, Some("Debug"));
        controller.set_log_level(&key, CATEGORIES[1], Some("Information"));
        let spec = Spec { program: "dotnet".into(), args: vec!["--".into()], dir: ".".into(), env: Vec::new() };
        let command =
            controller.with_log_levels(&key, ServerCommand { spec, label: "dotnet run".into(), script: None });
        let last = command.spec.args.last().unwrap().to_string_lossy().into_owned();
        assert_eq!(last, format!("--Logging:LogLevel:{}=Information", CATEGORIES[1]));
        controller.keep_log_levels(&key);
        controller.set_log_level(&key, DEFAULT, None);
        let kept = load(controller.state_folder());
        assert_eq!(kept[&key].iter().map(|level| level.category.as_str()).collect::<Vec<_>>(), [CATEGORIES[1]]);
        controller.reset_log_levels(&key);
        assert!(controller.log_levels(&key).is_empty() && load(controller.state_folder()).is_empty());
    }
}
