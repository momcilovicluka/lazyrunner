//! Save points and rewinding (#110), in the background: one database job at a time - making a save point, rewinding
//! to one, dropping one - and the tables counted for what changed since. A rewind stops the .NET apps on the database,
//! takes it for itself (`SINGLE_USER`), restores it and starts them again; a start meanwhile waits for it. While a save
//! point is pinned, a restart rewinds to it first (`"rewindPinned"`: the restarts you ask for, or every one).

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use tokio::sync::mpsc::UnboundedSender;

use super::Controller;
use crate::output::Hue;
use crate::run::database_files::{Chore, Done, Progress};
use crate::run::save_points::{self as points, Kind, News, SavePoint, Table};
use crate::run::sql::{self, Sql};
use crate::run::{Event, secrets};

/// `"rewindPinned"`: which restarts rewind to a pinned save point first.
pub const REWIND_PINNED: &str = "rewindPinned";
pub const EVERY_RESTART: &str = "everyRestart";
pub const MANUAL_RESTARTS: &str = "manualRestarts";

/// What a database job does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Doing {
    Making(Kind),
    Rewinding(String),
    Dropping(String),
    /// A copy, backup, export, import or drop of the database.
    Database(Chore),
}

/// The database job running: what, on which database, since when, its share done as the server last said it (the one
/// before, the newest, when it came), the apps it stopped and starts again, and a restart's start that waits for it.
pub struct DbWork {
    pub job: u64,
    pub doing: Doing,
    pub server: String,
    pub database: String,
    pub started: Instant,
    pub share: Option<(f64, f64, Instant)>,
    /// A pinned save point's rewind, on a restart.
    pub pinned: bool,
    pub stopped: Vec<String>,
    then_start: Option<(Option<Vec<String>>, bool)>,
    /// A new save point's id and label.
    pub made: Option<(String, String)>,
    /// A copy's, backup's, export's or import's progress as it reports it.
    pub progress: Progress,
}

/// The last database job done: what, on which database, when it ran, whether it went well, the apps it started again.
#[derive(Clone, Debug)]
pub struct DbDone {
    pub doing: Doing,
    pub server: String,
    pub database: String,
    pub started: Instant,
    pub ended: Instant,
    pub ok: bool,
    pub apps: Vec<String>,
    /// The share done the server last reported, as the job had it.
    pub share: Option<(f64, f64, Instant)>,
    /// A copy's, backup's, export's or import's last progress, and what it made.
    pub progress: Progress,
    pub made: Option<Done>,
}

/// A database's tables as last counted (and the job counting them again).
#[derive(Clone, Debug, Default)]
pub struct Counts {
    pub tables: Vec<Table>,
    pub at: Option<Instant>,
    pub error: Option<String>,
    pub reading: Option<u64>,
}

/// An answer that comes from a thread: `None` until it's there.
pub type Pending<T> = Arc<Mutex<Option<T>>>;

/// Who else is connected to a database, found in the background (or why it couldn't be).
pub type Sessions = Pending<Result<Vec<String>, String>>;

/// How a database is known among the counts: its server and name, ignoring case.
pub fn db_key(server: &str, database: &str) -> String {
    format!("{}|{}", server.to_lowercase(), database.to_lowercase())
}

/// Whether a snapshot is one of lazyrunner's save points of `database`.
pub(super) fn ours(snapshot: &str, database: &str) -> bool {
    snapshot.to_lowercase().starts_with(&format!("{}{}", database.to_lowercase(), points::NAME_PART))
}

impl Controller {
    /// The secrets.json the connection strings are in.
    pub fn secrets_file(&self) -> Option<PathBuf> {
        self.secrets_at.clone().or_else(|| secrets::secrets_path(&self.root, &self.apps))
    }

    pub(super) fn secrets_text(&self) -> Option<String> {
        std::fs::read_to_string(self.secrets_file()?).ok()
    }

    /// The connection string to `database` on `server`, password and all: for running SQL, never to show.
    pub(super) fn connection(&self, server: &str, database: &str) -> Result<String, String> {
        let text = self.secrets_text().ok_or_else(|| "no secrets.json with its connection strings".to_string())?;
        secrets::connection_for(&text, server, database)
            .ok_or_else(|| format!("no connection string for {database} @ {server} in secrets.json"))
    }

    /// The .NET apps on a database: those that use the database, when it's the one in use.
    pub fn apps_on(&self, server: &str, database: &str) -> Vec<String> {
        let text = self.secrets_text().unwrap_or_default();
        let in_use = secrets::connections(&text).iter().any(|entry| {
            entry.active && entry.server.eq_ignore_ascii_case(server) && entry.database.eq_ignore_ascii_case(database)
        });
        if !in_use {
            return Vec::new();
        }
        self.database_users().into_iter().filter(|key| self.app(key).is_some_and(|app| app.is_dotnet())).collect()
    }

