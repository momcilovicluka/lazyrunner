//! Pulling a copy of a database from another environment (#110), as a background job while the apps keep running on
//! the database in use: the check, export, import and after-pull scripts in a thread (`pull_steps`), then here the
//! migrations (`migrateAfterPull`: the migrations screen's own `database update`, on the new copy) and the switch
//! (`Enter`'s: its connection string added and made the one in use, the APIs restarted). The same steps only export
//! a source's database, or import it without the switch, or update a linked database (#114): its twin imported
//! beside it, then swapped in under its name (`swap`). A source's password comes from the credential store, or as
//! typed; it's never kept anywhere else. `x` cancels; `keepPulled` offers to drop the older copies. One database job
//! at a time.

use std::sync::Arc;
use std::time::Instant;

use tokio::sync::mpsc::UnboundedSender;

use super::Controller;
use super::pull_steps::{self, Plan};
use crate::output::Hue;
use crate::profile::sources::DatabaseSource;
use crate::run::cancel::{CANCELLED, Cancel};
use crate::run::credentials::Password;
use crate::run::database_files::{self as files, Progress};
use crate::run::pull::{self, Mode, News, Pulled, State, Step};
use crate::run::{Event, secrets, sql};

/// The settings that override the profile's on this machine.
pub const KEEP_PULLED: &str = "keepPulled";
pub const MIGRATE_AFTER_PULL: &str = "migrateAfterPull";

/// A step of the pull: where it is, when it started and ended, what it says under its name, and its output.
#[derive(Clone, Debug, Default)]
pub struct StepRun {
    pub state: State,
    pub started: Option<Instant>,
    pub ended: Option<Instant>,
    pub detail: String,
    pub output: Vec<String>,
}

/// Where the migrations step is: finding the contexts, or applying what's pending.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Migrating {
    Finding,
    Applying,
}

/// The pull running, or the last one: what it makes (`mode`), from which source (its name, its database there, and
/// where it is), into which new database on which server, each step, the export's and import's progress, how it
/// failed, and the apps the switch or the swap restarts (each with the run it had before, so its new one is told
/// apart). An update also has the linked database it replaces, and its old copy's name once swapped.
pub struct PullRun {
    pub job: u64,
    pub mode: Mode,
    pub source: String,
    pub remote: String,
    pub from: String,
    pub server: String,
    pub database: String,
    pub file: String,
    pub started: Instant,
    pub ended: Option<Instant>,
    pub steps: Vec<StepRun>,
    pub progress: Progress,
    /// The share done as last reported: the one before, the newest, when it came.
    pub share: Option<(f64, f64, Instant)>,
    pub error: Option<(Step, String)>,
    pub cancelled: bool,
    pub apps: Vec<(String, u64)>,
    /// An update's linked database, and its old copy's name.
    pub target: Option<String>,
    pub old: Option<String>,
    pub(super) cancel: Arc<Cancel>,
    pub(super) migrating: Option<Migrating>,
    /// The connection string in use before the migrations step made the new copy's the one in use.
    pub(super) before: Option<secrets::Entry>,
    /// The local server's connection string, for an update's swap.
    pub(super) local: String,
    /// The apps an update's swap stopped, started again after it.
    pub(super) stopped: Vec<String>,
}

impl PullRun {
    pub fn step(&self, step: Step) -> &StepRun {
        &self.steps[step.index()]
    }

    pub(super) fn step_mut(&mut self, step: Step) -> &mut StepRun {
        &mut self.steps[step.index()]
    }

    /// Its steps, in the order they run.
    pub fn steps(&self) -> &'static [Step] {
        self.mode.steps()
    }

    /// The step running, if one is.
    pub fn running(&self) -> Option<Step> {
        self.steps().iter().copied().find(|step| self.step(*step).state == State::Running)
    }

    pub fn is_over(&self) -> bool {
        self.ended.is_some()
    }

    pub fn went_well(&self) -> bool {
        self.is_over() && self.error.is_none() && !self.cancelled
    }

    /// Its remote database as the screen says it: `qa/Shop`.
    pub fn twin(&self) -> String {
        format!("{}/{}", self.source, self.remote)
    }
}

pub(super) fn say(events: &UnboundedSender<Event>, job: u64, news: News) {
    let _ = events.send(Event::Pull { job, news });
}

