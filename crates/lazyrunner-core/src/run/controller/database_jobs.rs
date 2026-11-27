//! The work of a copy, backup, export, import or drop of a database (#110), in its own thread: the SQL it runs
//! through sqlcmd and the SqlPackage it runs, in order, each step's progress reported as it comes. The controller
//! starts them and hears how they went (`controller::database_files`).

use std::path::Path;

use super::save_points::{MASTER, ours};
use crate::run::database_files::{self as files, Done, Progress};
use crate::run::save_points as points;
use crate::run::sql::{self, Sql};
use crate::run::sqlpackage::{self, Action, Package};

/// The note of a drop that found the database already gone: only its connection strings are left to remove.
pub(super) const ALREADY_GONE: &str = "already gone from the server";

/// Whether the signed-in user may create databases: 1 or 0.
pub(super) const CAN_CREATE_SQL: &str = "SET NOCOUNT ON; SELECT HAS_PERMS_BY_NAME(NULL, NULL, 'CREATE ANY DATABASE')";

/// What a job reports its progress through.
pub(super) type Report<'a> = &'a dyn Fn(Progress);

/// The first column of a query's first row.
pub(super) fn first(
    sql: &dyn Sql,
    connection: &str,
    database: Option<&str>,
    text: &str,
) -> Result<Option<String>, String> {
    let rows = sql::query(sql, connection, database, text, 1)?;
    Ok(rows.into_iter().next().and_then(|row| row.into_iter().next()).filter(|value| value != "NULL"))
}

/// Whether the Windows user may create a database on the server: an import then signs in as it, as SSMS does - not as
/// the connection string's login, which a `.bacpac` with a user of the same name can't be imported as (a sysadmin
/// login is `dbo` in the new database, so its own user clashes).
fn can_create_databases(sql: &dyn Sql, windows: &str) -> bool {
    first(sql, windows, Some(MASTER), CAN_CREATE_SQL).is_ok_and(|can| can.as_deref() == Some("1"))
}

/// The server's backup folder.
fn backup_folder(sql: &dyn Sql, connection: &str) -> Result<String, String> {
    first(sql, connection, Some(MASTER), points::BACKUP_FOLDER_SQL)?
        .filter(|folder| !folder.is_empty())
        .ok_or_else(|| "the server has no default backup folder".to_string())
}

/// Where the server puts new data and log files.
fn data_folders(sql: &dyn Sql, connection: &str) -> Result<(String, String), String> {
    let rows = sql::query(sql, connection, Some(MASTER), files::DATA_FOLDERS_SQL, 2)?;
    let [data, log] = rows.into_iter().next().unwrap_or_default().try_into().unwrap_or_default();
    if data.is_empty() || data == "NULL" {
        return Err("the server has no default data folder".into());
    }
    let log = if log.is_empty() || log == "NULL" { data.clone() } else { log };
    Ok((data, log))
}

/// Whether `database` is on the server.
pub(super) fn exists(sql: &dyn Sql, connection: &str, database: &str) -> Result<bool, String> {
    Ok(first(sql, connection, Some(MASTER), &files::exists_sql(database))?.is_some_and(|id| id != "0"))
}