    /// The apps whose projects use a database, read from them when nothing has yet (as the stack map does).
    pub fn know_database_apps(&mut self) {
        if self.database_apps.is_empty() {
            self.find_configured_calls();
        }
    }

    /// What the database job does, when one runs: a database job runs one at a time.
    pub fn database_busy(&self) -> Option<String> {
        let Some(work) = self.db_work.as_ref() else { return self.pull_busy() };
        let what = match &work.doing {
            Doing::Making(kind) => format!("making a {} save point of {}", kind.name(), work.database),
            Doing::Rewinding(id) => format!("rewinding {} to {}", work.database, self.save_point_label(id)),
            Doing::Dropping(id) => format!("dropping {}", self.save_point_label(id)),
            Doing::Database(chore) => chore.describe(&work.database),
        };
        Some(what)
    }

    fn save_point_label(&self, id: &str) -> String {
        self.save_point(id).map_or_else(|| id.to_string(), |point| format!("\"{}\"", point.label))
    }

    pub fn save_point(&self, id: &str) -> Option<&SavePoint> {
        self.save_points.iter().find(|point| point.id == id)
    }

    /// The database's save points, the newest first.
    pub fn save_points_of(&self, server: &str, database: &str) -> Vec<&SavePoint> {
        points::newest_first(&self.save_points, server, database)
    }

    /// A database's tables as last counted.
    pub fn counts(&self, server: &str, database: &str) -> Option<&Counts> {
        self.db_counts.get(&db_key(server, database))
    }

    pub(super) fn keep_save_points(&mut self) {
        if let Err(error) = points::save(&self.state_dir, &self.save_points) {
            self.add_log(format!("! Couldn't keep the save points ({error})"), Hue::Yellow);
        }
    }

    pub(super) fn new_db_job(&mut self) -> u64 {
        self.next_job += 1;
        self.next_job
    }

    pub(super) fn ready_for_sql(&self) -> Result<(), String> {
        self.sql.ready()?;
        match self.database_busy() {
            Some(busy) => Err(format!("lazyrunner is {busy} - one database job at a time")),
            None => Ok(()),
        }
    }

    /// `s` / `S`: makes a save point of `database` on `server` in the background - labelled `label`, else by its
    /// time; its label.
    pub fn make_save_point(
        &mut self,
        server: &str,
        database: &str,
        kind: Kind,
        label: Option<&str>,
    ) -> Result<String, String> {
        self.ready_for_sql()?;
        let connection = self.connection(server, database)?;
        let at = jiff::ZonedDateTime::now();
        let (id, by_time) = points::name(database, &at);
        let label = label.map(clean_label).filter(|label| !label.is_empty()).unwrap_or(by_time);
        let job = self.new_db_job();
        let point = SavePoint {
            id: id.clone(),
            label: label.clone(),
            kind,
            database: database.to_string(),
            server: server.to_string(),
            created: crate::time::rfc3339(&at),
            snapshot: None,
            backup: None,
            pinned: false,
            tables: Vec::new(),
            gone: false,
        };
        let (sql, events) = (Arc::clone(&self.sql), self.events.clone());
        std::thread::spawn(move || {
            let result = make(sql.as_ref(), &connection, point, &|share| say(&events, job, News::Share(share)));
            say(&events, job, News::Made(result));
        });
        self.db_work = Some(self.work(job, Doing::Making(kind), server, database));
        if let Some(work) = self.db_work.as_mut() {
            work.made = Some((id, label.clone()));
        }
        let how = if kind == Kind::Snapshot { "a snapshot" } else { "a backup - it takes a while" };
        self.add_log(format!("Making save point \"{label}\" of {database} @ {server} ({how})…"), Hue::Cyan);
        Ok(label)
    }

    pub(super) fn work(&self, job: u64, doing: Doing, server: &str, database: &str) -> DbWork {
        DbWork {
            job,
            doing,
            server: server.to_string(),
            database: database.to_string(),
            started: (self.now)(),
            share: None,
            pinned: false,
            stopped: Vec::new(),
            then_start: None,
            made: None,
            progress: Progress::default(),
        }
    }

    /// The save points a rewind to `id` loses: SQL Server reverts from a database's one snapshot only, and restores
    /// a backup over one with none - so the database's other snapshots go first.
    pub fn lost_by_rewind(&self, id: &str) -> Vec<&SavePoint> {
        let Some(target) = self.save_point(id) else { return Vec::new() };
        self.save_points
            .iter()
            .filter(|point| point.id != id && point.kind == Kind::Snapshot && !point.gone)
            .filter(|point| point.of(&target.server, &target.database))
            .collect()
    }

