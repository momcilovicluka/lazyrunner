//! SQL Server through `sqlcmd` (the ODBC one or go-sqlcmd, whichever is on PATH): a query's rows, or a statement's
//! messages line by line as they come (`WITH STATS` says "10 percent processed."). The server, database and login
//! come from a connection string; its password goes to sqlcmd in `SQLCMDPASSWORD`, never on its command line, and
//! nothing said back names it. sqlcmd runs in a hidden console of its own, so it can't retitle lazyrunner's tab.

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

use super::cancel::{self, CANCELLED};
use super::secrets::mask_text;
use super::tools;

/// The program, as PATH names it.
pub const PROGRAM: &str = "sqlcmd";
/// What lazyrunner says when it isn't there.
pub const MISSING: &str = "sqlcmd isn't installed (or isn't on PATH) - save points run SQL through it: winget install \
                           Microsoft.Sqlcmd";
/// What installs it (the doctor's `f` copies it: winget may ask for admin rights).
pub const INSTALL: &str = "winget install Microsoft.Sqlcmd";
/// What `-?` starts its version with.
const VERSION_PREFIX: &str = "Version ";
/// Where sqlcmd reads the password of a SQL login from.
pub const PASSWORD_VARIABLE: &str = "SQLCMDPASSWORD";
/// Between a row's columns: put the column that can hold anything last.
pub const SEPARATOR: &str = "|";
/// How long a login may take, in seconds.
const LOGIN_SECONDS: &str = "10";
/// Output in UTF-8, so table names read as they are.
const UTF8: &str = "65001";
/// What starts the lines sqlcmd reports an error with: a server's `Msg 3101, Level 16, ...` (the message on the next
/// line), or its own `Sqlcmd: Error: ...`.
const SERVER_ERROR: &str = "Msg ";
const SQLCMD_ERROR: &str = "Sqlcmd:";
/// The words of a connection string's keys, and the values that mean yes.
const SERVER_KEYS: [&str; 5] = ["server", "datasource", "address", "addr", "networkaddress"];
const DATABASE_KEYS: [&str; 2] = ["database", "initialcatalog"];
const INTEGRATED_KEYS: [&str; 2] = ["integratedsecurity", "trustedconnection"];
const USER_KEYS: [&str; 3] = ["userid", "uid", "user"];
const PASSWORD_KEYS: [&str; 2] = ["password", "pwd"];
const TRUST_KEYS: [&str; 1] = ["trustservercertificate"];
const ENCRYPT_KEYS: [&str; 1] = ["encrypt"];
const YES: [&str; 5] = ["true", "yes", "sspi", "mandatory", "strict"];

/// sqlcmd's version, from the line `-?` prints it on.
pub fn version_of(line: &str) -> String {
    line.trim_start_matches(VERSION_PREFIX).to_string()
}

/// Where a connection string connects, and as whom.
#[derive(Clone, PartialEq, Eq)]
pub struct Target {
    pub server: String,
    pub database: Option<String>,
    /// Windows authentication (`-E`); else a SQL login.
    pub integrated: bool,
    pub user: Option<String>,
    password: Option<String>,
    /// TrustServerCertificate (`-C`) and Encrypt (`-N`).
    pub trust: bool,
    pub encrypt: bool,
}

impl std::fmt::Debug for Target {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("Target")
            .field("server", &self.server)
            .field("database", &self.database)
            .field("integrated", &self.integrated)
            .field("user", &self.user)
            .field("trust", &self.trust)
            .field("encrypt", &self.encrypt)
            .finish_non_exhaustive()
    }
}

/// A connection string's `key=value` parts, the keys lowercase without spaces or underscores, quoted values
/// (`'...'`, `"..."`) unquoted.
fn parts(connection: &str) -> Vec<(String, String)> {
    let mut found = Vec::new();
    let mut chars = connection.chars().peekable();
    loop {
        let mut key = String::new();
        for c in chars.by_ref() {
            if c == '=' {
                break;
            }
            key.push(c);
        }
        let key: String = key.chars().filter(|c| !c.is_whitespace() && *c != '_').collect::<String>().to_lowercase();
        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }
        let mut value = String::new();
        match chars.peek().copied() {
            Some(quote @ ('\'' | '"')) => {
                chars.next();
                while let Some(c) = chars.next() {
                    if c == quote {
                        if chars.peek() == Some(&quote) {
                            chars.next();
                            value.push(quote);
                            continue;
                        }
                        break;
                    }
                    value.push(c);
                }
                for c in chars.by_ref() {
                    if c == ';' {
                        break;
                    }
                }
            }
            _ => {
                for c in chars.by_ref() {
                    if c == ';' {
                        break;
                    }
                    value.push(c);
                }
            }
        }
        if !key.is_empty() {
            found.push((key, value.trim().to_string()));
        }
        if chars.peek().is_none() {
            return found;
        }
    }
}

