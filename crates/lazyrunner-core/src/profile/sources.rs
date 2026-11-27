//! Where a database is pulled from (#110): the profile's `databaseSources`, each another environment's database by
//! its server, database and login - or a whole connection string in an environment variable - and never a password.
//! A password is asked for once and kept in the OS's credential store (`run::credentials`). Beside them: the scripts
//! that run on a pulled copy (`afterPull`), how many pulled copies are kept (`keepPulled`) and whether its pending
//! migrations are applied (`migrateAfterPull`).

use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

use crate::json::{field, is_integer, list, text};

/// The fields of a source, and its `auth`s.
pub const FIELDS: &[&str] = &["name", "server", "database", "auth", "user", "connectionStringEnv"];
pub const AUTHS: [&str; 3] = ["sql", "windows", "entra"];
/// `keepPulled` keeps at least one: the copy just pulled.
const KEEP_LEAST: u64 = 1;

/// A source's name: what a pulled copy's name and its password's place carry - lowercase letters, digits, `-`, `_`.
static NAME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[a-z0-9][a-z0-9_-]*$").expect("a valid pattern"));
static VARIABLE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z_][A-Za-z0-9_]*$").expect("a valid pattern"));

/// How a source's login works: a SQL login (a user and a password), Windows authentication, or Microsoft Entra.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Auth {
    Sql,
    Windows,
    Entra,
}

impl Auth {
    pub fn name(self) -> &'static str {
        match self {
            Self::Sql => "sql",
            Self::Windows => "windows",
            Self::Entra => "entra",
        }
    }
}

/// How lazyrunner reaches a source: its server, database and login, or a connection string in a variable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reach {
    Server { server: String, database: String, auth: Auth, user: Option<String> },
    Env(String),
}

/// A database to pull a copy of: its name in the profile, and how it's reached.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DatabaseSource {
    pub name: String,
    pub reach: Reach,
}

impl DatabaseSource {
    /// Whether a pull needs a password that lazyrunner keeps: a SQL login's.
    pub fn needs_password(&self) -> bool {
        matches!(self.reach, Reach::Server { auth: Auth::Sql, .. })
    }

    /// Where it is, as lazyrunner says it - never a password: `Shop @ qa-sql.example.com`, or the variable's name.
    pub fn label(&self) -> String {
        match &self.reach {
            Reach::Server { server, database, .. } => format!("{database} @ {server}"),
            Reach::Env(name) => format!("${name}"),
        }
    }
}

/// What the profile says about pulling: its sources, the scripts after a pull, the copies kept, the migrations.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Pulling {
    pub sources: Vec<DatabaseSource>,
    pub after_pull: Vec<String>,
    pub keep_pulled: Option<usize>,
    pub migrate: bool,
}

impl Pulling {
    pub fn source(&self, name: &str) -> Option<&DatabaseSource> {
        self.sources.iter().find(|source| source.name.eq_ignore_ascii_case(name))
    }
}

/// The checked profile's sources and pull settings.
pub fn to_pulling(data: &Value) -> Pulling {
    Pulling {
        sources: list(field(data, "databaseSources")).into_iter().filter_map(to_source).collect(),
        after_pull: list(field(data, "afterPull")).into_iter().map(text).filter(|path| !path.is_empty()).collect(),
        keep_pulled: field(data, "keepPulled").as_u64().map(|keep| keep.max(KEEP_LEAST) as usize),
        migrate: field(data, "migrateAfterPull").as_bool().unwrap_or(false),
    }
}

fn to_source(source: &Value) -> Option<DatabaseSource> {
    let name = text(field(source, "name"));
    let variable = text(field(source, "connectionStringEnv"));
    if !variable.is_empty() {
        return Some(DatabaseSource { name, reach: Reach::Env(variable) });
    }
    let user = Some(text(field(source, "user"))).filter(|user| !user.is_empty());
    let auth = match text(field(source, "auth")).as_str() {
        "sql" => Auth::Sql,
        "entra" => Auth::Entra,
        "windows" => Auth::Windows,
        _ if user.is_some() => Auth::Sql,
        _ => Auth::Windows,
    };
    let (server, database) = (text(field(source, "server")), text(field(source, "database")));
    Some(DatabaseSource { name, reach: Reach::Server { server, database, auth, user } })
}

/// Every problem in the profile's sources and pull settings, each naming its field.
pub fn check(data: &Value, problems: &mut Vec<String>) {
    check_sources(field(data, "databaseSources"), problems);
    let after = field(data, "afterPull");
    if !after.is_null() {
        match after.as_array() {
            Some(paths) => {
                for (index, path) in paths.iter().enumerate() {
                    super::check::relative_path(path, &format!("afterPull[{index}]"), problems);
                }
            }
            None => {
                problems.push("afterPull must list .sql files from the repo root, e.g. [\"scripts/local.sql\"]".into())
            }
        }
    }
    let keep = field(data, "keepPulled");
    if !keep.is_null() && !(is_integer(keep) && keep.as_u64().is_some_and(|keep| keep >= KEEP_LEAST)) {
        problems.push("keepPulled must be a whole number, 1 or more: how many pulled copies are kept".into());
    }
    let migrate = field(data, "migrateAfterPull");
    if !migrate.is_null() && !migrate.is_boolean() {
        problems.push("migrateAfterPull must be true or false".into());
    }
}

