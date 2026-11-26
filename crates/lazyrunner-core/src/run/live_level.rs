//! A live log level (#112): a .NET app's log category at another level for a while, without a restart. It's written
//! into the app's user secrets, which ASP.NET Core reads again when they change - and its logging with them - and put
//! back as it was: the setting taken out again, or its old value back. Only what lazyrunner wrote is put back: a
//! value changed meanwhile stays. While one lasts, it's recorded in the project's state folder, so what a crash left
//! is put back when lazyrunner opens next.

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use super::secrets;
use crate::profile::{App, AppKind};

/// The levels, as .NET spells them.
pub const LEVELS: [&str; 7] = ["Trace", "Debug", "Information", "Warning", "Error", "Critical", "None"];
/// How long one lasts by default and at most, in minutes.
pub const MINUTES: u64 = 10;
pub const MOST_MINUTES: u64 = 120;
/// The category every other one falls back to.
pub const DEFAULT: &str = "Default";
const FILE: &str = "live-log-levels.json";
const CATEGORY_CHARS: usize = 200;
const LOGGING: &str = "Logging:LogLevel:";
const SERILOG_OVERRIDE: &str = "Serilog:MinimumLevel:Override:";
const SERILOG_DEFAULT: &str = "Serilog:MinimumLevel:Default";
const SERILOG: &str = "Serilog";
/// Serilog's names for the levels, in [`LEVELS`]' order (it has no None: Fatal is the quietest).
const SERILOG_LEVELS: [&str; 7] = ["Verbose", "Debug", "Information", "Warning", "Error", "Fatal", "Fatal"];

/// `text` as one of [`LEVELS`], any case.
pub fn level(text: &str) -> Option<&'static str> {
    LEVELS.iter().find(|level| level.eq_ignore_ascii_case(text.trim())).copied()
}

/// Whether `text` names a log category: a dotted name (letters, digits, `_`), `Default` among them.
pub fn is_category(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= CATEGORY_CHARS
        && text.split('.').all(|part| !part.is_empty() && part.chars().all(|c| c.is_alphanumeric() || c == '_'))
}

/// The settings that put `category` at `level`: ASP.NET Core's, and Serilog's too for an app that logs through it.
pub fn settings(category: &str, level: &str, serilog: bool) -> Vec<(String, String)> {
    let mut settings = vec![(format!("{LOGGING}{category}"), level.to_string())];
    if serilog {
        let index = LEVELS.iter().position(|each| *each == level).unwrap_or_default();
        let key = if category.eq_ignore_ascii_case(DEFAULT) {
            SERILOG_DEFAULT.to_string()
        } else {
            format!("{SERILOG_OVERRIDE}{category}")
        };
        settings.push((key, SERILOG_LEVELS[index].to_string()));
    }
    settings
}

/// Whether a .NET app logs through Serilog: its project or its appsettings name it.
pub fn uses_serilog(root: &Path, app: &App) -> bool {
    let AppKind::Dotnet { project, .. } = &app.kind else { return false };
    let project = root.join(project);
    let dir = project.parent().map_or_else(|| root.to_path_buf(), Path::to_path_buf);
    [project, dir.join("appsettings.json"), dir.join("appsettings.Development.json")]
        .iter()
        .any(|path| std::fs::read_to_string(path).is_ok_and(|text| text.contains(SERILOG)))
}

/// One setting lazyrunner wrote into a secrets.json.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Change {
    pub path: PathBuf,
    pub key: String,
    /// Its value before, as the file had it written; none when the file didn't set it.
    pub before: Option<String>,
    /// What lazyrunner wrote, as JSON: it's put back only while the file still says that.
    pub written: String,
}

/// Sets `key` to `value` in the secrets.json at `path` (made when there's none): where the file sets it already,
/// else on a line of its own. What it was before.
pub fn write(path: &Path, key: &str, value: &str) -> Result<Change> {
    let written = serde_json::to_string(value)?;
    let before = if path.exists() { secrets::value_in(path, key)? } else { None };
    match before {
        Some(_) => {
            secrets::replace(path, key, &written)?;
        }
        None => secrets::set(path, key, value)?,
    }
    Ok(Change { path: path.to_path_buf(), key: key.to_string(), before, written })
}

/// Puts a setting back as it was before lazyrunner wrote it. False when the file says something else by now (or is
/// gone): that's left as it is.
pub fn undo(change: &Change) -> Result<bool> {
    if !change.path.exists() || secrets::value_in(&change.path, &change.key)?.as_deref() != Some(&change.written) {
        return Ok(false);
    }
    match &change.before {
        Some(before) => {
            secrets::replace(&change.path, &change.key, before)?;
        }
        None => secrets::unset(&change.path, &change.key)?,
    }
    Ok(true)
}

/// A live level as the state folder records it while it lasts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Saved {
    pub app: String,
    pub category: String,
    pub level: String,
    pub changes: Vec<Change>,
}

