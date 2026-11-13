//! What changed for running when the branch changes (#104): of `git diff --name-status <old> <new>`, the files that
//! change how the apps run rather than what their code does - the profile, an app's `appsettings*.json` (which keys,
//! never their values) and `launchSettings.json`, its project file, an npm app's `package.json` / lock (npm ci),
//! migrations added or removed, `.env` files - each with what fixes it, when something does.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::Value;

use crate::profile::{App, AppKind, REPO_PROFILE_FILE};
use crate::run::tools;

/// How many of a settings file's changed keys are named.
const NAMED_KEYS: usize = 3;
const MIGRATIONS_FOLDER: &str = "/Migrations/";

/// What fixes a change: npm ci for the app, the migrations view, reloading the profile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fix {
    NpmCi,
    Migrations,
    ReloadProfile,
}

/// What a change is to: its icon in the box.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum What {
    Profile,
    Settings,
    Launch,
    Project,
    Packages,
    Migrations,
    Env,
}

/// A change that matters for running, as one line, the app it's about and what fixes it; a settings file's keys,
/// each `+` added, `-` taken away or `~` changed (never their values).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub app: Option<String>,
    pub what: What,
    pub text: String,
    /// Shorter, for the one line that sums them up.
    pub short: String,
    pub fix: Option<Fix>,
    pub keys: Vec<(char, String)>,
}

/// The changes between `from` and `to` (branches, or any revisions); none when git can't compare them.
pub fn between(root: &Path, from: &str, to: &str, apps: &[App]) -> Vec<Change> {
    let Some(listed) = tools::git(root, &["diff", "--name-status", from, to]) else { return Vec::new() };
    let entries: Vec<(char, String)> = listed
        .lines()
        .filter_map(|line| {
            let mut parts = line.split('\t');
            let status = parts.next()?.chars().next()?;
            Some((status, parts.next_back()?.replace('\\', "/")))
        })
        .collect();
    let show = |revision: &str, path: &str| tools::git(root, &["show", &format!("{revision}:{path}")]);
    classify(&entries, apps, root, |path| (show(from, path), show(to, path)))
}

/// Sorts `entries` (git's status letter and path) into the changes that matter for running; `versions` gives a
/// file's text before and after.
pub fn classify(
    entries: &[(char, String)],
    apps: &[App],
    root: &Path,
    versions: impl Fn(&str) -> (Option<String>, Option<String>),
) -> Vec<Change> {
    let folders: Vec<(&App, String)> = apps.iter().map(|app| (app, relative(root, app))).collect();
    let owner = |path: &str| {
        folders
            .iter()
            .filter(|(_, folder)| folder.is_empty() || path.starts_with(&format!("{folder}/")))
            .max_by_key(|(_, folder)| folder.len())
            .map(|(app, _)| *app)
    };
    let mut changes = Vec::new();
    let (mut added, mut removed) = (0, 0);
    for (status, path) in entries {
        let name = path.rsplit('/').next().unwrap_or(path);
        let app = owner(path);
        let app_name = app.map_or("", |app| app.name.as_str());
        let change = |what: What, text: String, short: String, fix: Option<Fix>| Change {
            app: app.map(|app| app.key.clone()),
            what,
            text,
            short,
            fix,
            keys: Vec::new(),
        };
        if path == REPO_PROFILE_FILE {
            let text = format!("The profile ({REPO_PROFILE_FILE}) - , then p reloads it");
            changes.push(Change {
                app: None,
                ..change(What::Profile, text, "the profile".into(), Some(Fix::ReloadProfile))
            });
        } else if is_migration(path) {
            match status {
                'A' => added += 1,
                'D' => removed += 1,
                _ => {}
            }
        } else if name.starts_with("appsettings") && name.ends_with(".json") {
            let (before, after) = versions(path);
            let keys = changed_keys(before.as_deref(), after.as_deref());
            let said = match keys.as_deref() {
                Some([]) | None => String::new(),
                Some(keys) => {
                    let named = keys.iter().take(NAMED_KEYS).map(|(_, key)| key.clone()).collect::<Vec<_>>().join(", ");
                    let more = if keys.len() > NAMED_KEYS { ", …" } else { "" };
                    format!(" ({} key{}: {named}{more})", keys.len(), if keys.len() == 1 { "" } else { "s" })
                }
            };
            let count = keys.as_ref().map_or(String::new(), |keys| format!(" ({} keys)", keys.len()));
            let short = format!("{app_name} appsettings{count}");
            let made = change(What::Settings, format!("{app_name} {name}{said}"), short, None);
            changes.push(Change { keys: keys.unwrap_or_default(), ..made });
        } else if name == "launchSettings.json" {
            let (text, short) = (format!("{app_name} launchSettings.json"), format!("{app_name} launch settings"));
            changes.push(change(What::Launch, text, short, None));
        } else if name.ends_with(".csproj") && app.is_some_and(|app| matches!(app.kind, AppKind::Dotnet { .. })) {
            let text = format!("{app_name} {name} (packages or build)");
            changes.push(change(What::Project, text, format!("{app_name} project"), None));
        } else if matches!(name, "package.json" | "package-lock.json") && app.is_some() {
            let short = format!("{app_name} {}", name.trim_end_matches(".json"));
            let text = format!("{app_name} {name} - i then c runs npm ci");
            changes.push(change(What::Packages, text, short, Some(Fix::NpmCi)));
        } else if name == ".env" || name.starts_with(".env.") {
            changes.push(change(What::Env, format!("{path} (environment)"), name.to_string(), None));
        }
    }
    if added + removed > 0 {
        let mut said = Vec::new();
        if added > 0 {
            said.push(format!("{added} migration{} added", if added == 1 { "" } else { "s" }));
        }
        if removed > 0 {
            said.push(format!("{removed} removed"));
        }
        let short = format!("{} migration{}", added + removed, if added + removed == 1 { "" } else { "s" });
        changes.push(Change {
            app: None,
            what: What::Migrations,
            text: format!("{} - M shows them", said.join(", ")),
            short,
            fix: Some(Fix::Migrations),
            keys: Vec::new(),
        });
    }
    changes
}

