//! Copies, backups, exports, imports and drops of a database (#110), in the background as the save points' database
//! job: one at a time. A copy or an import makes a new database - never over one that's there - and adds its
//! connection string beside the others; a drop takes the database's save point snapshots and its connection strings
//! with it, and never touches the one in use. Each database's size, the files the jobs make and SqlPackage's install
//! come back here too.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::mpsc::UnboundedSender;

use super::Controller;
use super::database_jobs::{backup, copy, delete_on_server, export, import, remove_database, restore};
use super::save_points::{Doing, MASTER};
use crate::output::Hue;
use crate::run::database_files::{self as files, Chore, DbFile, Done, Kind, Made, News, Progress};
use crate::run::sql;
#[cfg(any(test, feature = "test-support"))]
use crate::run::sql::Sql;
use crate::run::sqlpackage;
#[cfg(any(test, feature = "test-support"))]
use crate::run::sqlpackage::Package;
use crate::run::{Event, secrets};

/// The databases on a server and their sizes as last measured (and the job measuring them again).
#[derive(Clone, Debug, Default)]
pub struct Sizes {
    pub databases: Vec<(String, Option<u64>)>,
    pub at: Option<Instant>,
    pub error: Option<String>,
    pub reading: Option<u64>,
}

fn say(events: &UnboundedSender<Event>, job: u64, news: News) {
    let _ = events.send(Event::DatabaseFiles { job, news });
}

impl Controller {
    /// lazyrunner's own folder of database files for this project: exports, and files put there to import.
    pub fn files_folder(&self) -> PathBuf {
        self.state_dir.join(files::FOLDER)
    }

    /// The files listed again from what's on disk.
    fn list_files(&mut self) {
        self.db_files = files::listing(&self.files_folder(), &self.made_files);
    }

    /// The files listed again, and in the background, which of the backups lazyrunner made on each server are still
    /// there.
    pub fn refresh_files(&mut self) {
        self.list_files();
        let mut servers: Vec<String> = self.made_files.iter().map(|made| made.server.clone()).collect();
        servers.sort_by_key(|server| server.to_lowercase());
        servers.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
        for server in servers {
            let paths: Vec<String> = self
                .made_files
                .iter()
                .filter(|made| made.server.eq_ignore_ascii_case(&server))
                .map(|made| made.path.clone())
                .collect();
            let (Some(text), Ok(connection)) = (files::present_sql(&paths), self.connection_on(&server)) else {
                continue;
            };
            let (sql, events, job) = (Arc::clone(&self.sql), self.events.clone(), self.new_db_job());
            std::thread::spawn(move || {
                let result = sql::query(sql.as_ref(), &connection, Some(MASTER), &text, 2).map(|rows| {
                    rows.into_iter().filter_map(|row| Some((row.get(1)?.clone(), row.first()? == "1"))).collect()
                });
                say(&events, job, News::Present { server, result });
            });
        }
    }

    /// A connection string to `server`, password and all - the one in use first: for SQL that runs in `master`.
    fn connection_on(&self, server: &str) -> Result<String, String> {
        let entry = self.entry_on(server, None).ok_or_else(|| format!("no connection string to {server}"))?;
        self.connection(server, &entry.database)
    }

    /// A connection string's entry: to `database` on `server` (any database when `None`), the one in use first.
    fn entry_on(&self, server: &str, database: Option<&str>) -> Option<secrets::Entry> {
        let mut found: Vec<secrets::Entry> = secrets::connections(&self.secrets_text()?)
            .into_iter()
            .filter(|entry| entry.server.eq_ignore_ascii_case(server))
            .filter(|entry| database.is_none_or(|database| entry.database.eq_ignore_ascii_case(database)))
            .collect();
        found.sort_by_key(|entry| !entry.active);
        found.into_iter().next()
    }

