//! Updating a linked database from its remote twin (#114), the pull's `Mode::Update`: its twin exported and imported
//! beside it as a new database, the after-pull scripts and migrations run on that (`controller::pull`), then the swap:
//! the apps on the linked database stopped, the old one renamed `<name>_old_<yyyyMMdd>` and the new one given its name
//! in one batch (`pull::swap_sql`), the apps started again. Nothing is renamed or dropped before the new copy is fully
//! in; a failed swap puts every name back.

use std::sync::Arc;

use super::Controller;
use super::pull::{Launch, say};
use crate::output::Hue;
use crate::run::credentials::Password;
use crate::run::pull::{self, Mode, News, Pulled, State, Step};
use crate::run::{secrets, sql};

impl Controller {
    /// `U`: updates the linked `database` on `server` from its twin - its new copy imported beside it, the after-pull
    /// scripts and migrations run on that, then swapped in under its name (the old one kept as
    /// `<name>_old_<yyyyMMdd>`). Nothing is renamed or dropped before the new copy is fully in.
    pub fn update_linked(&mut self, server: &str, database: &str, typed: Option<Password>) -> Result<String, String> {
        let link = self
            .link_of(server, database)
            .cloned()
            .ok_or_else(|| format!("{database} isn't linked to a remote database - L links it"))?;
        let source = self.known_source(&link.source)?;
        self.ready_for_sql()?;
        self.package.ready()?;
        let password = self.password_for(&source, typed)?;
        let connection =
            pull::connection_to(&source, Some(&link.remote), password.as_ref(), |name| std::env::var(name).ok())?;
        let reached = sql::target(&connection)?;
        if reached.server.eq_ignore_ascii_case(server) {
            return Err(format!("{server} is {}'s own server - an update never writes to a source", source.name));
        }
        if !self.names_on(server).iter().any(|name| name.eq_ignore_ascii_case(database)) && self.sizes(server).is_some()
        {
            return Err(format!("{database} isn't on {server} any more"));
        }
        let local = self.connection(server, database).or_else(|_| self.server_connection(server))?;
        let launch = Launch {
            mode: Mode::Update,
            source,
            remote: link.remote.clone(),
            connection,
            server: server.to_string(),
            local,
            target: Some(database.to_string()),
        };
        self.launch(launch)
    }

    /// The swap: the connection string in use put back (the migrations step made the new copy's the one in use) and
    /// the new copy's taken out - the linked database keeps its own, and its name; the apps on the linked database
    /// stopped; then, in one batch, the old database renamed `<name>_old_<yyyyMMdd>` and the new one given its name.
    pub(super) fn start_swap_step(&mut self) {
        let Some(pull) = self.pull.as_mut() else { return };
        let (server, database, target, before) =
            (pull.server.clone(), pull.database.clone(), pull.target.clone().unwrap_or_default(), pull.before.take());
        let local = pull.local.clone();
        if let Some(path) = self.secrets_file() {
            if let Some(before) = before {
                let _ = secrets::switch(&path, &before);
            }
            let _ = secrets::remove(&path, &server, &database);
            self.refresh_db_label();
        }
        let old = pull::old_name(&target, &jiff::ZonedDateTime::now(), &self.names_on(&server));
        let apps = self.pull.as_ref().map(|pull| self.restarting_apps(pull)).unwrap_or_default();
        let stopping: Vec<String> = apps.iter().map(|(key, _)| key.clone()).collect();
        if !stopping.is_empty() {
            self.stop(Some(&stopping), true);
        }
        let now = (self.now)();
        let Some(pull) = self.pull.as_mut() else { return };
        (pull.apps, pull.stopped, pull.old) = (apps, stopping.clone(), Some(old.clone()));
        let run = pull.step_mut(Step::Swap);
        (run.state, run.started) = (State::Running, Some(now));
        let job = pull.job;
        let (sql, events) = (Arc::clone(&self.sql), self.events.clone());
        let batch = pull::swap_sql(&target, &database, &old);
        std::thread::spawn(move || {
            let result = sql.run(&local, Some(pull::MASTER), &batch, &mut |_| {});
            say(&events, job, News::Swapped(result));
        });
        let stopped =
            if stopping.is_empty() { String::new() } else { format!(" - {} stopped meanwhile", self.names(&stopping)) };
        self.add_log(format!("⇄ Swapping {database} in as {target}{stopped}…"), Hue::Cyan);
    }

    /// The swap's batch is done: the update finished, or - the names put back - failed; the apps it stopped start
    /// again either way.
    pub(super) fn swapped(&mut self, result: Result<(), String>) {
        let stopped = self.pull.as_mut().map(|pull| std::mem::take(&mut pull.stopped)).unwrap_or_default();
        match result {
            Ok(()) => self.update_finished(),
            Err(why) => self.pull_failed(Step::Swap, why),
        }
        if !stopped.is_empty() {
            self.start(Some(&stopped), false);
        }
    }

    /// An update went well: the linked database is the new copy under its name, its age now; the old copy offered
    /// for dropping (or kept, as `keepPulled` says).
    fn update_finished(&mut self) {
        let Some(pull) = self.pull.as_ref().filter(|pull| !pull.is_over()) else { return };
        let (target, old) = (pull.target.clone().unwrap_or_default(), pull.old.clone().unwrap_or_default());
        let detail = format!("{old} kept");
        let Some(took) = self.last_step_done(Step::Swap, detail) else { return };
        let Some(pull) = self.pull.as_ref() else { return };
        let (server, source, twin, remote) =
            (pull.server.clone(), pull.source.clone(), pull.twin(), pull.remote.clone());
        let now = crate::time::now_rfc3339();
        let before = self.link_of(&server, &target).and_then(|link| link.pulled.clone());
        if let Some(link) = self.db_links.iter_mut().find(|link| link.of(&server, &target)) {
            link.pulled = Some(now.clone());
        }
        self.keep_links();
        let whole = self.source(&source).is_some_and(|found| found.is_server());
        let remote = whole.then_some(remote);
        self.pulled.retain(|known| {
            !(known.server.eq_ignore_ascii_case(&server) && known.database.eq_ignore_ascii_case(&target))
        });
        let old_copy = Pulled {
            source: source.clone(),
            server: server.clone(),
            database: old.clone(),
            at: before.unwrap_or_else(|| now.clone()),
            remote: remote.clone(),
        };
        let new_copy = Pulled { source, server: server.clone(), database: target.clone(), at: now, remote };
        self.pulled.extend([old_copy.clone(), new_copy.clone()]);
        if let Err(error) = pull::save(&self.state_dir, &self.pulled) {
            self.add_log(format!("! Couldn't keep the list of pulls ({error})"), Hue::Yellow);
        }
        self.add_log(
            format!("✓ Updated {target} @ {server} from {twin} in {took} - the old copy is {old}"),
            Hue::Green,
        );
        self.offer_older(&new_copy, Some(old_copy));
        self.measure(&server);
        self.refresh_files();
    }
}
