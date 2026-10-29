//! Running dotnet ef for the migrations view (M): one job at a time in the background - finding an app's contexts,
//! listing a context's migrations, moving a context to a migration - waiting while an app builds (dotnet ef
//! --no-build loads the build it would be rewriting).

use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::time::Instant;

use serde_json::{Value, json};

use super::Controller;
use crate::config::Toggle;
use crate::output::{Hue, strip_ansi};
use crate::profile::{App, AppKind};
use crate::run::Tag;
use crate::run::child::Child;
use crate::run::migrations::{self, Context, Migration, Status};

/// The last check of each context, per database, kept between runs in the project's state folder:
/// `{ "<database>": { "<app>:<context>": { "checkedAt": "<RFC 3339>", "migrations": [{ "id", "name", "applied" }] } } }`.
const SAVED_CHECKS: &str = "migrations.json";

/// What a migrations job does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Work {
    /// `dotnet ef dbcontext list` for an app whose profile names no contexts.
    Find { app: String },
    /// `dotnet ef migrations list` for a context.
    List { context: String },
    /// `dotnet ef database update [target]` for a context; `what` is what the log says it did.
    Update { context: String, target: Option<String>, what: String },
}

/// The job dotnet ef runs now.
pub struct EfJob {
    pub id: u64,
    pub work: Work,
    pub started: Instant,
    lines: Vec<String>,
    child: Child,
}

impl Controller {
    /// The project with an app's migrations and its startup project (the app's own), when it has `"migrations"`.
    fn ef_projects(&self, app: &str) -> Option<(std::path::PathBuf, std::path::PathBuf)> {
        let app = self.app(app)?;
        let (AppKind::Dotnet { project: startup, .. }, Some(setup)) = (&app.kind, &app.migrations) else { return None };
        let startup = self.root.join(startup);
        let project = setup.project.as_ref().map_or_else(|| startup.clone(), |project| self.root.join(project));
        Some((project, startup))
    }

    /// M: the contexts, found the first time; checked against the database each time with `checkMigrationsOnOpen`
    /// (r and R in the view otherwise). False, said in the log, when there are none.
    pub fn open_migrations(&mut self) -> bool {
        let check = self.setting(Toggle::CheckMigrationsOnOpen);
        if self.contexts.is_some() {
            if check {
                self.list_migrations(None);
            }
            return true;
        }
        self.load_migrations(check, false)
    }

    /// The contexts, found the first time, without checking them; false (quietly) when there are none.
    pub fn find_migrations(&mut self) -> bool {
        self.contexts.is_some() || self.load_migrations(false, true)
    }

    /// R: the contexts found again (a branch may add or drop some) and all checked; while dotnet ef is busy, the
    /// others checked.
    pub fn reload_migrations(&mut self) -> bool {
        if self.migrations_active() {
            self.list_migrations(None);
            return true;
        }
        self.contexts = None;
        self.load_migrations(true, false)
    }

    /// Finds the contexts and shows their saved checks, checking them too when `check`; `quiet` doesn't say in the
    /// log that there are none (an agent asking).
    fn load_migrations(&mut self, check: bool, quiet: bool) -> bool {
        self.migrations_check = check;
        let contexts = self.find_contexts();
        if contexts.is_empty() && self.migration_queue.is_empty() {
            if quiet {
                return false;
            }
            self.add_log(
                "No migrations: no EF Core model snapshots in the .NET apps' projects or the ones they build - give \
                 an app \"migrations\" in the profile when they're elsewhere.",
                Hue::Yellow,
            );
            return false;
        }
        self.contexts = Some(contexts);
        self.restore_checks();
        if check {
            self.list_migrations(None);
        } else {
            self.pump_migrations();
        }
        true
    }

    fn saved_checks_path(&self) -> PathBuf {
        self.state_dir.join(SAVED_CHECKS)
    }

    /// The database the checks are of, as the log and the view name it.
    fn checked_database(&self) -> String {
        self.db_label.clone().unwrap_or_else(|| "the database".into())
    }