    /// Measures the databases on `server` in the background: their names and sizes.
    pub fn measure(&mut self, server: &str) {
        let key = server.to_lowercase();
        if self.db_sizes.get(&key).is_some_and(|sizes| sizes.reading.is_some()) {
            return;
        }
        let connection = self.sql.ready().and_then(|()| self.connection_on(server));
        let connection = match connection {
            Ok(connection) => connection,
            Err(why) => {
                self.db_sizes.entry(key).or_default().error = Some(why);
                return;
            }
        };
        let job = self.new_db_job();
        self.db_sizes.entry(key).or_default().reading = Some(job);
        let (sql, events, server) = (Arc::clone(&self.sql), self.events.clone(), server.to_string());
        std::thread::spawn(move || {
            let result = sql::query(sql.as_ref(), &connection, Some(MASTER), files::SIZES_SQL, 2)
                .map(|rows| files::read_sizes(&rows));
            say(&events, job, News::Sized { server, result });
        });
    }

    /// The databases on a server as last measured.
    pub fn sizes(&self, server: &str) -> Option<&Sizes> {
        self.db_sizes.get(&server.to_lowercase())
    }

    /// A database's size in bytes, as last measured.
    pub fn size_of(&self, server: &str, database: &str) -> Option<u64> {
        let sizes = self.sizes(server)?;
        sizes.databases.iter().find(|(name, _)| name.eq_ignore_ascii_case(database)).and_then(|(_, bytes)| *bytes)
    }

    /// The names taken on a server: its databases as last measured and those the connection strings name.
    pub fn names_on(&self, server: &str) -> Vec<String> {
        let mut taken: Vec<String> = self
            .sizes(server)
            .map(|sizes| sizes.databases.iter().map(|(name, _)| name.clone()).collect())
            .unwrap_or_default();
        let entries = secrets::connections(&self.secrets_text().unwrap_or_default());
        taken.extend(
            entries.into_iter().filter(|entry| entry.server.eq_ignore_ascii_case(server)).map(|entry| entry.database),
        );
        taken
    }

    /// A copy's name until you type another: `Shop_copy`, else `Shop_copy2`...
    pub fn copy_default(&self, server: &str, database: &str) -> String {
        files::copy_default(database, &self.names_on(server))
    }

    /// An import's name until you type another: the file's name without its extension, made free.
    pub fn import_default(&self, server: &str, file: &DbFile) -> String {
        files::free_name(&file.stem(), &self.names_on(server))
    }

    /// A new database's name, checked: valid, and not on the server already.
    fn new_name(&self, server: &str, name: &str) -> Result<String, String> {
        let name = files::valid_name(name)?;
        if self.names_on(server).iter().any(|taken| taken.eq_ignore_ascii_case(&name)) {
            return Err(format!("{name} is already on {server} - lazyrunner never overwrites a database"));
        }
        Ok(name)
    }

    /// The job starts: what it is, on which database, its size as the total it counts towards.
    fn start_chore(&mut self, job: u64, chore: Chore, server: &str, database: &str) {
        let total = self.size_of(server, database);
        let mut work = self.work(job, Doing::Database(chore), server, database);
        work.progress.total = total;
        self.db_work = Some(work);
    }

    /// `c`: copies `database` on `server` to a new database `to` on the same server, in the background.
    pub fn copy_database(&mut self, server: &str, database: &str, to: &str) -> Result<String, String> {
        self.ready_for_sql()?;
        let to = self.new_name(server, to)?;
        let connection = self.connection(server, database)?;
        let temp = files::copy_name(database, &jiff::ZonedDateTime::now());
        let (job, total) = (self.new_db_job(), self.size_of(server, database));
        let (sql, events, from, into) = (Arc::clone(&self.sql), self.events.clone(), database.to_string(), to.clone());
        std::thread::spawn(move || {
            let report = |progress| say(&events, job, News::Progress(progress));
            let result = copy(sql.as_ref(), &connection, &from, &into, &temp, total, &report);
            say(&events, job, News::Done(result));
        });
        self.start_chore(job, Chore::Copy { to: to.clone() }, server, database);
        let said = format!("copy {database} @ {server} to {to}");
        self.add_log(format!("Copying {database} @ {server} to {to} (a backup, then a restore)…"), Hue::Cyan);
        Ok(said)
    }