    /// `r`: rewinds a database to save point `id` in the background, its apps stopped meanwhile and started again
    /// after; `then_start` is a restart's start that waits for it. What it says it does.
    pub fn rewind_to(&mut self, id: &str, then_start: Option<(Option<Vec<String>>, bool)>) -> Result<String, String> {
        self.ready_for_sql()?;
        self.know_database_apps();
        let point = self.save_point(id).cloned().ok_or_else(|| format!("no save point {id}"))?;
        if point.gone {
            return Err(format!("\"{}\"'s snapshot is no longer on the server", point.label));
        }
        let connection = self.connection(&point.server, &point.database)?;
        let stopping: Vec<String> =
            self.apps_on(&point.server, &point.database).into_iter().filter(|key| self.is_running(key)).collect();
        if !stopping.is_empty() {
            self.stop(Some(&stopping), true);
        }
        let job = self.new_db_job();
        let (sql, events) = (Arc::clone(&self.sql), self.events.clone());
        let target = point.clone();
        std::thread::spawn(move || {
            let (dropped, result) =
                rewind(sql.as_ref(), &connection, &target, &|share| say(&events, job, News::Share(share)));
            say(&events, job, News::Rewound { dropped, result });
        });
        let mut back = stopping.clone();
        if let Some((only, _)) = &then_start {
            let restarting = |key: &String| only.as_ref().is_none_or(|only| only.contains(key));
            let on = self.apps_on(&point.server, &point.database);
            back.extend(on.into_iter().filter(|key| restarting(key) && !stopping.contains(key)));
        }
        let mut work = self.work(job, Doing::Rewinding(id.to_string()), &point.server, &point.database);
        work.pinned = then_start.is_some();
        work.stopped = back;
        work.then_start = then_start;
        self.db_work = Some(work);
        let stopped =
            if stopping.is_empty() { String::new() } else { format!(" - {} stopped meanwhile", self.names(&stopping)) };
        let what = format!("rewind {} @ {} to \"{}\" ({})", point.database, point.server, point.label, point.clock());
        self.add_log(format!("⟲ Rewinding {}{stopped}…", &what["rewind ".len()..]), Hue::Cyan);
        Ok(what)
    }

    /// While a rewind runs, a start of an app on its database waits for it (joining its start after); whether it
    /// waits.
    pub(super) fn start_waits_for_rewind(&mut self, only: Option<&[String]>, force_build: bool) -> bool {
        let Some(work) = self.db_work.as_ref().filter(|work| matches!(work.doing, Doing::Rewinding(_))) else {
            return false;
        };
        let on = self.apps_on(&work.server, &work.database);
        let starting = only.map_or_else(|| self.keys(), <[String]>::to_vec);
        if !starting.iter().any(|key| on.contains(key)) {
            return false;
        }
        let names = self.names(&starting);
        let Some(work) = self.db_work.as_mut() else { return false };
        work.then_start = Some(match work.then_start.take() {
            None => (only.map(<[String]>::to_vec), force_build),
            Some((None, forced)) => (None, forced || force_build),
            Some((Some(_), forced)) if only.is_none() => (None, forced || force_build),
            Some((Some(mut keys), forced)) => {
                keys.extend(starting.into_iter().filter(|key| !keys.contains(key)).collect::<Vec<_>>());
                (Some(keys), forced || force_build)
            }
        });
        self.add_log(format!("{names} start once the rewind is done"), Hue::DarkGray);
        true
    }

    /// A restart: when a save point of the database in use is pinned and the restart is one `"rewindPinned"` names,
    /// it rewinds there first and the restart's start waits for it; whether it does.
    pub(super) fn rewind_pinned_first(&mut self, by_hand: bool, only: Option<&[String]>, force_build: bool) -> bool {
        let every = self.config[REWIND_PINNED].as_str() == Some(EVERY_RESTART);
        if !by_hand && !every {
            return false;
        }
        if !self.save_points.iter().any(|point| point.pinned && !point.gone) {
            return false;
        }
        self.know_database_apps();
        let pinned = self
            .save_points
            .iter()
            .find(|point| point.pinned && !point.gone && !self.apps_on(&point.server, &point.database).is_empty());
        let Some(point) = pinned.cloned() else { return false };
        let on = self.apps_on(&point.server, &point.database);
        let restarting = only.map_or_else(|| self.keys(), <[String]>::to_vec);
        if !restarting.iter().any(|key| on.contains(key)) {
            return false;
        }
        match self.rewind_to(&point.id, Some((only.map(<[String]>::to_vec), force_build))) {
            Ok(_) => true,
            Err(why) => {
                self.add_log(format!("! Didn't rewind to the pinned \"{}\": {why}", point.label), Hue::Yellow);
                false
            }
        }
    }