    /// The contexts as last checked on this database, in this run or an earlier one; the others not checked.
    fn restore_checks(&mut self) {
        let db = self.checked_database();
        let saved = crate::json::read(&self.saved_checks_path()).unwrap_or(Value::Null);
        for context in self.contexts.iter_mut().flatten().filter(|context| !context.is_busy()) {
            let check = &saved[db.as_str()][context.key().as_str()];
            context.error.clear();
            match saved_check(check) {
                Some((at, items)) => {
                    (context.items, context.status) = (items, Status::Ok);
                    (context.checked_at, context.db) = (Some(at), db.clone());
                }
                None => {
                    (context.items, context.status) = (Vec::new(), Status::Idle);
                    (context.checked_at, context.db) = (None, String::new());
                }
            }
        }
    }

    /// Keeps a context's check for the next run (on the database it was made on).
    fn save_check(&mut self, key: &str) {
        let Some(context) = self.context(key) else { return };
        let path = self.saved_checks_path();
        let mut saved = crate::json::read(&path).filter(Value::is_object).unwrap_or_else(|| json!({}));
        let items: Vec<Value> = context
            .items
            .iter()
            .map(|item| json!({ "id": item.id, "name": item.name, "applied": item.applied }))
            .collect();
        let at = context.checked_at.map(|at| at.to_rfc3339());
        saved[context.db.as_str()][key] = json!({ "checkedAt": at, "migrations": items });
        if let Err(error) = crate::config::write(&path, &saved) {
            self.add_log(format!("Couldn't keep the migrations check for the next run: {error}"), Hue::Yellow);
        }
    }

    /// The .NET apps' contexts: those an app's `"migrations"` names, or its project's model snapshots have (else
    /// `dotnet ef dbcontext list` is queued to find them); then those of the model snapshots in the projects an app
    /// without it builds - a project once, for the first app that builds it.
    fn find_contexts(&mut self) -> Vec<Context> {
        let path_key = |path: &Path| path.display().to_string().to_lowercase();
        let dotnet: Vec<App> = self.apps.iter().filter(|app| app.traits().builds).cloned().collect();
        let mut contexts = Vec::new();
        let mut claimed: HashSet<String> = HashSet::new();
        for app in dotnet.iter().filter(|app| app.migrations.is_some()) {
            let Some((project, startup)) = self.ef_projects(&app.key) else { continue };
            claimed.insert(path_key(&project));
            let named = app.migrations.as_ref().map(|setup| setup.contexts.clone()).unwrap_or_default();
            let names = if named.is_empty() { migrations::snapshot_contexts(&project) } else { named };
            if names.is_empty() {
                self.migration_queue.push_back(Work::Find { app: app.key.clone() });
            }
            contexts.extend(names.iter().map(|name| Context::new(&app.key, name, project.clone(), startup.clone())));
        }
        for app in dotnet.iter().filter(|app| app.migrations.is_none()) {
            let AppKind::Dotnet { project: startup, .. } = &app.kind else { continue };
            let startup = self.root.join(startup);
            for (project, names) in migrations::discover(&startup) {
                if claimed.insert(path_key(&project)) {
                    let found = names.iter().map(|name| Context::new(&app.key, name, project.clone(), startup.clone()));
                    contexts.extend(found);
                }
            }
        }
        contexts.sort_by_key(|context| self.apps.iter().position(|app| app.key == context.app));
        contexts
    }

    pub fn context(&self, key: &str) -> Option<&Context> {
        self.contexts.as_ref()?.iter().find(|context| context.key() == key)
    }

    fn context_mut(&mut self, key: &str) -> Option<&mut Context> {
        self.contexts.as_mut()?.iter_mut().find(|context| context.key() == key)
    }

    /// r: checks the contexts' migrations again (`only` one).
    pub fn list_migrations(&mut self, only: Option<&str>) {
        let keys: Vec<String> = self
            .contexts
            .iter()
            .flatten()
            .filter(|context| only.is_none_or(|only| context.key() == only) && !context.is_busy())
            .map(Context::key)
            .collect();
        for key in keys {
            if let Some(context) = self.context_mut(&key) {
                context.status = Status::Queued;
            }
            self.migration_queue.push_back(Work::List { context: key });
        }
        self.pump_migrations();
    }

    /// After the confirmation: moves a context to `target` (a migration's id), or applies what's pending.
    pub fn migrate(&mut self, key: &str, target: Option<String>, what: String) {
        let Some(context) = self.context_mut(key) else { return };
        context.status = Status::Queued;
        self.migration_queue.push_back(Work::Update { context: key.to_string(), target, what });
        self.pump_migrations();
    }