    /// `b`: backs `database` up to `<database>_<yyyyMMdd_HHmm>.bak` in the server's backup folder, in the background.
    pub fn backup_database(&mut self, server: &str, database: &str) -> Result<String, String> {
        self.ready_for_sql()?;
        let connection = self.connection(server, database)?;
        let name = files::backup_name(database, &jiff::ZonedDateTime::now());
        let (job, total) = (self.new_db_job(), self.size_of(server, database));
        let (sql, events, of) = (Arc::clone(&self.sql), self.events.clone(), database.to_string());
        std::thread::spawn(move || {
            let report = |progress| say(&events, job, News::Progress(progress));
            let result = backup(sql.as_ref(), &connection, &of, &name, total, &report);
            say(&events, job, News::Done(result));
        });
        self.start_chore(job, Chore::Backup, server, database);
        self.add_log(format!("Backing up {database} @ {server} to the server's backup folder…"), Hue::Cyan);
        Ok(format!("back up {database} @ {server}"))
    }

    /// `e`: exports `database` to a `.bacpac` in lazyrunner's folder with SqlPackage, in the background.
    pub fn export_database(&mut self, server: &str, database: &str) -> Result<String, String> {
        self.ready_for_sql()?;
        self.package.ready()?;
        let connection = self.connection(server, database)?;
        let folder = self.files_folder();
        std::fs::create_dir_all(&folder).map_err(|error| format!("{}: {error}", folder.display()))?;
        let file =
            folder.join(files::export_name(database, &jiff::ZonedDateTime::now())).to_string_lossy().into_owned();
        let job = self.new_db_job();
        let (sql, package, events, private) =
            (Arc::clone(&self.sql), Arc::clone(&self.package), self.events.clone(), self.state_dir.clone());
        let (of, to) = (database.to_string(), file.clone());
        std::thread::spawn(move || {
            let report = |progress| say(&events, job, News::Progress(progress));
            let result = export(sql.as_ref(), package.as_ref(), &connection, &of, &to, &private, &report);
            say(&events, job, News::Done(result));
        });
        self.start_chore(job, Chore::Export, server, database);
        self.add_log(format!("Exporting {database} @ {server} to {file} (SqlPackage)…"), Hue::Cyan);
        Ok(format!("export {database} @ {server} to {file}"))
    }

    /// Whether SqlPackage is there; what installs it when it isn't.
    pub fn sqlpackage_ready(&self) -> Result<(), String> {
        self.package.ready()
    }

    /// The file at `path` in the files section.
    pub fn db_file(&self, path: &str) -> Option<&DbFile> {
        self.db_files.iter().find(|file| file.path.eq_ignore_ascii_case(path))
    }

    /// `i`: imports the file at `path` as the new database `to`, in the background: a `.bak` on the server whose
    /// backup folder it's in, one of lazyrunner's files on `server`.
    pub fn import_file(&mut self, path: &str, server: &str, to: &str) -> Result<String, String> {
        self.ready_for_sql()?;
        let file = self.db_file(path).cloned().ok_or_else(|| format!("no file {path} in the files section"))?;
        let server = file.server.clone().unwrap_or_else(|| server.to_string());
        if file.kind == Kind::Bak && file.server.is_none() && !is_local(&server) {
            return Err(format!(
                "{server} reads a .bak itself and can't see this machine's files - import a .bacpac instead, or put \
                 the .bak in that server's backup folder"
            ));
        }
        if file.kind == Kind::Bacpac {
            self.package.ready()?;
        }
        let to = self.new_name(&server, to)?;
        let connection = self.connection_on(&server)?;
        let job = self.new_db_job();
        let (sql, package, events, private) =
            (Arc::clone(&self.sql), Arc::clone(&self.package), self.events.clone(), self.state_dir.clone());
        let (from, into, kind) = (file.path.clone(), to.clone(), file.kind);
        std::thread::spawn(move || {
            let report = |progress| say(&events, job, News::Progress(progress));
            let result = match kind {
                Kind::Bak => restore(sql.as_ref(), &connection, &from, &into, &report),
                Kind::Bacpac => import(sql.as_ref(), package.as_ref(), &connection, &from, &into, &private, &report),
            };
            say(&events, job, News::Done(result));
        });
        let chore = Chore::Import { file: file.name.clone(), to: to.clone() };
        self.start_chore(job, chore, &server, &to);
        if let Some(work) = self.db_work.as_mut() {
            work.progress.total = file.bytes.filter(|_| kind == Kind::Bak);
        }
        self.add_log(format!("Importing {} as {to} @ {server}…", file.name), Hue::Cyan);
        Ok(format!("import {} as {to} @ {server}", file.name))
    }