/// One line for them all: `main → feature/x: Orders appsettings (2 keys), web package-lock, 1 migration`.
pub fn summary(from: &str, to: &str, changes: &[Change]) -> String {
    let shorts: Vec<&str> = changes.iter().map(|change| change.short.as_str()).collect();
    format!("{from} → {to}: {}", shorts.join(", "))
}

/// An app's folder from the repo root, with forward slashes ("" for the root itself).
fn relative(root: &Path, app: &App) -> String {
    let folder = crate::run::audit::folder(root, app);
    let relative = folder.strip_prefix(root).unwrap_or(&folder);
    relative.to_string_lossy().replace('\\', "/").trim_matches('/').to_string()
}

/// An EF Core migration's own file - not its designer file or the model snapshot.
fn is_migration(path: &str) -> bool {
    let path = format!("/{path}");
    path.contains(MIGRATIONS_FOLDER)
        && path.ends_with(".cs")
        && !path.ends_with(".Designer.cs")
        && !path.ends_with("ModelSnapshot.cs")
}

/// The keys (`Section:Key`) whose values differ between two versions of a settings file - `+` added, `-` taken away,
/// `~` changed; none when either doesn't read as JSON (comments, say).
fn changed_keys(before: Option<&str>, after: Option<&str>) -> Option<Vec<(char, String)>> {
    let parse = |text: Option<&str>| match text {
        None => Some(BTreeMap::new()),
        Some(text) => serde_json::from_str::<Value>(crate::json::strip_bom(text)).ok().map(|value| flat(&value)),
    };
    let (before, after) = (parse(before)?, parse(after)?);
    let mut keys: Vec<String> = before.keys().chain(after.keys()).cloned().collect();
    keys.sort();
    keys.dedup();
    let sign = |key: &String| match (before.get(key), after.get(key)) {
        (None, _) => '+',
        (_, None) => '-',
        _ => '~',
    };
    Some(keys.into_iter().filter(|key| before.get(key) != after.get(key)).map(|key| (sign(&key), key)).collect())
}

