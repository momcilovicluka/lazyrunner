//! The database switch: SQL connection strings in the APIs' user secrets (secrets.json), one active per key and the
//! others commented out with `//`. Choosing one rewrites only those lines: the chosen one active, the rest of the
//! same key commented. Passwords are never shown: entries read "database @ server".

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use anyhow::{Result, anyhow, bail};
use regex::{Captures, Regex};

use crate::json;
use crate::profile::{App, AppKind};

static LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"^(?<indent>\s*)(?<comment>//\s*)?"(?<key>[^"]+)"\s*:\s*"(?<value>(?:[^"\\]|\\.)*)"\s*(?<comma>,?)\s*$"#,
    )
    .expect("a valid pattern")
});
static SERVER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:^|;)\s*(?:server|data source|addr|address)\s*=\s*([^;]+)").expect("a valid pattern")
});
static DATABASE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(?:^|;)\s*(?:database|initial catalog)\s*=\s*([^;]+)").expect("a valid pattern"));
static SECRETS_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<UserSecretsId>\s*([^<\s]+)\s*</UserSecretsId>").expect("a valid pattern"));

/// The secrets.json the APIs read: `LAZYRUNNER_SECRETS_PATH`, else the first API with a UserSecretsId's.
pub fn secrets_path(root: &Path, apps: &[App]) -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("LAZYRUNNER_SECRETS_PATH").filter(|path| !path.is_empty()) {
        return Some(PathBuf::from(path));
    }
    apps.iter().filter(|app| app.traits().uses_secrets).find_map(|app| {
        let AppKind::Dotnet { project, .. } = &app.kind else { return None };
        let text = json::read_text(&root.join(project)).ok()?;
        let id = SECRETS_ID.captures(&text)?[1].to_string();
        Some(user_secrets_dir()?.join(id).join("secrets.json"))
    })
}

/// Where `dotnet user-secrets` keeps them on this OS.
fn user_secrets_dir() -> Option<PathBuf> {
    if cfg!(windows) {
        Some(PathBuf::from(std::env::var_os("APPDATA")?).join("Microsoft").join("UserSecrets"))
    } else {
        Some(std::env::home_dir()?.join(".microsoft").join("usersecrets"))
    }
}

/// One SQL connection string in secrets.json.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub line: usize,
    pub key: String,
    pub active: bool,
    pub server: String,
    pub database: String,
    pub label: String,
}

/// The SQL connection strings (with a server and a database; not storage or service bus ones).
pub fn connections(text: &str) -> Vec<Entry> {
    lines(text)
        .iter()
        .enumerate()
        .filter_map(|(line, text)| {
            let found = LINE.captures(text)?;
            let value = &found["value"];
            let server = SERVER.captures(value)?[1].trim().to_string();
            let database = DATABASE.captures(value)?[1].trim().to_string();
            Some(Entry {
                line,
                key: found["key"].to_string(),
                active: found.name("comment").is_none(),
                label: format!("{database} @ {server}"),
                server,
                database,
            })
        })
        .collect()
}

/// The entries per key, ASP.NET's usual DefaultConnection (else the biggest group) first.
pub fn groups(text: &str) -> Vec<(String, Vec<Entry>)> {
    let mut groups: Vec<(String, Vec<Entry>)> = Vec::new();
    for entry in connections(text) {
        match groups.iter_mut().find(|(key, _)| *key == entry.key) {
            Some((_, entries)) => entries.push(entry),
            None => groups.push((entry.key.clone(), vec![entry])),
        }
    }
    groups.sort_by_key(|(key, entries)| (key != "DefaultConnection", std::cmp::Reverse(entries.len())));
    groups
}

/// What the header says: the active database of the main group.
pub fn active_label(text: &str) -> Option<String> {
    let (_, entries) = groups(text).into_iter().next()?;
    let active: Vec<&Entry> = entries.iter().filter(|entry| entry.active).collect();
    Some(match active.as_slice() {
        [] => "none active".into(),
        [one] => one.label.clone(),
        [.., last] => format!("{} (+{} more active)", last.label, active.len() - 1),
    })
}