    /// Whether `database` on `server` may be dropped: not one of the server's own, not one in use or a running app's,
    /// not while another database job runs.
    pub fn can_drop(&self, server: &str, database: &str) -> Result<(), String> {
        if files::SYSTEM.iter().any(|system| system.eq_ignore_ascii_case(database)) {
            return Err(format!("{database} is one of the server's own databases"));
        }
        let text = self.secrets_text().unwrap_or_default();
        let in_use = secrets::connections(&text).into_iter().any(|entry| {
            entry.active && entry.server.eq_ignore_ascii_case(server) && entry.database.eq_ignore_ascii_case(database)
        });
        if in_use {
            return Err(format!("{database} @ {server} is the one in use - switch to another database first"));
        }
        let running: Vec<String> =
            self.apps_on(server, database).into_iter().filter(|key| self.is_running(key)).collect();
        if !running.is_empty() {
            return Err(format!("{} run on {database}", self.names(&running)));
        }
        match self.database_busy() {
            Some(busy) => Err(format!("lazyrunner is {busy} - one database job at a time")),
            None => Ok(()),
        }
    }

    /// `Del`: drops `database` on `server` in the background - its save point snapshots first, everyone else cut off -
    /// then forgets its save points and removes its connection strings.
    pub fn drop_database(&mut self, server: &str, database: &str) -> Result<String, String> {
        self.sql.ready()?;
        self.can_drop(server, database)?;
        let connection = self.connection(server, database)?;
        let job = self.new_db_job();
        let (sql, events, name) = (Arc::clone(&self.sql), self.events.clone(), database.to_string());
        std::thread::spawn(move || {
            let result = remove_database(sql.as_ref(), &connection, &name);
            say(&events, job, News::Done(result));
        });
        self.start_chore(job, Chore::Drop, server, database);
        self.add_log(format!("Dropping {database} @ {server}…"), Hue::Cyan);
        Ok(format!("drop {database} @ {server}"))
    }

    /// `Del` on a file: one of lazyrunner's is deleted here; a backup in a server's backup folder by the server
    /// (`xp_delete_file`), or here when this machine can reach it.
    pub fn delete_file(&mut self, path: &str) -> Result<String, String> {
        let file = self.db_file(path).cloned().ok_or_else(|| format!("no file {path} in the files section"))?;
        let Some(server) = file.server.clone() else {
            std::fs::remove_file(&file.path).map_err(|error| format!("{}: {error}", file.name))?;
            self.list_files();
            let said = format!("Deleted {}", file.name);
            self.add_log(said.clone(), Hue::Gray);
            return Ok(said);
        };
        self.sql.ready()?;
        let connection = self.connection_on(&server)?;
        let here = is_local(&server);
        let (sql, events, job, gone) =
            (Arc::clone(&self.sql), self.events.clone(), self.new_db_job(), file.path.clone());
        std::thread::spawn(move || {
            let result = delete_on_server(sql.as_ref(), &connection, &gone, here);
            say(&events, job, News::Deleted { path: gone, result });
        });
        Ok(format!("delete {} from {server}'s backup folder", file.name))
    }

    /// Installs SqlPackage as a .NET global tool, in the background: only when you said yes.
    pub fn install_sqlpackage(&mut self) {
        if self.installing_sqlpackage {
            return;
        }
        self.installing_sqlpackage = true;
        let (events, job, dotnet) = (self.events.clone(), self.new_db_job(), crate::run::tools::dotnet());
        std::thread::spawn(move || {
            let mut command = std::process::Command::new(dotnet);
            command.args(sqlpackage::INSTALL).stdin(std::process::Stdio::null());
            let output = crate::run::tools::own_console(&mut command).output();
            let result = match output {
                Ok(output) if output.status.success() => Ok(()),
                Ok(output) => {
                    Err(String::from_utf8_lossy(&output.stderr).lines().next().unwrap_or("failed").to_string())
                }
                Err(error) => Err(error.to_string()),
            };
            say(&events, job, News::Installed(result));
        });
        self.add_log(format!("Installing SqlPackage ({})…", sqlpackage::INSTALL_TEXT), Hue::Cyan);
    }