    /// Whether dotnet ef is running or waiting.
    pub fn migrations_active(&self) -> bool {
        self.migration_job.is_some() || !self.migration_queue.is_empty()
    }

    /// The jobs waiting, for the view.
    pub fn migration_queue(&self) -> &VecDeque<Work> {
        &self.migration_queue
    }

    /// The database was switched or the branch changed: what is applied, and which migrations exist, may differ -
    /// the contexts show their last check on this database, and are checked again with `checkMigrationsOnOpen`.
    pub(super) fn migrations_changed(&mut self) {
        if self.contexts.is_none() {
            return;
        }
        self.restore_checks();
        if self.setting(Toggle::CheckMigrationsOnOpen) {
            self.list_migrations(None);
        }
    }

    /// Starts the next job, unless one runs or an app builds.
    pub(super) fn pump_migrations(&mut self) {
        if self.migration_job.is_some() || self.building.is_some() {
            return;
        }
        let Some(work) = self.migration_queue.pop_front() else { return };
        let (app, context, project, startup) = match &work {
            Work::Find { app } => {
                let Some((project, startup)) = self.ef_projects(app) else {
                    return self.fail_migrations(app, &format!("{app} has no \"migrations\" in the profile any more"));
                };
                (app.clone(), None, project, startup)
            }
            Work::List { context } | Work::Update { context, .. } => match self.context(context) {
                Some(found) => {
                    (found.app.clone(), Some(found.name.clone()), found.project.clone(), found.startup.clone())
                }
                None => return self.pump_migrations(),
            },
        };
        let name = self.app(&app).map(|app| app.name.clone()).unwrap_or_default();
        if !migrations::built(&startup) {
            self.fail_migrations(&app, &format!("{name} has no build yet - b builds it, then r here"));
            return;
        }
        let command: Vec<&str> = match &work {
            Work::Find { .. } => vec!["dbcontext", "list", "--json"],
            Work::List { .. } => vec!["migrations", "list", "--json"],
            Work::Update { target, .. } => {
                let mut command = vec!["database", "update"];
                command.extend(target.as_deref());
                command
            }
        };
        let spec = self.commands.ef(&self.root, migrations::ef_args(&command, context.as_deref(), &project, &startup));
        if let Work::List { context: key } | Work::Update { context: key, .. } = &work
            && let Some(found) = self.context_mut(key)
        {
            found.status = if matches!(work, Work::Update { .. }) { Status::Migrating } else { Status::Loading };
            found.output.clear();
        }
        if let (Work::Update { what, .. }, Some(context)) = (&work, &context) {
            let db = self.db_label.clone().unwrap_or_else(|| "the database".into());
            self.add_log(format!("Migrating {context} on {db}: {what}…"), Hue::Cyan);
        }
        self.next_job += 1;
        let id = self.next_job;
        match Child::spawn(&spec, Tag::Migration { job: id }, self.events.clone(), self.guard.as_ref()) {
            Ok(child) => {
                let started = (self.now)();
                self.migration_job = Some(EfJob { id, work, started, lines: Vec::new(), child });
            }
            Err(error) => self.fail_migrations(&app, &format!("dotnet couldn't start ({error})")),
        }
    }

    /// The app's contexts can't be checked: their jobs drop out of the queue, with why.
    fn fail_migrations(&mut self, app: &str, why: &str) {
        let keys: Vec<String> =
            self.contexts.iter().flatten().filter(|context| context.app == app).map(Context::key).collect();
        self.migration_queue.retain(|work| match work {
            Work::Find { app: of } => of != app,
            Work::List { context } | Work::Update { context, .. } => !keys.contains(context),
        });
        for key in &keys {
            if let Some(context) = self.context_mut(key) {
                (context.status, context.error) = (Status::Error, why.to_string());
            }
        }
        self.add_log(format!("✗ Migrations: {why}"), Hue::Red);
        self.pump_migrations();
    }

    pub(super) fn migration_output(&mut self, id: u64, line: &str) {
        let Some(job) = self.migration_job.as_mut().filter(|job| job.id == id) else { return };
        let text = strip_ansi(line);
        job.lines.push(text.clone());
        if let Work::List { context } | Work::Update { context, .. } = job.work.clone()
            && let Some(context) = self.context_mut(&context)
        {
            context.receive(&text);
        }
    }

