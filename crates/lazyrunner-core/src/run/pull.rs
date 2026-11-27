//! Pulling a copy of a database from another environment (#110): its steps, the connection string to the source
//! (built from the profile and a password from the credential store, never kept), the pulled copy's name
//! (`<database>_<source>_<yyyyMMdd>`), the after-pull scripts' batches, and which older copies `keepPulled` would drop.
//! The same steps only export, or import without switching, or update a linked database (#114): its new copy imported
//! beside it, then swapped in by name. The steps run in the controller (`controller::pull`).

use std::path::Path;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::credentials::Password;
use super::database_files::{Progress, free_name};
use crate::profile::sources::{Auth, DatabaseSource, Reach};

/// The project's state folder keeps the pulls that went well here.
pub const FILE: &str = "pulls.json";
/// A pulled copy's day in its name.
const DAY: &str = "%Y%m%d";
/// What a script's batches are split at, as sqlcmd does: a line that's only `GO`.
const BATCH_END: &str = "go";
/// Reaching a source: encrypted, its certificate trusted (another environment's server seldom has a public one).
const SECURE: &str = "Encrypt=True;TrustServerCertificate=True";
/// The source's check: who lazyrunner is there.
pub const CHECK_SQL: &str = "SET NOCOUNT ON; SELECT SUSER_SNAME();";
/// The database a check and a listing run in.
pub const MASTER: &str = "master";
/// An updated database's old copy's name: `<name>_old_<yyyyMMdd>`.
const OLD_PART: &str = "_old_";

/// What a pull's steps make: a pull (new database, switched to), an import (new database, not switched to), an
/// export (the `.bacpac` only) or an update (a linked database's new copy swapped in under its name).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Pull,
    Import,
    Export,
    Update,
}

impl Mode {
    /// Its steps, in the order they run.
    pub fn steps(self) -> &'static [Step] {
        match self {
            Self::Pull => &[Step::Check, Step::Export, Step::Import, Step::AfterPull, Step::Migrations, Step::Switch],
            Self::Import => &[Step::Check, Step::Export, Step::Import, Step::AfterPull],
            Self::Export => &[Step::Check, Step::Export],
            Self::Update => &[Step::Check, Step::Export, Step::Import, Step::AfterPull, Step::Migrations, Step::Swap],
        }
    }

    /// What it's called: `pull`, `import`, `export`, `update`.
    pub fn name(self) -> &'static str {
        match self {
            Self::Pull => "pull",
            Self::Import => "import",
            Self::Export => "export",
            Self::Update => "update",
        }
    }

    /// Whether it makes a new database.
    pub fn imports(self) -> bool {
        self != Self::Export
    }
}

/// A pull's steps.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Step {
    Check,
    Export,
    Import,
    AfterPull,
    Migrations,
    Switch,
    Swap,
}

impl Step {
    pub const ALL: [Step; 7] =
        [Self::Check, Self::Export, Self::Import, Self::AfterPull, Self::Migrations, Self::Switch, Self::Swap];

    pub fn name(self) -> &'static str {
        match self {
            Self::Check => "check",
            Self::Export => "export",
            Self::Import => "import",
            Self::AfterPull => "after-pull",
            Self::Migrations => "migrations",
            Self::Switch => "switch",
            Self::Swap => "swap",
        }
    }

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|step| *step == self).unwrap_or_default()
    }

    /// Whether data flows while it runs (its dots go faster).
    pub fn flows(self) -> bool {
        matches!(self, Self::Export | Self::Import)
    }
}

/// Where a step is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum State {
    #[default]
    Waiting,
    Running,
    Done,
    Failed,
    Skipped,
}

/// A pull's news from its thread: a step started, a line it printed, the progress it reported, a step done (what
/// it says under its name) or skipped, and how the thread's steps ended.
#[derive(Debug)]
pub enum News {
    Started(Step),
    Line(Step, String),
    Progress(Progress),
    Done(Step, String),
    Skipped(Step, String),
    Ended(Result<(), (Step, String)>),
    /// The swap's batch is done, or why it failed (the names put back).
    Swapped(Result<(), String>),
}

/// A value as a connection string carries it: as it is, or in `'…'` (its `'` doubled) when it holds what would end it.
fn value(text: &str) -> String {
    let plain = !text.contains([';', '\'', '"', '=']) && text.trim() == text;
    if plain { text.to_string() } else { format!("'{}'", text.replace('\'', "''")) }
}

