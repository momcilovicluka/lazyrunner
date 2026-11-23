//! Copies, backups, exports and imports of a database (#110), and the files they make. `c` copies a database to a new
//! one on its server (a `COPY_ONLY` backup restored `WITH MOVE` to new files), `b` backs it up to a `.bak` in the
//! server's backup folder, `e` exports a `.bacpac` with SqlPackage into lazyrunner's own folder for the project, `i`
//! imports either into a new database - never over one that's there. The SQL, the names, the files listed and how
//! sizes and the time left read are here; the jobs run in the controller.

use std::path::Path;
use std::time::Duration;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::sql::{quote_name, quote_text};

/// The project's state folder keeps the `.bak` files lazyrunner wrote in servers' backup folders here, and its own
/// files (exports, files you put there to import) in this folder.
pub const FILE: &str = "database-files.json";
pub const FOLDER: &str = "databases";
pub const BAK: &str = "bak";
pub const BACPAC: &str = "bacpac";
/// A backup's or export's name: `<database>_<yyyyMMdd_HHmm>`; a copy's backup on the way: `<database>_lr_copy_…`.
const STAMP: &str = "%Y%m%d_%H%M";
const COPY_STAMP: &str = "%Y%m%d%H%M%S";
const COPY_PART: &str = "_lr_copy_";
/// A copy's name until you type another: `<database>_copy`, then `_copy2` and on.
const COPY_SUFFIX: &str = "_copy";
/// The server's own databases: never dropped.
pub const SYSTEM: [&str; 4] = ["master", "model", "msdb", "tempdb"];
/// A new database's name: at most this long, none of these (they'd break a connection string, a path or the SQL).
const NAME_MOST: usize = 100;
const NOT_IN_NAMES: [char; 14] = [';', '"', '\\', '=', '/', ':', '*', '?', '<', '>', '|', '[', ']', '\''];
/// Sizes in steps of this, with these units.
const STEP: f64 = 1024.0;
const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
/// Below this many of a unit, one decimal: `2.4 GB`, `612 MB`.
const DECIMAL_BELOW: f64 = 10.0;
/// The time left shows once this share is done and this long has gone by: the rate is guesswork before.
const ETA_FROM_SHARE: f64 = 0.02;
const ETA_FROM: Duration = Duration::from_secs(1);
/// What SQL Server says when its service account can't open a file.
const DENIED: [&str; 2] = ["Operating system error 5", "Access is denied"];

/// A file lazyrunner lists: a `.bak` or a `.bacpac`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    Bak,
    Bacpac,
}

impl Kind {
    /// The kind a file's name says, by its extension.
    pub fn of(path: &str) -> Option<Self> {
        let extension = Path::new(path).extension()?.to_string_lossy().to_lowercase();
        match extension.as_str() {
            BAK => Some(Self::Bak),
            BACPAC => Some(Self::Bacpac),
            _ => None,
        }
    }
}

/// A `.bak` lazyrunner wrote in a server's backup folder: where, of which database, when, how big.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Made {
    pub path: String,
    pub server: String,
    pub database: String,
    /// When, as RFC 3339.
    pub created: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
}

/// The `.bak` files lazyrunner made, oldest first; none when the file is missing or broken.
pub fn load(state_dir: &Path) -> Vec<Made> {
    match crate::json::read(&state_dir.join(FILE)) {
        Some(Value::Array(items)) => items.into_iter().filter_map(|item| serde_json::from_value(item).ok()).collect(),
        _ => Vec::new(),
    }
}

pub fn save(state_dir: &Path, all: &[Made]) -> Result<()> {
    crate::config::write(&state_dir.join(FILE), &serde_json::to_value(all)?)
}

/// A file in the files section: its path and name, what it is, its size and when it was written, and the server
/// whose backup folder it's in (none: lazyrunner's folder, on this machine).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DbFile {
    pub path: String,
    pub name: String,
    pub kind: Kind,
    pub bytes: Option<u64>,
    pub at: Option<jiff::Timestamp>,
    pub server: Option<String>,
}