/// What starts a pull's thread: the source and its database there, the connection string to it, the local server
/// (its name and connection string), and an update's linked database.
pub(super) struct Launch {
    pub(super) mode: Mode,
    pub(super) source: DatabaseSource,
    pub(super) remote: String,
    pub(super) connection: String,
    pub(super) server: String,
    pub(super) local: String,
    pub(super) target: Option<String>,
}

impl Controller {
    /// The sources there are to pull from: the profile's and this machine's.
    pub fn pull_sources(&self) -> Vec<DatabaseSource> {
        self.sources()
    }

    /// The source and database `P` pulls from again: the last pull's, else the newest that went well.
    pub fn last_pull(&self) -> Option<(String, Option<String>)> {
        let last = self.pull.as_ref().filter(|pull| pull.mode == Mode::Pull).map(|pull| {
            let whole = self.source(&pull.source).is_some_and(|source| source.is_server());
            (pull.source.clone(), whole.then(|| pull.remote.clone()))
        });
        last.or_else(|| self.pulled.last().map(|pulled| (pulled.source.clone(), pulled.remote.clone())))
    }

    /// The source `P` pulls from again.
    pub fn last_pull_source(&self) -> Option<String> {
        self.last_pull().map(|(source, _)| source)
    }

    /// Whether a pull is running; what it does then.
    pub(super) fn pull_busy(&self) -> Option<String> {
        let pull = self.pull.as_ref().filter(|pull| !pull.is_over())?;
        Some(match pull.mode {
            Mode::Export => format!("exporting {}", pull.twin()),
            Mode::Update => format!("updating {} from {}", pull.target.as_deref().unwrap_or_default(), pull.twin()),
            _ => format!("pulling a copy of {} into {}", pull.source, pull.database),
        })
    }

    /// `p`: pulls a copy of `source` as a new database on the server in use, in the background - `typed`, the password
    /// just typed (else the one kept). What it says it does.
    pub fn pull_database(&mut self, source: &str, typed: Option<Password>) -> Result<String, String> {
        self.pull_from(Mode::Pull, source, None, typed)
    }

    /// `p`, `e` or `i` on a remote database: pulls, only exports, or imports without switching to it - `database`, one
    /// of a whole server's (a database source's own without it).
    pub fn pull_from(
        &mut self,
        mode: Mode,
        source: &str,
        database: Option<&str>,
        typed: Option<Password>,
    ) -> Result<String, String> {
        if mode == Mode::Update {
            return Err("an update starts from a linked database (U)".into());
        }
        let source = self.known_source(source)?;
        self.ready_for_sql()?;
        self.package.ready()?;
        if source.is_server() && database.is_none() {
            return Err(format!("{} is a whole server - pick one of its databases", source.name));
        }
        let password = self.password_for(&source, typed)?;
        let connection = pull::connection_to(&source, database, password.as_ref(), |name| std::env::var(name).ok())?;
        let reached = sql::target(&connection)?;
        let remote = reached.database.clone().ok_or_else(|| format!("{} names no database", source.name))?;
        let in_use = self.entry_in_use().ok_or_else(|| "no connection string is in use to pull next to".to_string());
        let (server, local) = match (mode, in_use) {
            (Mode::Export, in_use) => (in_use.map(|entry| entry.server).unwrap_or_default(), String::new()),
            (_, Err(why)) => return Err(why),
            (_, Ok(in_use)) => {
                if reached.server.eq_ignore_ascii_case(&in_use.server) {
                    return Err(format!(
                        "{} is the server in use - a pull only reads from a source and never writes to one",
                        in_use.server
                    ));
                }
                let local = self.connection(&in_use.server, &in_use.database)?;
                (in_use.server, local)
            }
        };
        self.launch(Launch { mode, source, remote, connection, server, local, target: None })
    }

