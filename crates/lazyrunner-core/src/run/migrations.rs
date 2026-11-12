//! Migrations (M): the EF Core migrations of each DbContext - those of the model snapshots in the projects a .NET app
//! builds, or those its `"migrations"` names (or `dotnet ef dbcontext list` finds) - which are applied to the database
//! the app uses and which are pending. `dotnet ef` runs through the
//! app's project as the startup project with `--environment Development`, so it reads the same user secrets (the
//! database picked with d) as the app, and with `--no-build`, so it uses the app's last build.

use std::collections::{BTreeSet, VecDeque};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::Instant;

use regex::Regex;
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

/// What a migration's `Up()` does, a step each - `CreateTable Invoices`, `AddColumn Orders.Currency` - and whether
/// the step loses data (a drop, a delete).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    pub what: String,
    pub destructive: bool,
}

/// How deep the migrations' file is looked for under the project's folder.
const MIGRATION_DEPTH: usize = 6;

/// Migration `id`'s steps, read from its file (`<id>.cs`) under `project`'s folder; none when it isn't found. Read once.
pub fn steps(project: &Path, id: &str) -> Vec<Step> {
    static READ: LazyLock<std::sync::Mutex<std::collections::HashMap<String, Vec<Step>>>> =
        LazyLock::new(Default::default);
    let key = format!("{}|{id}", project.display());
    if let Some(known) = READ.lock().ok().and_then(|read| read.get(&key).cloned()) {
        return known;
    }
    let folder = project.parent().unwrap_or(project);
    let found =
        find_file(folder, &format!("{id}.cs"), MIGRATION_DEPTH).and_then(|file| std::fs::read_to_string(file).ok());
    let steps = found.map(|text| parse_steps(&text)).unwrap_or_default();
    if let Ok(mut read) = READ.lock() {
        read.insert(key, steps.clone());
    }
    steps
}

fn find_file(folder: &Path, name: &str, depth: usize) -> Option<PathBuf> {
    let entries: Vec<PathBuf> = std::fs::read_dir(folder).ok()?.flatten().map(|entry| entry.path()).collect();
    if let Some(file) = entries.iter().find(|path| path.file_name().is_some_and(|file| file == name)) {
        return Some(file.clone());
    }
    let skipped = |path: &&PathBuf| {
        path.file_name().is_some_and(|dir| matches!(dir.to_str(), Some("bin" | "obj" | "node_modules" | ".git")))
    };
    entries
        .iter()
        .filter(|path| path.is_dir() && !skipped(path))
        .take(64)
        .find_map(|dir| (depth > 0).then(|| find_file(dir, name, depth - 1)).flatten())
}