impl DbFile {
    /// Its name without the extension: the new database's name an import suggests.
    pub fn stem(&self) -> String {
        Path::new(&self.name).file_stem().map_or_else(|| self.name.clone(), |stem| stem.to_string_lossy().into_owned())
    }

    /// How old it is at `now`.
    pub fn age(&self, now: jiff::Timestamp) -> Option<Duration> {
        let seconds = now.duration_since(self.at?).as_secs().max(0);
        Some(Duration::from_secs(seconds as u64))
    }
}

/// The file's name in a path written either way.
fn name_of(path: &str) -> String {
    path.rsplit(['\\', '/']).next().unwrap_or(path).to_string()
}

/// The files to list: the `.bak` and `.bacpac` files in lazyrunner's `folder`, and the `.bak` files it made in servers'
/// backup folders (`made`) - those it can read here show their size now, the others as they were made. The newest
/// first.
pub fn listing(folder: &Path, made: &[Made]) -> Vec<DbFile> {
    let mut found: Vec<DbFile> = std::fs::read_dir(folder)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let path = entry.path().to_string_lossy().into_owned();
            let kind = Kind::of(&path)?;
            let meta = entry.metadata().ok().filter(std::fs::Metadata::is_file)?;
            let at = meta.modified().ok().and_then(|at| jiff::Timestamp::try_from(at).ok());
            Some(DbFile { name: name_of(&path), path, kind, bytes: Some(meta.len()), at, server: None })
        })
        .collect();
    for file in made {
        if found.iter().any(|known| known.path.eq_ignore_ascii_case(&file.path)) {
            continue;
        }
        let meta = std::fs::metadata(&file.path).ok();
        let at = file.created.parse::<jiff::Timestamp>().ok();
        found.push(DbFile {
            path: file.path.clone(),
            name: name_of(&file.path),
            kind: Kind::Bak,
            bytes: meta.as_ref().map(std::fs::Metadata::len).or(file.bytes),
            at,
            server: Some(file.server.clone()),
        });
    }
    found.sort_by(|a, b| b.at.cmp(&a.at).then_with(|| a.name.cmp(&b.name)));
    found
}

/// A backup's file name: `Shop_20261123_1042.bak`.
pub fn backup_name(database: &str, at: &jiff::ZonedDateTime) -> String {
    format!("{database}_{}.{BAK}", at.strftime(STAMP))
}

/// An export's file name: `Shop_20261123_1042.bacpac`.
pub fn export_name(database: &str, at: &jiff::ZonedDateTime) -> String {
    format!("{database}_{}.{BACPAC}", at.strftime(STAMP))
}

/// The backup a copy restores from, deleted after: `Shop_lr_copy_20261123104207.bak`.
pub fn copy_name(database: &str, at: &jiff::ZonedDateTime) -> String {
    format!("{database}{COPY_PART}{}.{BAK}", at.strftime(COPY_STAMP))
}

/// `wanted` when no database on the server has that name (ignoring case), else `wanted2`, `wanted3`...
pub fn free_name(wanted: &str, taken: &[String]) -> String {
    let free = |name: &str| !taken.iter().any(|other| other.eq_ignore_ascii_case(name));
    if free(wanted) {
        return wanted.to_string();
    }
    (2..).map(|n| format!("{wanted}{n}")).find(|name| free(name)).unwrap_or_else(|| wanted.to_string())
}

/// A copy's name until you type another: `Shop_copy`, `Shop_copy2`...
pub fn copy_default(database: &str, taken: &[String]) -> String {
    free_name(&format!("{database}{COPY_SUFFIX}"), taken)
}

/// A new database's name as typed, trimmed; an error that says why it can't be one.
pub fn valid_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("a new database needs a name".into());
    }
    if name.chars().count() > NAME_MOST {
        return Err(format!("a database name is at most {NAME_MOST} characters here"));
    }
    if name.chars().any(|c| c.is_control() || NOT_IN_NAMES.contains(&c)) {
        return Err(format!("'{name}' can't be a database name here: no {}", NOT_IN_NAMES.iter().collect::<String>()));
    }
    if SYSTEM.iter().any(|system| system.eq_ignore_ascii_case(name)) {
        return Err(format!("{name} is one of the server's own databases"));
    }
    Ok(name.to_string())
}