/// The server, database and login a connection string names; an error when it names no server.
pub fn target(connection: &str) -> Result<Target, String> {
    let parts = parts(connection);
    let value = |keys: &[&str]| {
        parts.iter().rev().find(|(key, _)| keys.contains(&key.as_str())).map(|(_, value)| value.clone())
    };
    let yes = |keys: &[&str]| value(keys).is_some_and(|value| YES.contains(&value.to_lowercase().as_str()));
    let server = value(&SERVER_KEYS).filter(|server| !server.is_empty());
    let server = server.ok_or_else(|| "the connection string names no server".to_string())?;
    let user = value(&USER_KEYS).filter(|user| !user.is_empty());
    Ok(Target {
        server,
        database: value(&DATABASE_KEYS).filter(|database| !database.is_empty()),
        integrated: yes(&INTEGRATED_KEYS) || user.is_none(),
        user,
        password: value(&PASSWORD_KEYS),
        trust: yes(&TRUST_KEYS),
        encrypt: yes(&ENCRYPT_KEYS),
    })
}

/// sqlcmd's arguments and environment for `sql` on `target`, in `database` (else the connection string's): the
/// password, when there is one, in the environment only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Invocation {
    pub args: Vec<String>,
    /// What the child's environment gets (`Some`) or loses (`None`).
    pub env: Vec<(String, Option<String>)>,
}

pub fn invocation(target: &Target, database: Option<&str>, sql: &str) -> Invocation {
    let fixed = ["-l", LOGIN_SECONDS, "-b", "-h", "-1", "-W", "-s", SEPARATOR, "-f", UTF8];
    connected(target, database, &fixed, sql)
}

/// sqlcmd's arguments and environment for a query an agent asks for (#112): each value whole as the server sends it
/// (`-y 0`: no header, no padding, nothing cut under 1 MB), the query given up after `seconds`.
pub fn read_invocation(target: &Target, database: Option<&str>, sql: &str, seconds: u32) -> Invocation {
    let seconds = seconds.to_string();
    let fixed = ["-l", LOGIN_SECONDS, "-b", "-y", "0", "-f", UTF8, "-t", seconds.as_str()];
    connected(target, database, &fixed, sql)
}

/// sqlcmd's arguments to reach `target` (in `database`, else its own), then `fixed`, then `sql`.
fn connected(target: &Target, database: Option<&str>, fixed: &[&str], sql: &str) -> Invocation {
    let mut args: Vec<String> = vec!["-S".into(), target.server.clone()];
    if let Some(database) = database.or(target.database.as_deref()) {
        args.extend(["-d".into(), database.to_string()]);
    }
    match (&target.user, target.integrated) {
        (Some(user), false) => args.extend(["-U".into(), user.clone()]),
        _ => args.push("-E".into()),
    }
    if target.trust {
        args.push("-C".into());
    }
    if target.encrypt {
        args.push("-N".into());
    }
    args.extend(fixed.iter().map(|arg| arg.to_string()));
    args.extend(["-Q".into(), sql.to_string()]);
    let password = target.password.clone().filter(|_| !target.integrated);
    Invocation { args, env: vec![(PASSWORD_VARIABLE.into(), password)] }
}

/// What runs SQL: sqlcmd, or the tests' stand-in.
pub trait Sql: Send + Sync {
    /// Runs `sql` against what `connection` names, in `database` (else its own), each line it prints (messages and
    /// rows) to `line` as it comes; the error it reported when it failed.
    fn run(
        &self,
        connection: &str,
        database: Option<&str>,
        sql: &str,
        line: &mut dyn FnMut(&str),
    ) -> Result<(), String>;

    /// Runs an agent's query (#112) as `run` does, each value whole ([`read_invocation`]) and given up after
    /// `seconds`.
    fn read(
        &self,
        connection: &str,
        database: Option<&str>,
        sql: &str,
        _seconds: u32,
        line: &mut dyn FnMut(&str),
    ) -> Result<(), String> {
        self.run(connection, database, sql, line)
    }

    /// Whether it can run at all; why not when it can't.
    fn ready(&self) -> Result<(), String>;
}

/// sqlcmd on PATH.
pub struct Sqlcmd;

impl Sql for Sqlcmd {
    fn run(
        &self,
        connection: &str,
        database: Option<&str>,
        sql: &str,
        line: &mut dyn FnMut(&str),
    ) -> Result<(), String> {
        let program = tools::find_on_path(PROGRAM).ok_or_else(|| MISSING.to_string())?;
        spawn(program, &invocation(&target(connection)?, database, sql), line)
    }