    /// `R`: pins save point `id` (a restart rewinds to it first), or unpins it; a database has one pinned at most.
    /// Whether it's pinned now.
    pub fn pin_save_point(&mut self, id: &str) -> Result<bool, String> {
        let point = self.save_point(id).cloned().ok_or_else(|| format!("no save point {id}"))?;
        let pinning = !point.pinned;
        for other in &mut self.save_points {
            if other.of(&point.server, &point.database) {
                other.pinned = pinning && other.id == id;
            }
        }
        self.keep_save_points();
        let when = if self.config[REWIND_PINNED].as_str() == Some(EVERY_RESTART) {
            "every restart"
        } else {
            "a restart you start"
        };
        let said = if pinning {
            format!("⟲ Pinned \"{}\": {when} rewinds {} to it first", point.label, point.database)
        } else {
            format!("Unpinned \"{}\"", point.label)
        };
        self.add_log(said, Hue::Cyan);
        Ok(pinning)
    }

    /// Renames save point `id`.
    pub fn rename_save_point(&mut self, id: &str, label: &str) -> Result<(), String> {
        let label = clean_label(label);
        if label.is_empty() {
            return Err("a save point needs a name".into());
        }
        let point =
            self.save_points.iter_mut().find(|point| point.id == id).ok_or_else(|| format!("no save point {id}"))?;
        point.label = label;
        self.keep_save_points();
        Ok(())
    }

    /// `Del`: forgets save point `id` - a snapshot is dropped on the server; a backup's file stays where it is. What it
    /// says it does.
    pub fn forget_save_point(&mut self, id: &str) -> Result<String, String> {
        let point = self.save_point(id).cloned().ok_or_else(|| format!("no save point {id}"))?;
        let Some(snapshot) = point.snapshot.clone().filter(|_| point.kind == Kind::Snapshot && !point.gone) else {
            self.save_points.retain(|other| other.id != id);
            self.keep_save_points();
            let file = point.backup.map_or_else(String::new, |file| format!(" - its file stays: {file}"));
            let said = format!("Forgot save point \"{}\"{file}", point.label);
            self.add_log(said.clone(), Hue::Gray);
            return Ok(said);
        };
        self.ready_for_sql()?;
        let connection = self.connection(&point.server, &point.database)?;
        let job = self.new_db_job();
        let (sql, events) = (Arc::clone(&self.sql), self.events.clone());
        let id = id.to_string();
        let dropping = id.clone();
        std::thread::spawn(move || {
            let result = sql.run(&connection, Some(MASTER), &points::drop_sql(&snapshot), &mut |_| {});
            say(&events, job, News::Dropped { id: dropping, result });
        });
        self.db_work = Some(self.work(job, Doing::Dropping(id), &point.server, &point.database));
        Ok(format!("drop save point \"{}\" (its snapshot)", point.label))
    }

    /// Counts a database's tables again in the background, and looks which of its snapshots are still there; the
    /// checksums of the tables whose count is as `compare` has it.
    pub fn count_tables(&mut self, server: &str, database: &str) {
        let key = db_key(server, database);
        if self.db_counts.get(&key).is_some_and(|counts| counts.reading.is_some()) {
            return;
        }
        let connection = self.sql.ready().and_then(|()| self.connection(server, database));
        let connection = match connection {
            Ok(connection) => connection,
            Err(why) => {
                self.db_counts.entry(key).or_default().error = Some(why);
                return;
            }
        };
        let compare: Vec<Table> = self
            .save_points_of(server, database)
            .first()
            .map(|point| point.tables.iter().filter(|table| table.checksum.is_some()).cloned().collect())
            .unwrap_or_default();
        let job = self.new_db_job();
        self.db_counts.entry(key).or_default().reading = Some(job);
        let (sql, events) = (Arc::clone(&self.sql), self.events.clone());
        let (server, database) = (server.to_string(), database.to_string());
        std::thread::spawn(move || {
            let result = count_now(sql.as_ref(), &connection, &database, &compare);
            say(&events, job, News::Counted { server, database, result });
        });
    }