    pub(super) fn migration_exited(&mut self, id: u64, code: Option<i32>) {
        let Some(job) = self.migration_job.take_if(|job| job.id == id) else { return };
        let db = self.checked_database();
        match job.work {
            Work::Find { app } => self.found_contexts(&app, &job.lines, code),
            Work::List { context: key } => self.listed(&key, &job.lines, code, &db),
            Work::Update { context: key, what, .. } => {
                let name = self.context(&key).map(|context| context.name.clone()).unwrap_or_default();
                if code == Some(0) {
                    self.add_log(format!("✓ {name} on {db}: {what}"), Hue::Green);
                    self.happened("migrations.applied", None, format!("{name} on {db}: {what}"));
                } else {
                    let failure = migrations::failure(&job.lines, code);
                    if let Some(context) = self.context_mut(&key) {
                        context.error = failure.clone();
                    }
                    self.add_log(format!("✗ Migrating {name} failed: {failure}"), Hue::Red);
                    self.notifications.push(("Migration failed".into(), format!("{name} on {db}")));
                }
                if let Some(context) = self.context_mut(&key) {
                    context.status = Status::Queued;
                }
                self.migration_queue.push_front(Work::List { context: key }); // what is applied now
            }
        }
        self.pump_migrations();
    }

    /// `dotnet ef migrations list` is done.
    fn listed(&mut self, key: &str, lines: &[String], code: Option<i32>, db: &str) {
        let listed = migrations::parse_list(lines).ok().filter(|_| code == Some(0));
        let failure = migrations::failure(lines, code);
        let Some(context) = self.context_mut(key) else { return };
        let name = context.name.clone();
        let Some(items) = listed else {
            (context.status, context.error) = (Status::Error, failure.clone());
            self.add_log(format!("✗ Migrations of {name}: {failure}"), Hue::Red);
            return;
        };
        let pending = items.iter().filter(|item| item.applied == Some(false)).count();
        (context.items, context.status, context.error) = (items, Status::Ok, String::new());
        (context.checked_at, context.db) = (Some(chrono::Local::now()), db.to_string());
        self.save_check(key);
        if pending > 0 {
            let plural = if pending == 1 { "" } else { "s" };
            self.add_log(format!("{name}: {pending} pending migration{plural} on {db}"), Hue::Yellow);
        }
    }

    /// `dotnet ef dbcontext list` is done: the app's contexts, each listed next.
    fn found_contexts(&mut self, app: &str, lines: &[String], code: Option<i32>) {
        let Some((project, startup)) = self.ef_projects(app) else { return };
        let names = migrations::parse_contexts(lines).ok().filter(|names| code == Some(0) && !names.is_empty());
        let Some(names) = names else {
            let mut context = Context::new(app, "DbContext", project, startup);
            (context.status, context.error) = (Status::Error, migrations::failure(lines, code));
            let name = self.app(app).map(|app| app.name.clone()).unwrap_or_default();
            let why = format!("✗ {name}'s DbContexts weren't found: {}", context.error);
            self.contexts.get_or_insert_with(Vec::new).push(context);
            self.add_log(why, Hue::Red);
            return;
        };
        for name in names {
            let mut context = Context::new(app, &name, project.clone(), startup.clone());
            if self.migrations_check {
                context.status = Status::Queued;
                self.migration_queue.push_back(Work::List { context: context.key() });
            }
            self.contexts.get_or_insert_with(Vec::new).push(context);
        }
    }

    /// An app is about to build: dotnet ef --no-build would load what the build rewrites, so a running job stops
    /// and runs again first after the build.
    pub(super) fn pause_migrations(&mut self) {
        let Some(job) = self.migration_job.take() else { return };
        job.child.kill();
        if let Work::List { context } | Work::Update { context, .. } = &job.work
            && let Some(context) = self.context_mut(context)
        {
            context.status = Status::Queued;
        }
        self.migration_queue.push_front(job.work);
    }

    /// Quitting: dotnet ef stops, the queue empties.
    pub(super) fn stop_migrations(&mut self) {
        if let Some(job) = self.migration_job.take() {
            job.child.kill();
        }
        self.migration_queue.clear();
    }
}