/// Each database on the server and its size in bytes (`-1`: its files can't be seen), a row each: (bytes, name).
/// Snapshots are left out.
pub const SIZES_SQL: &str = "SET NOCOUNT ON; SELECT ISNULL(SUM(CAST(f.size AS bigint)) * 8192, -1), d.name FROM \
                             sys.databases d LEFT JOIN sys.master_files f ON f.database_id = d.database_id WHERE \
                             d.source_database_id IS NULL GROUP BY d.name;";

/// The databases and their sizes as [`SIZES_SQL`]'s rows give them.
pub fn read_sizes(rows: &[Vec<String>]) -> Vec<(String, Option<u64>)> {
    rows.iter()
        .filter_map(|row| {
            let [bytes, name] = row.as_slice() else { return None };
            Some((name.clone(), bytes.parse::<u64>().ok()))
        })
        .collect()
}

/// Where the server puts new data and log files: (data folder, log folder).
pub const DATA_FOLDERS_SQL: &str = "SET NOCOUNT ON; SELECT CAST(SERVERPROPERTY('InstanceDefaultDataPath') AS \
                                    nvarchar(4000)), CAST(SERVERPROPERTY('InstanceDefaultLogPath') AS nvarchar(4000));";

/// How many tables a database has: what an export's progress counts towards.
pub const TABLES_SQL: &str = "SET NOCOUNT ON; SELECT COUNT(*) FROM sys.tables WHERE is_ms_shipped = 0;";

/// A database's files on its server: (type, logical name, physical path) a row - `0` data, `1` log, `2` FILESTREAM,
/// `4` full-text.
pub fn master_files_sql(database: &str) -> String {
    format!(
        "SET NOCOUNT ON; SELECT type, name, physical_name FROM sys.master_files WHERE database_id = DB_ID({});",
        quote_text(database)
    )
}

/// A backup file's files: (logical name, physical path, type, the rest) a row - `D` data, `L` log, `S` FILESTREAM,
/// `F` full-text.
pub fn file_list_sql(path: &str) -> String {
    format!("RESTORE FILELISTONLY FROM DISK = {};", quote_text(path))
}

/// What a database file is, for where a restore puts it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    Data,
    Log,
    /// FILESTREAM or full-text: a folder, not a file.
    Folder,
}

/// A file of a database: its logical name and what it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DataFile {
    pub logical: String,
    pub part: Part,
}

/// The files [`master_files_sql`]'s rows name.
pub fn read_master_files(rows: &[Vec<String>]) -> Vec<DataFile> {
    rows.iter()
        .filter_map(|row| {
            let [kind, logical, _] = row.as_slice() else { return None };
            let part = match kind.as_str() {
                "0" => Part::Data,
                "1" => Part::Log,
                _ => Part::Folder,
            };
            Some(DataFile { logical: logical.clone(), part })
        })
        .collect()
}

/// The files [`file_list_sql`]'s rows name (read with four columns: the last takes the rest).
pub fn read_file_list(rows: &[Vec<String>]) -> Vec<DataFile> {
    rows.iter()
        .filter_map(|row| {
            let [logical, _, kind, _] = row.as_slice() else { return None };
            let part = match kind.as_str() {
                "D" => Part::Data,
                "L" => Part::Log,
                "S" | "F" => Part::Folder,
                _ => return None,
            };
            Some(DataFile { logical: logical.clone(), part })
        })
        .collect()
}

/// Where a restore as `database` puts each file: `<data folder>\<database>_<logical>.mdf` for the first data file
/// (`.ndf` for the others), `.ldf` in the log folder for the log, a folder for FILESTREAM.
pub fn moves(database: &str, files: &[DataFile], data: &str, log: &str) -> Vec<(String, String)> {
    let mut first_data = true;
    files
        .iter()
        .map(|file| {
            let (folder, extension) = match file.part {
                Part::Data if std::mem::take(&mut first_data) => (data, ".mdf"),
                Part::Data => (data, ".ndf"),
                Part::Log => (log, ".ldf"),
                Part::Folder => (data, ""),
            };
            let name = format!("{database}_{}{extension}", file.logical);
            (file.logical.clone(), super::save_points::in_folder(folder, &name))
        })
        .collect()
}

