//! A pull's first four steps (#110), in its own thread: check the source answers and the login works, export a
//! `.bacpac` from it - the only things ever run there, and both only read - import it as a new database on the local
//! server, and run the after-pull scripts on that. Each step's lines and progress are reported as they come. A
//! cancel (`x`) kills the program running; a half-imported database is dropped after, the job no longer watched.
//! An export-only job stops after the export (#114). The migrations, the switch and an update's swap run in the
//! controller (`controller::pull`).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use super::database_jobs::{exists, export, first, import, remove_database};
use crate::run::cancel::{CANCELLED, Cancel};
use crate::run::database_files::{self as files, Progress};
use crate::run::pull::{self, Mode, News, Step};
use crate::run::secrets::mask_text;
use crate::run::sql::Sql;
use crate::run::sqlpackage::{Action, Package};

/// What a pull's thread does: what it makes, its source (by name, and its database, connection string and all), the
/// local server's connection string, the new database, the `.bacpac` it goes through, the private folder for
/// SqlPackage's response file, and the after-pull scripts (each its name and text).
pub(super) struct Plan {
    pub mode: Mode,
    pub source: String,
    pub source_database: String,
    pub source_connection: String,
    pub local: String,
    pub database: String,
    pub file: String,
    pub private: PathBuf,
    pub scripts: Vec<(String, String)>,
}

/// What the thread tells the controller its news with.
pub(super) type Say<'a> = &'a (dyn Fn(News) + Sync);

/// What runs SQL, each line it prints told as the step's too.
struct TeedSql<'a> {
    inner: &'a dyn Sql,
    step: Step,
    say: Say<'a>,
}

impl Sql for TeedSql<'_> {
    fn run(
        &self,
        connection: &str,
        database: Option<&str>,
        sql: &str,
        line: &mut dyn FnMut(&str),
    ) -> Result<(), String> {
        self.inner.run(connection, database, sql, &mut |text| {
            (self.say)(News::Line(self.step, mask_text(text)));
            line(text);
        })
    }

    fn ready(&self) -> Result<(), String> {
        self.inner.ready()
    }
}

/// What runs SqlPackage, each line it prints told as the step's too.
struct TeedPackage<'a> {
    inner: &'a dyn Package,
    step: Step,
    say: Say<'a>,
}

impl Package for TeedPackage<'_> {
    fn run(
        &self,
        action: Action,
        connection: &str,
        file: &str,
        private: &Path,
        line: &mut dyn FnMut(&str),
    ) -> Result<(), String> {
        self.inner.run(action, connection, file, private, &mut |text| {
            (self.say)(News::Line(self.step, mask_text(text)));
            line(text);
        })
    }

    fn ready(&self) -> Result<(), String> {
        self.inner.ready()
    }
}

/// Runs the steps, then - when the import stopped half-way (a failure, a cancel) - drops what it made; how they ended
/// is told last.
pub(super) fn run(sql: &dyn Sql, package: &dyn Package, plan: &Plan, cancel: &Arc<Cancel>, say: Say<'_>) {
    let watching = cancel.watch();
    let mut made = false;
    let result = steps(sql, package, plan, &mut made, say);
    drop(watching);
    if let Err((Step::Import, _)) = &result
        && made
        && exists(sql, &plan.local, &plan.database).unwrap_or(true)
    {
        let dropped = remove_database(sql, &plan.local, &plan.database);
        let said = match dropped {
            Ok(_) => format!("{} was half-imported: dropped", plan.database),
            Err(why) => format!("{} was half-imported and couldn't be dropped: {why}", plan.database),
        };
        say(News::Line(Step::Import, said));
    }
    say(News::Ended(result));
}

/// A step starts, said - unless the job was cancelled meanwhile.
fn start(step: Step, say: Say<'_>) -> Result<(), (Step, String)> {
    if crate::run::cancel::asked() {
        return Err((step, CANCELLED.into()));
    }
    say(News::Started(step));
    Ok(())
}

