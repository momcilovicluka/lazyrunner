//! Pulling a copy of a database from another environment (#110): its steps, the connection string to the source
//! (built from the profile and a password from the credential store, never kept), the pulled copy's name
//! (`<database>_<source>_<yyyyMMdd>`), the after-pull scripts' batches, and which older copies `keepPulled` would drop.
//! The steps run in the controller (`controller::pull`).

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

/// A pull's steps, in the order they run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Step {
    Check,
    Export,
    Import,
    AfterPull,
    Migrations,
    Switch,
}

impl Step {
    pub const ALL: [Step; 6] =
        [Self::Check, Self::Export, Self::Import, Self::AfterPull, Self::Migrations, Self::Switch];

    pub fn name(self) -> &'static str {
        match self {
            Self::Check => "check",
            Self::Export => "export",
            Self::Import => "import",
            Self::AfterPull => "after-pull",
            Self::Migrations => "migrations",
            Self::Switch => "switch",
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
}

/// A value as a connection string carries it: as it is, or in `'…'` (its `'` doubled) when it holds what would end it.
fn value(text: &str) -> String {
    let plain = !text.contains([';', '\'', '"', '=']) && text.trim() == text;
    if plain { text.to_string() } else { format!("'{}'", text.replace('\'', "''")) }
}

/// The connection string to a source, password and all - for sqlcmd and SqlPackage only, never to keep or show: a
/// variable's, or one built from its server, database and login. `env` reads a variable.
pub fn connection(
    source: &DatabaseSource,
    password: Option<&Password>,
    env: impl Fn(&str) -> Option<String>,
) -> Result<String, String> {
    let (server, database, auth, user) = match &source.reach {
        Reach::Env(name) => {
            return env(name)
                .filter(|text| !text.trim().is_empty())
                .ok_or_else(|| format!("{name} isn't set - it holds {}'s connection string", source.name));
        }
        Reach::Server { server, database, auth, user } => (server, database, auth, user),
    };
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

/// A pull that went well: from which source, the copy it made where, when (RFC 3339).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pulled {
    pub source: String,
    pub server: String,
    pub database: String,
    pub at: String,
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
        DatabaseSource { name: "qa".into(), reach }
    }

    fn server(auth: Auth, user: Option<&str>) -> Reach {
        Reach::Server {
            server: "qa-sql.example.com,1433".into(),
            database: "Shop".into(),
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