/// Stops a batch when `database` is already there: lazyrunner never restores over a database.
pub fn guard_sql(database: &str) -> String {
    let said = format!("{database} already exists - lazyrunner never overwrites a database");
    format!("IF DB_ID({}) IS NOT NULL THROW 50000, {}, 1;", quote_text(database), quote_text(&said))
}

/// Whether `database` is there: its id, or 0.
pub fn exists_sql(database: &str) -> String {
    format!("SET NOCOUNT ON; SELECT ISNULL(DB_ID({}), 0);", quote_text(database))
}

/// Restores the backup at `path` as the new database `database`, each file moved (`moves`): stopped first when
/// `database` is there.
pub fn restore_sql(database: &str, path: &str, moves: &[(String, String)]) -> String {
    let moved: Vec<String> =
        moves.iter().map(|(logical, to)| format!("MOVE {} TO {}", quote_text(logical), quote_text(to))).collect();
    format!(
        "{} RESTORE DATABASE {} FROM DISK = {} WITH {}, STATS = 5;",
        guard_sql(database),
        quote_name(database),
        quote_text(path),
        moved.join(", ")
    )
}

/// Deletes a backup file on the server (`xp_delete_file` takes only backup files).
pub fn delete_backup_sql(path: &str) -> String {
    format!("EXECUTE master.sys.xp_delete_file 0, {};", quote_text(path))
}

/// Whether each of `paths` is there on the server: (1 or 0, path) a row.
pub fn present_sql(paths: &[String]) -> Option<String> {
    if paths.is_empty() {
        return None;
    }
    let each: Vec<String> = paths
        .iter()
        .map(|path| format!("SELECT file_exists, {0} FROM sys.dm_os_file_exists({0})", quote_text(path)))
        .collect();
    Some(format!("SET NOCOUNT ON; {};", each.join(" UNION ALL ")))
}

/// How big a backup the server just wrote at `path` is, as msdb's history has it.
pub fn backup_size_sql(path: &str) -> String {
    format!(
        "SET NOCOUNT ON; SELECT TOP 1 b.compressed_backup_size FROM msdb.dbo.backupset b JOIN \
         msdb.dbo.backupmediafamily m ON m.media_set_id = b.media_set_id WHERE m.physical_device_name = {} ORDER BY \
         b.backup_finish_date DESC;",
        quote_text(path)
    )
}

/// Drops a database: everyone else's sessions cut off first.
pub fn drop_database_sql(database: &str) -> String {
    format!("{} DROP DATABASE {};", super::save_points::single_user_sql(database), quote_name(database))
}

/// Whether an error says the server's service account can't read a file.
pub fn denied(error: &str) -> bool {
    DENIED.iter().any(|said| error.contains(said))
}

/// A size as people read it: `2.4 GB`, `612 MB`, `0 B`.
pub fn size(bytes: u64) -> String {
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= STEP && unit + 1 < UNITS.len() {
        value /= STEP;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", UNITS[0])
    } else if value < DECIMAL_BELOW {
        format!("{value:.1} {}", UNITS[unit])
    } else {
        format!("{} {}", value.round() as u64, UNITS[unit])
    }
}

/// How long is left of a job `share` done after `elapsed`, at the rate so far; none while that's guesswork.
pub fn eta(share: f64, elapsed: Duration) -> Option<Duration> {
    if !(ETA_FROM_SHARE..1.0).contains(&share) || elapsed < ETA_FROM {
        return None;
    }
    Some(Duration::from_secs_f64(elapsed.as_secs_f64() * (1.0 - share) / share))
}

/// What a database job does, besides save points (#110).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Chore {
    Copy { to: String },
    Backup,
    Export,
    Import { file: String, to: String },
    Drop,
}