/// What a `WITH STATS` line reports: its share, and the bytes that is of `total`.
fn stats(report: Report<'_>, restoring: bool, total: Option<u64>) -> impl FnMut(&str) + '_ {
    move |line: &str| {
        if let Some(share) = sql::percent(line) {
            let bytes = total.map(|total| (total as f64 * share) as u64);
            report(Progress { share: Some(share), bytes, restoring, ..Progress::default() });
        }
    }
}

/// A step's name, as a job's progress says it.
fn step(said: &str, restoring: bool) -> Progress {
    Progress { step: Some(said.into()), restoring, ..Progress::default() }
}

/// Backs `database` up to `name` in the server's backup folder; the file, and its size as msdb has it.
pub(super) fn backup(
    sql: &dyn Sql,
    connection: &str,
    database: &str,
    name: &str,
    total: Option<u64>,
    report: Report<'_>,
) -> Result<Done, String> {
    let path = points::in_folder(&backup_folder(sql, connection)?, name);
    sql.run(connection, Some(MASTER), &points::backup_sql(database, &path), &mut stats(report, false, total))?;
    let bytes = first(sql, connection, Some(MASTER), &files::backup_size_sql(&path)).ok().flatten();
    Ok(Done { file: Some(path), bytes: bytes.and_then(|bytes| bytes.parse().ok()), ..Done::default() })
}

/// Copies `database` to `to`: a `COPY_ONLY` backup to `temp` in the backup folder, restored as `to` with each file
/// moved to its own name, the backup deleted after (or left, said so).
pub(super) fn copy(
    sql: &dyn Sql,
    connection: &str,
    database: &str,
    to: &str,
    temp: &str,
    total: Option<u64>,
    report: Report<'_>,
) -> Result<Done, String> {
    if exists(sql, connection, to)? {
        return Err(format!("{to} already exists - lazyrunner never overwrites a database"));
    }
    let files =
        files::read_master_files(&sql::query(sql, connection, Some(MASTER), &files::master_files_sql(database), 3)?);
    if files.is_empty() {
        return Err(format!("{database}'s files couldn't be read (sys.master_files)"));
    }
    let (data, log) = data_folders(sql, connection)?;
    let path = points::in_folder(&backup_folder(sql, connection)?, temp);
    report(step("backing up", false));
    sql.run(connection, Some(MASTER), &points::backup_sql(database, &path), &mut stats(report, false, total))?;
    report(Progress { share: Some(0.0), bytes: Some(0), ..step("restoring", true) });
    let moves = files::moves(to, &files, &data, &log);
    let restored =
        sql.run(connection, Some(MASTER), &files::restore_sql(to, &path, &moves), &mut stats(report, true, total));
    let note = remove_backup(sql, connection, &path);
    restored?;
    Ok(Done { database: Some(to.to_string()), note: Some(note), ..Done::default() })
}

/// Deletes a copy's backup on the server; what became of it, in words.
fn remove_backup(sql: &dyn Sql, connection: &str, path: &str) -> String {
    match delete_on_server(sql, connection, path, false) {
        Ok(()) => "its backup on the way is deleted".into(),
        Err(why) => format!("its backup on the way is left in the server's backup folder ({why}): {path}"),
    }
}

/// Deletes a backup file on the server with `xp_delete_file` (it takes only backup files), or here when the server
/// is this machine (`here`) and lazyrunner can reach the file; an error when it's still there.
pub(super) fn delete_on_server(sql: &dyn Sql, connection: &str, path: &str, here: bool) -> Result<(), String> {
    let deleted = sql.run(connection, Some(MASTER), &files::delete_backup_sql(path), &mut |_| {});
    let still_there = |sql: &dyn Sql| {
        let text = files::present_sql(&[path.to_string()]).unwrap_or_default();
        sql::query(sql, connection, Some(MASTER), &text, 2).map(|rows| rows.iter().any(|row| row[0] == "1"))
    };
    match (deleted, still_there(sql)) {
        (_, Ok(false)) => Ok(()),
        (deleted, _) if here && Path::new(path).is_file() => {
            std::fs::remove_file(path).map_err(|error| match deleted {
                Ok(()) => error.to_string(),
                Err(why) => format!("{why}; {error}"),
            })
        }
        (Err(why), _) => Err(why),
        (Ok(()), Ok(true)) => Err("the server kept it (xp_delete_file takes only backups it can read)".into()),
        (Ok(()), Err(_)) => Ok(()),
    }
}

/// What a failed restore says, plainer when the server's service account can't read the file.
fn explain(why: String, path: &str) -> String {
    if !files::denied(&why) {
        return why;
    }
    format!(
        "the server can't read {path}: its service account has no access to that folder - put the .bak in the \
         server's backup folder and import it from there"
    )
}

/// Restores the backup at `path` as the new database `to`, each file moved to its own name.
pub(super) fn restore(
    sql: &dyn Sql,
    connection: &str,
    path: &str,
    to: &str,
    report: Report<'_>,
) -> Result<Done, String> {
    let listed =
        sql::query(sql, connection, Some(MASTER), &files::file_list_sql(path), 4).map_err(|why| explain(why, path))?;
    let files = files::read_file_list(&listed);
    if files.is_empty() {
        return Err(format!("{path} lists no database files - is it a database backup?"));
    }
    let (data, log) = data_folders(sql, connection)?;
    report(step("restoring", true));
    let moves = files::moves(to, &files, &data, &log);
    let text = files::restore_sql(to, path, &moves);
    sql.run(connection, Some(MASTER), &text, &mut stats(report, true, None)).map_err(|why| explain(why, path))?;
    Ok(Done { database: Some(to.to_string()), ..Done::default() })
}

/// What a SqlPackage line reports: the table it's on (of `tables`, the share done, the file's size so far), else
/// the step it's at.
fn package_line<'a>(
    report: Report<'a>,
    tables: Option<usize>,
    file: &'a str,
    restoring: bool,
) -> impl FnMut(&str) + 'a {
    let mut at = 0;
    move |line: &str| {
        let Some(table) = sqlpackage::table_of(line) else {
            let line = line.trim().trim_end_matches('.');
            if !line.is_empty() && !line.starts_with("***") {
                report(step(&line.to_lowercase(), restoring));
            }
            return;
        };
        at += 1;
        let share = tables.map(|of| ((at - 1) as f64 / of.max(1) as f64).min(1.0));
        let bytes = std::fs::metadata(file).ok().map(|meta| meta.len()).filter(|_| !restoring);
        let table = table.strip_prefix("dbo.").map_or(table.clone(), str::to_string);
        let tables = tables.map(|of| (at, of.max(at)));
        report(Progress { share, bytes, tables, table: Some(table), restoring, ..Progress::default() });
    }
}

