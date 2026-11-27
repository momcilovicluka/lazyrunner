//! A local database linked to its remote twin (#114): `Shop` here is QA's `Shop` as of the day it was copied, so one
//! key brings it up to date. Links are per machine (local names differ between machines), kept in the state folder,
//! never in the profile. A pull or an import from a source links what it made; linking by hand suggests the closest
//! twin first - the same name, then the same name without a pull's suffix (`Shop_qa_20261123` is `Shop`), then the
//! most alike.

use std::path::Path;
use std::sync::LazyLock;

use anyhow::Result;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The project's state folder keeps the links.
pub const FILE: &str = "links.json";

/// What a pull, a copy or an update adds after a database's name: `_qa_20261123`, `_old_20261127`, `_copy`.
static SUFFIX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^(.+?)(_[a-z0-9-]+_\d{8}\d*|_copy\d*)$").expect("a valid pattern"));

/// A local database (its server and name) and its twin: a source, and its database there; when the local copy was
/// taken (RFC 3339), when known.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Link {
    pub server: String,
    pub database: String,
    pub source: String,
    pub remote: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pulled: Option<String>,
}

impl Link {
    /// Whether it's the link of `database` on `server`.
    pub fn of(&self, server: &str, database: &str) -> bool {
        self.server.eq_ignore_ascii_case(server) && self.database.eq_ignore_ascii_case(database)
    }

    /// Its twin as the screen says it: `qa/Shop`.
    pub fn twin(&self) -> String {
        format!("{}/{}", self.source, self.remote)
    }
}

/// A remote database a local one could be linked to: a source and its database there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Remote {
    pub source: String,
    pub database: String,
}

/// A database's name without a pull's, an update's or a copy's suffix.
pub fn base_name(database: &str) -> &str {
    SUFFIX.captures(database).and_then(|found| found.get(1)).map_or(database, |base| base.as_str())
}

/// How many characters two names have in common at their start, ignoring case: the suggested twin's part that
/// matches is underlined.
pub fn common_start(a: &str, b: &str) -> usize {
    a.chars().zip(b.chars()).take_while(|(a, b)| a.eq_ignore_ascii_case(b)).count()
}

/// How far apart two names are, in single-character edits, ignoring case.
fn distance(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.to_lowercase().chars().collect(), b.to_lowercase().chars().collect());
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let next = (row[j + 1] + 1).min(row[j] + 1).min(diagonal + usize::from(ca != cb));
            diagonal = row[j + 1];
            row[j + 1] = next;
        }
    }
    row[b.len()]
}

/// How close a local database's name is to a remote one's, lower closer: the same name, then the same without a
/// pull's suffix, then by how far apart they are.
pub fn rank(local: &str, remote: &str) -> (usize, usize) {
    let base = base_name(local);
    if remote.eq_ignore_ascii_case(local) {
        (0, 0)
    } else if remote.eq_ignore_ascii_case(base) {
        (1, 0)
    } else {
        (2, distance(base, remote))
    }
}

/// `remotes` in the order a local database's twin is suggested: the same name, then the same name without a suffix,
/// then the most alike first.
pub fn suggest(local: &str, remotes: &[Remote]) -> Vec<Remote> {
    let mut ranked: Vec<Remote> = remotes.to_vec();
    ranked.sort_by_key(|remote| (rank(local, &remote.database), remote.source.clone(), remote.database.to_lowercase()));
    ranked
}

/// The links; none when the file is missing or broken.
pub fn load(state_dir: &Path) -> Vec<Link> {
    match crate::json::read(&state_dir.join(FILE)) {
        Some(Value::Array(items)) => items.into_iter().filter_map(|item| serde_json::from_value(item).ok()).collect(),
        _ => Vec::new(),
    }
}

pub fn save(state_dir: &Path, all: &[Link]) -> Result<()> {
    crate::config::write(&state_dir.join(FILE), &serde_json::to_value(all)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn remote(source: &str, database: &str) -> Remote {
        Remote { source: source.into(), database: database.into() }
    }

    #[test]
    fn a_pulls_suffix_comes_off_and_the_closest_twin_is_suggested_first() {
        assert_eq!(base_name("Shop_qa_20261123"), "Shop");
        assert_eq!(base_name("Shop_Audit_old_202611272"), "Shop_Audit");
        assert_eq!(base_name("Shop_copy"), "Shop");
        assert_eq!(base_name("Shop_Audit"), "Shop_Audit", "a name of its own keeps its parts");
        let remotes = [
            remote("qa", "Tenant_Acme"),
            remote("qa", "Shop_Audit"),
            remote("qa", "Shop"),
            remote("staging", "Shop_qa_20261123"),
        ];
        let named: Vec<String> = suggest("Shop_qa_20261123", &remotes)
            .iter()
            .map(|found| format!("{}/{}", found.source, found.database))
            .collect();
        assert_eq!(named, ["staging/Shop_qa_20261123", "qa/Shop", "qa/Shop_Audit", "qa/Tenant_Acme"]);
        let named: Vec<String> = suggest("Shop_Audt", &remotes).iter().map(|found| found.database.clone()).collect();
        assert_eq!(named[0], "Shop_Audit", "then the most alike");
        assert_eq!(common_start("Shop_qa_20261123", "shop"), 4);
    }

    #[test]
    fn links_are_kept_per_machine_in_the_state_folder() {
        let link = Link {
            server: "localhost".into(),
            database: "Shop".into(),
            source: "qa".into(),
            remote: "Shop".into(),
            pulled: Some("2026-11-15T09:00:00+01:00".into()),
        };
        assert!(link.of("LOCALHOST", "shop") && link.twin() == "qa/Shop");
        let temp = crate::testing::TempDir::new("links");
        assert!(load(temp.path()).is_empty());
        save(temp.path(), std::slice::from_ref(&link)).unwrap();
        assert_eq!(load(temp.path()), [link]);
    }
}