impl Chore {
    /// What it's doing, as the row says it: `backing up`.
    pub fn verb(&self) -> &'static str {
        match self {
            Self::Copy { .. } => "copying",
            Self::Backup => "backing up",
            Self::Export => "exporting",
            Self::Import { .. } => "importing",
            Self::Drop => "dropping",
        }
    }

    /// What it does to `database`, in words: `copying Shop to Shop_copy`.
    pub fn describe(&self, database: &str) -> String {
        match self {
            Self::Copy { to } => format!("copying {database} to {to}"),
            Self::Import { file, to } => format!("importing {file} as {to}"),
            other => format!("{} {database}", other.verb()),
        }
    }

    /// The new database it makes, if it makes one.
    pub fn new_database(&self) -> Option<&str> {
        match self {
            Self::Copy { to } | Self::Import { to, .. } => Some(to),
            _ => None,
        }
    }
}

/// How far a job has come, as it says: its share done, the bytes written so far and of how many, the tables done
/// and of how many and the one it's on, the step it's at, and whether a copy is restoring (its backup is done).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Progress {
    pub share: Option<f64>,
    pub bytes: Option<u64>,
    pub total: Option<u64>,
    pub tables: Option<(usize, usize)>,
    pub table: Option<String>,
    pub step: Option<String>,
    pub restoring: bool,
}

impl Progress {
    /// This, with what `news` says on top.
    pub fn merged(&self, news: Progress) -> Progress {
        Progress {
            share: news.share.or(self.share),
            bytes: news.bytes.or(self.bytes),
            total: news.total.or(self.total),
            tables: news.tables.or(self.tables),
            table: news.table.or_else(|| self.table.clone()),
            step: news.step.or_else(|| self.step.clone()),
            restoring: news.restoring || self.restoring,
        }
    }
}

/// What a job made: a file (and its size), a new database, and a note on its way (a copy's backup deleted or left).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Done {
    pub file: Option<String>,
    pub bytes: Option<u64>,
    pub database: Option<String>,
    pub note: Option<String>,
}

/// The news of a copy, backup, export, import or drop, from its thread; and of the sizes and files around them.
#[derive(Debug)]
pub enum News {
    Progress(Progress),
    Done(Result<Done, String>),
    /// The databases on a server and their sizes, or why they couldn't be read.
    Sized {
        server: String,
        result: Result<Vec<(String, Option<u64>)>, String>,
    },
    /// Which of the `.bak` files lazyrunner made on a server are still there.
    Present {
        server: String,
        result: Result<Vec<(String, bool)>, String>,
    },
    /// A file deleted, or why it couldn't be.
    Deleted {
        path: String,
        result: Result<(), String>,
    },
    /// SqlPackage installed, or why it couldn't be.
    Installed(Result<(), String>),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TempDir;

    fn rows(rows: &[&[&str]]) -> Vec<Vec<String>> {
        rows.iter().map(|row| row.iter().map(|part| part.to_string()).collect()).collect()
    }

