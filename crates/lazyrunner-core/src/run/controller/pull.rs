//! Pulling a copy of a database from another environment (#110), as a background job while the apps keep running on
//! the database in use: the check, export, import and after-pull scripts in a thread (`pull_steps`), then here the
//! migrations (`migrateAfterPull`: the migrations screen's own `database update`, on the new copy) and the switch
//! (`Enter`'s: its connection string added and made the one in use, the APIs restarted). A source's password comes
//! from the credential store, or as typed; it's never kept anywhere else. `x` cancels; `keepPulled` offers to drop the
//! older copies. One database job at a time.

use std::sync::Arc;
use std::time::Instant;

use tokio::sync::mpsc::UnboundedSender;

use super::Controller;
use super::pull_steps::{self, Plan};
use crate::output::Hue;
use crate::profile::sources::DatabaseSource;
use crate::run::cancel::{CANCELLED, Cancel};
use crate::run::credentials::{self, Password};
use crate::run::database_files::{self as files, Progress};
use crate::run::pull::{self, News, Pulled, State, Step};
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
enum Migrating {
    Finding,
    Applying,
}

/// The pull running, or the last one: from which source (its name and where it is), into which new database on which
/// server, each step, the export's and import's progress, how it failed, and the apps the switch restarts (each with
/// the run it had before, so its new one is told apart).
pub struct PullRun {
    pub job: u64,
    pub source: String,
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
    cancel: Arc<Cancel>,
    migrating: Option<Migrating>,
    /// The connection string in use before the migrations step made the new copy's the one in use.
    before: Option<secrets::Entry>,
}

impl PullRun {
    pub fn step(&self, step: Step) -> &StepRun {
        &self.steps[step.index()]
    }

    fn step_mut(&mut self, step: Step) -> &mut StepRun {
        &mut self.steps[step.index()]
    }

    /// The step running, if one is.
    pub fn running(&self) -> Option<Step> {
        Step::ALL.into_iter().find(|step| self.step(*step).state == State::Running)
    }

    pub fn is_over(&self) -> bool {
        self.ended.is_some()
    }

    pub fn went_well(&self) -> bool {
        self.is_over() && self.error.is_none() && !self.cancelled
    }
}

fn say(events: &UnboundedSender<Event>, job: u64, news: News) {
    let _ = events.send(Event::Pull { job, news });
}

impl Controller {
    /// The profile's sources.
    pub fn pull_sources(&self) -> &[DatabaseSource] {
        &self.project.pulling.sources
    }

    fn pull_source(&self, name: &str) -> Result<DatabaseSource, String> {
        let known: Vec<&str> = self.pull_sources().iter().map(|source| source.name.as_str()).collect();
        self.project.pulling.source(name).cloned().ok_or_else(|| {
            if known.is_empty() {
                "the profile has no databaseSources to pull from".to_string()
            } else {
                format!("no source '{name}' - the profile's: {}", known.join(", "))
            }
        })
    }

