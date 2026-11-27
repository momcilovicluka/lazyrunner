//! A connection string pasted into the add-source form (#114), read into a source's fields: its server, login and
//! database, its password (for the credential store, never a file) - and everything a pull sets itself (`Encrypt`,
//! `TrustServerCertificate`, timeouts, `Application Name`, `Pooling`…) left out and listed, so nothing is lost
//! silently. Values may be quoted (`'…'` or `"…"`, the quote doubled inside), and text copied out of a JSON file may
//! have its backslashes doubled and its quotes around it.

use crate::run::credentials::Password;

use super::sources::Auth;

const SERVER: [&str; 5] = ["data source", "server", "address", "addr", "network address"];
const DATABASE: [&str; 2] = ["initial catalog", "database"];
const USER: [&str; 4] = ["user id", "uid", "user", "username"];
const PASSWORD: [&str; 2] = ["password", "pwd"];
const INTEGRATED: [&str; 2] = ["integrated security", "trusted_connection"];
const AUTHENTICATION: &str = "authentication";
/// What turns integrated security on.
const YES: [&str; 3] = ["true", "yes", "sspi"];
/// The authentication values that are Microsoft Entra's.
const ENTRA: &str = "active directory";

/// What a pasted connection string fills in.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Pasted {
    pub server: Option<String>,
    pub database: Option<String>,
    pub user: Option<String>,
    pub password: Option<Password>,
    pub auth: Option<Auth>,
    /// The keys left out, as written.
    pub dropped: Vec<String>,
}

/// Whether text looks like a connection string rather than a name: a `key=value` naming a server.
pub fn looks_like(text: &str) -> bool {
    let read = parts(&unwrap(text));
    read.iter().any(|(key, _)| SERVER.contains(&normal(key).as_str())) && text.contains('=')
}

