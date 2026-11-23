//! Save points of a database (#110): a SQL Server database snapshot (`s`: copy-on-write, made in a moment) or a
//! `COPY_ONLY` backup in the server's backup folder (`S`: slower, as many as you like). Each keeps the tables' row
//! counts - and a checksum of the small ones - as they were, so what changed since shows without opening the
//! snapshot. They're kept in the project's state folder; the SQL that makes, rewinds to and drops them is here.

use std::path::Path;

use anyhow::Result;
use jiff::ZonedDateTime;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::sql::{quote_name, quote_text};

/// The file in the project's state folder.
pub const FILE: &str = "save-points.json";
/// What a save point's snapshot database or backup file is named: `<database>_lr_<yyyyMMddHHmmss>`.
pub const NAME_PART: &str = "_lr_";
const STAMP: &str = "%Y%m%d%H%M%S";
/// A save point's label until you rename it: `before 10:42`.
const LABEL: &str = "before %H:%M";
/// A snapshot's sparse files, and a backup's file, end with these.
const SNAPSHOT_FILE: &str = ".ss";
const BACKUP_FILE: &str = ".bak";
/// Tables with at most this many rows get a checksum too: a change that keeps the count still shows.
pub const CHECKSUM_MOST_ROWS: u64 = 50_000;
/// The schema whose tables are named without it.
const DEFAULT_SCHEMA: &str = "dbo";

/// What a save point is on the server.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    Snapshot,
    Backup,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Snapshot => "snapshot",
            Self::Backup => "backup",
        }
    }
}

/// A table's rows when it was counted, and its checksum when it's small.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Table {
    pub schema: String,
    pub name: String,
    pub rows: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checksum: Option<i64>,
}

impl Table {
    /// How it's shown: `Orders`, or `sales.Orders` outside `dbo`.
    pub fn shown(&self) -> String {
        if self.schema.eq_ignore_ascii_case(DEFAULT_SCHEMA) {
            self.name.clone()
        } else {
            format!("{}.{}", self.schema, self.name)
        }
    }

    fn same(&self, other: &Table) -> bool {
        self.schema == other.schema && self.name == other.name
    }
}

/// A save point as kept.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavePoint {
    /// Its snapshot database's name, or its backup file's without `.bak`: `Shop_lr_20261123104200`.
    pub id: String,
    pub label: String,
    pub kind: Kind,
    pub database: String,
    pub server: String,
    /// When, as RFC 3339.
    pub created: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backup: Option<String>,
    /// A backend restart rewinds to it first.
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub tables: Vec<Table>,
    /// Its snapshot is no longer on the server (dropped outside lazyrunner).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub gone: bool,
}

impl SavePoint {
    /// Whether it's of this database on this server.
    pub fn of(&self, server: &str, database: &str) -> bool {
        self.server.eq_ignore_ascii_case(server) && self.database.eq_ignore_ascii_case(database)
    }

    /// The time of day it was made: `10:42`.
    pub fn clock(&self) -> String {
        crate::time::local(&self.created).map_or_else(String::new, |at| at.strftime("%H:%M").to_string())
    }
}

/// The save points kept, oldest first; none when the file is missing or broken.
pub fn load(state_dir: &Path) -> Vec<SavePoint> {
    match crate::json::read(&state_dir.join(FILE)) {
        Some(Value::Array(items)) => items.into_iter().filter_map(|item| serde_json::from_value(item).ok()).collect(),
        _ => Vec::new(),
    }
}

pub fn save(state_dir: &Path, all: &[SavePoint]) -> Result<()> {
    crate::config::write(&state_dir.join(FILE), &serde_json::to_value(all)?)
}

/// A database's save points, the newest first.
pub fn newest_first<'a>(all: &'a [SavePoint], server: &str, database: &str) -> Vec<&'a SavePoint> {
    let mut found: Vec<&SavePoint> = all.iter().filter(|point| point.of(server, database)).collect();
    found.sort_by(|a, b| b.created.cmp(&a.created).then_with(|| b.id.cmp(&a.id)));
    found
}