    /// A copy's, backup's, export's, import's or drop's news, and the sizes and files around them.
    pub(super) fn files_news(&mut self, job: u64, news: News) {
        match news {
            News::Progress(progress) => self.progressed(job, progress),
            News::Done(result) => self.chore_done(job, result),
            News::Sized { server, result } => {
                let sizes = self.db_sizes.entry(server.to_lowercase()).or_default();
                if sizes.reading != Some(job) {
                    return;
                }
                sizes.reading = None;
                match result {
                    Ok(databases) => (sizes.databases, sizes.at, sizes.error) = (databases, Some((self.now)()), None),
                    Err(why) => sizes.error = Some(why),
                }
            }
            News::Present { server, result: Ok(present) } => {
                let gone = |made: &Made| {
                    made.server.eq_ignore_ascii_case(&server)
                        && present.iter().any(|(path, there)| !there && path.eq_ignore_ascii_case(&made.path))
                };
                if self.made_files.iter().any(gone) {
                    self.made_files.retain(|made| !gone(made));
                    self.keep_made_files();
                    self.list_files();
                }
            }
            News::Present { .. } => {}
            News::Deleted { path, result } => match result {
                Ok(()) => {
                    self.made_files.retain(|made| !made.path.eq_ignore_ascii_case(&path));
                    self.keep_made_files();
                    self.list_files();
                    self.add_log(format!("Deleted {path}"), Hue::Gray);
                }
                Err(why) => self.add_log(format!("✗ Couldn't delete {path}: {why}"), Hue::Red),
            },
            News::Installed(Ok(())) => {
                self.installing_sqlpackage = false;
                self.add_log("✓ SqlPackage installed - e exports now", Hue::Green);
            }
            News::Installed(Err(why)) => {
                self.installing_sqlpackage = false;
                self.add_log(
                    format!("✗ SqlPackage didn't install ({why}) - run {} yourself", sqlpackage::INSTALL_TEXT),
                    Hue::Red,
                );
            }
        }
    }

    fn progressed(&mut self, job: u64, progress: Progress) {
        let now = (self.now)();
        let Some(work) = self.db_work.as_mut().filter(|work| work.job == job) else { return };
        if let Some(share) = progress.share {
            let restarted = progress.restoring && !work.progress.restoring;
            let before = if restarted { 0.0 } else { work.share.map_or(0.0, |(_, newest, _)| newest) };
            work.share = Some((before, share, now));
        }
        work.progress = work.progress.merged(progress);
    }

    fn keep_made_files(&mut self) {
        if let Err(error) = files::save(&self.state_dir, &self.made_files) {
            self.add_log(format!("! Couldn't keep the list of backups ({error})"), Hue::Yellow);
        }
    }

    fn chore_done(&mut self, job: u64, result: Result<Done, String>) {
        let Some(work) = self.finish_work(job, result.is_ok()) else { return };
        let Doing::Database(chore) = &work.doing else { return };
        let took = crate::run::format_duration((self.now)().saturating_duration_since(work.started).as_secs_f64());
        let (server, database) = (work.server.clone(), work.database.clone());
        let done = match result {
            Ok(done) => done,
            Err(why) => {
                let what = chore.describe(&database);
                let what = format!("{}{}", what[..1].to_uppercase(), &what[1..]);
                self.add_log(format!("✗ {what} @ {server} failed: {why}"), Hue::Red);
                return;
            }
        };
        if let Some(last) = self.db_done.as_mut() {
            last.made = Some(done.clone());
        }
        let size = done.bytes.map_or_else(String::new, |bytes| format!(", {}", files::size(bytes)));
        match chore {
            Chore::Backup => {
                let path = done.file.clone().unwrap_or_default();
                let created = crate::time::now_rfc3339();
                let made = Made {
                    path: path.clone(),
                    server: server.clone(),
                    database: database.clone(),
                    created,
                    bytes: done.bytes,
                };
                self.made_files.push(made);
                self.keep_made_files();
                self.list_files();
                self.add_log(format!("✓ Backed up {database} @ {server} in {took}{size}: {path}"), Hue::Green);
            }
            Chore::Export => {
                self.list_files();
                let path = done.file.clone().unwrap_or_default();
                self.add_log(format!("✓ Exported {database} @ {server} in {took}{size}: {path}"), Hue::Green);
            }
            Chore::Copy { to } | Chore::Import { to, .. } => {
                let source = if matches!(chore, Chore::Copy { .. }) { Some(database.as_str()) } else { None };
                let added = self.add_connection(&server, source, to);
                let what = match chore {
                    Chore::Copy { .. } => format!("Copied {database} to {to} @ {server}"),
                    _ => format!("Imported {to} @ {server}"),
                };
                let note = done.note.as_ref().map_or_else(String::new, |note| format!(" - {note}"));
                self.add_log(format!("✓ {what} in {took}{note} - {added}"), Hue::Green);
                self.measure(&server);
            }
            Chore::Drop => self.dropped(&server, &database, took),
        }
    }