    #[test]
    fn a_restore_moves_every_file_to_the_new_databases_own_names() {
        let files = read_master_files(&rows(&[
            &["0", "Shop", r"C:\Data\Shop.mdf"],
            &["1", "Shop_log", r"C:\Data\Shop_log.ldf"],
            &["0", "Shop_2", r"C:\Data\Shop_2.ndf"],
            &["2", "Docs", r"C:\Data\Docs"],
        ]));
        let moved = moves("Shop_copy", &files, r"C:\Data\", r"D:\Logs");
        assert_eq!(
            moved,
            [
                ("Shop".to_string(), r"C:\Data\Shop_copy_Shop.mdf".to_string()),
                ("Shop_log".into(), r"D:\Logs\Shop_copy_Shop_log.ldf".into()),
                ("Shop_2".into(), r"C:\Data\Shop_copy_Shop_2.ndf".into()),
                ("Docs".into(), r"C:\Data\Shop_copy_Docs".into()),
            ]
        );
        let listed = read_file_list(&rows(&[
            &["Shop", r"C:\Old\Shop.mdf", "D", "PRIMARY|8388608|1"],
            &["Shop_log", r"C:\Old\Shop_log.ldf", "L", "NULL|8388608|2"],
        ]));
        assert_eq!(listed.iter().map(|file| file.part).collect::<Vec<_>>(), [Part::Data, Part::Log]);
        let sql = restore_sql("Shop_copy", r"C:\Backup\Shop_lr_copy_1.bak", &moved[..2]);
        assert_eq!(
            sql,
            r"IF DB_ID(N'Shop_copy') IS NOT NULL THROW 50000, N'Shop_copy already exists - lazyrunner never overwrites a database', 1; RESTORE DATABASE [Shop_copy] FROM DISK = N'C:\Backup\Shop_lr_copy_1.bak' WITH MOVE N'Shop' TO N'C:\Data\Shop_copy_Shop.mdf', MOVE N'Shop_log' TO N'D:\Logs\Shop_copy_Shop_log.ldf', STATS = 5;"
        );
        assert!(!sql.contains("REPLACE"), "never over a database that's there");
    }

    #[test]
    fn names_are_quoted_and_guarded_and_drops_cut_everyone_off_first() {
        assert_eq!(
            guard_sql("O'Shop"),
            "IF DB_ID(N'O''Shop') IS NOT NULL THROW 50000, N'O''Shop already exists - lazyrunner never overwrites a \
             database', 1;"
        );
        assert_eq!(exists_sql("Sh]op"), "SET NOCOUNT ON; SELECT ISNULL(DB_ID(N'Sh]op'), 0);");
        assert_eq!(
            drop_database_sql("Sh]op"),
            "ALTER DATABASE [Sh]]op] SET SINGLE_USER WITH ROLLBACK IMMEDIATE; DROP DATABASE [Sh]]op];"
        );
        assert_eq!(delete_backup_sql(r"C:\B\it's.bak"), r"EXECUTE master.sys.xp_delete_file 0, N'C:\B\it''s.bak';");
        assert!(master_files_sql("O'Shop").contains("DB_ID(N'O''Shop')"));
        assert_eq!(file_list_sql(r"C:\B\x.bak"), r"RESTORE FILELISTONLY FROM DISK = N'C:\B\x.bak';");
        let present = present_sql(&[r"C:\B\a.bak".into(), r"C:\B\b.bak".into()]).unwrap();
        assert!(present.contains(" UNION ALL ") && present.contains(r"sys.dm_os_file_exists(N'C:\B\b.bak')"));
        assert_eq!(present_sql(&[]), None);
        assert!(backup_size_sql(r"C:\B\it's.bak").contains(r"= N'C:\B\it''s.bak'"));
        assert_eq!(
            read_sizes(&rows(&[&["2576980378", "Shop"], &["-1", "Hidden"], &["x"]])),
            [("Shop".to_string(), Some(2_576_980_378)), ("Hidden".into(), None)]
        );
    }

    #[test]
    fn new_names_are_free_valid_and_never_a_system_database() {
        let taken = vec!["Shop".to_string(), "shop_COPY".into(), "Shop_copy2".into()];
        assert_eq!(copy_default("Shop", &taken), "Shop_copy3");
        assert_eq!(copy_default("Orders", &taken), "Orders_copy");
        assert_eq!(free_name("Orders", &taken), "Orders");
        assert_eq!(valid_name("  Shop_qa ").as_deref(), Ok("Shop_qa"));
        assert!(valid_name("").is_err() && valid_name("a;b").is_err() && valid_name("a]b").is_err());
        assert!(valid_name("MSDB").is_err() && valid_name(&"x".repeat(NAME_MOST + 1)).is_err());
        let at = crate::time::as_written("2026-11-23T10:42:07+01:00").unwrap();
        assert_eq!(backup_name("Shop", &at), "Shop_20261123_1042.bak");
        assert_eq!(export_name("Shop", &at), "Shop_20261123_1042.bacpac");
        assert_eq!(copy_name("Shop", &at), "Shop_lr_copy_20261123104207.bak");
    }

    #[test]
    fn sizes_read_as_people_say_them_and_the_time_left_comes_from_the_rate() {
        assert_eq!(size(0), "0 B");
        assert_eq!(size(1023), "1023 B");
        assert_eq!(size(48 * 1024), "48 KB");
        assert_eq!(size(612 * 1024 * 1024), "612 MB");
        assert_eq!(size(2_576_980_378), "2.4 GB");
        assert_eq!(size(9_900_000), "9.4 MB");
        assert_eq!(eta(0.5, Duration::from_secs(20)), Some(Duration::from_secs(20)));
        assert_eq!(eta(0.64, Duration::from_secs(36)).map(|left| left.as_secs()), Some(20));
        assert_eq!(eta(0.01, Duration::from_secs(20)), None, "too early to say");
        assert_eq!(eta(0.5, Duration::from_millis(500)), None);
        assert_eq!(eta(1.0, Duration::from_secs(9)), None, "done");
        assert!(denied("Cannot open backup device 'C:\\x.bak'. Operating system error 5(Access is denied.)."));
        assert!(!denied("Incorrect syntax"));
    }

    #[test]
    fn the_files_are_lazyrunners_own_and_the_backups_it_made_the_newest_first() {
        let temp = TempDir::new("database-files");
        let folder = temp.join(FOLDER);
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("Shop_20261120_0900.bacpac"), "1234").unwrap();
        std::fs::write(folder.join("Shop.bak"), "12").unwrap();
        std::fs::write(folder.join("notes.txt"), "x").unwrap();
        let made = vec![
            Made {
                path: r"C:\Server\Backup\Shop_20261123_1042.bak".into(),
                server: "localhost".into(),
                database: "Shop".into(),
                created: "2099-01-01T00:00:00Z".into(),
                bytes: Some(2_576_980_378),
            },
            Made {
                path: folder.join("Shop.bak").to_string_lossy().into_owned(),
                server: "localhost".into(),
                database: "Shop".into(),
                created: "2026-11-20T09:00:00Z".into(),
                bytes: None,
            },
        ];
        save(temp.path(), &made).unwrap();
        assert_eq!(load(temp.path()), made);
        let files = listing(&folder, &load(temp.path()));
        let shown: Vec<(&str, Kind, Option<u64>, bool)> =
            files.iter().map(|file| (file.name.as_str(), file.kind, file.bytes, file.server.is_some())).collect();
        assert_eq!(shown[0], ("Shop_20261123_1042.bak", Kind::Bak, Some(2_576_980_378), true), "{shown:?}");
        assert_eq!(shown.len(), 3, "the .txt left out, the one in the folder once: {shown:?}");
        assert!(shown.contains(&("Shop_20261120_0900.bacpac", Kind::Bacpac, Some(4), false)));
        assert_eq!(files[0].stem(), "Shop_20261123_1042");
        let later = "2099-01-01T00:00:30Z".parse().unwrap();
        assert_eq!(files[0].age(later), Some(Duration::from_secs(30)));
        assert!(listing(&temp.join("nowhere"), &[]).is_empty());
    }

    #[test]
    fn progress_keeps_what_it_knew_and_chores_say_what_they_do() {
        let started = Progress { total: Some(10), share: Some(0.1), ..Progress::default() };
        let later = started.merged(Progress { share: Some(0.5), table: Some("Orders".into()), ..Progress::default() });
        assert_eq!((later.share, later.total, later.table.as_deref()), (Some(0.5), Some(10), Some("Orders")));
        assert!(later.merged(Progress { restoring: true, ..Progress::default() }).restoring);
        assert_eq!(Chore::Copy { to: "Shop_copy".into() }.describe("Shop"), "copying Shop to Shop_copy");
        assert_eq!(Chore::Backup.describe("Shop"), "backing up Shop");
        assert_eq!(Chore::Import { file: "a.bak".into(), to: "A".into() }.new_database(), Some("A"));
        assert_eq!(Kind::of(r"C:\x\Shop.BACPAC"), Some(Kind::Bacpac));
        assert_eq!(Kind::of("x.txt"), None);
    }
}