/// A new save point's id, and its label until it's renamed.
pub fn name(database: &str, at: &ZonedDateTime) -> (String, String) {
    (format!("{database}{NAME_PART}{}", at.strftime(STAMP)), at.strftime(LABEL).to_string())
}

/// The folder a file of the server's is in: `C:\Data\Shop.mdf` is in `C:\Data\`.
fn folder_of(path: &str) -> &str {
    path.rfind(['\\', '/']).map_or("", |at| &path[..=at])
}

/// `file` in `folder`, as the server writes paths: with the folder's own separator.
pub fn in_folder(folder: &str, file: &str) -> String {
    let separator = if folder.contains('/') && !folder.contains('\\') { '/' } else { '\\' };
    if folder.is_empty() || folder.ends_with(['\\', '/']) {
        format!("{folder}{file}")
    } else {
        format!("{folder}{separator}{file}")
    }
}

/// The data files of a database: (logical name, physical path) - one row each.
pub fn files_sql(database: &str) -> String {
    format!(
        "SET NOCOUNT ON; SELECT name, physical_name FROM sys.master_files WHERE database_id = DB_ID({}) AND type = 0;",
        quote_text(database)
    )
}

/// Makes `snapshot` of `database`: a sparse file beside each of its data files (`files` as [`files_sql`] reads them).
pub fn snapshot_sql(database: &str, snapshot: &str, files: &[(String, String)]) -> String {
    let files: Vec<String> = files
        .iter()
        .map(|(logical, physical)| {
            let file = in_folder(folder_of(physical), &format!("{snapshot}_{logical}{SNAPSHOT_FILE}"));
            format!("(NAME = {}, FILENAME = {})", quote_name(logical), quote_text(&file))
        })
        .collect();
    format!("CREATE DATABASE {} ON {} AS SNAPSHOT OF {};", quote_name(snapshot), files.join(", "), quote_name(database))
}

/// The server's own backup folder: it writes the file, so a folder it can reach.
pub const BACKUP_FOLDER_SQL: &str =
    "SET NOCOUNT ON; SELECT CAST(SERVERPROPERTY('InstanceDefaultBackupPath') AS nvarchar(4000));";

/// A backup save point's file in the server's backup folder.
pub fn backup_path(folder: &str, id: &str) -> String {
    in_folder(folder, &format!("{id}{BACKUP_FILE}"))
}

pub fn backup_sql(database: &str, path: &str) -> String {
    format!("BACKUP DATABASE {} TO DISK = {} WITH COPY_ONLY, INIT, STATS = 5;", quote_name(database), quote_text(path))
}

pub fn single_user_sql(database: &str) -> String {
    format!("ALTER DATABASE {} SET SINGLE_USER WITH ROLLBACK IMMEDIATE;", quote_name(database))
}

pub fn multi_user_sql(database: &str) -> String {
    format!("ALTER DATABASE {} SET MULTI_USER;", quote_name(database))
}

pub fn restore_snapshot_sql(database: &str, snapshot: &str) -> String {
    format!("RESTORE DATABASE {} FROM DATABASE_SNAPSHOT = {};", quote_name(database), quote_text(snapshot))
}

pub fn restore_backup_sql(database: &str, path: &str) -> String {
    format!("RESTORE DATABASE {} FROM DISK = {} WITH REPLACE, STATS = 5;", quote_name(database), quote_text(path))
}

pub fn drop_sql(snapshot: &str) -> String {
    format!("DROP DATABASE {};", quote_name(snapshot))
}

/// The snapshots of a database on its server: their names, one a row.
pub fn snapshots_sql(database: &str) -> String {
    format!(
        "SET NOCOUNT ON; SELECT name FROM sys.databases WHERE source_database_id = DB_ID({});",
        quote_text(database)
    )
}

/// Who else is connected to a database: (session, client process id, host, program) a row.
pub fn sessions_sql(database: &str) -> String {
    format!(
        "SET NOCOUNT ON; SELECT session_id, ISNULL(host_process_id, 0), ISNULL(host_name, N''), ISNULL(program_name, \
         N'') FROM sys.dm_exec_sessions WHERE database_id = DB_ID({}) AND session_id <> @@SPID AND is_user_process = \
         1;",
        quote_text(database)
    )
}