    /// Who else is connected to a database - not lazyrunner's own apps - each as "program on host"; found in the
    /// background.
    pub fn other_sessions(&self, server: &str, database: &str) -> Sessions {
        let pending: Sessions = Arc::default();
        let answer = Arc::clone(&pending);
        let found = self.connection(server, database);
        let roots: Vec<u32> = self.states.iter().filter_map(|state| Some(state.child.as_ref()?.pid)).collect();
        let (sql, database) = (Arc::clone(&self.sql), database.to_string());
        std::thread::spawn(move || {
            let result = found.and_then(|connection| sessions(sql.as_ref(), &connection, &database, &roots));
            if let Ok(mut slot) = answer.lock() {
                *slot = Some(result);
            }
        });
        pending
    }

    /// The database job's news.
    pub(super) fn sql_news(&mut self, job: u64, news: News) {
        let now = (self.now)();
        match news {
            News::Share(share) => {
                if let Some(work) = self.db_work.as_mut().filter(|work| work.job == job) {
                    let before = work.share.map_or(0.0, |(_, newest, _)| newest);
                    work.share = Some((before, share, now));
                }
            }
            News::Made(result) => {
                let Some(work) = self.finish_work(job, result.is_ok()) else { return };
                match result {
                    Ok(point) => {
                        let tables = point.tables.len();
                        let key = db_key(&point.server, &point.database);
                        let counts = Counts { tables: point.tables.clone(), at: Some(now), error: None, reading: None };
                        self.db_counts.insert(key, counts);
                        self.add_log(
                            format!(
                                "✓ Save point \"{}\" of {} @ {} ({}, {tables} tables) - r rewinds to it",
                                point.label,
                                point.database,
                                point.server,
                                point.kind.name()
                            ),
                            Hue::Green,
                        );
                        self.save_points.push(point);
                        self.keep_save_points();
                    }
                    Err(why) => {
                        let label = work.made.map_or_else(String::new, |(_, label)| format!(" \"{label}\""));
                        self.add_log(format!("✗ The save point{label} of {} failed: {why}", work.database), Hue::Red);
                    }
                }
            }
            News::Rewound { dropped, result } => self.rewound(job, dropped, result),
            News::Dropped { id, result } => {
                if self.finish_work(job, result.is_ok()).is_none() {
                    return;
                }
                let label = self.save_point_label(&id);
                match result {
                    Ok(()) => {
                        self.save_points.retain(|point| point.id != id);
                        self.keep_save_points();
                        self.add_log(format!("Dropped save point {label} (its snapshot)"), Hue::Gray);
                    }
                    Err(why) => self.add_log(format!("✗ Couldn't drop save point {label}: {why}"), Hue::Red),
                }
            }
            News::Counted { server, database, result } => self.counted(job, &server, &database, result, now),
        }
    }

    /// The job is over: kept as the last one done, and what it was.
    pub(super) fn finish_work(&mut self, job: u64, ok: bool) -> Option<DbWork> {
        let work = self.db_work.take_if(|work| work.job == job)?;
        let apps = work.stopped.clone();
        self.db_done = Some(DbDone {
            doing: work.doing.clone(),
            server: work.server.clone(),
            database: work.database.clone(),
            started: work.started,
            ended: (self.now)(),
            ok,
            apps,
            share: work.share,
            progress: work.progress.clone(),
            made: None,
        });
        Some(work)
    }

    fn rewound(&mut self, job: u64, dropped: Vec<String>, result: Result<(), String>) {
        let Some(mut work) = self.finish_work(job, result.is_ok()) else { return };
        let lost: Vec<String> = self
            .save_points
            .iter()
            .filter(|point| point.snapshot.as_ref().is_some_and(|snapshot| dropped.contains(snapshot)))
            .map(|point| format!("\"{}\"", point.label))
            .collect();
        if !dropped.is_empty() {
            self.save_points.retain(|point| point.snapshot.as_ref().is_none_or(|snapshot| !dropped.contains(snapshot)));
            self.keep_save_points();
        }
        let Doing::Rewinding(id) = &work.doing else { return };
        let label = self.save_point_label(id);
        let took = crate::run::format_duration((self.now)().saturating_duration_since(work.started).as_secs_f64());
        let dropped = if lost.is_empty() { String::new() } else { format!(" ({} dropped first)", lost.join(", ")) };
        match &result {
            Ok(()) => self.add_log(format!("✓ {} is back at {label} in {took}{dropped}", work.database), Hue::Green),
            Err(why) => self.add_log(format!("✗ Rewinding {} failed: {why}", work.database), Hue::Red),
        }
        let (server, database) = (work.server.clone(), work.database.clone());
        let stopped = std::mem::take(&mut work.stopped);
        match work.then_start.take() {
            Some((None, force_build)) => self.start(None, force_build),
            Some((Some(mut keys), force_build)) => {
                keys.extend(stopped.into_iter().filter(|key| !keys.contains(key)).collect::<Vec<_>>());
                self.start(Some(&keys), force_build);
            }
            None if !stopped.is_empty() => self.start(Some(&stopped), false),
            None => {}
        }
        self.count_tables(&server, &database);
    }