/// Exports `database` to the `.bacpac` at `file`: its tables counted first, for the progress.
pub(super) fn export(
    sql: &dyn Sql,
    package: &dyn Package,
    connection: &str,
    database: &str,
    file: &str,
    private: &Path,
    report: Report<'_>,
) -> Result<Done, String> {
    let tables = first(sql, connection, Some(database), files::TABLES_SQL)?.and_then(|count| count.parse().ok());
    report(Progress { tables: tables.map(|of| (0, of)), ..step("reading its schema", false) });
    package.run(Action::Export, connection, file, private, &mut package_line(report, tables, file, false))?;
    let bytes = std::fs::metadata(file).ok().map(|meta| meta.len());
    Ok(Done { file: Some(file.to_string()), bytes, ..Done::default() })
}

/// Imports the `.bacpac` at `path` as the new database `to`.
pub(super) fn import(
    sql: &dyn Sql,
    package: &dyn Package,
    connection: &str,
    path: &str,
    to: &str,
    private: &Path,
    report: Report<'_>,
) -> Result<Done, String> {
    if exists(sql, connection, to)? {
        return Err(format!("{to} already exists - lazyrunner never overwrites a database"));
    }
    let tables = sqlpackage::tables_in(Path::new(path));
    report(Progress { tables: tables.map(|of| (0, of)), ..step("making its schema", true) });
    let windows = sql::as_windows_user(connection).filter(|windows| can_create_databases(sql, windows));
    let target = sqlpackage::with_database(windows.as_deref().unwrap_or(connection), to);
    package.run(Action::Import, &target, path, private, &mut package_line(report, tables, path, true))?;
    Ok(Done { database: Some(to.to_string()), ..Done::default() })
}