    /// Starts a pull's thread and keeps it as the one running. What it says it does.
    pub(super) fn launch(&mut self, launch: Launch) -> Result<String, String> {
        let Launch { mode, source, remote, connection, server, local, target } = launch;
        let scripts = if mode.imports() { self.after_pull_scripts()? } else { Vec::new() };
        let database = pull::pulled_name(&remote, &source.name, &jiff::ZonedDateTime::now(), &self.names_on(&server));
        let database = files::valid_name(&database)?;
        let folder = self.files_folder();
        std::fs::create_dir_all(&folder).map_err(|error| format!("{}: {error}", folder.display()))?;
        let taken: Vec<String> = self.db_files.iter().map(|file| file.stem()).collect();
        let file = folder.join(format!("{}.{}", files::free_name(&database, &taken), files::BACPAC));
        let file = file.to_string_lossy().into_owned();
        let job = self.new_db_job();
        let cancel = Arc::new(Cancel::default());
        let plan = Plan {
            mode,
            source: source.name.clone(),
            source_database: remote.clone(),
            source_connection: connection,
            local: local.clone(),
            database: database.clone(),
            file: file.clone(),
            private: self.state_dir.clone(),
            scripts,
        };
        let (sql, package, events, watched) =
            (Arc::clone(&self.sql), Arc::clone(&self.package), self.events.clone(), Arc::clone(&cancel));
        std::thread::spawn(move || {
            let tell = |news| say(&events, job, news);
            pull_steps::run(sql.as_ref(), package.as_ref(), &plan, &watched, &tell);
        });
        let from = source.on(&remote).label();
        let mut pull = PullRun {
            job,
            mode,
            source: source.name.clone(),
            remote: remote.clone(),
            from: from.clone(),
            server: server.clone(),
            database: database.clone(),
            file: file.clone(),
            started: (self.now)(),
            ended: None,
            steps: vec![StepRun::default(); Step::ALL.len()],
            progress: Progress::default(),
            share: None,
            error: None,
            cancelled: false,
            apps: Vec::new(),
            target: target.clone(),
            old: None,
            cancel,
            migrating: None,
            before: None,
            local,
            stopped: Vec::new(),
        };
        pull.apps = self.restarting_apps(&pull);
        self.pull = Some(pull);
        let (said, log) = match (mode, &target) {
            (Mode::Export, _) => {
                let name = std::path::Path::new(&file).file_name().map(|name| name.to_string_lossy().into_owned());
                let name = name.unwrap_or_default();
                (
                    format!("export {} ({from}) to {name}", source.name),
                    format!("⇣ Exporting {} ({from}) to {name}…", source.name),
                )
            }
            (Mode::Update, Some(target)) => (
                format!("update {target} @ {server} from {}/{remote}", source.name),
                format!(
                    "⇣ Updating {target} @ {server} from {}/{remote} - its new copy comes in beside it…",
                    source.name
                ),
            ),
            _ => (
                format!("{} {} ({from}) into {database} @ {server}", mode.name(), source.name),
                format!("⇣ {} {} ({from}) into {database} @ {server}…", capital(mode.name()), source.name),
            ),
        };
        self.add_log(log, Hue::Cyan);
        Ok(said)
    }

    /// `P`: pulls again from the last source.
    pub fn pull_again(&mut self) -> Result<String, String> {
        let (source, database) =
            self.last_pull().ok_or_else(|| "nothing pulled yet - p pulls from a source".to_string())?;
        self.pull_from(Mode::Pull, &source, database.as_deref(), None)
    }

    /// `x`: cancels the pull's running step. The source is untouched; a half-imported database is dropped, and so is
    /// an update's new copy - its linked database stays as it was.
    pub fn cancel_pull(&mut self) -> Result<String, String> {
        let pull = self.pull.as_mut().filter(|pull| !pull.is_over()).ok_or_else(|| "no pull is running".to_string())?;
        let running = pull.running();
        if running == Some(Step::Swap) {
            return Err(
                "the swap is under way - it isn't stopped half-way (it puts every name back if it fails)".into()
            );
        }
        pull.cancelled = true;
        pull.cancel.cancel();
        let what = pull.mode.name();
        let said = match running {
            Some(Step::Switch) => "the switch is done - the APIs restart on the new copy".to_string(),
            Some(step) => format!("cancelling the {what} at {}", step.name()),
            None => format!("cancelling the {what}"),
        };
        match running {
            Some(Step::Migrations) => {
                self.stop_migrations();
                self.pull_failed(Step::Migrations, CANCELLED.into());
            }
            Some(Step::Switch) => self.pull_finished(),
            _ => {}
        }
        Ok(said)
    }

    /// The connection string in use: the main group's active one.
    pub(super) fn entry_in_use(&self) -> Option<secrets::Entry> {
        let groups = secrets::groups(&self.secrets_text()?);
        let (_, entries) = groups.into_iter().next()?;
        entries.into_iter().find(|entry| entry.active)
    }