    fn counted(
        &mut self,
        job: u64,
        server: &str,
        database: &str,
        result: Result<(Vec<Table>, Vec<String>), String>,
        now: Instant,
    ) {
        let counts = self.db_counts.entry(db_key(server, database)).or_default();
        if counts.reading != Some(job) {
            return;
        }
        counts.reading = None;
        let present = match result {
            Ok((tables, present)) => {
                (counts.tables, counts.at, counts.error) = (tables, Some(now), None);
                present
            }
            Err(why) => {
                counts.error = Some(why);
                return;
            }
        };
        let present: HashSet<String> = present.into_iter().map(|name| name.to_lowercase()).collect();
        let mut changed = false;
        for point in self.save_points.iter_mut().filter(|point| point.of(server, database)) {
            let Some(snapshot) = point.snapshot.as_ref().filter(|_| point.kind == Kind::Snapshot) else { continue };
            let gone = !present.contains(&snapshot.to_lowercase());
            if gone != point.gone {
                point.gone = gone;
                changed = true;
            }
        }
        if changed {
            self.keep_save_points();
        }
    }
}

/// A save point's name as typed, without control characters, at most [`LABEL_MOST`] long.
fn clean_label(label: &str) -> String {
    label.chars().filter(|c| !c.is_control()).take(LABEL_MOST).collect::<String>().trim().to_string()
}

/// A save point's name is at most this long.
const LABEL_MOST: usize = 60;

/// The server's own database, where lazyrunner's SQL runs when it mustn't be in the one it changes.
pub(super) const MASTER: &str = "master";

fn say(events: &UnboundedSender<Event>, job: u64, news: News) {
    let _ = events.send(Event::Sql { job, news });
}

/// Makes `point` on the server: a snapshot beside the data files, or a backup in the server's backup folder; then
/// its tables counted, the small ones' checksums too.
fn make(sql: &dyn Sql, connection: &str, mut point: SavePoint, share: &dyn Fn(f64)) -> Result<SavePoint, String> {
    let database = point.database.clone();
    let counted_in = match point.kind {
        Kind::Snapshot => {
            let files: Vec<(String, String)> =
                sql::query(sql, connection, Some(MASTER), &points::files_sql(&database), 2)?
                    .into_iter()
                    .filter_map(|row| Some((row.first()?.clone(), row.get(1)?.clone())))
                    .collect();
            if files.is_empty() {
                return Err(format!("{database}'s data files couldn't be read (sys.master_files)"));
            }
            sql.run(connection, Some(MASTER), &points::snapshot_sql(&database, &point.id, &files), &mut |_| {})?;
            point.snapshot = Some(point.id.clone());
            point.id.clone()
        }
        Kind::Backup => {
            let folder = sql::query(sql, connection, Some(MASTER), points::BACKUP_FOLDER_SQL, 1)?
                .into_iter()
                .next()
                .and_then(|row| row.into_iter().next())
                .filter(|folder| !folder.is_empty() && folder != "NULL")
                .ok_or_else(|| "the server has no default backup folder".to_string())?;
            let path = points::backup_path(&folder, &point.id);
            let mut on_line = |line: &str| {
                if let Some(done) = sql::percent(line) {
                    share(done);
                }
            };
            sql.run(connection, Some(MASTER), &points::backup_sql(&database, &path), &mut on_line)?;
            point.backup = Some(path);
            database.clone()
        }
    };
    let tables = points::read_counts(&sql::query(sql, connection, Some(&counted_in), points::COUNTS_SQL, 3)?);
    let small = points::small(&tables);
    let sums = match points::checksums_sql(&small) {
        Some(text) => sql::query(sql, connection, Some(&counted_in), &text, 3)?,
        None => Vec::new(),
    };
    point.tables = points::with_checksums(tables, &sums);
    Ok(point)
}