/// Each table's rows, from the partitions (cheap, no scan): (rows, schema, table) a row.
pub const COUNTS_SQL: &str = "SET NOCOUNT ON; SELECT SUM(p.rows), s.name, t.name FROM sys.tables t JOIN sys.schemas s \
                              ON s.schema_id = t.schema_id JOIN sys.partitions p ON p.object_id = t.object_id AND \
                              p.index_id IN (0, 1) WHERE t.is_ms_shipped = 0 GROUP BY s.name, t.name;";

/// The checksums of `tables`, one batch: (checksum, schema, table) a row; a table whose columns `BINARY_CHECKSUM`
/// can't take is left out rather than failing the rest.
pub fn checksums_sql(tables: &[&Table]) -> Option<String> {
    if tables.is_empty() {
        return None;
    }
    let each: Vec<String> = tables
        .iter()
        .map(|table| {
            format!(
                "BEGIN TRY SELECT ISNULL(CHECKSUM_AGG(BINARY_CHECKSUM(*)), 0), {}, {} FROM {}.{}; END TRY BEGIN CATCH \
                 END CATCH",
                quote_text(&table.schema),
                quote_text(&table.name),
                quote_name(&table.schema),
                quote_name(&table.name)
            )
        })
        .collect();
    Some(format!("SET NOCOUNT ON; {}", each.join("; ")))
}

/// The tables as [`COUNTS_SQL`]'s rows give them.
pub fn read_counts(rows: &[Vec<String>]) -> Vec<Table> {
    rows.iter()
        .filter_map(|row| {
            let [rows, schema, name] = row.as_slice() else { return None };
            Some(Table { schema: schema.clone(), name: name.clone(), rows: rows.parse().ok()?, checksum: None })
        })
        .collect()
}

/// `tables` with the checksums [`checksums_sql`]'s rows give.
pub fn with_checksums(mut tables: Vec<Table>, rows: &[Vec<String>]) -> Vec<Table> {
    for row in rows {
        let [sum, schema, name] = row.as_slice() else { continue };
        let Ok(sum) = sum.parse::<i64>() else { continue };
        if let Some(table) = tables.iter_mut().find(|table| table.schema == *schema && table.name == *name) {
            table.checksum = Some(sum);
        }
    }
    tables
}

/// The tables small enough for a checksum.
pub fn small(tables: &[Table]) -> Vec<&Table> {
    tables.iter().filter(|table| table.rows <= CHECKSUM_MOST_ROWS).collect()
}

/// What changed in a table since a save point: its rows then and now, and whether its rows changed though their count
/// didn't (the checksums differ).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub table: String,
    pub then: u64,
    pub now: u64,
    pub changed: bool,
}

impl Change {
    /// Rows added (more than 0) or removed (less).
    pub fn delta(&self) -> i64 {
        self.now as i64 - self.then as i64
    }
}

/// What changed from `then` to `now`, the biggest change first, then those whose rows changed in place, then by name;
/// a table that's new or gone counts from or to 0.
pub fn changes(then: &[Table], now: &[Table]) -> Vec<Change> {
    let mut found: Vec<Change> = Vec::new();
    for table in now {
        let before = then.iter().find(|old| old.same(table));
        let changed = match (before.and_then(|old| old.checksum), table.checksum) {
            (Some(old), Some(new)) => old != new,
            _ => false,
        };
        let then_rows = before.map_or(0, |old| old.rows);
        if then_rows != table.rows || changed {
            found.push(Change { table: table.shown(), then: then_rows, now: table.rows, changed });
        }
    }
    for gone in then.iter().filter(|old| !now.iter().any(|table| table.same(old))) {
        found.push(Change { table: gone.shown(), then: gone.rows, now: 0, changed: false });
    }
    found.sort_by(|a, b| {
        b.delta()
            .unsigned_abs()
            .cmp(&a.delta().unsigned_abs())
            .then(b.changed.cmp(&a.changed))
            .then(a.table.cmp(&b.table))
    });
    found
}