    /// The after-pull scripts, read: each its name and text.
    fn after_pull_scripts(&self) -> Result<Vec<(String, String)>, String> {
        self.project
            .pulling
            .after_pull
            .iter()
            .map(|path| {
                let text = std::fs::read_to_string(self.root.join(crate::profile::native(path)))
                    .map_err(|error| format!("afterPull's {path}: {error}"))?;
                Ok((path.clone(), text))
            })
            .collect()
    }

    /// The running APIs the switch restarts (those that read the connection strings) or an update's swap stops and
    /// starts again (those on its linked database), each with its run now.
    pub(super) fn restarting_apps(&self, pull: &PullRun) -> Vec<(String, u64)> {
        let keys: Vec<String> = match (pull.mode, &pull.target) {
            (Mode::Pull, _) => self
                .apps
                .iter()
                .filter(|app| app.traits().uses_secrets && self.is_running(&app.key))
                .map(|app| app.key.clone())
                .collect(),
            (Mode::Update, Some(target)) => {
                self.apps_on(&pull.server, target).into_iter().filter(|key| self.is_running(key)).collect()
            }
            _ => Vec::new(),
        };
        keys.into_iter().filter_map(|key| Some((key.clone(), self.state(&key)?.generation))).collect()
    }

    /// Whether the pull applies the pending migrations: the setting, else the profile's `migrateAfterPull`.
    fn migrate_after_pull(&self) -> bool {
        self.config[MIGRATE_AFTER_PULL].as_bool().unwrap_or(self.project.pulling.migrate)
    }

    /// How many pulled copies of a source are kept: the setting, else the profile's `keepPulled`.
    pub fn keep_pulled(&self) -> Option<usize> {
        let set = self.config[KEEP_PULLED].as_u64().map(|keep| keep.max(1) as usize);
        set.or(self.project.pulling.keep_pulled)
    }

    /// The pull's news from its thread.
    pub(super) fn pull_news(&mut self, job: u64, news: News) {
        let now = (self.now)();
        let Some(pull) = self.pull.as_mut().filter(|pull| pull.job == job && !pull.is_over()) else { return };
        match news {
            News::Started(step) => {
                let run = pull.step_mut(step);
                (run.state, run.started) = (State::Running, Some(now));
                if step.flows() {
                    (pull.progress, pull.share) = (Progress::default(), None);
                }
            }
            News::Line(step, line) => pull.step_mut(step).output.push(line),
            News::Progress(progress) => {
                if let Some(share) = progress.share {
                    let before = pull.share.map_or(0.0, |(_, newest, _)| newest);
                    pull.share = Some((before, share, now));
                }
                pull.progress = pull.progress.merged(progress);
            }
            News::Done(step, detail) => {
                let run = pull.step_mut(step);
                (run.state, run.ended, run.detail) = (State::Done, Some(now), detail);
            }
            News::Skipped(step, detail) => {
                let run = pull.step_mut(step);
                (run.state, run.detail) = (State::Skipped, detail);
            }
            News::Ended(Err((step, why))) => self.pull_failed(step, why),
            News::Ended(Ok(())) if pull.cancelled => self.pull_failed(Step::Migrations, CANCELLED.into()),
            News::Ended(Ok(())) => match pull.mode {
                Mode::Export | Mode::Import => self.pull_finished(),
                Mode::Pull | Mode::Update => self.start_migrations_step(),
            },
            News::Swapped(result) => self.swapped(result),
        }
    }

    /// The migrations step: the new copy made the one in use (quietly - the APIs restart at the switch), the contexts
    /// found, then each one's pending migrations applied; skipped when it's off or there are none.
    fn start_migrations_step(&mut self) {
        if !self.migrate_after_pull() {
            return self.skip_step(Step::Migrations, "off (migrateAfterPull)");
        }
        if !self.find_migrations() {
            return self.skip_step(Step::Migrations, "no EF Core migrations");
        }
        let Some((server, database)) = self.pull.as_ref().map(|pull| (pull.server.clone(), pull.database.clone()))
        else {
            return;
        };
        let before = self.entry_in_use();
        let made_active = self
            .new_entry(&server, &database)
            .and_then(|entry| self.secrets_file().map(|path| (path, entry)))
            .ok_or_else(|| "its connection string couldn't be added".to_string())
            .and_then(|(path, entry)| secrets::switch(&path, &entry).map_err(|error| error.to_string()));
        if let Err(why) = made_active {
            return self.pull_failed(Step::Migrations, why);
        }
        let now = (self.now)();
        if let Some(pull) = self.pull.as_mut() {
            pull.before = before;
            pull.migrating = Some(Migrating::Finding);
            let run = pull.step_mut(Step::Migrations);
            (run.state, run.started) = (State::Running, Some(now));
        }
        self.add_log(format!("Applying the pending migrations to {database}…"), Hue::Cyan);
    }