/// A JSON object's leaves by their `Section:Key` path (an array is one leaf).
fn flat(value: &Value) -> BTreeMap<String, String> {
    let mut leaves = BTreeMap::new();
    let mut stack = vec![(String::new(), value)];
    while let Some((path, value)) = stack.pop() {
        match value {
            Value::Object(fields) => {
                for (name, field) in fields {
                    let at = if path.is_empty() { name.clone() } else { format!("{path}:{name}") };
                    stack.push((at, field));
                }
            }
            leaf => {
                leaves.insert(path, leaf.to_string());
            }
        }
    }
    leaves
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::Health;

    fn app(key: &str, kind: AppKind) -> App {
        App {
            key: key.into(),
            name: key.to_uppercase(),
            icon: None,
            nerd_icon: None,
            kind,
            tests: Vec::new(),
            migrations: None,
            env: Vec::new(),
            depends_on: Vec::new(),
            health: Health::default(),
            restart_on_crash: false,
        }
    }

    #[test]
    fn between_two_real_branches_a_changed_setting_is_named_by_its_key() {
        let temp = crate::testing::TempDir::new("branch-changes");
        let root = temp.path();
        let git = |args: &[&str]| {
            let mut all = vec!["-c", "user.name=t", "-c", "user.email=t@example.com", "-c", "commit.gpgsign=false"];
            all.extend_from_slice(args);
            tools::git(root, &all)
        };
        if git(&["init", "-q", "-b", "main"]).is_none() {
            return; // no git here
        }
        let settings = root.join("src/Orders/appsettings.json");
        std::fs::create_dir_all(settings.parent().unwrap()).unwrap();
        std::fs::write(&settings, r#"{ "Db": "a", "Port": 1 }"#).unwrap();
        git(&["add", "-A"]).unwrap();
        git(&["commit", "-q", "-m", "one"]).unwrap();
        git(&["checkout", "-q", "-b", "feature"]).unwrap();
        std::fs::write(&settings, r#"{ "Db": "b", "Port": 1 }"#).unwrap();
        git(&["commit", "-q", "-am", "two"]).unwrap();
        let dotnet = AppKind::Dotnet {
            project: "src/Orders/Orders.csproj".into(),
            launch_profile: "Orders".into(),
            swagger: None,
            args: Vec::new(),
        };
        let changes = between(root, "main", "feature", &[app("orders", dotnet)]);
        let texts: Vec<&str> = changes.iter().map(|change| change.text.as_str()).collect();
        assert_eq!(texts, ["ORDERS appsettings.json (1 key: Db)"]);
        assert!(between(root, "main", "no-such-branch", &[]).is_empty(), "git can't compare: nothing");
    }

    #[test]
    fn only_what_changes_running_is_kept_settings_by_their_keys_and_migrations_counted() {
        let root = Path::new("/repo");
        let dotnet = AppKind::Dotnet {
            project: "src/Orders/Orders.csproj".into(),
            launch_profile: "Orders".into(),
            swagger: None,
            args: Vec::new(),
        };
        let npm = AppKind::Npm { dir: "web".into(), variants: Vec::new() };
        let apps = [app("orders", dotnet), app("web", npm)];
        let entries: Vec<(char, String)> = [
            ('M', "src/Orders/appsettings.Development.json"),
            ('M', "src/Orders/OrdersController.cs"),
            ('M', "web/package-lock.json"),
            ('A', "src/Data/Migrations/20261113_AddCurrency.cs"),
            ('A', "src/Data/Migrations/20261113_AddCurrency.Designer.cs"),
            ('M', ".lazyrunner.json"),
        ]
        .iter()
        .map(|(status, path)| (*status, path.to_string()))
        .collect();
        let versions = |_: &str| {
            let before = r#"{ "Logging": { "Level": "Info" }, "Db": "a", "Same": 1 }"#;
            let after = r#"{ "Logging": { "Level": "Debug" }, "Db": "b", "Same": 1 }"#;
            (Some(before.to_string()), Some(after.to_string()))
        };
        let changes = classify(&entries, &apps, root, versions);
        let texts: Vec<&str> = changes.iter().map(|change| change.text.as_str()).collect();
        assert_eq!(texts[0], "ORDERS appsettings.Development.json (2 keys: Db, Logging:Level)");
        assert_eq!(texts[1], "WEB package-lock.json - i then c runs npm ci");
        assert!(texts.iter().all(|text| !text.contains("Controller") && !text.contains("Designer")), "{texts:?}");
        assert_eq!(changes.last().unwrap().text, "1 migration added - M shows them");
        let line = summary("main", "feature/x", &changes);
        assert_eq!(line, "main → feature/x: ORDERS appsettings (2 keys), WEB package-lock, the profile, 1 migration");
    }
}