fn check_sources(sources: &Value, problems: &mut Vec<String>) {
    if sources.is_null() {
        return;
    }
    let Some(sources) = sources.as_array() else {
        problems.push(
            "databaseSources must be a list, e.g. [{ \"name\": \"qa\", \"server\": \"qa-sql.example.com\", \
             \"database\": \"Shop\", \"auth\": \"sql\", \"user\": \"reader\" }]"
                .into(),
        );
        return;
    };
    let mut names = std::collections::HashSet::new();
    for (index, source) in sources.iter().enumerate() {
        let mut at = format!("databaseSources[{index}]");
        if !source.is_object() {
            problems.push(format!("{at} must be an object: its name, and its server and database"));
            continue;
        }
        let name = text(field(source, "name"));
        if !NAME.is_match(&name) {
            problems.push(format!("{at}.name is required: lowercase letters, digits, - and _, e.g. qa"));
        } else if !names.insert(name.clone()) {
            problems.push(format!("{at}.name '{name}' is used twice"));
        } else {
            at = format!("{at} ({name})");
        }
        super::check::known_fields(source, FIELDS, &at, problems);
        check_reach(source, &at, problems);
    }
}

/// A source is reached by its server and database, or by a variable - one of them.
fn check_reach(source: &Value, at: &str, problems: &mut Vec<String>) {
    let variable = field(source, "connectionStringEnv");
    let named = ["server", "database", "auth", "user"].into_iter().filter(|name| !field(source, name).is_null());
    let named: Vec<&str> = named.collect();
    if !variable.is_null() {
        if !variable.as_str().is_some_and(|name| VARIABLE.is_match(name)) {
            problems.push(format!("{at}.connectionStringEnv must name an environment variable, e.g. SHOP_QA_DB"));
        }
        if !named.is_empty() {
            problems.push(format!("{at} has connectionStringEnv: it can't have {} too", named.join(", ")));
        }
        return;
    }
    if text(field(source, "server")).trim().is_empty() {
        problems.push(format!("{at}.server is required (or connectionStringEnv), e.g. qa-sql.example.com,1433"));
    }
    let database = text(field(source, "database"));
    if let Err(why) = crate::run::database_files::valid_name(&database) {
        problems.push(format!("{at}.database: {why}"));
    }
    let auth = field(source, "auth");
    if !auth.is_null() && !auth.as_str().is_some_and(|auth| AUTHS.contains(&auth)) {
        problems.push(format!("{at}.auth must be one of {}", AUTHS.join(", ")));
    }
    let user = field(source, "user");
    if !user.is_null() && text(user).trim().is_empty() {
        problems.push(format!("{at}.user must be a login's name"));
    }
    if auth.as_str() == Some("sql") && user.is_null() {
        problems.push(format!("{at}.user is required with auth sql: the login's name (its password is asked for)"));
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn sources_read_as_a_server_and_login_or_a_variable_and_never_carry_a_password() {
        let pulling = to_pulling(&json!({
            "databaseSources": [
                { "name": "qa", "server": "qa-sql.example.com,1433", "database": "Shop", "auth": "sql", "user": "reader" },
                { "name": "staging", "connectionStringEnv": "SHOP_STAGING_DB" },
                { "name": "dev", "server": "dev-sql", "database": "Shop" },
            ],
            "afterPull": ["scripts/local-urls.sql"],
            "keepPulled": 2,
        }));
        assert_eq!(pulling.sources.len(), 3);
        let qa = pulling.source("QA").unwrap();
        assert!(qa.needs_password() && qa.label() == "Shop @ qa-sql.example.com,1433");
        assert_eq!(pulling.source("staging").unwrap().reach, Reach::Env("SHOP_STAGING_DB".into()));
        assert!(matches!(pulling.source("dev").unwrap().reach, Reach::Server { auth: Auth::Windows, .. }));
        assert_eq!((pulling.after_pull.len(), pulling.keep_pulled, pulling.migrate), (1, Some(2), false));
        assert_eq!(to_pulling(&json!({})), Pulling::default());
    }

    #[test]
    fn every_problem_in_the_sources_names_its_field() {
        let mut problems = Vec::new();
        check(
            &json!({
                "databaseSources": [
                    { "name": "qa", "server": "qa-sql", "database": "Shop", "auth": "sql" },
                    { "name": "qa", "connectionStringEnv": "SHOP DB", "server": "x" },
                    { "name": "Bad Name", "server": "", "database": "a;b", "auth": "kerberos", "password": "x" },
                ],
                "afterPull": ["C:/scripts/x.sql"],
                "keepPulled": 0,
                "migrateAfterPull": "yes",
            }),
            &mut problems,
        );
        let said = problems.join("\n");
        for expected in [
            "databaseSources[0] (qa).user is required with auth sql",
            "databaseSources[1].name 'qa' is used twice",
            "databaseSources[1].connectionStringEnv must name an environment variable",
            "databaseSources[1] has connectionStringEnv: it can't have server too",
            "databaseSources[2].name is required",
            "databaseSources[2].password is not a profile field",
            "databaseSources[2].server is required",
            "databaseSources[2].database: 'a;b' can't be a database name",
            "databaseSources[2].auth must be one of sql, windows, entra",
            "afterPull[0] must be relative to the repo root",
            "keepPulled must be a whole number",
            "migrateAfterPull must be true or false",
        ] {
            assert!(said.contains(expected), "{expected}\n---\n{said}");
        }
        let mut none = Vec::new();
        check(&json!({ "databaseSources": [{ "name": "qa", "server": "qa", "database": "Shop" }] }), &mut none);
        assert!(none.is_empty(), "{none:?}");
    }
}
