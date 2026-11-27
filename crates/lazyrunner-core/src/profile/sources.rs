//! Where a database is pulled from (#110): the profile's `databaseSources`, each another environment's database by
//! its server, database and login - or a whole connection string in an environment variable - and never a password.
//! Without a database a source is a whole server, whose databases are listed and picked (#114); the same list under
//! the project in lazyrunner.config.json is this machine's only, and wins by name. A password is asked for once and
//! kept in the OS's credential store (`run::credentials`), one per server and login. Beside them: the scripts that
//! run on a pulled copy (`afterPull`), how many pulled copies are kept (`keepPulled`) and whether its pending
//! migrations are applied (`migrateAfterPull`).

use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

use crate::json::{field, is_integer, list, text};

/// The fields of a source, and its `auth`s.
pub const FIELDS: &[&str] = &["name", "server", "database", "auth", "user", "connectionStringEnv"];
pub const AUTHS: [&str; 3] = ["sql", "windows", "entra"];
/// Where this machine's own sources are, in its project's section of lazyrunner.config.json.
pub const MINE: &str = "databaseSources";
/// `keepPulled` keeps at least one: the copy just pulled.
const KEEP_LEAST: u64 = 1;
/// What a suggested name is when the server gives none.
const FALLBACK_NAME: &str = "source";

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

/// How lazyrunner reaches a source: its server, database (none: the whole server) and login, or a connection string
/// in a variable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reach {
    Server { server: String, database: Option<String>, auth: Auth, user: Option<String> },
    Env(String),
}

/// A database to pull a copy of, or a server to pick one on: its name, how it's reached, and whether it's this
/// machine's only (lazyrunner.config.json) rather than the team's (the profile).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DatabaseSource {
    pub name: String,
    pub reach: Reach,
    pub mine: bool,
}

impl DatabaseSource {
    pub fn new(name: impl Into<String>, reach: Reach) -> Self {
        Self { name: name.into(), reach, mine: false }
    }

    /// Whether a pull needs a password that lazyrunner keeps: a SQL login's.
    pub fn needs_password(&self) -> bool {
        matches!(self.reach, Reach::Server { auth: Auth::Sql, .. })
    }

    /// Where it is, as lazyrunner says it - never a password: `Shop @ qa-sql.example.com`, the server alone for a
    /// whole server, or the variable's name.
    pub fn label(&self) -> String {
        match &self.reach {
            Reach::Server { server, database: Some(database), .. } => format!("{database} @ {server}"),
            Reach::Server { server, .. } => server.clone(),
            Reach::Env(name) => format!("${name}"),
        }
    }

    /// Its database, when it names one.
    pub fn database(&self) -> Option<&str> {
        match &self.reach {
            Reach::Server { database, .. } => database.as_deref(),
            Reach::Env(_) => None,
        }
    }

    /// Its server, when it names one.
    pub fn server(&self) -> Option<&str> {
        match &self.reach {
            Reach::Server { server, .. } => Some(server),
            Reach::Env(_) => None,
        }
    }

    /// Whether it's a whole server, its databases picked from a list.
    pub fn is_server(&self) -> bool {
        matches!(self.reach, Reach::Server { database: None, .. })
    }

    /// The same source on another of its server's databases.
    pub fn on(&self, database: &str) -> Self {
        let mut on = self.clone();
        if let Reach::Server { database: named, .. } = &mut on.reach {
            *named = Some(database.to_string());
        }
        on
    }

    /// Its login and server, as the password kept for them is known (`reader@qa-sql.example.com,1433`): sources with
    /// the same share it.
    pub fn login(&self) -> Option<String> {
        match &self.reach {
            Reach::Server { server, auth: Auth::Sql, user: Some(user), .. } => {
                Some(format!("{}@{}", user.trim().to_lowercase(), server.trim().to_lowercase()))
            }
            _ => None,
        }
    }