/// Rewinds `point`'s database to it: the database's other save point snapshots dropped (SQL Server needs that),
/// then, in one batch, the database taken for itself, restored, and given back - given back on a failure too. The
/// snapshots dropped, and how it went.
fn rewind(
    sql: &dyn Sql,
    connection: &str,
    point: &SavePoint,
    share: &dyn Fn(f64),
) -> (Vec<String>, Result<(), String>) {
    let database = &point.database;
    let present = match sql::query(sql, connection, Some(MASTER), &points::snapshots_sql(database), 1) {
        Ok(rows) => rows.into_iter().filter_map(|row| row.into_iter().next()).collect::<Vec<String>>(),
        Err(why) => return (Vec::new(), Err(why)),
    };
    let keep = point.snapshot.as_deref().filter(|_| point.kind == Kind::Snapshot);
    let mut dropped = Vec::new();
    for snapshot in present.iter().filter(|name| ours(name, database) && Some(name.as_str()) != keep) {
        if let Err(why) = sql.run(connection, Some(MASTER), &points::drop_sql(snapshot), &mut |_| {}) {
            return (dropped, Err(why));
        }
        dropped.push(snapshot.clone());
    }
    let restore = match (point.kind, &point.snapshot, &point.backup) {
        (Kind::Snapshot, Some(snapshot), _) => points::restore_snapshot_sql(database, snapshot),
        (Kind::Backup, _, Some(path)) => points::restore_backup_sql(database, path),
        _ => return (dropped, Err(format!("\"{}\" has nothing to restore from", point.label))),
    };
    let batch = format!("{} {restore} {}", points::single_user_sql(database), points::multi_user_sql(database));
    let mut on_line = |line: &str| {
        if let Some(done) = sql::percent(line) {
            share(done);
        }
    };
    let result = sql.run(connection, Some(MASTER), &batch, &mut on_line);
    if result.is_err() {
        let _ = sql.run(connection, Some(MASTER), &points::multi_user_sql(database), &mut |_| {});
    }
    (dropped, result)
}

/// A database's tables now and its snapshots on the server; the checksums of those in `compare` with the same count.
fn count_now(
    sql: &dyn Sql,
    connection: &str,
    database: &str,
    compare: &[Table],
) -> Result<(Vec<Table>, Vec<String>), String> {
    let present = sql::query(sql, connection, Some(MASTER), &points::snapshots_sql(database), 1)?
        .into_iter()
        .filter_map(|row| row.into_iter().next())
        .collect();
    let tables = points::read_counts(&sql::query(sql, connection, Some(database), points::COUNTS_SQL, 3)?);
    let same: Vec<&Table> = tables
        .iter()
        .filter(|table| {
            compare.iter().any(|old| old.schema == table.schema && old.name == table.name && old.rows == table.rows)
        })
        .collect();
    let sums = match points::checksums_sql(&same) {
        Some(text) => sql::query(sql, connection, Some(database), &text, 3)?,
        None => Vec::new(),
    };
    Ok((points::with_checksums(tables, &sums), present))
}