/// A migration's `Up()` as steps.
pub fn parse_steps(text: &str) -> Vec<Step> {
    static CALL: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"migrationBuilder\s*\.\s*(\w+)").expect("a valid pattern"));
    static NAME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"name:\s*"([^"]+)""#).expect("a valid pattern"));
    static TABLE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"table:\s*"([^"]+)""#).expect("a valid pattern"));
    let up = text.split("void Down(").next().unwrap_or(text);
    let calls: Vec<(usize, String)> =
        CALL.captures_iter(up).filter_map(|found| Some((found.get(0)?.start(), found[1].to_string()))).collect();
    calls
        .iter()
        .enumerate()
        .map(|(at, (start, call))| {
            let end = calls.get(at + 1).map_or(up.len(), |(next, _)| *next);
            let body = &up[*start..end];
            let name = NAME.captures(body).map(|found| found[1].to_string());
            let table = TABLE.captures(body).map(|found| found[1].to_string());
            let target = match (table, name) {
                (Some(table), Some(name)) => format!(" {table}.{name}"),
                (None, Some(name)) => format!(" {name}"),
                (Some(table), None) => format!(" {table}"),
                (None, None) => String::new(),
            };
            Step {
                what: format!("{call}{target}"),
                destructive: call.starts_with("Drop") || call.starts_with("Delete"),
            }
        })
        .collect()
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

/// A `database update`, under way or done: where the database was (its newest applied migration's id), the
/// migrations it moves in the order it moves them, whether back, when EF began each (`Applying migration '<id>'.`,
/// `Reverting migration '<id>'.`), and how it ended - when, and why it failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    pub from: Option<String>,
    pub back: bool,
    pub planned: Vec<String>,
    pub began: Vec<Instant>,
    pub ended: Option<(Instant, Option<String>)>,
}

impl Run {
    /// When each planned migration was done: one when the next began, and a clean end finishes the rest.
    pub fn done(&self) -> Vec<Instant> {
        let mut done: Vec<Instant> = self.began.iter().skip(1).copied().collect();
        if let Some((at, None)) = self.ended {
            done.resize(self.planned.len().max(done.len()), at);
        }
        done.truncate(self.planned.len());
        done
    }

    /// Why it failed, once it has.
    pub fn failure(&self) -> Option<&str> {
        self.ended.as_ref().and_then(|(_, failure)| failure.as_deref())
    }
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
    /// Its last `database update`, while this database is picked.
    pub run: Option<Run>,
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
            run: None,
        }
    }

    /// How a context is told from another: its app and its name.
    /// When it was last checked: the time today, the day and time before (an earlier run's check).
    pub fn checked(&self) -> Option<String> {
        let at = self.checked_at?;
        let format = if at.date_naive() == chrono::Local::now().date_naive() { "%H:%M:%S" } else { "%-d %b %H:%M" };
        Some(at.format(format).to_string())
    }

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

    /// Plans a `database update` to `target` (a migration's id; `None`: what's pending) from where it is now.
    pub fn plan(&mut self, target: Option<&str>) {
        let current = self.current();
        let to = target.and_then(|id| self.items.iter().position(|item| item.id == id));
        let id = |item: &Migration| item.id.clone();
        let (back, planned) = match (to, current) {
            (Some(to), Some(current)) if to < current => {
                (true, self.items[to + 1..=current].iter().rev().map(id).collect())
            }
            (Some(to), _) => {
                (false, self.items[..=to].iter().filter(|item| item.applied != Some(true)).map(id).collect())
            }
            (None, _) => (false, self.pending().into_iter().map(id).collect()),
        };
        let from = current.map(|at| self.items[at].id.clone());
        self.run = Some(Run { from, back, planned, began: Vec::new(), ended: None });
    }

    /// A line of a `database update`: EF beginning a migration moves its run on.
    pub(crate) fn hop(&mut self, line: &str, now: Instant) {
        let Some(run) = self.run.as_mut().filter(|run| run.ended.is_none()) else { return };
        if line.contains("Applying migration '") || line.contains("Reverting migration '") {
            run.began.push(now);
        }
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

/// Folders a search for model snapshots skips: build output, packages, tools' own.
const SKIPPED: [&str; 5] = ["bin", "obj", "node_modules", ".git", ".vs"];
/// How many folders deep under a project's own snapshots are looked for.
const SNAPSHOT_DEPTH: usize = 6;

/// Where an app's migrations are when its profile doesn't say: the projects its project builds (itself first) with
/// EF Core model snapshots, and their DbContexts.
pub fn discover(startup: &Path) -> Vec<(PathBuf, Vec<String>)> {
    crate::run::build::projects(startup)
        .into_iter()
        .map(|project| {
            let contexts = snapshot_contexts(&project);
            (project, contexts)
        })
        .filter(|(_, contexts)| !contexts.is_empty())
        .collect()
}

/// The DbContexts with migrations in a project: the `[DbContext(typeof(...))]` of each `*ModelSnapshot.cs` in its
/// folder (not in another project's under it), sorted.
pub fn snapshot_contexts(project: &Path) -> Vec<String> {
    static CONTEXT: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\[DbContext\(typeof\(([\w.]+)\)\)\]").expect("a valid pattern"));
    let mut names = BTreeSet::new();
    let mut folders = vec![(project.parent().unwrap_or(Path::new(".")).to_path_buf(), 0)];
    while let Some((folder, depth)) = folders.pop() {
        let Ok(entries) = std::fs::read_dir(&folder) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if path.is_dir() {
                let skipped = SKIPPED.iter().any(|skip| name.eq_ignore_ascii_case(skip));
                if depth < SNAPSHOT_DEPTH && !skipped && !has_project(&path) {
                    folders.push((path, depth + 1));
                }
            } else if name.ends_with("ModelSnapshot.cs")
                && let Ok(text) = std::fs::read_to_string(&path)
            {
                let found =
                    CONTEXT.captures_iter(&text).filter_map(|found| found[1].rsplit('.').next().map(String::from));
                names.extend(found);
            }
        }
    }
    names.into_iter().collect()
}

fn has_project(folder: &Path) -> bool {
    std::fs::read_dir(folder).is_ok_and(|entries| {
        entries.flatten().any(|entry| entry.path().extension().is_some_and(|extension| extension == "csproj"))
    })
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
mod step_tests {
    #[test]
    fn a_migrations_up_reads_as_its_steps_drops_marked() {
        let text = r#"
            protected override void Up(MigrationBuilder migrationBuilder) {
                migrationBuilder.CreateTable(name: "Invoices", columns: table => new { Id = table.Column<int>() });
                migrationBuilder.AddColumn<string>(name: "Currency", table: "Orders", nullable: true);
                migrationBuilder.DropIndex(name: "IX_Old", table: "Orders");
            }
            protected override void Down(MigrationBuilder migrationBuilder) {
                migrationBuilder.DropTable(name: "Invoices");
            }"#;
        let steps = super::parse_steps(text);
        let said: Vec<&str> = steps.iter().map(|step| step.what.as_str()).collect();
        assert_eq!(said, ["CreateTable Invoices", "AddColumn Orders.Currency", "DropIndex Orders.IX_Old"]);
        assert!(steps[2].destructive && !steps[0].destructive);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &[&str]) -> Vec<String> {
        text.iter().map(|line| line.to_string()).collect()
    }

    #[test]
    fn a_run_plans_its_migrations_and_each_is_done_when_the_next_begins() {
        use std::time::Duration;
        let migration = |id: &str, applied| Migration { id: id.into(), name: id.into(), applied: Some(applied) };
        let mut context = Context::new("orders", "ShopDbContext", PathBuf::new(), PathBuf::new());
        context.items = vec![
            migration("1_Init", true),
            migration("2_Orders", true),
            migration("3_Invoices", false),
            migration("4_Refunds", false),
        ];
        let planned = |context: &Context| context.run.as_ref().map(|run| (run.back, run.planned.join(" ")));
        context.plan(None);
        assert_eq!(planned(&context), Some((false, "3_Invoices 4_Refunds".into())), "what's pending");
        context.plan(Some("1_Init"));
        assert_eq!(planned(&context), Some((true, "2_Orders".into())), "back: the newer ones, newest first");
        context.plan(Some("4_Refunds"));
        assert_eq!(context.run.as_ref().and_then(|run| run.from.clone()), Some("2_Orders".into()));
        let start = Instant::now();
        let at = |ms| start + Duration::from_millis(ms);
        context.hop("info:    Applying migration '3_Invoices'.", at(10));
        context.hop("info:    Done.", at(15));
        assert!(context.run.as_ref().is_some_and(|run| run.done().is_empty()), "the first is still going");
        context.hop("info:    Applying migration '4_Refunds'.", at(20));
        assert_eq!(context.run.as_ref().map(Run::done), Some(vec![at(20)]));
        if let Some(run) = &mut context.run {
            run.ended = Some((at(30), None));
        }
        assert_eq!(context.run.as_ref().map(Run::done), Some(vec![at(20), at(30)]), "a clean end finishes it");
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
    fn migrations_are_found_by_their_model_snapshots_in_the_projects_an_app_builds() {
        let temp = crate::testing::TempDir::new("snapshots");
        let write = |path: &str, text: &str| {
            let path = temp.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        let reference =
            |to: &str| format!(r#"<Project><ItemGroup><ProjectReference Include="{to}" /></ItemGroup></Project>"#);
        write("Api/Api.csproj", &reference(r"..\Data\Data.csproj"));
        write("Data/Data.csproj", "<Project />");
        let snapshot = |context: &str| format!("[DbContext(typeof({context}))]\npartial class Snapshot {{}}");
        write("Data/Migrations/ShopDbContextModelSnapshot.cs", &snapshot("ShopDbContext"));
        write("Data/Migrations/Audit/AuditDbContextModelSnapshot.cs", &snapshot("Shop.Data.AuditDbContext"));
        write("Data/bin/Debug/Old/StaleDbContextModelSnapshot.cs", &snapshot("StaleDbContext"));
        write("Data/Tools/Tools.csproj", "<Project />");
        write("Data/Tools/ToolDbContextModelSnapshot.cs", &snapshot("ToolDbContext"));
        let found = discover(&temp.join("Api/Api.csproj"));
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].0.ends_with("Data.csproj"));
        assert_eq!(found[0].1, ["AuditDbContext", "ShopDbContext"], "not build output's, nor another project's");
        assert!(
            discover(&temp.join("Data/Tools/Tools.csproj")).iter().all(|(_, contexts)| contexts == &["ToolDbContext"])
        );
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
