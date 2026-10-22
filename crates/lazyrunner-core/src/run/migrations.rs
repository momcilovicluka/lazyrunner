//! Migrations (M): the EF Core migrations of each DbContext a .NET app's `"migrations"` names (or `dotnet ef dbcontext
//! list` finds), which are applied to the database the app uses and which are pending. `dotnet ef` runs through the
//! app's project as the startup project with `--environment Development`, so it reads the same user secrets (the
//! database picked with d) as the app, and with `--no-build`, so it uses the app's last build.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde_json::Value;

/// Lines of a job's output a context keeps to show.
pub const MAX_OUTPUT: usize = 200;
/// What `dotnet` says when the dotnet-ef tool isn't installed.
const NO_EF: [&str; 2] =
    ["dotnet-ef does not exist", "Could not execute because the specified command or file was not found"];

/// One migration, oldest first; `applied` is `None` when the database couldn't be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Migration {
    pub id: String,
    pub name: String,
    pub applied: Option<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Idle,
    Queued,
    Loading,
    Migrating,
    Ok,
    Error,
}

/// A DbContext of an app, and what its last check found.
#[derive(Clone, Debug)]
pub struct Context {
    /// The app whose project is the startup project (its key).
    pub app: String,
    pub name: String,
    /// The project with the migrations, and the startup project.
    pub project: PathBuf,
    pub startup: PathBuf,
    pub status: Status,
    pub items: Vec<Migration>,
    pub error: String,
    /// What dotnet ef said last, without its data lines.
    pub output: VecDeque<String>,
    pub checked_at: Option<chrono::DateTime<chrono::Local>>,
    /// The database it was checked on.
    pub db: String,
}

impl Context {
    pub fn new(app: &str, name: &str, project: PathBuf, startup: PathBuf) -> Self {
        Self {
            app: app.into(),
            name: name.into(),
            project,
            startup,
            status: Status::Idle,
            items: Vec::new(),
            error: String::new(),
            output: VecDeque::new(),
            checked_at: None,
            db: String::new(),
        }
    }

    /// How a context is told from another: its app and its name.
    pub fn key(&self) -> String {
        format!("{}:{}", self.app, self.name)
    }

    /// The newest applied migration (an index into `items`).
    pub fn current(&self) -> Option<usize> {
        self.items.iter().rposition(|migration| migration.applied == Some(true))
    }

    pub fn pending(&self) -> Vec<&Migration> {
        self.items.iter().filter(|migration| migration.applied == Some(false)).collect()
    }

    /// Whether the database's history could be read (applied or not is known for each migration).
    pub fn known(&self) -> bool {
        self.items.iter().all(|migration| migration.applied.is_some())
    }

    pub fn is_busy(&self) -> bool {
        matches!(self.status, Status::Queued | Status::Loading | Status::Migrating)
    }

    /// A line of dotnet ef's output worth showing (not its data or verbose lines, nor build warnings).
    pub(crate) fn receive(&mut self, line: &str) {
        if line.starts_with("data:") || line.starts_with("verbose:") || line.contains(": warning ") {
            return;
        }
        let shown = ["info:", "warn:", "error:"].iter().find_map(|prefix| line.strip_prefix(prefix)).unwrap_or(line);
        self.output.push_back(shown.trim().to_string());
        while self.output.len() > MAX_OUTPUT {
            self.output.pop_front();
        }
    }
}

/// `dotnet ef <command>` for a context (or for none: `dbcontext list`), through the startup project, with the apps'
/// Development settings and their last build; `--prefix-output` marks its lines (data:, error: ...).
pub fn ef_args(command: &[&str], context: Option<&str>, project: &Path, startup: &Path) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec!["ef".into()];
    args.extend(command.iter().map(OsString::from));
    args.extend(["--project".into(), project.into(), "--startup-project".into(), startup.into()]);
    if let Some(context) = context {
        args.extend(["--context".into(), context.into()]);
    }
    args.extend(["--no-build", "--prefix-output", "--", "--environment", "Development"].map(OsString::from));
    args
}

/// The JSON dotnet ef printed (its `data:` lines).
fn data(lines: &[String]) -> Result<Value, String> {
    let text: String = lines.iter().filter_map(|line| line.strip_prefix("data:")).collect::<Vec<_>>().join("\n");
    serde_json::from_str(&text).map_err(|_| "dotnet ef printed no list".to_string())
}