/// Who else is connected to `database`: not the processes under `roots` (lazyrunner's apps).
fn sessions(sql: &dyn Sql, connection: &str, database: &str, roots: &[u32]) -> Result<Vec<String>, String> {
    let rows = sql::query(sql, connection, Some(MASTER), &points::sessions_sql(database), 4)?;
    let own = crate::metrics::tree_pids(roots);
    let found: Vec<String> = rows
        .into_iter()
        .filter(|row| row.get(1).and_then(|pid| pid.parse::<u32>().ok()).is_none_or(|pid| !own.contains(&pid)))
        .map(|row| {
            let (host, program) = (row.get(2).cloned().unwrap_or_default(), row.get(3).cloned().unwrap_or_default());
            match (program.is_empty(), host.is_empty()) {
                (false, false) => format!("{program} on {host}"),
                (false, true) => program,
                (true, false) => format!("a session from {host}"),
                (true, true) => format!("session {}", row.first().cloned().unwrap_or_default()),
            }
        })
        .collect();
    let mut counted: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for one in found {
        *counted.entry(one).or_default() += 1;
    }
    Ok(counted.into_iter().map(|(one, many)| if many == 1 { one } else { format!("{one} (×{many})") }).collect())
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use serde_json::json;

    use super::super::tests::controller;
    use super::super::tests::scripted::Scripts;
    use super::*;
    use crate::testing::TempDir;

    /// SQL that answers from a script and keeps what it was asked.
    #[derive(Default)]
    struct Scripted {
        asked: Mutex<Vec<String>>,
    }

    impl Sql for Scripted {
        fn run(
            &self,
            _connection: &str,
            _database: Option<&str>,
            sql: &str,
            line: &mut dyn FnMut(&str),
        ) -> Result<(), String> {
            self.asked.lock().unwrap().push(sql.to_string());
            if sql.contains("source_database_id") {
                line("Shop_lr_20261123090000");
                line("Shop_lr_20261123104200");
                line("Other_snapshot");
            } else if sql.contains("sys.partitions") {
                line("5|dbo|Orders");
            } else if sql.contains("RESTORE") {
                line("50 percent processed.");
            }
            Ok(())
        }

        fn ready(&self) -> Result<(), String> {
            Ok(())
        }
    }

    fn point(id: &str, pinned: bool) -> SavePoint {
        SavePoint {
            id: id.into(),
            label: format!("before {}", &id[id.len() - 6..id.len() - 2]),
            kind: Kind::Snapshot,
            database: "Shop".into(),
            server: "localhost".into(),
            created: "2026-11-23T10:42:00+01:00".into(),
            snapshot: Some(id.into()),
            backup: None,
            pinned,
            tables: vec![Table { schema: "dbo".into(), name: "Orders".into(), rows: 3, checksum: None }],
            gone: false,
        }
    }

    #[tokio::test]
    async fn a_restart_by_hand_rewinds_to_the_pinned_save_point_first_and_starts_once_its_done() {
        let temp = TempDir::new("pinned-rewind");
        let (mut controller, mut rx) = controller(&temp, json!({ "skipUnchangedBuilds": false }));
        controller.commands = Arc::new(Scripts { failing: Vec::new() });
        let secrets = temp.join("secrets.json");
        std::fs::write(
            &secrets,
            "{\n  \"DefaultConnection\": \"Server=localhost;Database=Shop;Trusted_Connection=True\"\n}\n",
        )
        .unwrap();
        controller.secrets_at = Some(secrets);
        let api = controller.apps.iter().find(|app| app.is_dotnet()).map(|app| app.key.clone()).unwrap();
        controller.database_apps = vec![api.clone()];
        let sql = Arc::new(Scripted::default());
        controller.sql = sql.clone();
        controller.save_points = vec![point("Shop_lr_20261123090000", false), point("Shop_lr_20261123104200", true)];

        assert!(
            !controller.rewind_pinned_first(false, Some(std::slice::from_ref(&api)), false),
            "a restart that wasn't asked for doesn't rewind (manualRestarts)"
        );
        controller.restart_from("requested from the panel", Some(std::slice::from_ref(&api)), false, true);
        assert!(controller.database_busy().is_some_and(|busy| busy.contains("rewinding Shop")));
        assert!(!controller.is_running(&api), "the restart waits for the rewind");
        let news = loop {
            match rx.recv().await.unwrap() {
                Event::Sql { job, news: news @ News::Rewound { .. } } => break (job, news),
                _ => continue,
            }
        };
        let asked = sql.asked.lock().unwrap().clone();
        assert!(asked.iter().any(|text| text == "DROP DATABASE [Shop_lr_20261123090000];"), "{asked:?}");
        assert!(!asked.iter().any(|text| text.contains("DROP DATABASE [Other_snapshot]")), "not ours: left alone");
        let batch = asked.iter().find(|text| text.contains("RESTORE")).unwrap();
        assert!(batch.starts_with("ALTER DATABASE [Shop] SET SINGLE_USER") && batch.ends_with("SET MULTI_USER;"));
        assert!(batch.contains("FROM DATABASE_SNAPSHOT = N'Shop_lr_20261123104200'"), "{batch}");
        controller.handle(Event::Sql { job: news.0, news: news.1 });
        assert!(controller.db_work.is_none());
        assert_eq!(controller.save_points.len(), 1, "the other snapshot was dropped: its save point is gone");
        assert!(controller.db_done.as_ref().is_some_and(|done| done.ok));
        assert!(
            controller.log.iter().any(|entry| entry.text.starts_with("✓ Shop is back at \"before 1042\"")),
            "{:?}",
            controller.log
        );
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
        while !controller.is_running(&api) {
            match tokio::time::timeout_at(deadline, rx.recv()).await {
                Ok(Some(event)) => controller.handle(event),
                _ => panic!("{api} didn't start after the rewind: {:?}", controller.log),
            }
        }
    }

    #[test]
    fn a_rewind_loses_the_databases_other_snapshots_and_only_ours_are_dropped() {
        let temp = TempDir::new("rewind-loses");
        let (mut controller, _rx) = controller(&temp, json!({}));
        let mut backup = point("Shop_lr_20261123110000", false);
        backup.kind = Kind::Backup;
        controller.save_points =
            vec![point("Shop_lr_20261123090000", false), point("Shop_lr_20261123104200", false), backup];
        let lost: Vec<&str> =
            controller.lost_by_rewind("Shop_lr_20261123104200").iter().map(|point| point.id.as_str()).collect();
        assert_eq!(lost, ["Shop_lr_20261123090000"]);
        assert_eq!(controller.lost_by_rewind("Shop_lr_20261123110000").len(), 2, "a backup: every snapshot");
        assert!(ours("shop_LR_20261123", "Shop") && !ours("Shop_copy", "Shop"));
    }
}