    /// As the profile writes it.
    pub fn to_json(&self) -> Value {
        let mut out = serde_json::Map::new();
        out.insert("name".into(), Value::from(self.name.as_str()));
        match &self.reach {
            Reach::Env(variable) => {
                out.insert("connectionStringEnv".into(), Value::from(variable.as_str()));
            }
            Reach::Server { server, database, auth, user } => {
                out.insert("server".into(), Value::from(server.as_str()));
                if let Some(database) = database {
                    out.insert("database".into(), Value::from(database.as_str()));
                }
                out.insert("auth".into(), Value::from(auth.name()));
                if let Some(user) = user {
                    out.insert("user".into(), Value::from(user.as_str()));
                }
            }
        }
        Value::Object(out)
    }
}

/// Whether a source's name may be used: lowercase letters, digits, `-` and `_`.
pub fn valid_name(name: &str) -> bool {
    NAME.is_match(name)
}

/// A source's name from its server, as the add form suggests it: its host's first part, lowercase, without `sql`
/// around it (`qa-sql.example.com` is `qa`), and not one of `taken`.
pub fn suggest_name(server: &str, taken: &[String]) -> String {
    let server = server.trim();
    let server = server.strip_prefix("tcp:").unwrap_or(server);
    let host = server.split([',', '\\', ':', '.']).next().unwrap_or_default();
    let mut name: String = host
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
        .collect();
    for part in ["-sql", "_sql"] {
        if let Some(cut) = name.strip_suffix(part).filter(|cut| !cut.is_empty()) {
            name = cut.to_string();
        }
    }
    for part in ["sql-", "sql_"] {
        if let Some(cut) = name.strip_prefix(part).filter(|cut| !cut.is_empty()) {
            name = cut.to_string();
        }
    }
    let name = name.trim_matches(['-', '_']).to_string();
    let name = if valid_name(&name) { name } else { FALLBACK_NAME.to_string() };
    free(&name, taken)
}

/// `name`, else `name-2`, `name-3`... - one `taken` doesn't have (ignoring case).
pub fn free(name: &str, taken: &[String]) -> String {
    let used = |candidate: &str| taken.iter().any(|known| known.eq_ignore_ascii_case(candidate));
    if !used(name) {
        return name.to_string();
    }
    (2..).map(|n| format!("{name}-{n}")).find(|candidate| !used(candidate)).unwrap_or_default()
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
        sources: to_sources(field(data, "databaseSources")),
        after_pull: list(field(data, "afterPull")).into_iter().map(text).filter(|path| !path.is_empty()).collect(),
        keep_pulled: field(data, "keepPulled").as_u64().map(|keep| keep.max(KEEP_LEAST) as usize),
        migrate: field(data, "migrateAfterPull").as_bool().unwrap_or(false),
    }
}

/// A list of sources as written: the profile's, or this machine's (each with a name; the others are left out).
pub fn to_sources(sources: &Value) -> Vec<DatabaseSource> {
    list(sources).into_iter().filter_map(to_source).collect()
}

/// The sources this machine has: the profile's, then `mine` (lazyrunner.config.json's) - one of mine replacing the
/// profile's of the same name.
pub fn merged(team: &[DatabaseSource], mine: &Value) -> Vec<DatabaseSource> {
    let mine: Vec<DatabaseSource> =
        to_sources(mine).into_iter().map(|source| DatabaseSource { mine: true, ..source }).collect();
    let mut all: Vec<DatabaseSource> = team
        .iter()
        .filter(|source| !mine.iter().any(|own| own.name.eq_ignore_ascii_case(&source.name)))
        .cloned()
        .collect();
    all.extend(mine);
    all
}