/// What the changes add up to: `+16 −2 rows in 5 tables`, `~1 changed`, or `nothing changed`.
pub fn summary(changes: &[Change]) -> String {
    let added: i64 = changes.iter().map(Change::delta).filter(|delta| *delta > 0).sum();
    let removed: i64 = -changes.iter().map(Change::delta).filter(|delta| *delta < 0).sum::<i64>();
    let in_place = changes.iter().filter(|change| change.delta() == 0).count();
    let mut parts = Vec::new();
    if added > 0 {
        parts.push(format!("+{added}"));
    }
    if removed > 0 {
        parts.push(format!("−{removed}"));
    }
    let counted = changes.len() - in_place;
    let tables = |count: usize| if count == 1 { "1 table".to_string() } else { format!("{count} tables") };
    let mut said =
        if parts.is_empty() { String::new() } else { format!("{} rows in {}", parts.join(" "), tables(counted)) };
    if in_place > 0 {
        let more = format!("~{in_place} changed");
        said = if said.is_empty() { more } else { format!("{said}, {more}") };
    }
    if said.is_empty() { "nothing changed".into() } else { said }
}

/// The news of a database job, from its thread.
#[derive(Debug)]
pub enum News {
    /// A backup or restore's share done, as `WITH STATS` says it.
    Share(f64),
    /// A save point made, or why it couldn't be.
    Made(Result<SavePoint, String>),
    /// A rewind done (and the snapshots it dropped first), or why it failed.
    Rewound { dropped: Vec<String>, result: Result<(), String> },
    /// A save point's snapshot dropped (or a backup's forgotten), or why it couldn't be.
    Dropped { id: String, result: Result<(), String> },
    /// A database's tables counted now, and the snapshots on its server - or why they couldn't be.
    Counted { server: String, database: String, result: Result<(Vec<Table>, Vec<String>), String> },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TempDir;

    fn table(name: &str, rows: u64, checksum: Option<i64>) -> Table {
        Table { schema: "dbo".into(), name: name.into(), rows, checksum }
    }

    #[test]
    fn the_sql_names_what_it_touches_quoted() {
        let files =
            [("Shop".to_string(), r"C:\Data\Shop.mdf".to_string()), ("Shop_2".into(), r"D:\x\Shop_2.ndf".into())];
        assert_eq!(
            snapshot_sql("Shop", "Shop_lr_20261123104200", &files),
            r"CREATE DATABASE [Shop_lr_20261123104200] ON (NAME = [Shop], FILENAME = N'C:\Data\Shop_lr_20261123104200_Shop.ss'), (NAME = [Shop_2], FILENAME = N'D:\x\Shop_lr_20261123104200_Shop_2.ss') AS SNAPSHOT OF [Shop];"
        );
        let path = backup_path(r"C:\Backup", "Shop_lr_1");
        assert_eq!(path, r"C:\Backup\Shop_lr_1.bak");
        assert_eq!(backup_path("/var/opt/mssql/data/", "x"), "/var/opt/mssql/data/x.bak");
        assert_eq!(
            backup_sql("Sh]op", &path),
            r"BACKUP DATABASE [Sh]]op] TO DISK = N'C:\Backup\Shop_lr_1.bak' WITH COPY_ONLY, INIT, STATS = 5;"
        );
        assert_eq!(
            restore_snapshot_sql("Shop", "Shop_lr_1"),
            "RESTORE DATABASE [Shop] FROM DATABASE_SNAPSHOT = N'Shop_lr_1';"
        );
        assert_eq!(
            restore_backup_sql("Shop", r"C:\B\it's.bak"),
            r"RESTORE DATABASE [Shop] FROM DISK = N'C:\B\it''s.bak' WITH REPLACE, STATS = 5;"
        );
        assert_eq!(single_user_sql("Shop"), "ALTER DATABASE [Shop] SET SINGLE_USER WITH ROLLBACK IMMEDIATE;");
        assert_eq!(multi_user_sql("Shop"), "ALTER DATABASE [Shop] SET MULTI_USER;");
        assert_eq!(drop_sql("Shop_lr_1"), "DROP DATABASE [Shop_lr_1];");
        assert!(snapshots_sql("O'Shop").contains("DB_ID(N'O''Shop')"));
        let small = table("Orders", 3, None);
        let sql = checksums_sql(&[&small]).unwrap();
        assert!(sql.contains("FROM [dbo].[Orders]") && sql.contains("BEGIN CATCH"), "{sql}");
        assert_eq!(checksums_sql(&[]), None);
    }