/// The steps in order, each told as it starts and ends; the step that failed, and why. `made`: the import began
/// making the new database (it wasn't there before).
fn steps(
    sql: &dyn Sql,
    package: &dyn Package,
    plan: &Plan,
    made: &mut bool,
    say: Say<'_>,
) -> Result<(), (Step, String)> {
    let at = |step: Step| move |why: String| (step, why);
    start(Step::Check, say)?;
    let checking = TeedSql { inner: sql, step: Step::Check, say };
    let started = Instant::now();
    let login = first(&checking, &plan.source_connection, None, pull::CHECK_SQL).map_err(at(Step::Check))?;
    let took = started.elapsed().as_millis();
    let login = login.map_or_else(String::new, |login| format!(" · as {login}"));
    say(News::Done(Step::Check, format!("{} · {took} ms{login}", plan.source)));

    start(Step::Export, say)?;
    let report = |progress: Progress| say(News::Progress(progress));
    let exporting = TeedPackage { inner: package, step: Step::Export, say };
    let exported =
        export(sql, &exporting, &plan.source_connection, &plan.source_database, &plan.file, &plan.private, &report)
            .map_err(at(Step::Export))?;
    say(News::Done(Step::Export, exported.bytes.map(files::size).unwrap_or_default()));
    if plan.mode == Mode::Export {
        return Ok(());
    }

    start(Step::Import, say)?;
    if exists(sql, &plan.local, &plan.database).map_err(at(Step::Import))? {
        return Err((
            Step::Import,
            format!("{} already exists - lazyrunner never overwrites a database", plan.database),
        ));
    }
    *made = true;
    let importing = TeedPackage { inner: package, step: Step::Import, say };
    import(sql, &importing, &plan.local, &plan.file, &plan.database, &plan.private, &report)
        .map_err(at(Step::Import))?;
    say(News::Done(Step::Import, plan.database.clone()));

    if plan.scripts.is_empty() {
        say(News::Skipped(Step::AfterPull, "no afterPull scripts".into()));
        return Ok(());
    }
    start(Step::AfterPull, say)?;
    let scripting = TeedSql { inner: sql, step: Step::AfterPull, say };
    for (name, text) in &plan.scripts {
        say(News::Line(Step::AfterPull, format!("▸ {name}")));
        for batch in pull::batches(text) {
            scripting
                .run(&plan.local, Some(&plan.database), &batch, &mut |_| {})
                .map_err(|why| (Step::AfterPull, format!("{name}: {why}")))?;
        }
    }
    let count = plan.scripts.len();
    say(News::Done(Step::AfterPull, format!("{count} script{}", if count == 1 { "" } else { "s" })));
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;

    /// SQL that answers a pull's queries, keeps what it was asked and fails what names `fail`.
    #[derive(Default)]
    struct Scripted {
        asked: Mutex<Vec<(Option<String>, String)>>,
        fail: Option<&'static str>,
        imported: AtomicBool,
    }

    impl Sql for Scripted {
        fn run(&self, _: &str, database: Option<&str>, sql: &str, line: &mut dyn FnMut(&str)) -> Result<(), String> {
            self.asked.lock().unwrap().push((database.map(String::from), sql.to_string()));
            if self.fail.is_some_and(|fail| sql.contains(fail)) {
                return Err(format!("Msg 50000: {sql} failed"));
            }
            if sql == pull::CHECK_SQL {
                line("reader");
            } else if sql == files::TABLES_SQL {
                line("2");
            } else if sql.contains("ISNULL(DB_ID(") {
                line(if self.imported.load(Ordering::SeqCst) { "9" } else { "0" });
            }
            Ok(())
        }

        fn ready(&self) -> Result<(), String> {
            Ok(())
        }
    }

    /// SqlPackage that names its tables; its import fails, or is cancelled half-way, when asked to.
    struct Packaged<'a> {
        sql: &'a Scripted,
        import: Result<(), &'static str>,
        cancel: Option<Arc<Cancel>>,
    }

    impl Package for Packaged<'_> {
        fn run(&self, action: Action, _: &str, _: &str, _: &Path, line: &mut dyn FnMut(&str)) -> Result<(), String> {
            line("Processing Table '[dbo].[Orders]'.");
            line("Processing Table '[dbo].[OrderLines]'.");
            if action == Action::Export {
                return Ok(());
            }
            self.sql.imported.store(true, Ordering::SeqCst);
            if let Some(cancel) = &self.cancel {
                cancel.cancel();
                return Err(CANCELLED.into());
            }
            self.import.map_err(String::from)
        }

        fn ready(&self) -> Result<(), String> {
            Ok(())
        }
    }

    fn plan(scripts: &[(&str, &str)]) -> Plan {
        Plan {
            mode: Mode::Pull,
            source: "qa".into(),
            source_database: "Shop".into(),
            source_connection: "Server=qa-sql.example.com;Database=Shop;User ID=reader;Password=s3cret".into(),
            local: "Server=localhost;Database=Shop;Trusted_Connection=True".into(),
            database: "Shop_qa_20261127".into(),
            file: "Shop_qa_20261127.bacpac".into(),
            private: PathBuf::from("."),
            scripts: scripts.iter().map(|(name, text)| (name.to_string(), text.to_string())).collect(),
        }
    }

    /// Runs the steps; what they said.
    fn ran(sql: &Scripted, package: &dyn Package, plan: &Plan, cancel: &Arc<Cancel>) -> Vec<String> {
        let said = Mutex::new(Vec::new());
        run(sql, package, plan, cancel, &|news| said.lock().unwrap().push(format!("{news:?}")));
        said.into_inner().unwrap()
    }

    fn ended(said: &[String]) -> &str {
        said.last().map(String::as_str).unwrap_or_default()
    }

    fn dropped(sql: &Scripted) -> bool {
        sql.asked.lock().unwrap().iter().any(|(_, text)| text.ends_with("DROP DATABASE [Shop_qa_20261127];"))
    }

    const THREE: &str = "UPDATE A SET X = 1;\nGO\nUPDATE B SET Y = 2;\nGO\nUPDATE C SET Z = 3;";

    #[test]
    fn a_pull_checks_exports_imports_and_runs_its_scripts_in_order() {
        let sql = Scripted::default();
        let package = Packaged { sql: &sql, import: Ok(()), cancel: None };
        let plan =
            plan(&[("scripts/a.sql", "UPDATE A SET X = 1;\nGO\nUPDATE B SET Y = 2;"), ("b.sql", "DELETE FROM C;")]);
        let said = ran(&sql, &package, &plan, &Arc::default());
        assert_eq!(ended(&said), "Ended(Ok(()))", "{said:#?}");
        let started: Vec<&String> = said.iter().filter(|news| news.starts_with("Started")).collect();
        assert_eq!(started, ["Started(Check)", "Started(Export)", "Started(Import)", "Started(AfterPull)"]);
        assert!(said.iter().any(|news| news.starts_with("Done(Check, \"qa · ") && news.contains("as reader")));
        assert!(said.iter().any(|news| news.contains("table: Some(\"OrderLines\")")), "the tables tick: {said:#?}");
        assert!(said.iter().any(|news| news == "Done(AfterPull, \"2 scripts\")"));
        let asked = sql.asked.lock().unwrap().clone();
        let scripts: Vec<&str> = asked
            .iter()
            .filter(|(database, _)| database.as_deref() == Some("Shop_qa_20261127"))
            .map(|(_, text)| text.trim())
            .collect();
        assert_eq!(
            scripts,
            ["UPDATE A SET X = 1;", "UPDATE B SET Y = 2;", "DELETE FROM C;"],
            "batch by batch, in order"
        );
        assert!(!said.iter().any(|news| news.contains("s3cret")), "no password in what it says");
        assert!(!dropped(&sql), "nothing dropped");
    }

    #[test]
    fn a_failure_stops_the_pull_at_its_step_and_says_why() {
        for (fail, step) in [(pull::CHECK_SQL, "Check"), (files::TABLES_SQL, "Export"), ("UPDATE B", "AfterPull")] {
            let sql = Scripted { fail: Some(fail), ..Scripted::default() };
            let package = Packaged { sql: &sql, import: Ok(()), cancel: None };
            let said = ran(&sql, &package, &plan(&[("scripts/a.sql", THREE)]), &Arc::default());
            assert!(ended(&said).starts_with(&format!("Ended(Err(({step}, ")), "{step}: {said:#?}");
            let asked = sql.asked.lock().unwrap().clone();
            assert!(!asked.iter().any(|(_, text)| text.contains("UPDATE C")), "nothing runs after the failure");
            assert!(!dropped(&sql), "{step}: a whole copy is never dropped");
        }
        let sql = Scripted { fail: Some("UPDATE B"), ..Scripted::default() };
        let package = Packaged { sql: &sql, import: Ok(()), cancel: None };
        let said = ran(&sql, &package, &plan(&[("scripts/a.sql", "UPDATE B SET Y = 2;")]), &Arc::default());
        assert!(ended(&said).contains("scripts/a.sql: Msg 50000"), "the script's name and its error: {said:#?}");
    }

    #[test]
    fn a_failed_or_cancelled_import_drops_the_half_imported_database_and_never_one_that_was_there() {
        let sql = Scripted::default();
        let package = Packaged { sql: &sql, import: Err("Could not import package"), cancel: None };
        let said = ran(&sql, &package, &plan(&[]), &Arc::default());
        assert!(ended(&said).starts_with("Ended(Err((Import, \"Could not import package\""), "{said:#?}");
        assert!(dropped(&sql), "half-imported: dropped");

        let sql = Scripted::default();
        let cancel = Arc::new(Cancel::default());
        let package = Packaged { sql: &sql, import: Ok(()), cancel: Some(Arc::clone(&cancel)) };
        let said = ran(&sql, &package, &plan(&[("scripts/a.sql", "UPDATE A SET X = 1;")]), &cancel);
        assert_eq!(ended(&said), "Ended(Err((Import, \"cancelled\")))", "{said:#?}");
        assert!(dropped(&sql), "cancelled half-way: dropped, though the job is cancelled");
        assert!(said.iter().any(|news| news.contains("was half-imported: dropped")));
        assert!(!sql.asked.lock().unwrap().iter().any(|(_, text)| text.contains("UPDATE A")), "no script after");

        let sql = Scripted::default();
        let package = Packaged { sql: &sql, import: Ok(()), cancel: None };
        let export = Plan { mode: Mode::Export, ..plan(&[("scripts/a.sql", "UPDATE A SET X = 1;")]) };
        let said = ran(&sql, &package, &export, &Arc::default());
        let started: Vec<&String> = said.iter().filter(|news| news.starts_with("Started")).collect();
        assert_eq!(started, ["Started(Check)", "Started(Export)"], "an export only exports: {said:#?}");
        assert_eq!(ended(&said), "Ended(Ok(()))");

        let there = Scripted { imported: AtomicBool::new(true), ..Scripted::default() };
        let package = Packaged { sql: &there, import: Ok(()), cancel: None };
        let said = ran(&there, &package, &plan(&[]), &Arc::default());
        assert!(ended(&said).contains("already exists - lazyrunner never overwrites a database"), "{said:#?}");
        assert!(!dropped(&there), "one that was there is never dropped");
    }
}