    fn read(
        &self,
        connection: &str,
        database: Option<&str>,
        sql: &str,
        seconds: u32,
        line: &mut dyn FnMut(&str),
    ) -> Result<(), String> {
        let program = tools::find_on_path(PROGRAM).ok_or_else(|| MISSING.to_string())?;
        spawn(program, &read_invocation(&target(connection)?, database, sql, seconds), line)
    }

    fn ready(&self) -> Result<(), String> {
        tools::find_on_path(PROGRAM).map(|_| ()).ok_or_else(|| MISSING.to_string())
    }
}

/// Runs sqlcmd (`program`) as `invocation` says, each line it prints to `line`; the error it reported when it failed.
/// Its thread's job cancelled ([`cancel`]), it doesn't start, or is killed.
fn spawn(program: std::path::PathBuf, invocation: &Invocation, line: &mut dyn FnMut(&str)) -> Result<(), String> {
    if cancel::asked() {
        return Err(CANCELLED.into());
    }
    let mut command = Command::new(program);
    command.args(&invocation.args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    for (name, value) in &invocation.env {
        match value {
            Some(value) => command.env(name, value),
            None => command.env_remove(name),
        };
    }
    let mut child = tools::own_console(&mut command).spawn().map_err(|error| format!("{PROGRAM}: {error}"))?;
    let watched = cancel::running(child.id());
    let errors = child.stderr.take().map(|stderr| {
        std::thread::spawn(move || BufReader::new(stderr).lines().map_while(Result::ok).collect::<Vec<String>>())
    });
    let mut said = Vec::new();
    if let Some(stdout) = child.stdout.take() {
        for text in BufReader::new(stdout).split(b'\n').map_while(Result::ok) {
            let text = String::from_utf8_lossy(&text);
            let text = text.trim_end_matches('\r');
            line(text);
            said.push(text.to_string());
        }
    }
    said.extend(errors.and_then(|errors| errors.join().ok()).unwrap_or_default());
    let status = child.wait().map_err(|error| format!("{PROGRAM}: {error}"))?;
    drop(watched);
    if status.success() {
        return Ok(());
    }
    if cancel::asked() {
        return Err(CANCELLED.into());
    }
    Err(error_of(&said).unwrap_or_else(|| format!("{PROGRAM} failed ({status})")))
}

/// The error sqlcmd reported among the lines it printed, masked: a server's message (the line after its `Msg`), else
/// sqlcmd's own.
pub fn error_of(lines: &[String]) -> Option<String> {
    let server = lines.iter().position(|line| line.starts_with(SERVER_ERROR)).and_then(|at| lines.get(at + 1));
    let own = || lines.iter().find(|line| line.starts_with(SQLCMD_ERROR));
    let found = server.or_else(own)?;
    Some(mask_text(found.trim()))
}

/// Whether a line is one of sqlcmd's messages rather than a row.
fn is_message(line: &str) -> bool {
    line.starts_with(SERVER_ERROR) || line.starts_with(SQLCMD_ERROR)
}

/// A query's rows: each line with `columns` columns (the last takes the rest of the line, separators and all).
pub fn rows(lines: &[String], columns: usize) -> Vec<Vec<String>> {
    lines
        .iter()
        .filter(|line| !line.trim().is_empty() && !is_message(line))
        .map(|line| line.splitn(columns.max(1), SEPARATOR).map(|part| part.trim().to_string()).collect::<Vec<_>>())
        .filter(|row| row.len() == columns)
        .collect()
}

/// Runs a query and gives its rows of `columns` columns.
pub fn query(
    sql: &dyn Sql,
    connection: &str,
    database: Option<&str>,
    text: &str,
    columns: usize,
) -> Result<Vec<Vec<String>>, String> {
    let mut lines = Vec::new();
    sql.run(connection, database, text, &mut |line| lines.push(line.to_string()))?;
    Ok(rows(&lines, columns))
}

/// `[name]`, its `]` doubled: an identifier that can't end the brackets early.
pub fn quote_name(name: &str) -> String {
    format!("[{}]", name.replace(']', "]]"))
}

/// `N'text'`, its `'` doubled: a string that can't end early.
pub fn quote_text(text: &str) -> String {
    format!("N'{}'", text.replace('\'', "''"))
}

/// The share done a `WITH STATS` message says: `45 percent processed.` is 0.45.
pub fn percent(line: &str) -> Option<f64> {
    let (number, rest) = line.trim().split_once(' ')?;
    let percent: f64 = number.parse().ok()?;
    rest.starts_with("percent processed").then_some((percent / 100.0).clamp(0.0, 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_connection_string_reads_as_its_server_database_and_login() {
        let windows =
            target("Server=localhost;Database=Shop;Trusted_Connection=True;TrustServerCertificate=True").unwrap();
        assert_eq!((windows.server.as_str(), windows.database.as_deref()), ("localhost", Some("Shop")));
        assert!(windows.integrated && windows.trust && !windows.encrypt);
        let login = target(
            "Data Source=tcp:qa-sql.example.com,1433; Initial Catalog='Shop;Orders'; User ID=reader; \
                            Password=\"p;a\"\"ss\"; Encrypt=yes",
        )
        .unwrap();
        assert_eq!(login.server, "tcp:qa-sql.example.com,1433");
        assert_eq!(login.database.as_deref(), Some("Shop;Orders"), "quoted: its ; is the name's");
        assert_eq!((login.user.as_deref(), login.password.as_deref()), (Some("reader"), Some("p;a\"ss")));
        assert!(!login.integrated && login.encrypt);
        assert!(!format!("{login:?}").contains("p;a"), "Debug never shows the password");
        assert!(target("Database=Shop").is_err());
    }

    #[test]
    fn the_password_goes_to_sqlcmd_in_its_environment_never_on_its_command_line() {
        let login = target("Server=qa-sql.example.com;Database=Shop;User Id=reader;Password=s3cret").unwrap();
        let call = invocation(&login, Some("master"), "SELECT 1");
        assert!(!call.args.iter().any(|arg| arg.contains("s3cret")), "{:?}", call.args);
        assert_eq!(call.env, [(PASSWORD_VARIABLE.to_string(), Some("s3cret".to_string()))]);
        let head: Vec<&str> = call.args.iter().take(6).map(String::as_str).collect();
        assert_eq!(head, ["-S", "qa-sql.example.com", "-d", "master", "-U", "reader"]);
        assert_eq!(call.args.last().map(String::as_str), Some("SELECT 1"));
        assert!(call.args.windows(2).any(|pair| pair == ["-s", SEPARATOR]) && call.args.contains(&"-b".to_string()));

        let windows = target("Server=.;Database=Shop;Integrated Security=SSPI;TrustServerCertificate=true").unwrap();
        let call = invocation(&windows, None, "SELECT 1");
        assert!(call.args.contains(&"-E".to_string()) && call.args.contains(&"-C".to_string()));
        assert!(!call.args.contains(&"-U".to_string()));
        assert_eq!(call.env, [(PASSWORD_VARIABLE.to_string(), None)], "an inherited one is taken away");
        assert_eq!(call.args[2..4], ["-d".to_string(), "Shop".to_string()], "its own database when none is given");
    }

    #[test]
    fn an_agents_query_gets_whole_values_a_time_limit_and_the_password_in_the_environment_only() {
        let login = target("Server=qa-sql.example.com;Database=Shop;User Id=reader;Password=s3cret").unwrap();
        let call = read_invocation(&login, None, "SELECT 1", 30);
        assert!(!call.args.iter().any(|arg| arg.contains("s3cret")), "{:?}", call.args);
        assert_eq!(call.env, [(PASSWORD_VARIABLE.to_string(), Some("s3cret".to_string()))]);
        assert!(
            call.args.windows(2).any(|pair| pair == ["-y", "0"])
                && call.args.windows(2).any(|pair| pair == ["-t", "30"])
        );
        assert!(
            !call.args.contains(&"-W".to_string()) && !call.args.contains(&"-h".to_string()),
            "sqlcmd refuses them with -y 0"
        );
        assert_eq!(call.args[call.args.len() - 2..], ["-Q".to_string(), "SELECT 1".to_string()]);
    }

    #[test]
    fn names_and_texts_are_quoted_so_they_cant_end_early() {
        assert_eq!(quote_name("Shop"), "[Shop]");
        assert_eq!(quote_name("we]ird"), "[we]]ird]");
        assert_eq!(quote_text("C:\\O'Brien\\x.bak"), "N'C:\\O''Brien\\x.bak'");
    }

    #[test]
    fn rows_and_progress_and_errors_read_from_what_sqlcmd_prints() {
        let lines: Vec<String> =
            ["12|dbo|Orders", "", "0|sales|Odd|Name", "Msg 208, Level 16, State 1, Server x, Line 1", "Invalid"]
                .iter()
                .map(|line| line.to_string())
                .collect();
        assert_eq!(rows(&lines, 3), [vec!["12", "dbo", "Orders"], vec!["0", "sales", "Odd|Name"]]);
        assert_eq!(error_of(&lines).as_deref(), Some("Invalid"));
        let own = ["Sqlcmd: Error: Login failed; Password=x".to_string()];
        assert_eq!(error_of(&own).as_deref(), Some("Sqlcmd: Error: Login failed; Password=***"));
        assert_eq!(percent("45 percent processed."), Some(0.45));
        assert_eq!(percent("100 percent processed."), Some(1.0));
        assert_eq!(percent("Processed 368 pages for database 'Shop'"), None);
    }
}