/// Drops `database`: lazyrunner's snapshots of it first (SQL Server won't drop a database with snapshots), then the
/// database, everyone else cut off.
pub(super) fn remove_database(sql: &dyn Sql, connection: &str, database: &str) -> Result<Done, String> {
    if !exists(sql, connection, database)? {
        return Ok(Done { note: Some(ALREADY_GONE.to_string()), ..Done::default() });
    }
    let snapshots = sql::query(sql, connection, Some(MASTER), &points::snapshots_sql(database), 1)?;
    for snapshot in snapshots.into_iter().filter_map(|row| row.into_iter().next()).filter(|name| ours(name, database)) {
        sql.run(connection, Some(MASTER), &points::drop_sql(&snapshot), &mut |_| {})?;
    }
    sql.run(connection, Some(MASTER), &files::drop_database_sql(database), &mut |_| {})?;
    Ok(Done::default())
}
#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    /// SQL whose Windows user may (or may not) create databases.
    struct Server {
        windows_can_create: bool,
    }

    impl Sql for Server {
        fn run(&self, connection: &str, _: Option<&str>, text: &str, line: &mut dyn FnMut(&str)) -> Result<(), String> {
            if text == CAN_CREATE_SQL {
                assert!(connection.ends_with("Integrated Security=True"), "asked as the Windows user: {connection}");
                line(if self.windows_can_create { "1" } else { "0" });
            } else if text.contains("DB_ID(") {
                line("0");
            }
            Ok(())
        }

        fn ready(&self) -> Result<(), String> {
            Ok(())
        }
    }

    /// SqlPackage that keeps the connection it imported with.
    #[derive(Default)]
    struct Imported(Mutex<Option<String>>);

    impl Package for Imported {
        fn run(&self, _: Action, connection: &str, _: &str, _: &Path, _: &mut dyn FnMut(&str)) -> Result<(), String> {
            *self.0.lock().unwrap() = Some(connection.to_string());
            Ok(())
        }

        fn ready(&self) -> Result<(), String> {
            Ok(())
        }
    }

    fn imported_as(windows_can_create: bool) -> String {
        let package = Imported::default();
        let login = "Server=Shop-PC;Database=Shop;User ID=shop;Password=pw;TrustServerCertificate=True";
        let done =
            import(&Server { windows_can_create }, &package, login, "Shop.bacpac", "Shop_qa", Path::new("."), &|_| {});
        assert_eq!(done.unwrap().database.as_deref(), Some("Shop_qa"));
        package.0.lock().unwrap().clone().unwrap()
    }

    #[test]
    fn an_import_signs_in_as_the_windows_user_as_ssms_does_else_as_the_login() {
        let windows = imported_as(true);
        assert!(windows.contains("Database=Shop_qa") && windows.contains("Integrated Security=True"), "{windows}");
        assert!(!windows.contains("User ID") && !windows.contains("Password"), "{windows}");
        assert!(windows.contains("TrustServerCertificate=True"), "the rest kept: {windows}");
        let login = imported_as(false);
        assert!(login.contains("User ID=shop") && login.contains("Database=Shop_qa"), "{login}");
    }

    /// SQL on a server without the database: it keeps what it was asked to run.
    #[derive(Default)]
    struct Without(Mutex<Vec<String>>);

    impl Sql for Without {
        fn run(&self, _: &str, _: Option<&str>, text: &str, line: &mut dyn FnMut(&str)) -> Result<(), String> {
            self.0.lock().unwrap().push(text.to_string());
            if text.contains("DB_ID(") {
                line("0");
            }
            Ok(())
        }

        fn ready(&self) -> Result<(), String> {
            Ok(())
        }
    }

    #[test]
    fn a_drop_of_a_database_already_gone_runs_nothing_and_says_so() {
        let sql = Without::default();
        let done = remove_database(&sql, "Server=Shop-PC;Database=Shop_old;Integrated Security=True", "Shop_old");
        assert_eq!(done.unwrap().note.as_deref(), Some(ALREADY_GONE));
        let ran = sql.0.lock().unwrap().clone();
        assert!(ran.iter().all(|text| !text.contains("DROP DATABASE")), "{ran:?}");
    }
}