fn lines(text: &str) -> Vec<String> {
    text.split('\n').map(|line| line.strip_suffix('\r').unwrap_or(line).to_string()).collect()
}

/// The file's text, whether it had a byte order mark, and its line ending.
fn read(path: &Path) -> Result<(String, bool, &'static str)> {
    let bytes = std::fs::read(path)?;
    let bom = bytes.starts_with(&[0xEF, 0xBB, 0xBF]);
    let text = String::from_utf8(bytes[if bom { 3 } else { 0 }..].to_vec())?;
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    Ok((text, bom, newline))
}

/// Writes the new text after checking it is still JSON (comments and trailing commas allowed), keeping a backup.
fn write(path: &Path, text: &str, bom: bool, what: &str) -> Result<()> {
    serde_json::from_str::<serde_json::Value>(&strip_jsonc(text))
        .map_err(|error| anyhow!("secrets.json would not be valid JSON {what} ({error}) - left unchanged"))?;
    let mut backup = path.as_os_str().to_owned();
    backup.push(".lazyrunner-backup");
    std::fs::copy(path, PathBuf::from(backup))?;
    let mut bytes = if bom { vec![0xEF, 0xBB, 0xBF] } else { Vec::new() };
    bytes.extend_from_slice(text.as_bytes());
    // Whole or not at all: an API reading it meanwhile never sees half a file.
    let mut temp = path.as_os_str().to_owned();
    temp.push(".lazyrunner-new");
    let temp = PathBuf::from(temp);
    std::fs::write(&temp, bytes)?;
    std::fs::rename(&temp, path)?;
    Ok(())
}

fn indent_of(line: &str) -> String {
    LINE.captures(line).map_or(String::new(), |found| found["indent"].to_string())
}

fn uncommented(line: &str, indent: &str) -> String {
    let body = &line[indent.len().min(line.len())..];
    body.strip_prefix("//").map_or(body, str::trim_start).to_string()
}

/// `entry` as it is in the file now (it may have been edited since it was read).
fn current(text: &str, entry: &Entry) -> Result<Entry> {
    connections(text)
        .into_iter()
        .find(|now| now.line == entry.line && now.key == entry.key && now.label == entry.label)
        .ok_or_else(|| anyhow!("secrets.json changed since it was read - pick the database again"))
}

/// Makes `entry` the active one of its key; the others of that key get commented out.
pub fn switch(path: &Path, entry: &Entry) -> Result<()> {
    let (text, bom, newline) = read(path)?;
    let entry = &current(&text, entry)?;
    let mut lines = lines(&text);
    for other in connections(&text).iter().filter(|other| other.key == entry.key) {
        let indent = indent_of(&lines[other.line]);
        let body = uncommented(&lines[other.line], &indent);
        lines[other.line] =
            if other.line == entry.line { format!("{indent}{body}") } else { format!("{indent}//{body}") };
    }
    // The newly active line needs a comma when another active property follows it.
    let followed = lines.iter().skip(entry.line + 1).any(|line| {
        let line = line.trim();
        !line.is_empty() && !line.starts_with("//") && !line.starts_with('}')
    });
    if followed && !lines[entry.line].trim_end().ends_with(',') {
        lines[entry.line] = format!("{},", lines[entry.line].trim_end());
    }
    write(path, &lines.join(newline), bom, "after the switch")
}

/// Adds a commented-out (inactive) copy of `entry` with another database, right below it.
pub fn add(path: &Path, entry: &Entry, database: &str) -> Result<()> {
    let database = database.trim();
    if database.is_empty() || database.contains([';', '"', '\\', '=']) {
        bail!("'{database}' is not a usable database name");
    }
    let (text, bom, newline) = read(path)?;
    let entry = &current(&text, entry)?;
    let mut lines = lines(&text);
    let indent = indent_of(&lines[entry.line]);
    let body = uncommented(&lines[entry.line], &indent).trim_end().to_string();
    static NAME: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"(?i)((?:database|initial catalog)\s*=\s*)[^;"]*"#).expect("a valid pattern"));
    let mut body = NAME.replace_all(&body, |found: &Captures| format!("{}{database}", &found[1])).into_owned();
    if !body.ends_with(',') {
        body.push(',');
    }
    lines.insert(entry.line + 1, format!("{indent}//{body}"));
    write(path, &lines.join(newline), bom, "with the new line")
}

