//! The session kept when lazyrunner closes (#85): the apps that were running and what the screen showed, per branch,
//! in the project's state folder. The next lazyrunner offers it at its start - for a week - and starts those apps
//! instead of all of them; the screen part is the UI's own (`view`), kept as it gave it.

use std::path::Path;
use std::time::Duration;

use anyhow::Result;
use chrono::{DateTime, FixedOffset};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// The file in the project's state folder, the snapshots in it by branch.
pub const FILE: &str = "session.json";
/// A snapshot older than this isn't offered any more.
pub const MOST_AGE: Duration = Duration::from_secs(7 * 24 * 3600);
const MINUTE: u64 = 60;
const HOUR: u64 = 3600;
const DAY: u64 = 24 * HOUR;
/// The snapshots kept for each branch (and offered), the newest first.
const KEPT: usize = 3;

/// What lazyrunner had when it closed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    /// When, as RFC 3339.
    pub at: String,
    pub branch: String,
    /// The running apps' keys.
    pub running: Vec<String>,
    /// The screen: what the UI keeps of it.
    #[serde(default)]
    pub view: Value,
}

/// How old a snapshot is, as its row is coloured: within the hour, within the day, or older.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Freshness {
    Recent,
    Today,
    Older,
}

impl Snapshot {
    /// How long ago it was kept; none when its time doesn't read.
    pub fn age(&self, now: DateTime<FixedOffset>) -> Option<Duration> {
        let at = DateTime::parse_from_rfc3339(&self.at).ok()?;
        Some(Duration::from_secs((now - at).num_seconds().max(0) as u64))
    }

    pub fn freshness(&self, now: DateTime<FixedOffset>) -> Freshness {
        match self.age(now).map_or(u64::MAX, |age| age.as_secs()) {
            age if age < HOUR => Freshness::Recent,
            age if age < DAY => Freshness::Today,
            _ => Freshness::Older,
        }
    }

    /// Its row in the startup checklist: `2 h ago · 3 apps were running`, and the branch when it's another.
    pub fn summary(&self, branch: &str, now: DateTime<FixedOffset>) -> String {
        let age = self.age(now).map_or_else(|| "some time ago".to_string(), ago);
        let running = match self.running.len() {
            0 => "nothing was running".to_string(),
            1 => "1 app was running".to_string(),
            count => format!("{count} apps were running"),
        };
        let from = if self.branch == branch { String::new() } else { format!(" · from {}", self.branch) };
        format!("{age} · {running}{from}")
    }
}

/// How long ago, as the checklist says it: `just now`, `12 min ago`, `2 h ago`, `3 d ago`.
pub fn ago(age: Duration) -> String {
    match age.as_secs() {
        seconds if seconds < MINUTE => "just now".into(),
        seconds if seconds < HOUR => format!("{} min ago", seconds / MINUTE),
        seconds if seconds < DAY => format!("{} h ago", seconds / HOUR),
        seconds => format!("{} d ago", seconds / DAY),
    }
}

fn all(state_dir: &Path) -> Map<String, Value> {
    crate::json::read(&state_dir.join(FILE)).and_then(|value| value.as_object().cloned()).unwrap_or_default()
}

/// A branch's snapshots in the file: a list, the newest first (one on its own, as 1.87 kept it).
fn snapshots_in(value: Value) -> Vec<Snapshot> {
    match value {
        Value::Array(items) => items.into_iter().filter_map(|item| serde_json::from_value(item).ok()).collect(),
        single => serde_json::from_value(single).ok().into_iter().collect(),
    }
}

/// Keeps `snapshot` as its branch's newest, the last few before it kept too, beside the other branches'.
pub fn save(state_dir: &Path, snapshot: &Snapshot) -> Result<()> {
    let mut all = all(state_dir);
    let mut kept = all.remove(&snapshot.branch).map(snapshots_in).unwrap_or_default();
    kept.insert(0, snapshot.clone());
    kept.truncate(KEPT);
    all.insert(snapshot.branch.clone(), serde_json::to_value(kept)?);
    crate::config::write(&state_dir.join(FILE), &Value::Object(all))
}

/// The snapshots to offer on `branch`, the first resumed unless another is picked: its own (the newest first), then
/// the other branches' - at most `KEPT`, none older than a week.
pub fn history(state_dir: &Path, branch: &str, now: DateTime<FixedOffset>) -> Vec<Snapshot> {
    let fresh = |snapshot: &Snapshot| snapshot.age(now).is_some_and(|age| age < MOST_AGE);
    let (mut own, mut others) = (Vec::new(), Vec::new());
    for (name, value) in all(state_dir) {
        let kept: Vec<Snapshot> = snapshots_in(value).into_iter().filter(fresh).collect();
        if name == branch { own = kept } else { others.extend(kept) }
    }
    others.sort_by_key(|snapshot| snapshot.age(now));
    own.into_iter().chain(others).take(KEPT).collect()
}

/// The snapshot to offer on `branch`: its own newest, else the newest of another branch's.
pub fn load(state_dir: &Path, branch: &str, now: DateTime<FixedOffset>) -> Option<Snapshot> {
    history(state_dir, branch, now).into_iter().next()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TempDir;

    fn at(text: &str) -> DateTime<FixedOffset> {
        DateTime::parse_from_rfc3339(text).unwrap()
    }

    fn snapshot(branch: &str, at: &str, running: &[&str]) -> Snapshot {
        let running = running.iter().map(|key| key.to_string()).collect();
        Snapshot { at: at.into(), branch: branch.into(), running, view: Value::Null }
    }

    #[test]
    fn a_branch_gets_its_own_snapshot_else_the_newest_other_and_none_after_a_week() {
        let temp = TempDir::new("session");
        save(temp.path(), &snapshot("main", "2026-11-10T09:00:00+01:00", &["orders"])).unwrap();
        save(temp.path(), &snapshot("feature", "2026-11-12T09:00:00+01:00", &["orders", "web"])).unwrap();
        let now = at("2026-11-12T11:00:00+01:00");
        assert_eq!(load(temp.path(), "main", now).unwrap().running, ["orders"], "its own, though older");
        let other = load(temp.path(), "fix", now).unwrap();
        assert_eq!(other.branch, "feature", "the newest of the others");
        assert_eq!(other.summary("fix", now), "2 h ago · 2 apps were running · from feature");
        assert_eq!(other.freshness(now), Freshness::Today);
        let later = at("2026-11-17T10:00:00+01:00");
        assert!(load(temp.path(), "main", later).is_some_and(|kept| kept.branch == "feature"), "main's is too old");
        assert!(load(temp.path(), "main", at("2026-11-20T10:00:00+01:00")).is_none(), "a week on, nothing");
    }

    #[test]
    fn ages_read_short() {
        assert_eq!(ago(Duration::from_secs(20)), "just now");
        assert_eq!(ago(Duration::from_secs(12 * 60)), "12 min ago");
        assert_eq!(ago(Duration::from_secs(3 * 24 * 3600)), "3 d ago");
    }
}