/// The connection string to a source, password and all - for sqlcmd and SqlPackage only, never to keep or show: a
/// variable's, or one built from its server, database (`master` for a whole server) and login. `env` reads a
/// variable.
pub fn connection(
    source: &DatabaseSource,
    password: Option<&Password>,
    env: impl Fn(&str) -> Option<String>,
) -> Result<String, String> {
    connection_to(source, None, password, env)
}

/// The connection string to `database` on a source's server (else its own database).
pub fn connection_to(
    source: &DatabaseSource,
    database: Option<&str>,
    password: Option<&Password>,
    env: impl Fn(&str) -> Option<String>,
) -> Result<String, String> {
    let (server, named, auth, user) = match &source.reach {
        Reach::Env(name) => {
            let text = env(name)
                .filter(|text| !text.trim().is_empty())
                .ok_or_else(|| format!("{name} isn't set - it holds {}'s connection string", source.name))?;
            return Ok(match database {
                Some(database) => super::sqlpackage::with_database(&text, database),
                None => text,
            });
        }
        Reach::Server { server, database, auth, user } => (server, database, auth, user),
    };
    let database = database.or(named.as_deref()).unwrap_or(MASTER);
    let start = format!("Server={};Database={}", value(server), value(database));
    match auth {
        Auth::Windows => Ok(format!("{start};Integrated Security=True;{SECURE}")),
        Auth::Sql => {
            let user = user.as_deref().ok_or_else(|| format!("{} has no user", source.name))?;
            let password = password.ok_or_else(|| format!("{}'s password isn't there", source.name))?;
            Ok(format!("{start};User ID={};Password={};{SECURE}", value(user), value(password.expose())))
        }
        Auth::Entra => Err(format!(
            "{} logs in with Microsoft Entra, which pulling doesn't support yet - give it connectionStringEnv, or a \
             SQL login (auth sql)",
            source.name
        )),
    }
}

/// A pulled copy's name: `Shop_qa_20261123`, made free on the server (`Shop_qa_202611232`...).
pub fn pulled_name(database: &str, source: &str, at: &jiff::ZonedDateTime, taken: &[String]) -> String {
    free_name(&format!("{database}_{source}_{}", at.strftime(DAY)), taken)
}

/// An updated database's old copy's name: `Shop_old_20261127`, made free on the server.
pub fn old_name(database: &str, at: &jiff::ZonedDateTime, taken: &[String]) -> String {
    free_name(&format!("{database}{OLD_PART}{}", at.strftime(DAY)), taken)
}

/// The swap, in one batch on the local server's `master`: the old database taken for itself (`SINGLE_USER`,
/// everyone else cut off) and renamed `old`, given back; then the new one taken and renamed to the old one's name,
/// given back. On any failure, what was done is undone in reverse - the names put back, both given back
/// (`MULTI_USER`) - and the error raised.
pub fn swap_sql(database: &str, new: &str, old: &str) -> String {
    let (database, new, old) = (quote_name(database), quote_name(new), quote_name(old));
    let undo = |stage: u8, sql: String| format!("IF @stage >= {stage} BEGIN TRY {sql} END TRY BEGIN CATCH END CATCH;");
    let single = |name: &str| format!("ALTER DATABASE {name} SET SINGLE_USER WITH ROLLBACK IMMEDIATE;");
    let multi = |name: &str| format!("ALTER DATABASE {name} SET MULTI_USER;");
    let rename = |from: &str, to: &str| format!("ALTER DATABASE {from} MODIFY NAME = {to};");
    [
        "SET NOCOUNT ON; DECLARE @stage int = 0; DECLARE @why nvarchar(4000); BEGIN TRY".to_string(),
        format!("{} SET @stage = 1;", single(&database)),
        format!("{} SET @stage = 2;", rename(&database, &old)),
        format!("{} SET @stage = 3;", multi(&old)),
        format!("{} SET @stage = 4;", single(&new)),
        format!("{} SET @stage = 5;", rename(&new, &database)),
        format!("{} SET @stage = 6;", multi(&database)),
        "END TRY BEGIN CATCH SET @why = ERROR_MESSAGE();".to_string(),
        undo(5, rename(&database, &new)),
        undo(4, multi(&new)),
        undo(2, rename(&old, &database)),
        undo(1, multi(&database)),
        "RAISERROR(@why, 16, 1); END CATCH".to_string(),
    ]
    .join("\n")
}

/// A database's name with `]` doubled, in brackets.
fn quote_name(name: &str) -> String {
    super::sql::quote_name(name)
}