fn to_source(source: &Value) -> Option<DatabaseSource> {
    let name = text(field(source, "name"));
    if name.is_empty() {
        return None;
    }
    let variable = text(field(source, "connectionStringEnv"));
    if !variable.is_empty() {
        return Some(DatabaseSource::new(name, Reach::Env(variable)));
    }
    let user = Some(text(field(source, "user"))).filter(|user| !user.is_empty());
    let auth = match text(field(source, "auth")).as_str() {
        "sql" => Auth::Sql,
        "entra" => Auth::Entra,
        "windows" => Auth::Windows,
        _ if user.is_some() => Auth::Sql,
        _ => Auth::Windows,
    };
    let server = text(field(source, "server"));
    let database = Some(text(field(source, "database"))).filter(|database| !database.is_empty());
    Some(DatabaseSource::new(name, Reach::Server { server, database, auth, user }))
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

/// Every problem in a list of sources, as lazyrunner.config.json's or a form's are checked.
pub fn check_list(sources: &Value) -> Vec<String> {
    let mut problems = Vec::new();
    check_sources(sources, &mut problems);
    problems
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
            problems.push(format!("{at} must be an object: its name, and its server (and a database, or none)"));
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

/// A source is reached by its server (and a database, or none for the whole server), or by a variable - one of them.
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
    let database = field(source, "database");
    if !database.is_null()
        && let Err(why) = crate::run::database_files::valid_name(&text(database))
    {
        problems.push(format!("{at}.database: {why} - leave it out for the whole server"));
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
                { "name": "qa-all", "server": "qa-sql.example.com,1433", "user": "Reader" },
            ],
            "afterPull": ["scripts/local-urls.sql"],
            "keepPulled": 2,
        }));
        assert_eq!(pulling.sources.len(), 4);
        let qa = pulling.source("QA").unwrap();
        assert!(qa.needs_password() && qa.label() == "Shop @ qa-sql.example.com,1433" && !qa.is_server());
        assert_eq!(pulling.source("staging").unwrap().reach, Reach::Env("SHOP_STAGING_DB".into()));
        assert!(matches!(pulling.source("dev").unwrap().reach, Reach::Server { auth: Auth::Windows, .. }));
        let all = pulling.source("qa-all").unwrap();
        assert!(all.is_server() && all.label() == "qa-sql.example.com,1433" && all.database().is_none());
        assert_eq!(all.login(), qa.login(), "one server and login: one password");
        assert_eq!(all.on("Shop_Audit").label(), "Shop_Audit @ qa-sql.example.com,1433");
        assert_eq!((pulling.after_pull.len(), pulling.keep_pulled, pulling.migrate), (1, Some(2), false));
        assert_eq!(to_pulling(&json!({})), Pulling::default());
        assert_eq!(to_sources(&json!([qa.to_json(), all.to_json()])), [qa.clone(), all.clone()], "written as read");
    }

    #[test]
    fn this_machines_sources_join_the_teams_and_win_by_name() {
        let team = to_sources(&json!([{ "name": "qa", "server": "a" }, { "name": "dev", "server": "b" }]));
        let all = merged(&team, &json!([{ "name": "QA", "server": "mine" }, { "name": "local2", "server": "c" }]));
        let names: Vec<(&str, bool)> = all.iter().map(|source| (source.name.as_str(), source.mine)).collect();
        assert_eq!(names, [("dev", false), ("QA", true), ("local2", true)]);
        assert_eq!(all[1].server(), Some("mine"));
    }

    #[test]
    fn a_suggested_name_comes_from_the_server_and_is_free() {
        assert_eq!(suggest_name("qa-sql.example.com,1433", &[]), "qa");
        assert_eq!(suggest_name("tcp:SQL-Staging\\INST", &[]), "staging");
        assert_eq!(suggest_name("qa-sql", &["qa".into(), "QA-2".into()]), "qa-3");
        assert_eq!(suggest_name("(localdb)", &[]), "localdb");
        assert_eq!(suggest_name("", &[]), "source");
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
            "- leave it out for the whole server",
            "databaseSources[2].auth must be one of sql, windows, entra",
            "afterPull[0] must be relative to the repo root",
            "keepPulled must be a whole number",
            "migrateAfterPull must be true or false",
        ] {
            assert!(said.contains(expected), "{expected}\n---\n{said}");
        }
        let mut none = Vec::new();
        let both = json!({
            "databaseSources": [{ "name": "qa", "server": "qa", "database": "Shop" }, { "name": "all", "server": "qa" }]
        });
        check(&both, &mut none);
        assert!(none.is_empty(), "{none:?}");
        assert!(check_list(&json!([{ "name": "x" }]))[0].contains("server is required"));
    }
}