    /// Adds a connection string to the new database `to`, as `n` does: a copy of `source`'s (else one to its server),
    /// inactive. What it says it did.
    fn add_connection(&mut self, server: &str, source: Option<&str>, to: &str) -> String {
        let entry =
            source.and_then(|source| self.entry_on(server, Some(source))).or_else(|| self.entry_on(server, None));
        let (Some(entry), Some(path)) = (entry, self.secrets_file()) else {
            return "no connection string to copy for it".into();
        };
        match secrets::add(&path, &entry, to) {
            Ok(()) => "its connection string is added (inactive) - Enter switches to it".into(),
            Err(error) => format!("its connection string couldn't be added ({error})"),
        }
    }

    fn dropped(&mut self, server: &str, database: &str, took: String) {
        let before = self.save_points.len();
        self.save_points.retain(|point| !point.of(server, database));
        if self.save_points.len() != before {
            self.keep_save_points();
        }
        self.db_counts.remove(&super::save_points::db_key(server, database));
        let removed = match self.secrets_file().map(|path| secrets::remove(&path, server, database)) {
            Some(Ok(0)) | None => String::new(),
            Some(Ok(1)) => " and its connection string".into(),
            Some(Ok(many)) => format!(" and its {many} connection strings"),
            Some(Err(error)) => format!(" (its connection strings stay: {error})"),
        };
        self.add_log(format!("✓ Dropped {database} @ {server}{removed} in {took}"), Hue::Green);
        self.measure(server);
    }

    /// For the tests of the screen: SQL and SqlPackage that answer from a script, and the secrets.json they use.
    #[cfg(any(test, feature = "test-support"))]
    pub fn use_sql(&mut self, sql: Arc<dyn Sql>, package: Arc<dyn Package>, secrets: PathBuf) {
        (self.sql, self.package, self.secrets_at) = (sql, package, Some(secrets));
    }

    /// For the tests of the screen: `chore` on a database (server, name) as if it had run since `started`.
    #[cfg(any(test, feature = "test-support"))]
    pub fn pretend_chore(
        &mut self,
        chore: Chore,
        (server, database): (&str, &str),
        started: Instant,
        progress: Progress,
    ) {
        let job = self.new_db_job();
        self.start_chore(job, chore, server, database);
        if let Some(work) = self.db_work.as_mut() {
            work.started = started;
            work.share = progress.share.map(|share| (share, share, started));
            work.progress = work.progress.merged(progress);
        }
    }
}