/// Records the live levels that last (none: the record goes).
pub fn save(state_dir: &Path, levels: &[Saved]) -> Result<()> {
    let path = state_dir.join(FILE);
    if levels.is_empty() {
        if path.exists() {
            std::fs::remove_file(path)?;
        }
        return Ok(());
    }
    crate::config::write(&path, &serde_json::to_value(levels)?)
}

/// The live levels recorded: those a lazyrunner that closed without putting them back left.
pub fn load(state_dir: &Path) -> Vec<Saved> {
    crate::json::read(&state_dir.join(FILE)).and_then(|value| serde_json::from_value(value).ok()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{TempDir, write as write_file};

    const SECRETS: &str = "{\r\n  // the database\r\n  \"DefaultConnection\": \"Server=localhost;Database=Shop\",\r\n  \"Logging\": {\r\n    \"LogLevel\": {\r\n      \"Default\": \"Warning\" /* quiet */\r\n    }\r\n  }\r\n}\r\n";

    #[test]
    fn a_setting_written_and_undone_leaves_the_file_as_it_was_comments_and_all() {
        let temp = TempDir::new("live-level");
        let path = temp.join("secrets.json");
        write_file(&path, SECRETS);
        let added = write(&path, "Logging:LogLevel:Microsoft.EntityFrameworkCore.Database.Command", "Debug").unwrap();
        let nested = write(&path, "logging:loglevel:default", "Trace").unwrap();
        assert_eq!((added.before.as_deref(), nested.before.as_deref()), (None, Some("\"Warning\"")));
        let text = std::fs::read_to_string(&path).unwrap();
        let read = secrets::flatten(&text);
        assert!(read.contains(&("Logging:LogLevel:Default".into(), "Trace".into())), "{text}");
        assert!(
            read.contains(&("Logging:LogLevel:Microsoft.EntityFrameworkCore.Database.Command".into(), "Debug".into()))
        );
        assert!(text.contains("\"Default\": \"Trace\" /* quiet */") && text.contains("// the database"), "{text}");
        assert!(undo(&added).unwrap() && undo(&nested).unwrap());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), SECRETS);
        assert!(!undo(&added).unwrap(), "put back once");
    }

    #[test]
    fn a_value_changed_meanwhile_stays_and_a_missing_file_is_made() {
        let temp = TempDir::new("live-level-changed");
        let path = temp.join("secrets.json");
        write_file(&path, SECRETS);
        let change = write(&path, "Logging:LogLevel:Default", "Debug").unwrap();
        secrets::replace(&path, "Logging:LogLevel:Default", "\"Error\"").unwrap();
        assert!(!undo(&change).unwrap());
        assert_eq!(secrets::value_in(&path, "Logging:LogLevel:Default").unwrap().as_deref(), Some("\"Error\""));

        let fresh = temp.join("new").join("secrets.json");
        let change = write(&fresh, "Logging:LogLevel:Default", "Debug").unwrap();
        assert!(undo(&change).unwrap() && secrets::flatten(&std::fs::read_to_string(&fresh).unwrap()).is_empty());
    }

    #[test]
    fn levels_and_categories_are_checked_and_serilog_gets_its_own_setting() {
        assert_eq!((level("debug"), level(" NONE "), level("verbose")), (Some("Debug"), Some("None"), None));
        for good in ["Default", "Microsoft.EntityFrameworkCore.Database.Command", "Orders_Api.V2"] {
            assert!(is_category(good), "{good}");
        }
        for bad in ["", "Orders.", "Orders..Api", "Orders\":\"x", "a b", "Orders:Api"] {
            assert!(!is_category(bad), "{bad}");
        }
        assert_eq!(
            settings("Orders.Api", "Trace", true),
            [
                ("Logging:LogLevel:Orders.Api".to_string(), "Trace".to_string()),
                ("Serilog:MinimumLevel:Override:Orders.Api".to_string(), "Verbose".to_string())
            ]
        );
        assert_eq!(settings("default", "Critical", true)[1], ("Serilog:MinimumLevel:Default".into(), "Fatal".into()));
        assert_eq!(settings("Default", "Debug", false).len(), 1);
    }

    #[test]
    fn the_record_lasts_while_any_does() {
        let temp = TempDir::new("live-level-saved");
        let change = Change { path: temp.join("s.json"), key: "k".into(), before: None, written: "\"Debug\"".into() };
        let saved =
            Saved { app: "orders".into(), category: "Default".into(), level: "Debug".into(), changes: vec![change] };
        save(temp.path(), std::slice::from_ref(&saved)).unwrap();
        assert_eq!(load(temp.path()), [saved]);
        save(temp.path(), &[]).unwrap();
        assert!(load(temp.path()).is_empty() && !temp.join(FILE).exists());
    }
}