/// `dotnet ef migrations list --json`: the migrations, oldest first.
pub fn parse_list(lines: &[String]) -> Result<Vec<Migration>, String> {
    let list = data(lines)?;
    let items = list.as_array().ok_or("dotnet ef printed no list")?;
    Ok(items
        .iter()
        .map(|item| Migration {
            id: item["id"].as_str().unwrap_or_default().to_string(),
            name: item["name"].as_str().or_else(|| item["id"].as_str()).unwrap_or_default().to_string(),
            applied: item["applied"].as_bool(),
        })
        .collect())
}

/// `dotnet ef dbcontext list --json`: the contexts' names.
pub fn parse_contexts(lines: &[String]) -> Result<Vec<String>, String> {
    let list = data(lines)?;
    let items = list.as_array().ok_or("dotnet ef printed no list")?;
    Ok(items
        .iter()
        .filter_map(|item| ["safeName", "name", "fullName"].iter().find_map(|field| item[field].as_str()))
        .map(str::to_string)
        .collect())
}

/// What went wrong, in dotnet ef's words (its `error:` lines), or that dotnet ef isn't installed.
pub fn failure(lines: &[String], code: Option<i32>) -> String {
    if lines.iter().any(|line| NO_EF.iter().any(|sign| line.contains(sign))) {
        return "dotnet ef isn't installed - dotnet tool install --global dotnet-ef".into();
    }
    let errors: Vec<&str> = lines
        .iter()
        .filter_map(|line| line.strip_prefix("error:"))
        .map(str::trim)
        .filter(|error| !error.is_empty())
        .collect();
    if errors.is_empty() {
        let exit = code.map_or_else(|| "?".to_string(), |code| code.to_string());
        format!("dotnet ef exited with code {exit}")
    } else {
        errors.join(" ")
    }
}

/// Whether the startup project has a build for `--no-build` to use (bin/Debug/<framework>/<name>.dll).
pub fn built(startup: &Path) -> bool {
    let Some(name) = startup.file_stem() else { return false };
    let dll = format!("{}.dll", name.to_string_lossy());
    let debug = startup.parent().unwrap_or(Path::new("")).join("bin").join("Debug");
    std::fs::read_dir(debug)
        .map(|entries| entries.flatten().any(|entry| entry.path().join(&dll).is_file()))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &[&str]) -> Vec<String> {
        text.iter().map(|line| line.to_string()).collect()
    }

    #[test]
    fn the_list_comes_from_the_data_lines() {
        let output = lines(&[
            "info: Build started...",
            "data: [",
            r#"data:   { "id": "20260101000000_Init", "name": "Init", "safeName": "Init", "applied": true },"#,
            r#"data:   { "id": "20260201000000_Orders", "name": "Orders", "safeName": "Orders", "applied": false }"#,
            "data: ]",
        ]);
        let items = parse_list(&output).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!((items[0].name.as_str(), items[0].applied), ("Init", Some(true)));
        let mut context = Context::new("api", "ShopDbContext", "p".into(), "s".into());
        context.items = items;
        assert_eq!(context.current(), Some(0));
        assert_eq!(context.pending().len(), 1);
        assert!(context.known());
        context.receive("info: Build started...");
        context.receive("data: [");
        assert_eq!(context.output, ["Build started..."]);
    }

    #[test]
    fn contexts_and_failures_are_read() {
        let output = lines(&[
            r#"data: [{ "fullName": "Shop.Data.ShopDbContext", "safeName": "ShopDbContext", "name": "ShopDbContext" }]"#,
        ]);
        assert_eq!(parse_contexts(&output).unwrap(), ["ShopDbContext"]);
        assert_eq!(failure(&lines(&["error: Login failed for user 'x'."]), Some(1)), "Login failed for user 'x'.");
        let missing = lines(&["Could not execute because the specified command or file was not found."]);
        assert!(failure(&missing, Some(1)).starts_with("dotnet ef isn't installed"));
        assert_eq!(failure(&[], Some(3)), "dotnet ef exited with code 3");
    }

    #[test]
    fn dotnet_ef_runs_through_the_startup_project_without_building() {
        let (project, startup) = (Path::new("Data.csproj"), Path::new("Api.csproj"));
        let args = ef_args(&["migrations", "list", "--json"], Some("ShopDbContext"), project, startup);
        let args: Vec<String> = args.iter().map(|arg| arg.to_string_lossy().into_owned()).collect();
        let expected = [
            "ef",
            "migrations",
            "list",
            "--json",
            "--project",
            "Data.csproj",
            "--startup-project",
            "Api.csproj",
            "--context",
            "ShopDbContext",
            "--no-build",
            "--prefix-output",
            "--",
            "--environment",
            "Development",
        ];
        assert_eq!(args, expected);
    }
}