    /// The connection string to the new copy: added beside the one in use (as a copy's is), inactive.
    fn new_entry(&mut self, server: &str, database: &str) -> Option<secrets::Entry> {
        if self.entry_on(server, Some(database)).is_none() {
            let in_use = self.entry_in_use().map(|entry| entry.database);
            self.add_connection(server, in_use.as_deref(), database);
        }
        self.entry_on(server, Some(database))
    }

    fn skip_step(&mut self, step: Step, why: &str) {
        if let Some(pull) = self.pull.as_mut() {
            let run = pull.step_mut(step);
            (run.state, run.detail) = (State::Skipped, why.to_string());
        }
        if step == Step::Migrations {
            self.after_migrations();
        }
    }

    /// After the migrations: a pull's switch, an update's swap.
    fn after_migrations(&mut self) {
        match self.pull.as_ref().map(|pull| pull.mode) {
            Some(Mode::Update) => self.start_swap_step(),
            Some(_) => self.start_switch_step(),
            None => {}
        }
    }

    /// The switch: the new copy's connection string made the one in use and the APIs restarted on it, as `Enter`
    /// does; done once each restarted API answers again.
    fn start_switch_step(&mut self) {
        let Some((server, database)) = self.pull.as_ref().map(|pull| (pull.server.clone(), pull.database.clone()))
        else {
            return;
        };
        let now = (self.now)();
        let apps = self.pull.as_ref().map(|pull| self.restarting_apps(pull)).unwrap_or_default();
        if let Some(pull) = self.pull.as_mut() {
            pull.apps = apps;
            let run = pull.step_mut(Step::Switch);
            (run.state, run.started) = (State::Running, Some(now));
        }
        let Some(entry) = self.new_entry(&server, &database) else {
            return self.pull_failed(Step::Switch, "its connection string couldn't be added to secrets.json".into());
        };
        self.switch_database(&entry);
        self.tick_pull();
    }

    /// Each tick: the migrations step's dotnet ef done, or the switch's APIs back up.
    pub(super) fn tick_pull(&mut self) {
        let Some(pull) = self.pull.as_ref().filter(|pull| !pull.is_over()) else { return };
        match (pull.running(), pull.migrating) {
            (Some(Step::Migrations), Some(phase)) if !self.migrations_active() => self.migrations_moved(phase),
            (Some(Step::Switch), _) => {
                let back = pull.apps.iter().all(|(key, before)| {
                    self.state(key)
                        .is_none_or(|state| !state.is_running() || (state.generation > *before && state.is_up()))
                });
                if back {
                    self.pull_finished();
                }
            }
            _ => {}
        }
    }

    /// dotnet ef is done finding the contexts (they're each updated next) or applying what was pending.
    fn migrations_moved(&mut self, phase: Migrating) {
        let keys: Vec<String> = self.contexts.iter().flatten().map(|context| context.key()).collect();
        if phase == Migrating::Finding {
            if let Some(pull) = self.pull.as_mut() {
                pull.migrating = Some(Migrating::Applying);
            }
            for key in &keys {
                self.migrate(key, None, "applied what was pending after the pull".into());
            }
            return;
        }
        let failed: Vec<String> = self
            .contexts
            .iter()
            .flatten()
            .filter_map(|context| context.run.as_ref()?.failure().map(|why| format!("{}: {why}", context.name)))
            .collect();
        if let Some(why) = failed.first() {
            return self.pull_failed(Step::Migrations, why.clone());
        }
        let now = (self.now)();
        if let Some(pull) = self.pull.as_mut() {
            let run = pull.step_mut(Step::Migrations);
            let plural = if keys.len() == 1 { "" } else { "s" };
            (run.state, run.ended, run.detail) = (State::Done, Some(now), format!("{} context{plural}", keys.len()));
        }
        self.after_migrations();
    }