/// JSON with comments and trailing commas made plain JSON (for checking it).
pub fn strip_jsonc(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let (mut in_string, mut escaped) = (false, false);
    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            match (escaped, c) {
                (true, _) => escaped = false,
                (false, '\\') => escaped = true,
                (false, '"') => in_string = false,
                _ => {}
            }
            continue;
        }
        match (c, chars.peek()) {
            ('"', _) => {
                in_string = true;
                out.push(c);
            }
            ('/', Some('/')) => {
                for c in chars.by_ref() {
                    if c == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            ('/', Some('*')) => {
                chars.next();
                let mut previous = ' ';
                for c in chars.by_ref() {
                    if previous == '*' && c == '/' {
                        break;
                    }
                    previous = c;
                }
            }
            _ => out.push(c),
        }
    }
    static TRAILING: LazyLock<Regex> = LazyLock::new(|| Regex::new(r",(\s*[}\]])").expect("a valid pattern"));
    TRAILING.replace_all(&out, "$1").into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{TempDir, write as write_file};

    const SECRETS: &str = "{\r\n  //\"DefaultConnection\": \"Server=dev-sql;Database=Shop;User Id=x;Password=secret\",\r\n  \"DefaultConnection\": \"Server=localhost;Database=ShopLocal;Trusted_Connection=True\",\r\n  \"Storage\": \"UseDevelopmentStorage=true\",\r\n  \"Other\": \"Data Source=reports;Initial Catalog=Reports\"\r\n}\r\n";

    #[test]
    fn reads_the_sql_connection_strings_without_their_passwords() {
        let entries = connections(SECRETS);
        let labels: Vec<(&str, bool)> = entries.iter().map(|entry| (entry.label.as_str(), entry.active)).collect();
        assert_eq!(labels, [("Shop @ dev-sql", false), ("ShopLocal @ localhost", true), ("Reports @ reports", true)]);
        assert!(entries.iter().all(|entry| !entry.label.contains("secret")));
        assert_eq!(groups(SECRETS)[0].0, "DefaultConnection");
        assert_eq!(active_label(SECRETS).as_deref(), Some("ShopLocal @ localhost"));
    }

    #[test]
    fn switching_comments_out_the_others_and_keeps_the_file_json() {
        let dir = TempDir::new("secrets");
        let path = dir.join("secrets.json");
        write_file(&path, SECRETS);
        let dev = connections(SECRETS).into_iter().next().unwrap();
        switch(&path, &dev).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(active_label(&text).as_deref(), Some("Shop @ dev-sql"));
        assert!(text.contains("\r\n"), "keeps its line endings");
        assert!(dir.join("secrets.json.lazyrunner-backup").is_file());
        serde_json::from_str::<serde_json::Value>(&strip_jsonc(&text)).unwrap();

        let local = connections(&text).into_iter().find(|entry| entry.database == "ShopLocal").unwrap();
        add(&path, &local, "ShopTest").unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let added = connections(&text).into_iter().find(|entry| entry.database == "ShopTest").unwrap();
        assert!(!added.active, "added inactive");
        assert!(add(&path, &local, "bad;name").is_err());
    }

    #[test]
    fn jsonc_becomes_json() {
        let text = "{ // note\n \"a\": \"//not a comment\", /* block */ \"b\": [1, 2,], }";
        assert_eq!(serde_json::from_str::<serde_json::Value>(&strip_jsonc(text)).unwrap()["a"], "//not a comment");
    }
}