/// A script's batches: its text split at `GO` lines, the empty ones left out.
pub fn batches(text: &str) -> Vec<String> {
    let mut found = vec![String::new()];
    for line in text.lines() {
        if line.trim().eq_ignore_ascii_case(BATCH_END) {
            found.push(String::new());
            continue;
        }
        if let Some(batch) = found.last_mut() {
            batch.push_str(line);
            batch.push('\n');
        }
    }
    found.into_iter().filter(|batch| !batch.trim().is_empty()).collect()
}

/// A pull that went well: from which source (and which of its server's databases, for a whole server), the copy it
/// made where, when (RFC 3339).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pulled {
    pub source: String,
    pub server: String,
    pub database: String,
    pub at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
}

/// The pulls that went well, oldest first; none when the file is missing or broken.
pub fn load(state_dir: &Path) -> Vec<Pulled> {
    match crate::json::read(&state_dir.join(FILE)) {
        Some(Value::Array(items)) => items.into_iter().filter_map(|item| serde_json::from_value(item).ok()).collect(),
        _ => Vec::new(),
    }
}

pub fn save(state_dir: &Path, all: &[Pulled]) -> Result<()> {
    crate::config::write(&state_dir.join(FILE), &serde_json::to_value(all)?)
}

/// The copies of `source` past the newest `keep` - never one `in_use` says is in use: what `keepPulled` offers to
/// drop, the oldest first.
pub fn beyond_kept(pulled: &[Pulled], source: &str, keep: usize, in_use: impl Fn(&Pulled) -> bool) -> Vec<Pulled> {
    let mut of: Vec<&Pulled> = pulled.iter().filter(|pull| pull.source.eq_ignore_ascii_case(source)).collect();
    of.sort_by(|a, b| b.at.cmp(&a.at));
    let mut older: Vec<Pulled> = of.into_iter().skip(keep).filter(|pull| !in_use(pull)).cloned().collect();
    older.reverse();
    older
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(reach: Reach) -> DatabaseSource {
        DatabaseSource::new("qa", reach)
    }

    fn server(auth: Auth, user: Option<&str>) -> Reach {
        Reach::Server {
            server: "qa-sql.example.com,1433".into(),
            database: Some("Shop".into()),
            auth,
            user: user.map(Into::into),
        }
    }

    #[test]
    fn a_sources_connection_string_carries_its_password_quoted_and_the_others_say_what_is_missing() {
        let password = Password::new("p;a's");
        let qa = connection(&source(server(Auth::Sql, Some("reader"))), Some(&password), |_| None).unwrap();
        assert_eq!(
            qa,
            "Server=qa-sql.example.com,1433;Database=Shop;User ID=reader;Password='p;a''s';Encrypt=True;\
             TrustServerCertificate=True"
        );
        let target = super::super::sql::target(&qa).unwrap();
        assert_eq!((target.user.as_deref(), target.integrated, target.trust), (Some("reader"), false, true));
        assert!(connection(&source(server(Auth::Sql, Some("reader"))), None, |_| None).is_err(), "no password");
        let windows = connection(&source(server(Auth::Windows, None)), None, |_| None).unwrap();
        assert!(windows.contains("Integrated Security=True") && !windows.contains("Password"));
        assert!(connection(&source(server(Auth::Entra, None)), None, |_| None).unwrap_err().contains("Entra"));
        let staging = source(Reach::Env("SHOP_STAGING_DB".into()));
        assert_eq!(
            connection(&staging, None, |_| Some("Server=s;Database=Shop".into())).unwrap(),
            "Server=s;Database=Shop"
        );
        assert!(connection(&staging, None, |_| None).unwrap_err().starts_with("SHOP_STAGING_DB isn't set"));
        let other = connection_to(&staging, Some("Shop_Audit"), None, |_| Some("Server=s;Database=Shop".into()));
        assert_eq!(other.unwrap(), "Server=s;Database=Shop_Audit");
        let Reach::Server { server, auth, user, .. } = server(Auth::Windows, None) else { unreachable!() };
        let whole = source(Reach::Server { server, database: None, auth, user });
        assert!(connection(&whole, None, |_| None).unwrap().contains(";Database=master;"), "a whole server: master");
        assert!(connection_to(&whole, Some("Shop_Audit"), None, |_| None).unwrap().contains(";Database=Shop_Audit;"));
    }

    #[test]
    fn each_mode_runs_its_own_steps_and_an_update_ends_in_the_swap() {
        assert_eq!(Mode::Export.steps(), [Step::Check, Step::Export]);
        assert_eq!(Mode::Import.steps().last(), Some(&Step::AfterPull));
        assert_eq!(Mode::Update.steps().last(), Some(&Step::Swap));
        assert_eq!(Mode::Pull.steps().last(), Some(&Step::Switch));
        assert!(!Mode::Export.imports() && Mode::Update.imports());
        let at = crate::time::as_written("2026-11-27T10:42:07+01:00").unwrap();
        assert_eq!(old_name("Shop", &at, &["shop_old_20261127".into()]), "Shop_old_202611272");
    }

    #[test]
    fn the_swap_renames_in_one_batch_and_puts_every_name_back_on_a_failure() {
        let batch = swap_sql("Shop", "Shop_qa_20261127", "Shop_old_20261127");
        let lines: Vec<&str> = batch.lines().collect();
        assert_eq!(lines[1], "ALTER DATABASE [Shop] SET SINGLE_USER WITH ROLLBACK IMMEDIATE; SET @stage = 1;");
        assert_eq!(lines[2], "ALTER DATABASE [Shop] MODIFY NAME = [Shop_old_20261127]; SET @stage = 2;");
        assert_eq!(lines[5], "ALTER DATABASE [Shop_qa_20261127] MODIFY NAME = [Shop]; SET @stage = 5;");
        assert_eq!(lines[6], "ALTER DATABASE [Shop] SET MULTI_USER; SET @stage = 6;");
        let undo: Vec<&str> = lines.iter().filter(|line| line.starts_with("IF @stage")).copied().collect();
        assert_eq!(undo.len(), 4, "each step that changes a name or a mode is undone");
        assert!(undo[0].contains("ALTER DATABASE [Shop] MODIFY NAME = [Shop_qa_20261127]"), "the new one's name back");
        assert!(undo[2].contains("ALTER DATABASE [Shop_old_20261127] MODIFY NAME = [Shop]"), "the old one's name back");
        assert!(undo[3].contains("ALTER DATABASE [Shop] SET MULTI_USER"), "and given back");
        assert!(batch.ends_with("RAISERROR(@why, 16, 1); END CATCH"));
        assert!(swap_sql("a]b", "n", "o").contains("[a]]b]"), "names quoted");
    }

    #[test]
    fn a_pulled_copy_is_named_by_its_database_source_and_day_and_is_never_one_thats_there() {
        let at = crate::time::as_written("2026-11-23T10:42:07+01:00").unwrap();
        assert_eq!(pulled_name("Shop", "qa", &at, &[]), "Shop_qa_20261123");
        assert_eq!(pulled_name("Shop", "qa", &at, &["shop_QA_20261123".into()]), "Shop_qa_202611232");
    }

    #[test]
    fn a_script_runs_batch_by_batch_split_at_its_go_lines() {
        let script = "UPDATE Settings SET Url = 'http://localhost';\nGO\n\n  go  \nDELETE FROM Logins;\nSELECT 1;\nGO";
        assert_eq!(
            batches(script),
            ["UPDATE Settings SET Url = 'http://localhost';\n", "DELETE FROM Logins;\nSELECT 1;\n"]
        );
        assert!(batches("GO\n").is_empty());
    }

    #[test]
    fn keep_pulled_offers_the_older_copies_of_that_source_never_the_one_in_use() {
        let pull = |source: &str, database: &str, at: &str| Pulled {
            source: source.into(),
            server: "localhost".into(),
            database: database.into(),
            at: at.into(),
            remote: None,
        };
        let pulled = vec![
            pull("qa", "Shop_qa_20261101", "2026-11-01T09:00:00Z"),
            pull("qa", "Shop_qa_20261110", "2026-11-10T09:00:00Z"),
            pull("staging", "Shop_staging_20261102", "2026-11-02T09:00:00Z"),
            pull("qa", "Shop_qa_20261120", "2026-11-20T09:00:00Z"),
            pull("qa", "Shop_qa_20261105", "2026-11-05T09:00:00Z"),
        ];
        let older = beyond_kept(&pulled, "qa", 2, |pull| pull.database == "Shop_qa_20261105");
        let names: Vec<&str> = older.iter().map(|pull| pull.database.as_str()).collect();
        assert_eq!(names, ["Shop_qa_20261101"], "the newest two kept, the one in use never offered");
        assert!(beyond_kept(&pulled, "staging", 1, |_| false).is_empty());
        let temp = crate::testing::TempDir::new("pulls");
        save(temp.path(), &pulled).unwrap();
        assert_eq!(load(temp.path()), pulled);
    }
}