    /// The pull stopped at `step`: why, said; the source untouched. A copy that's whole (its scripts or migrations
    /// failed) stays, its connection string added - and the one in use put back, if the migrations step changed it.
    /// An update's new copy is dropped before its swap, so the linked database is just as it was; a failed swap has
    /// put every name back.
    pub(super) fn pull_failed(&mut self, step: Step, why: String) {
        let now = (self.now)();
        let Some(pull) = self.pull.as_mut().filter(|pull| !pull.is_over()) else { return };
        let cancelled = pull.cancelled || why == CANCELLED;
        let run = pull.step_mut(step);
        (run.state, run.ended) = (State::Failed, Some(now));
        (pull.error, pull.ended, pull.cancelled) = (Some((step, why.clone())), Some(now), cancelled);
        let (mode, source, server, database, before) =
            (pull.mode, pull.source.clone(), pull.server.clone(), pull.database.clone(), pull.before.take());
        let imported = pull.step(Step::Import).state == State::Done;
        let mut kept = String::new();
        if let (Some(before), Some(path)) = (before, self.secrets_file()) {
            let _ = secrets::switch(&path, &before);
            self.refresh_db_label();
        }
        match mode {
            Mode::Update if step == Step::Swap => {
                kept = format!(" - the names are as they were; {database} stays (Del drops it)");
            }
            Mode::Update if imported => {
                kept = format!(" - its new copy {database} is dropped; the linked database is as it was");
                self.pull_drops.push((server.clone(), database.clone()));
            }
            Mode::Pull | Mode::Import if matches!(step, Step::AfterPull | Step::Migrations | Step::Switch) => {
                self.new_entry(&server, &database);
                kept = format!(" - {database} stays, its connection string added (Del drops it)");
            }
            _ => {}
        }
        let what = mode.name();
        let said = if cancelled {
            format!("■ The {what} from {source} was cancelled at {}{kept}", step.name())
        } else {
            let first = why.lines().next().unwrap_or_default();
            format!("✗ The {what} from {source} failed at {}: {first}{kept}", step.name())
        };
        self.add_log(said, if cancelled { Hue::Yellow } else { Hue::Red });
        self.measure(&server);
        self.refresh_files();
        self.drop_next_pulled();
    }

    /// The step running is done, and the pull with it.
    pub(super) fn last_step_done(&mut self, step: Step, detail: String) -> Option<String> {
        let now = (self.now)();
        let pull = self.pull.as_mut().filter(|pull| !pull.is_over())?;
        let run = pull.step_mut(step);
        if run.state != State::Skipped {
            (run.state, run.ended) = (State::Done, Some(now));
            if !detail.is_empty() {
                run.detail = detail;
            }
        }
        pull.ended = Some(now);
        Some(crate::run::format_duration(now.saturating_duration_since(pull.started).as_secs_f64()))
    }