    /// What the credential store is called: `Credential Manager`.
    pub fn credential_store(&self) -> &'static str {
        self.credentials.name()
    }

    /// Whether a source's password is kept in the credential store.
    pub fn password_kept(&self, source: &str) -> bool {
        let target = credentials::target(&self.project.id, source);
        self.credentials.get(&target).is_ok_and(|found| found.is_some())
    }

    /// Keeps a source's password in the credential store (never anywhere else); where it went.
    pub fn keep_password(&mut self, source: &str, password: &Password) -> Result<&'static str, String> {
        let source = self.pull_source(source)?;
        let user = match &source.reach {
            crate::profile::sources::Reach::Server { user: Some(user), .. } => user.clone(),
            _ => return Err(format!("{} logs in without a password lazyrunner keeps", source.name)),
        };
        self.credentials.set(&credentials::target(&self.project.id, &source.name), &user, password)?;
        let store = self.credentials.name();
        self.add_log(format!("{}'s password is kept in {store}", source.name), Hue::Gray);
        Ok(store)
    }

    /// Forgets a source's password; what it says.
    pub fn forget_password(&mut self, source: &str) -> Result<String, String> {
        let target = credentials::target(&self.project.id, source);
        let said = match self.credentials.forget(&target)? {
            true => format!("Forgot {source}'s password - the next pull asks for it"),
            false => format!("No password of {source}'s was kept"),
        };
        self.add_log(said.clone(), Hue::Gray);
        Ok(said)
    }

    /// The source `P` pulls from again: the last pull's, else the newest that went well.
    pub fn last_pull_source(&self) -> Option<String> {
        let last = self.pull.as_ref().map(|pull| pull.source.clone());
        last.or_else(|| self.pulled.last().map(|pulled| pulled.source.clone()))
    }

    /// Why a pull of `source` can't start without the user: its password isn't kept, and an agent can't type it.
    pub fn password_missing(&self, source: &str) -> Option<String> {
        let found = self.project.pulling.source(source).filter(|found| found.needs_password())?;
        (!self.password_kept(&found.name)).then(|| {
            format!(
                "{}'s password isn't kept yet - ask the user to pull once from lazyrunner (p on the database screen): \
                 it asks for the password and keeps it in {}",
                found.name,
                self.credentials.name()
            )
        })
    }

    /// Whether a pull is running; what it does then.
    pub(super) fn pull_busy(&self) -> Option<String> {
        let pull = self.pull.as_ref().filter(|pull| !pull.is_over())?;
        Some(format!("pulling a copy of {} into {}", pull.source, pull.database))
    }

    /// `p`: pulls a copy of `source` as a new database on the server in use, in the background - `typed`, the password
    /// just typed (else the one kept). What it says it does.
    pub fn pull_database(&mut self, source: &str, typed: Option<Password>) -> Result<String, String> {
        let source = self.pull_source(source)?;
        self.ready_for_sql()?;
        self.package.ready()?;
        let password = match typed {
            Some(password) => Some(password),
            None if source.needs_password() => {
                let kept = self.credentials.get(&credentials::target(&self.project.id, &source.name))?;
                Some(kept.ok_or_else(|| self.password_missing(&source.name).unwrap_or_default())?)
            }
            None => None,
        };
        let source_connection = pull::connection(&source, password.as_ref(), |name| std::env::var(name).ok())?;
        let reached = sql::target(&source_connection)?;
        let in_use = self.entry_in_use().ok_or_else(|| "no connection string is in use to pull next to".to_string())?;
        if reached.server.eq_ignore_ascii_case(&in_use.server) {
            return Err(format!(
                "{} is the server in use - a pull only reads from a source and never writes to one",
                in_use.server
            ));
        }
        let source_database = reached.database.clone().ok_or_else(|| format!("{} names no database", source.name))?;
        let scripts = self.after_pull_scripts()?;
        let local = self.connection(&in_use.server, &in_use.database)?;
        let database = pull::pulled_name(
            &source_database,
            &source.name,
            &jiff::ZonedDateTime::now(),
            &self.names_on(&in_use.server),
        );
        let database = files::valid_name(&database)?;
        let folder = self.files_folder();
        std::fs::create_dir_all(&folder).map_err(|error| format!("{}: {error}", folder.display()))?;
        let taken: Vec<String> = self.db_files.iter().map(|file| file.stem()).collect();
        let file = folder.join(format!("{}.{}", files::free_name(&database, &taken), files::BACPAC));
        let file = file.to_string_lossy().into_owned();
        let job = self.new_db_job();
        let cancel = Arc::new(Cancel::default());
        let plan = Plan {
            source: source.name.clone(),
            source_database,
            source_connection,
            local,
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
        let apps = self.restarting_apps();
        self.pull = Some(PullRun {
            job,
            source: source.name.clone(),
            from: source.label(),
            server: in_use.server.clone(),
            database: database.clone(),
            file,
            started: (self.now)(),
            ended: None,
            steps: vec![StepRun::default(); Step::ALL.len()],
            progress: Progress::default(),
            share: None,
            error: None,
            cancelled: false,
            apps,
            cancel,
            migrating: None,
            before: None,
        });
        let said = format!("pull {} ({}) into {database} @ {}", source.name, source.label(), in_use.server);
        self.add_log(
            format!("⇣ Pulling {} ({}) into {database} @ {}…", source.name, source.label(), in_use.server),
            Hue::Cyan,
        );
        Ok(said)
    }

    /// `P`: pulls again from the last source.
    pub fn pull_again(&mut self) -> Result<String, String> {
        let source = self.last_pull_source().ok_or_else(|| "nothing pulled yet - p pulls from a source".to_string())?;
        self.pull_database(&source, None)
    }

    /// `x`: cancels the pull's running step. The source is untouched; a half-imported database is dropped.
    pub fn cancel_pull(&mut self) -> Result<String, String> {
        let pull = self.pull.as_mut().filter(|pull| !pull.is_over()).ok_or_else(|| "no pull is running".to_string())?;
        pull.cancelled = true;
        pull.cancel.cancel();
        let running = pull.running();
        let said = match running {
            Some(Step::Switch) => "the switch is done - the APIs restart on the new copy".to_string(),
            Some(step) => format!("cancelling the pull at {}", step.name()),
            None => "cancelling the pull".to_string(),
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
    fn entry_in_use(&self) -> Option<secrets::Entry> {
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

    /// The running APIs that read the connection strings, each with its run now: what the switch restarts.
    fn restarting_apps(&self) -> Vec<(String, u64)> {
        self.apps
            .iter()
            .filter(|app| app.traits().uses_secrets && self.is_running(&app.key))
            .filter_map(|app| Some((app.key.clone(), self.state(&app.key)?.generation)))
            .collect()
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
            News::Ended(Ok(())) => self.start_migrations_step(),
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
            self.start_switch_step();
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
        let apps = self.restarting_apps();
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
        self.start_switch_step();
    }

    /// The pull stopped at `step`: why, said; the source untouched. A copy that's whole (its scripts or migrations
    /// failed) stays, its connection string added - and the one in use put back, if the migrations step changed it.
    fn pull_failed(&mut self, step: Step, why: String) {
        let now = (self.now)();
        let Some(pull) = self.pull.as_mut().filter(|pull| !pull.is_over()) else { return };
        let cancelled = pull.cancelled || why == CANCELLED;
        let run = pull.step_mut(step);
        (run.state, run.ended) = (State::Failed, Some(now));
        (pull.error, pull.ended, pull.cancelled) = (Some((step, why.clone())), Some(now), cancelled);
        let (source, server, database, before) =
            (pull.source.clone(), pull.server.clone(), pull.database.clone(), pull.before.take());
        let mut kept = String::new();
        if matches!(step, Step::AfterPull | Step::Migrations | Step::Switch) {
            self.new_entry(&server, &database);
            kept = format!(" - {database} stays, its connection string added (Del drops it)");
            if let (Some(before), Some(path)) = (before, self.secrets_file()) {
                let _ = secrets::switch(&path, &before);
                self.refresh_db_label();
            }
        }
        let said = if cancelled {
            format!("■ The pull from {source} was cancelled at {}{kept}", step.name())
        } else {
            let first = why.lines().next().unwrap_or_default();
            format!("✗ The pull from {source} failed at {}: {first}{kept}", step.name())
        };
        self.add_log(said, if cancelled { Hue::Yellow } else { Hue::Red });
        self.measure(&server);
        self.refresh_files();
    }

    /// The pull went well: kept as one, the older copies `keepPulled` would drop found, and said.
    fn pull_finished(&mut self) {
        let now = (self.now)();
        let Some(pull) = self.pull.as_mut().filter(|pull| !pull.is_over()) else { return };
        let restarted = pull.apps.len();
        let run = pull.step_mut(Step::Switch);
        (run.state, run.ended) = (State::Done, Some(now));
        run.detail = format!("{restarted} API{} restarted", if restarted == 1 { "" } else { "s" });
        pull.ended = Some(now);
        let took = crate::run::format_duration(now.saturating_duration_since(pull.started).as_secs_f64());
        let pulled = Pulled {
            source: pull.source.clone(),
            server: pull.server.clone(),
            database: pull.database.clone(),
            at: crate::time::now_rfc3339(),
        };
        self.pulled.push(pulled.clone());
        if let Err(error) = pull::save(&self.state_dir, &self.pulled) {
            self.add_log(format!("! Couldn't keep the list of pulls ({error})"), Hue::Yellow);
        }
        self.add_log(
            format!(
                "✓ Pulled {} in {took}: {} @ {} is the one in use - o opens the folder with its .bacpac",
                pulled.source, pulled.database, pulled.server
            ),
            Hue::Green,
        );
        self.pull_stale = self.keep_pulled().map_or_else(Vec::new, |keep| {
            pull::beyond_kept(&self.pulled, &pulled.source, keep, |old| {
                old.database.eq_ignore_ascii_case(&pulled.database)
                    || self.can_drop(&old.server, &old.database).is_err()
            })
        });
        if !self.pull_stale.is_empty() {
            let names: Vec<&str> = self.pull_stale.iter().map(|old| old.database.as_str()).collect();
            self.add_log(
                format!(
                    "keepPulled: {} older pulled copies ({}) - the database screen offers to drop them",
                    names.len(),
                    names.join(", ")
                ),
                Hue::Gray,
            );
        }
        self.measure(&pulled.server);
        self.refresh_files();
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

    /// A dropped database is no longer a pulled copy.
    pub(super) fn forget_pulled(&mut self, server: &str, database: &str) {
        let before = self.pulled.len();
        self.pulled
            .retain(|pull| !(pull.server.eq_ignore_ascii_case(server) && pull.database.eq_ignore_ascii_case(database)));
        if self.pulled.len() != before {
            let _ = pull::save(&self.state_dir, &self.pulled);
        }
    }

    /// For the tests: the credential store a pull reads its passwords from.
    #[cfg(any(test, feature = "test-support"))]
    pub fn use_credentials(&mut self, store: Arc<dyn credentials::Store>) {
        self.credentials = store;
    }

    /// For the tests of the screen: a pull as if it had run since `started`, its steps where `states` says, the
    /// running one's progress `progress`.
    #[cfg(any(test, feature = "test-support"))]
    pub fn pretend_pull(
        &mut self,
        (source, database): (&str, &str),
        started: Instant,
        states: &[State],
        progress: Progress,
    ) {
        let job = self.new_db_job();
        let mut steps = vec![StepRun::default(); Step::ALL.len()];
        for (at, state) in states.iter().enumerate() {
            steps[at].state = *state;
            if *state != State::Waiting {
                steps[at].started = Some(started);
            }
        }
        let apps = self.apps.iter().filter(|app| app.traits().uses_secrets).map(|app| (app.key.clone(), 0)).collect();
        let source = self.project.pulling.source(source).map_or_else(|| source.to_string(), |found| found.name.clone());
        self.pull = Some(PullRun {
            job,
            from: self.project.pulling.source(&source).map(DatabaseSource::label).unwrap_or_default(),
            source,
            server: "localhost".into(),
            database: database.into(),
            file: String::new(),
            started,
            ended: None,
            steps,
            share: progress.share.map(|share| (share, share, started)),
            progress,
            error: None,
            cancelled: false,
            apps,
            cancel: Arc::default(),
            migrating: None,
            before: None,
        });
    }

    /// For the tests of the screen: the pretended pull's step `step` failed (`why`) or, without one, all done.
    #[cfg(any(test, feature = "test-support"))]
    pub fn pretend_pull_over(&mut self, failed: Option<(Step, &str)>, at: Instant) {
        let Some(pull) = self.pull.as_mut() else { return };
        pull.ended = Some(at);
        for (index, run) in pull.steps.iter_mut().enumerate() {
            run.ended = run.ended.or(run.started.map(|_| at));
            if failed.is_none() {
                run.state = State::Done;
                run.started = run.started.or(Some(pull.started));
                run.ended = Some(pull.started + std::time::Duration::from_secs(10 * (index as u64 + 1)));
            }
        }
        if let Some((step, why)) = failed {
            pull.step_mut(step).state = State::Failed;
            pull.error = Some((step, why.to_string()));
        }
    }
}