    #[test]
    fn counts_and_checksums_read_from_their_rows() {
        let rows = |rows: &[[&str; 3]]| -> Vec<Vec<String>> {
            rows.iter().map(|row| row.iter().map(|part| part.to_string()).collect()).collect()
        };
        let counted = read_counts(&rows(&[["12", "dbo", "Orders"], ["x", "dbo", "Bad"], ["7", "sales", "Leads"]]));
        assert_eq!(
            counted,
            [
                table("Orders", 12, None),
                Table { schema: "sales".into(), name: "Leads".into(), rows: 7, checksum: None }
            ]
        );
        assert_eq!(counted[1].shown(), "sales.Leads");
        let summed = with_checksums(counted, &rows(&[["-42", "dbo", "Orders"]]));
        assert_eq!(summed[0].checksum, Some(-42));
        assert_eq!(summed[1].checksum, None);
    }

    #[test]
    fn what_changed_is_the_biggest_change_first_then_rows_changed_in_place() {
        let then = [
            table("AuditLog", 88_020, None),
            table("Drafts", 47, Some(1)),
            table("Approvals", 412, Some(5)),
            table("Same", 9, Some(3)),
            table("Dropped", 4, None),
        ];
        let now = [
            table("AuditLog", 88_032, None),
            table("Drafts", 45, Some(2)),
            table("Approvals", 412, Some(6)),
            table("Same", 9, Some(3)),
            table("Fresh", 1, None),
        ];
        let found = changes(&then, &now);
        let shown: Vec<(&str, i64, bool)> =
            found.iter().map(|change| (change.table.as_str(), change.delta(), change.changed)).collect();
        assert_eq!(
            shown,
            [
                ("AuditLog", 12, false),
                ("Dropped", -4, false),
                ("Drafts", -2, true),
                ("Fresh", 1, false),
                ("Approvals", 0, true)
            ]
        );
        assert_eq!(summary(&found), "+13 −6 rows in 4 tables, ~1 changed");
        assert_eq!(summary(&[]), "nothing changed");
    }

    #[test]
    fn save_points_keep_in_the_state_folder() {
        let temp = TempDir::new("save-points");
        let at = crate::time::as_written("2026-11-23T10:42:07+01:00").unwrap();
        let (id, label) = name("Shop", &at);
        assert_eq!((id.as_str(), label.as_str()), ("Shop_lr_20261123104207", "before 10:42"));
        let point = SavePoint {
            id: id.clone(),
            label,
            kind: Kind::Snapshot,
            database: "Shop".into(),
            server: "localhost".into(),
            created: "2026-11-23T10:42:07+01:00".into(),
            snapshot: Some(id),
            backup: None,
            pinned: true,
            tables: vec![table("Orders", 3, Some(9))],
            gone: false,
        };
        let older = SavePoint { id: "Shop_lr_1".into(), created: "2026-11-22T09:00:00+01:00".into(), ..point.clone() };
        save(temp.path(), &[older.clone(), point.clone()]).unwrap();
        let kept = load(temp.path());
        assert_eq!(kept, [older, point.clone()]);
        let text = std::fs::read_to_string(temp.join(FILE)).unwrap();
        assert!(text.contains("\"kind\": \"snapshot\"") && !text.contains("gone"), "{text}");
        let newest = newest_first(&kept, "LOCALHOST", "shop");
        assert_eq!(newest[0].id, point.id, "the newest first, server and database ignoring case");
        assert!(newest_first(&kept, "localhost", "Other").is_empty());
        assert!(load(&temp.join("nowhere")).is_empty());
    }
}