/// Whether a server is this machine: `.`, `localhost`, `(local)`, a loopback address, LocalDB, or this machine's
/// name - with an instance or a port after it, or `tcp:` before.
pub fn is_local(server: &str) -> bool {
    let server = server.trim().trim_start_matches("tcp:").trim_start_matches("np:");
    if server.to_lowercase().starts_with("(localdb)") {
        return true;
    }
    let host = server.split([',', '\\']).next().unwrap_or_default().trim().to_lowercase();
    let machine = std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME")).unwrap_or_default();
    matches!(host.as_str(), "." | "localhost" | "(local)" | "127.0.0.1" | "::1" | "[::1]")
        || (!machine.is_empty() && host.eq_ignore_ascii_case(&machine))
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Mutex;

    use serde_json::json;

    use super::super::tests::controller;
    use super::*;
    use crate::run::sqlpackage::Action;
    use crate::testing::TempDir;

    /// SQL that answers from a script and keeps what it was asked.
    #[derive(Default)]
    struct Scripted {
        asked: Mutex<Vec<String>>,
    }

    impl Sql for Scripted {
        fn run(&self, _: &str, _: Option<&str>, sql: &str, line: &mut dyn FnMut(&str)) -> Result<(), String> {
            self.asked.lock().unwrap().push(sql.to_string());
            if sql.contains("DB_ID(N'Taken')") {
                line("7");
            } else if sql.contains("DB_ID(") && sql.contains("ISNULL") {
                line("0");
            } else if sql.contains("sys.master_files WHERE") {
                line("0|Shop|C:\\Data\\Shop.mdf");
                line("1|Shop_log|C:\\Data\\Shop_log.ldf");
            } else if sql.contains("InstanceDefaultDataPath") {
                line("C:\\Data\\|C:\\Logs\\");
            } else if sql.contains("InstanceDefaultBackupPath") {
                line("C:\\Backup");
            } else if sql.contains("BACKUP DATABASE") || sql.contains("RESTORE DATABASE") {
                line("50 percent processed.");
                line("100 percent processed.");
            } else if sql.contains("dm_os_file_exists") {
                line("0|C:\\Backup\\x.bak");
            } else if sql.contains("source_database_id = DB_ID") {
                line("Shop_copy_lr_20261123104200");
                line("Someone_elses");
            } else if sql.contains("GROUP BY d.name") {
                line("2576980378|Shop");
            }
            Ok(())
        }

        fn ready(&self) -> Result<(), String> {
            Ok(())
        }
    }

    /// SqlPackage that prints a table a time.
    struct Packaged;

    impl Package for Packaged {
        fn run(
            &self,
            _: Action,
            connection: &str,
            _: &str,
            _: &Path,
            line: &mut dyn FnMut(&str),
        ) -> Result<(), String> {
            assert!(connection.contains("Database=Shop_qa"), "{connection}");
            line("Processing Table '[dbo].[Orders]'.");
            line("Processing Table '[dbo].[OrderLines]'.");
            Ok(())
        }

        fn ready(&self) -> Result<(), String> {
            Ok(())
        }
    }

    fn with_secrets(temp: &TempDir, controller: &mut Controller, text: &str) -> Arc<Scripted> {
        let secrets = temp.join("secrets.json");
        std::fs::write(&secrets, text).unwrap();
        let sql = Arc::new(Scripted::default());
        controller.use_sql(sql.clone(), Arc::new(Packaged), secrets);
        sql
    }

    async fn done(rx: &mut tokio::sync::mpsc::UnboundedReceiver<Event>, controller: &mut Controller) {
        loop {
            match rx.recv().await.unwrap() {
                Event::DatabaseFiles { job, news: news @ News::Done(_) } => {
                    controller.files_news(job, news);
                    return;
                }
                Event::DatabaseFiles { job, news } => controller.files_news(job, news),
                _ => continue,
            }
        }
    }

    #[tokio::test]
    async fn a_copy_backs_up_restores_to_new_files_and_adds_its_connection_string() {
        let temp = TempDir::new("copy-database");
        let (mut controller, mut rx) = controller(&temp, json!({}));
        let secrets = "{\n  \"DefaultConnection\": \"Server=localhost;Database=Shop;Trusted_Connection=True\"\n}\n";
        let sql = with_secrets(&temp, &mut controller, secrets);
        assert!(controller.copy_database("localhost", "Shop", "Shop").is_err(), "a name that's taken");
        controller.copy_database("localhost", "Shop", "Shop_copy").unwrap();
        assert!(controller.database_busy().is_some_and(|busy| busy == "copying Shop to Shop_copy"));
        done(&mut rx, &mut controller).await;
        let asked = sql.asked.lock().unwrap().clone();
        let backup = asked
            .iter()
            .position(|text| text.starts_with("BACKUP DATABASE [Shop] TO DISK = N'C:\\Backup\\Shop_lr_copy_"));
        let restore = asked.iter().position(|text| text.contains("RESTORE DATABASE [Shop_copy]"));
        assert!(backup.is_some() && restore > backup, "{asked:?}");
        let restore = &asked[restore.unwrap()];
        assert!(restore.starts_with("IF DB_ID(N'Shop_copy') IS NOT NULL THROW"), "{restore}");
        assert!(restore.contains(r"MOVE N'Shop_log' TO N'C:\Logs\Shop_copy_Shop_log.ldf'"), "{restore}");
        assert!(
            asked
                .iter()
                .any(|text| text.starts_with("EXECUTE master.sys.xp_delete_file 0, N'C:\\Backup\\Shop_lr_copy_"))
        );
        let text = std::fs::read_to_string(temp.join("secrets.json")).unwrap();
        assert!(text.contains("//\"DefaultConnection\": \"Server=localhost;Database=Shop_copy;"), "{text}");
        let said = controller.log.back().unwrap().text.clone();
        assert!(
            said.starts_with("✓ Copied Shop to Shop_copy @ localhost") && said.contains("backup on the way is deleted"),
            "{said}"
        );
        assert!(controller.db_done.as_ref().is_some_and(|done| done.ok && done.made.is_some()));
    }

    #[tokio::test]
    async fn a_drop_takes_our_snapshots_first_and_never_the_database_in_use() {
        let temp = TempDir::new("drop-database");
        let (mut controller, mut rx) = controller(&temp, json!({}));
        let secrets = "{\n  \"DefaultConnection\": \"Server=localhost;Database=Shop;Trusted_Connection=True\",\n  //\"DefaultConnection\": \"Server=localhost;Database=Shop_copy;Trusted_Connection=True\",\n}\n";
        let sql = with_secrets(&temp, &mut controller, secrets);
        assert!(controller.drop_database("localhost", "Shop").unwrap_err().contains("in use"));
        assert!(controller.drop_database("localhost", "master").is_err());
        controller.drop_database("localhost", "Shop_copy").unwrap();
        done(&mut rx, &mut controller).await;
        let asked = sql.asked.lock().unwrap().clone();
        let ours = asked.iter().position(|text| text == "DROP DATABASE [Shop_copy_lr_20261123104200];");
        let database = asked.iter().position(|text| text.ends_with("DROP DATABASE [Shop_copy];"));
        assert!(ours.is_some() && database > ours, "{asked:?}");
        assert!(!asked.iter().any(|text| text.contains("Someone_elses")), "not ours: left alone");
        let text = std::fs::read_to_string(temp.join("secrets.json")).unwrap();
        assert!(!text.contains("Shop_copy") && text.contains("Database=Shop;"), "{text}");
        assert!(temp.join("secrets.json.lazyrunner-backup").is_file(), "the file before is kept");
    }

    #[tokio::test]
    async fn an_import_counts_the_tables_sqlpackage_names_and_a_file_of_lazyrunners_is_deleted_here() {
        let temp = TempDir::new("import-bacpac");
        let (mut controller, mut rx) = controller(&temp, json!({}));
        let secrets = "{\n  \"DefaultConnection\": \"Server=localhost;Database=Shop;Trusted_Connection=True\"\n}\n";
        with_secrets(&temp, &mut controller, secrets);
        let folder = controller.files_folder();
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("Shop_qa.bacpac"), "x").unwrap();
        controller.refresh_files();
        let path = controller.db_files[0].path.clone();
        assert_eq!(controller.import_default("localhost", &controller.db_files[0].clone()), "Shop_qa");
        assert!(controller.import_file(&path, "localhost", "Shop").is_err(), "a name that's taken");
        controller.import_file(&path, "localhost", "Shop_qa").unwrap();
        let mut seen = Vec::new();
        loop {
            match rx.recv().await.unwrap() {
                Event::DatabaseFiles { job, news: News::Progress(progress) } => {
                    seen.push(progress.clone());
                    controller.files_news(job, News::Progress(progress));
                }
                Event::DatabaseFiles { job, news: news @ News::Done(_) } => {
                    controller.files_news(job, news);
                    break;
                }
                _ => {}
            }
        }
        assert!(seen.iter().any(|progress| progress.table.as_deref() == Some("OrderLines")), "{seen:?}");
        assert!(
            controller.log.back().unwrap().text.starts_with("✓ Imported Shop_qa @ localhost"),
            "{:?}",
            controller.log
        );
        controller.delete_file(&path).unwrap();
        assert!(controller.db_files.is_empty() && !Path::new(&path).exists());
    }

    #[test]
    fn a_local_server_is_this_machines() {
        assert!(is_local(".") && is_local("localhost\\SQLEXPRESS") && is_local("tcp:127.0.0.1,1433"));
        assert!(is_local("(localdb)\\MSSQLLocalDB") && is_local("(local)"));
        assert!(!is_local("qa-sql.example.com,1433"));
    }
}