/// A key as compared: lowercase, its spaces single.
fn normal(key: &str) -> String {
    key.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

/// The text without what copying it out of JSON adds: quotes around it, its backslashes doubled.
fn unwrap(text: &str) -> String {
    let text = text.trim();
    let text = text.strip_suffix(',').unwrap_or(text).trim();
    match text.strip_prefix('"').and_then(|inner| inner.strip_suffix('"')) {
        Some(inner) if !inner.contains("\"\"") => inner.replace("\\\\", "\\").replace("\\\"", "\""),
        _ => text.replace("\\\\", "\\"),
    }
}

/// Its `key=value` pairs, in order: a value in quotes keeps its `;`s, the quote doubled inside is one.
fn parts(text: &str) -> Vec<(String, String)> {
    let mut found = Vec::new();
    let mut chars = text.chars().peekable();
    loop {
        let mut key = String::new();
        for c in chars.by_ref() {
            if c == '=' {
                break;
            }
            key.push(c);
        }
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
        let key = key.trim().trim_start_matches(';').trim().to_string();
        if !key.is_empty() {
            found.push((key, value.trim().to_string()));
        }
        if chars.peek().is_none() {
            return found;
        }
    }
}

/// A server as a source keeps it: `host,port\instance` keeps `host,port` (the port is all that's needed).
fn server_of(value: &str) -> String {
    let value = value.trim();
    match value.split_once(',') {
        Some((host, rest)) => {
            let port = rest.split('\\').next().unwrap_or_default().trim();
            format!("{},{port}", host.trim())
        }
        None => value.to_string(),
    }
}

/// Reads a pasted connection string.
pub fn read(text: &str) -> Pasted {
    let mut pasted = Pasted::default();
    let mut integrated = None;
    for (key, value) in parts(&unwrap(text)) {
        let named = normal(&key);
        let named = named.as_str();
        let value = Some(value).filter(|value| !value.is_empty());
        if SERVER.contains(&named) {
            pasted.server = value.map(|value| server_of(&value));
        } else if DATABASE.contains(&named) {
            pasted.database = value;
        } else if USER.contains(&named) {
            pasted.user = value;
        } else if PASSWORD.contains(&named) {
            pasted.password = value.map(Password::new);
        } else if INTEGRATED.contains(&named) {
            integrated = value.map(|value| YES.contains(&value.to_lowercase().as_str()));
        } else if named == AUTHENTICATION && value.as_ref().is_some_and(|value| value.to_lowercase().contains(ENTRA)) {
            pasted.auth = Some(Auth::Entra);
        } else {
            pasted.dropped.push(key);
        }
    }
    if pasted.auth.is_none() {
        pasted.auth = match (integrated, &pasted.user) {
            (Some(true), _) => Some(Auth::Windows),
            (_, Some(_)) => Some(Auth::Sql),
            (Some(false), None) | (None, None) => pasted.server.as_ref().map(|_| Auth::Windows),
        };
    }
    pasted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pasted_string_fills_the_server_login_and_database_and_drops_what_a_pull_sets_itself() {
        let pasted = read(
            "Data Source=qa-sql.example.com,1433;Initial Catalog=Shop;Persist Security Info=False;User ID=reader;\
             Password=s3cret;Pooling=False;MultipleActiveResultSets=False;Encrypt=True;TrustServerCertificate=True;\
             Connect Timeout=30;Packet Size=4096;Application Name=\"SQL Server Management Studio\";Command Timeout=0",
        );
        assert_eq!(pasted.server.as_deref(), Some("qa-sql.example.com,1433"));
        assert_eq!((pasted.database.as_deref(), pasted.user.as_deref()), (Some("Shop"), Some("reader")));
        assert_eq!(pasted.password.as_ref().map(Password::expose), Some("s3cret"));
        assert_eq!(pasted.auth, Some(Auth::Sql));
        assert_eq!(
            pasted.dropped,
            [
                "Persist Security Info",
                "Pooling",
                "MultipleActiveResultSets",
                "Encrypt",
                "TrustServerCertificate",
                "Connect Timeout",
                "Packet Size",
                "Application Name",
                "Command Timeout"
            ]
        );
        assert!(!format!("{pasted:?}").contains("s3cret"), "the password never shows");
    }

    #[test]
    fn quoted_values_keep_their_semicolons_and_doubled_quotes() {
        let pasted = read("Server=tcp:qa;Database='Sh;op';User Id=\"re\"\"ader\";Pwd='p;a''ss';Trusted_Connection=no");
        assert_eq!(pasted.server.as_deref(), Some("tcp:qa"));
        assert_eq!(pasted.database.as_deref(), Some("Sh;op"));
        assert_eq!(pasted.user.as_deref(), Some("re\"ader"));
        assert_eq!(pasted.password.as_ref().map(Password::expose), Some("p;a'ss"));
        assert!(pasted.dropped.is_empty() && pasted.auth == Some(Auth::Sql));
    }

    #[test]
    fn a_named_instance_keeps_its_port_and_json_copied_text_is_unwrapped() {
        let pasted = read("\"Server=qa-sql.example.com,1433\\\\SQL2019;Database=Shop;Integrated Security=SSPI\",");
        assert_eq!(pasted.server.as_deref(), Some("qa-sql.example.com,1433"));
        assert_eq!(pasted.auth, Some(Auth::Windows));
        let instance = read("Server=qa-sql\\\\SQL2019;Trusted_Connection=True");
        assert_eq!(instance.server.as_deref(), Some("qa-sql\\SQL2019"), "doubled backslashes are one");
        assert_eq!(instance.database, None, "no database: the whole server");
        let entra = read("Server=qa;Authentication=Active Directory Interactive;User ID=me@example.com");
        assert_eq!(entra.auth, Some(Auth::Entra));
    }

    #[test]
    fn only_a_string_that_names_a_server_looks_like_one() {
        assert!(looks_like("Server=qa;Database=Shop"));
        assert!(looks_like("  data  source = qa"));
        assert!(!looks_like("qa"));
        assert!(!looks_like("Database=Shop"));
    }
}