/// A check kept by an earlier run: when, and the migrations it found.
fn saved_check(check: &Value) -> Option<(chrono::DateTime<chrono::Local>, Vec<Migration>)> {
    let at = chrono::DateTime::parse_from_rfc3339(check["checkedAt"].as_str()?).ok()?.with_timezone(&chrono::Local);
    let items = check["migrations"]
        .as_array()?
        .iter()
        .map(|item| Migration {
            id: crate::json::text(&item["id"]),
            name: crate::json::text(&item["name"]),
            applied: item["applied"].as_bool(),
        })
        .collect();
    Some((at, items))
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::time::Duration;

    use serde_json::json;
    use tokio::sync::mpsc::UnboundedReceiver;

    use crate::paths::Paths;
    use crate::profile::{App, Migrations, ProfileDirs, Project, import};
    use crate::run::child::Spec;
    use crate::run::migrations::Status;
    use crate::run::tools::{Commands, ServerCommand};
    use crate::run::{Controller, Event};
    use crate::testing::{TempDir, sample_profile, sample_repo, write};

    /// dotnet ef as a script: `dbcontext list` finds one context, `migrations list` prints two migrations (the second
    /// pending until a `database update`), `database update` remembers it ran.
    struct Ef {
        updated: PathBuf,
    }

    fn shell(script: String, dir: &Path) -> Spec {
        let (program, flag) = if cfg!(windows) { ("cmd", "/C") } else { ("sh", "-c") };
        Spec {
            program: program.into(),
            args: vec![flag.into(), script.into()],
            dir: dir.to_path_buf(),
            env: Vec::new(),
        }
    }

    impl Commands for Ef {
        fn server(&self, root: &Path, app: &App, _: &[App]) -> anyhow::Result<ServerCommand> {
            Ok(ServerCommand { spec: shell("exit 0".into(), root), label: app.name.clone(), script: None })
        }

        fn build(&self, root: &Path, _: &str) -> Spec {
            shell("exit 0".into(), root)
        }

        fn npm_ci(&self, dir: &Path) -> anyhow::Result<Spec> {
            Ok(shell("exit 0".into(), dir))
        }

        fn ef(&self, root: &Path, args: Vec<OsString>) -> Spec {
            let args: Vec<String> = args.iter().map(|arg| arg.to_string_lossy().into_owned()).collect();
            let applied = self.updated.is_file();
            let text = match args[1].as_str() {
                "dbcontext" => r#"data: [{"safeName": "ShopDbContext"}]"#.to_string(),
                "migrations" => format!(
                    r#"data: [{{"id": "1_Init", "name": "Init", "applied": true}}, {{"id": "2_Orders", "name": "Orders", "applied": {applied}}}]"#
                ),
                _ => {
                    std::fs::write(&self.updated, "").unwrap();
                    "info: Done.".to_string()
                }
            };
            // Printed from a file: cmd.exe would take the quotes of an echo apart.
            let file = self.updated.with_extension(format!("{}.txt", args[1]));
            std::fs::write(&file, text).unwrap();
            let show = if cfg!(windows) { "type" } else { "cat" };
            shell(format!("{show} {}", file.display()), root)
        }
    }

    fn sample(temp: &TempDir, contexts: Option<Vec<String>>, built: bool) -> (Controller, UnboundedReceiver<Event>) {
        sample_with(temp, contexts, built, json!({ "checkMigrationsOnOpen": true }))
    }

    fn sample_with(
        temp: &TempDir,
        contexts: Option<Vec<String>>,
        built: bool,
        config: serde_json::Value,
    ) -> (Controller, UnboundedReceiver<Event>) {
        let repo = sample_repo(&temp.join("repo"));
        if built {
            write(repo.join("services/Orders/bin/Debug/net10.0/Orders.dll"), ""); // a build for --no-build
        }
        let profiles =
            ProfileDirs { user: temp.join("profiles"), bundled: temp.join("bundled"), only: Some(sample_profile()) };
        let paths = Paths { config_file: temp.join("config.json"), state_dir: temp.join("state"), profiles };
        crate::config::write(&paths.config_file, &config).unwrap();
        let mut project: Project = import(&sample_profile(), &paths.profiles).unwrap();
        project.apps[0].migrations = contexts.map(|contexts| Migrations { project: None, contexts });
        let (events, rx) = tokio::sync::mpsc::unbounded_channel();
        let setup = crate::run::controller::Setup { project, root: repo, paths, events, guard: None };
        let mut controller = Controller::new(setup).unwrap();
        controller.commands = Arc::new(Ef { updated: temp.join("updated") });
        (controller, rx)
    }

    async fn run_until(
        controller: &mut Controller,
        events: &mut UnboundedReceiver<Event>,
        done: impl Fn(&Controller) -> bool,
    ) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        while !done(controller) {
            match tokio::time::timeout_at(deadline, events.recv()).await {
                Ok(Some(event)) => controller.handle(event),
                _ => panic!("timed out: {:?}", controller.log.iter().map(|entry| &entry.text).collect::<Vec<_>>()),
            }
        }
    }

    #[tokio::test]
    async fn contexts_are_found_listed_and_migrated() {
        let temp = TempDir::new("migrations");
        let (mut sample, mut rx) = sample(&temp, Some(Vec::new()), true);
        assert!(sample.open_migrations());
        let found =
            |c: &Controller| c.context("orders:ShopDbContext").is_some_and(|context| context.status == Status::Ok);
        run_until(&mut sample, &mut rx, found).await;
        let context = sample.context("orders:ShopDbContext").expect("found by dotnet ef dbcontext list");
        assert_eq!((context.items.len(), context.pending().len(), context.current()), (2, 1, Some(0)));
        sample.migrate("orders:ShopDbContext", None, "applied 1 pending migration".into());
        run_until(&mut sample, &mut rx, |c| !c.migrations_active()).await;
        assert!(sample.context("orders:ShopDbContext").unwrap().pending().is_empty(), "listed again after it");
        let log: Vec<&String> = sample.log.iter().map(|entry| &entry.text).collect();
        let done =
            |line: &&&String| line.starts_with("✓ ShopDbContext on ") && line.ends_with("applied 1 pending migration");
        assert!(log.iter().any(|line| done(&line)), "{log:?}");
    }

    #[tokio::test]
    async fn without_a_build_or_any_migrations_it_says_what_to_do() {
        let temp = TempDir::new("migrations-unbuilt");
        let (mut unbuilt, _rx) = sample(&temp, Some(vec!["ShopDbContext".into()]), false);
        assert!(unbuilt.open_migrations());
        let context = unbuilt.context("orders:ShopDbContext").unwrap();
        assert_eq!(
            (context.status, context.error.as_str()),
            (Status::Error, "Orders has no build yet - b builds it, then r here")
        );
        let none = TempDir::new("migrations-none");
        let (mut plain, _rx) = sample(&none, None, false);
        assert!(!plain.open_migrations());
        assert!(plain.log.back().unwrap().text.starts_with("No migrations: no EF Core model snapshots"));
    }

    #[tokio::test]
    async fn the_model_snapshots_are_found_and_checked_only_when_asked() {
        let temp = TempDir::new("migrations-snapshots");
        let (mut sample, mut rx) = sample_with(&temp, None, true, json!({}));
        let snapshot = sample.root.join("services/Orders/Migrations/ShopDbContextModelSnapshot.cs");
        std::fs::create_dir_all(snapshot.parent().unwrap()).unwrap();
        std::fs::write(&snapshot, "[DbContext(typeof(ShopDbContext))]").unwrap();
        assert!(sample.open_migrations());
        let context = sample.context("orders:ShopDbContext").expect("found by its model snapshot");
        assert_eq!(context.status, Status::Idle, "not checked: checkMigrationsOnOpen is off");
        assert!(!sample.migrations_active());
        assert!(sample.reload_migrations());
        let checked =
            |c: &Controller| c.context("orders:ShopDbContext").is_some_and(|context| context.status == Status::Ok);
        run_until(&mut sample, &mut rx, checked).await;
        assert_eq!(sample.context("orders:ShopDbContext").unwrap().items.len(), 2);
        sample.db_label = Some("Other @ localhost".into());
        sample.migrations_changed();
        assert_eq!(sample.context("orders:ShopDbContext").unwrap().status, Status::Idle, "not checked on this one");

        // The next run shows the last check on each database.
        let (mut next, _rx) = sample_with(&temp, None, true, json!({}));
        assert_eq!(next.state_dir, sample.state_dir);
        std::fs::create_dir_all(snapshot.parent().unwrap()).unwrap();
        std::fs::write(&snapshot, "[DbContext(typeof(ShopDbContext))]").unwrap();
        next.db_label = None;
        assert!(next.open_migrations());
        let kept = next.context("orders:ShopDbContext").unwrap();
        assert_eq!((kept.status, kept.items.len(), kept.db.as_str()), (Status::Ok, 2, "the database"));
        assert!(kept.checked_at.is_some());
    }
}