    /// A pull, import or export went well: kept as one (a new database linked to its twin), the older copies
    /// `keepPulled` would drop found, and said.
    fn pull_finished(&mut self) {
        let Some(pull) = self.pull.as_ref().filter(|pull| !pull.is_over()) else { return };
        let (mode, file) = (pull.mode, pull.file.clone());
        let restarted = pull.apps.len();
        let (last, detail) = match mode {
            Mode::Pull => (Step::Switch, format!("{restarted} API{} restarted", if restarted == 1 { "" } else { "s" })),
            Mode::Import => (Step::AfterPull, String::new()),
            _ => (Step::Export, String::new()),
        };
        let Some(took) = self.last_step_done(last, detail) else { return };
        let Some(pull) = self.pull.as_ref() else { return };
        let twin = pull.twin();
        if mode == Mode::Export {
            let name = std::path::Path::new(&file).file_name().map(|name| name.to_string_lossy().into_owned());
            self.add_log(
                format!("✓ Exported {twin} in {took}: {} - o shows it in its folder", name.unwrap_or_default()),
                Hue::Green,
            );
            self.refresh_files();
            return;
        }
        let whole = self.source(&pull.source).is_some_and(|source| source.is_server());
        let pulled = Pulled {
            source: pull.source.clone(),
            server: pull.server.clone(),
            database: pull.database.clone(),
            at: crate::time::now_rfc3339(),
            remote: whole.then(|| pull.remote.clone()),
        };
        let remote = pull.remote.clone();
        self.pulled.push(pulled.clone());
        if let Err(error) = pull::save(&self.state_dir, &self.pulled) {
            self.add_log(format!("! Couldn't keep the list of pulls ({error})"), Hue::Yellow);
        }
        self.db_links.retain(|link| !link.of(&pulled.server, &pulled.database));
        self.db_links.push(crate::run::db_links::Link {
            server: pulled.server.clone(),
            database: pulled.database.clone(),
            source: pulled.source.clone(),
            remote,
            pulled: Some(pulled.at.clone()),
        });
        self.keep_links();
        let said = match mode {
            Mode::Import => {
                self.new_entry(&pulled.server, &pulled.database);
                format!(
                    "✓ Imported {twin} in {took} as {} @ {}, linked to it - Enter switches to it",
                    pulled.database, pulled.server
                )
            }
            _ => format!(
                "✓ Pulled {} in {took}: {} @ {} is the one in use, linked to {twin} - o opens the folder with its .bacpac",
                pulled.source, pulled.database, pulled.server
            ),
        };
        self.add_log(said, Hue::Green);
        self.offer_older(&pulled, None);
        self.measure(&pulled.server);
        self.refresh_files();
    }

    /// The older copies to offer for dropping after `pulled`: `keepPulled`'s, else an update's old copy.
    pub(super) fn offer_older(&mut self, pulled: &Pulled, old: Option<Pulled>) {
        let keep = self.keep_pulled();
        self.pull_stale = match (keep, old) {
            (Some(keep), _) => pull::beyond_kept(&self.pulled, &pulled.source, keep, |copy| {
                copy.database.eq_ignore_ascii_case(&pulled.database)
                    || self.can_drop(&copy.server, &copy.database).is_err()
            }),
            (None, Some(old)) => vec![old],
            (None, None) => Vec::new(),
        };
        if !self.pull_stale.is_empty() {
            let names: Vec<&str> = self.pull_stale.iter().map(|old| old.database.as_str()).collect();
            let why = if keep.is_some() { "keepPulled: " } else { "" };
            self.add_log(
                format!(
                    "{why}{} older cop{} ({}) - the database screen offers to drop {}",
                    names.len(),
                    if names.len() == 1 { "y" } else { "ies" },
                    names.join(", "),
                    if names.len() == 1 { "it" } else { "them" }
                ),
                Hue::Gray,
            );
        }
    }

    /// The yes to `keepPulled`'s offer: the older copies dropped one after another, as `Del` drops one.
    pub fn drop_pulled(&mut self, copies: &[Pulled]) -> Result<String, String> {
        self.pull_stale.clear();
        self.pull_drops = copies.iter().map(|copy| (copy.server.clone(), copy.database.clone())).collect();
        let names: Vec<&str> = copies.iter().map(|copy| copy.database.as_str()).collect();
        self.drop_next_pulled();
        Ok(format!("drop {}", names.join(", ")))
    }

    /// The next older copy to drop, when no database job runs.
    pub(super) fn drop_next_pulled(&mut self) {
        if self.database_busy().is_some() || self.pull_drops.is_empty() {
            return;
        }
        let (server, database) = self.pull_drops.remove(0);
        if let Err(why) = self.drop_database(&server, &database) {
            self.add_log(format!("✗ Couldn't drop {database}: {why}"), Hue::Red);
            self.drop_next_pulled();
        }
    }

    /// A dropped database is no longer a pulled copy, nor linked.
    pub(super) fn forget_pulled(&mut self, server: &str, database: &str) {
        let before = self.pulled.len();
        self.pulled
            .retain(|pull| !(pull.server.eq_ignore_ascii_case(server) && pull.database.eq_ignore_ascii_case(database)));
        if self.pulled.len() != before {
            let _ = pull::save(&self.state_dir, &self.pulled);
        }
        if self.link_of(server, database).is_some() {
            self.db_links.retain(|link| !link.of(server, database));
            self.keep_links();
        }
    }
}

/// A word with its first letter capital.
fn capital(word: &str) -> String {
    let mut chars = word.chars();
    chars.next().map_or_else(String::new, |first| first.to_uppercase().chain(chars).collect())
}
