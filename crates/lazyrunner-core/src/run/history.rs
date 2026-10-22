//! Durations (D): how long builds, app starts, npm ci and test suites took over their last runs, so one that got slower stands
//! out. Kept in history.json in the project's state folder (timings.json only feeds the progress estimates).

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::format_duration;
use crate::json;

/// Runs kept per item.
pub const RUNS_KEPT: usize = 40;
/// A run this much off its usual is worth a mention (0.15 = 15%)...
pub const TREND_THRESHOLD: f64 = 0.15;
/// ...and this many seconds off it (a 0.3s start after 0.2s ones isn't news).
pub const TREND_MIN_SECONDS: f64 = 1.0;
/// Earlier successful runs the latest one is compared with.
const TREND_BASE_RUNS: usize = 10;
/// Fewer earlier successful runs than this and there is no usual yet.
const TREND_MIN_RUNS: usize = 3;

/// What was timed; the D box shows them grouped in this order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Build,
    Start,
    NpmCi,
    Test,
}

impl Kind {
    pub const ALL: [Kind; 4] = [Kind::Build, Kind::Start, Kind::NpmCi, Kind::Test];

    pub fn title(self) -> &'static str {
        match self {
            Kind::Build => "Builds",
            Kind::Start => "Starts",
            Kind::NpmCi => "npm ci",
            Kind::Test => "Test suites",
        }
    }
}

/// One run.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Timed {
    /// When it finished (RFC 3339, local time).
    pub at: String,
    pub sec: f64,
    pub ok: bool,
}

/// One app's builds, starts or npm ci runs, oldest first.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Item {
    pub kind: Kind,
    pub key: String,
    /// The app's name when it last ran.
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub runs: Vec<Timed>,
}

/// The latest run against the median of the successful runs before it.
#[derive(Clone, Debug, PartialEq)]
pub struct Trend {
    pub last: f64,
    pub last_ok: bool,
    /// None until there are enough earlier successful runs.
    pub usual: Option<f64>,
    /// +0.18 = 18% slower than usual; None without a usual, and for a failed run (its time says little).
    pub change: Option<f64>,
}

impl Trend {
    /// The change when it's worth a mention: 15% or more, and a second or more.
    pub fn notable(&self) -> Option<f64> {
        let (usual, change) = (self.usual?, self.change?);
        (change.abs() >= TREND_THRESHOLD && (self.last - usual).abs() >= TREND_MIN_SECONDS).then_some(change)
    }
}

#[derive(Debug, Default)]
pub struct Durations {
    path: PathBuf,
    items: Vec<Item>,
    /// Entries this lazyrunner can't read (a newer kind, a hand edit): written back as they were.
    kept: Vec<Value>,
    /// The file couldn't be read (not even moved aside): runs are kept in memory only, so it isn't overwritten.
    read_only: bool,
    /// Saving failed and the log said so; the next failures stay quiet.
    warned: bool,
    /// What went wrong reading the file, for the log once.
    pub problem: Option<String>,
}

impl Durations {
    /// The project's history. An unreadable file is moved aside to history.json.bad and a new one starts.
    pub fn load(state: &Path) -> Self {
        let path = state.join("history.json");
        let mut durations = Self { path, ..Self::default() };
        let why = match json::read_for_update(&durations.path) {
            Ok(None) => return durations,
            Ok(Some(Value::Array(entries))) => {
                for entry in entries {
                    match serde_json::from_value(entry.clone()) {
                        Ok(item) => durations.items.push(item),
                        Err(_) => durations.kept.push(entry),
                    }
                }
                return durations;
            }
            Ok(Some(_)) => "it isn't a list".to_string(),
            Err(error) if error.kind() == ErrorKind::InvalidData => error.to_string(),
            Err(error) => {
                durations.read_only = true;
                durations.problem = Some(format!("Couldn't read the durations history ({error}) - not saving it now"));
                return durations;
            }
        };
        let aside = durations.path.with_extension("json.bad");
        durations.problem = Some(match std::fs::rename(&durations.path, &aside) {
            Ok(()) => {
                format!("The durations history was unreadable ({why}) - kept as {}, a new one starts", aside.display())
            }
            Err(error) => {
                durations.read_only = true;
                format!(
                    "The durations history is unreadable ({why}) and couldn't be moved aside ({error}) - not saving it now"
                )
            }
        });
        durations
    }

    /// Whether there is any run to show.
    pub fn is_empty(&self) -> bool {
        self.items.iter().all(|item| item.runs.is_empty())
    }

    pub fn item(&self, kind: Kind, key: &str) -> Option<&Item> {
        self.items.iter().find(|item| item.kind == kind && item.key == key)
    }

    /// The items with runs, of each kind that has any, by label.
    pub fn grouped(&self) -> Vec<(Kind, Vec<&Item>)> {
        Kind::ALL
            .into_iter()
            .filter_map(|kind| {
                let mut items: Vec<&Item> =
                    self.items.iter().filter(|item| item.kind == kind && !item.runs.is_empty()).collect();
                items.sort_by(|a, b| a.label.cmp(&b.label));
                (!items.is_empty()).then_some((kind, items))
            })
            .collect()
    }

    /// Records a finished run and saves the history. Returns why it couldn't be saved, the first time it can't.
    pub fn add(&mut self, kind: Kind, key: &str, label: &str, seconds: f64, ok: bool) -> Option<String> {
        let at = self.items.iter().position(|item| item.kind == kind && item.key == key).unwrap_or_else(|| {
            self.items.push(Item { kind, key: key.to_string(), label: String::new(), runs: Vec::new() });
            self.items.len() - 1
        });
        let item = &mut self.items[at];
        item.label = label.to_string();
        let sec = (seconds.max(0.0) * 10.0).round() / 10.0;
        item.runs.push(Timed { at: chrono::Local::now().to_rfc3339(), sec, ok });
        let extra = item.runs.len().saturating_sub(RUNS_KEPT);
        item.runs.drain(..extra);
        if self.read_only {
            return None;
        }
        match self.save() {
            Ok(()) => {
                self.warned = false;
                None
            }
            Err(why) if !self.warned => {
                self.warned = true;
                Some(why)
            }
            Err(_) => None,
        }
    }

    /// Writes the file whole or not at all, once (no retries: it's written on the controller's thread).
    fn save(&self) -> Result<(), String> {
        let mut entries: Vec<Value> = Vec::new();
        for item in &self.items {
            entries.push(serde_json::to_value(item).map_err(|error| error.to_string())?);
        }
        // An unreadable entry for an item recorded again since is replaced by it.
        let recorded = |entry: &&Value| {
            self.items.iter().any(|item| {
                serde_json::to_value(item.kind).is_ok_and(|kind| entry["kind"] == kind) && entry["key"] == item.key
            })
        };
        entries.extend(self.kept.iter().filter(|entry| !recorded(entry)).cloned());
        let text = serde_json::to_string_pretty(&Value::Array(entries)).map_err(|error| error.to_string())? + "\n";
        let temp = self.path.with_extension(format!("json.{}.tmp", std::process::id()));
        let written = std::fs::write(&temp, text).and_then(|()| std::fs::rename(&temp, &self.path));
        if written.is_err() {
            let _ = std::fs::remove_file(&temp);
        }
        written.map_err(|error| format!("couldn't write {} - {error}", self.path.display()))
    }

    /// " · 18% slower than usual (36s)" for the log line of the run just recorded, when it was notably off.
    pub fn note(&self, kind: Kind, key: &str) -> String {
        let Some(trend) = self.item(kind, key).and_then(|item| trend(&item.runs)) else { return String::new() };
        let (Some(change), Some(usual)) = (trend.notable(), trend.usual) else { return String::new() };
        let word = if change > 0.0 { "slower" } else { "faster" };
        format!(" · {}% {word} than usual ({})", (change.abs() * 100.0).round(), format_duration(usual))
    }
}

/// The latest run against the median of the up to 10 successful runs before it (once there are 3).
pub fn trend(runs: &[Timed]) -> Option<Trend> {
    let (last, before) = runs.split_last()?;
    let mut earlier: Vec<f64> =
        before.iter().filter(|run| run.ok).map(|run| run.sec).rev().take(TREND_BASE_RUNS).collect();
    let usual = (earlier.len() >= TREND_MIN_RUNS).then(|| median(&mut earlier));
    let change = usual.filter(|usual| *usual > 0.0 && last.ok).map(|usual| (last.sec - usual) / usual);
    Some(Trend { last: last.sec, last_ok: last.ok, usual, change })
}

fn median(values: &mut [f64]) -> f64 {
    values.sort_by(f64::total_cmp);
    let middle = values.len() / 2;
    if values.len() % 2 == 1 { values[middle] } else { (values[middle - 1] + values[middle]) / 2.0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{TempDir, write};

    fn runs(seconds: &[(f64, bool)]) -> Vec<Timed> {
        seconds.iter().map(|(sec, ok)| Timed { at: String::new(), sec: *sec, ok: *ok }).collect()
    }

    #[test]
    fn the_usual_is_the_median_of_the_earlier_successful_runs() {
        assert_eq!(trend(&[]), None);
        let two = trend(&runs(&[(10.0, true), (12.0, true), (30.0, true)])).unwrap();
        assert_eq!((two.last, two.usual, two.change), (30.0, None, None), "two earlier runs aren't enough");

        // The failed 99s run doesn't count; the median of 10, 12, 14, 16 is 13.
        let trend =
            trend(&runs(&[(10.0, true), (99.0, false), (12.0, true), (14.0, true), (16.0, true), (26.0, true)]))
                .unwrap();
        assert_eq!(trend.usual, Some(13.0));
        assert_eq!(trend.change, Some(1.0));
        assert_eq!(trend.notable(), Some(1.0));
    }

    #[test]
    fn a_failed_run_isnt_compared_with_the_usual() {
        let failed = trend(&runs(&[(10.0, true), (10.0, true), (10.0, true), (2.0, false)])).unwrap();
        assert_eq!((failed.usual, failed.change, failed.notable(), failed.last_ok), (Some(10.0), None, None, false));
    }

    #[test]
    fn a_change_of_under_a_second_isnt_notable() {
        let quick = trend(&runs(&[(0.3, true), (0.3, true), (0.3, true), (0.8, true)])).unwrap();
        assert!(quick.change.unwrap() > 1.0);
        assert_eq!(quick.notable(), None);
    }

    #[test]
    fn only_the_last_ten_successful_runs_make_the_usual() {
        let mut list: Vec<(f64, bool)> = vec![(100.0, true); 5];
        list.extend([(10.0, true); 10]);
        list.push((10.0, true));
        assert_eq!(trend(&runs(&list)).unwrap().usual, Some(10.0));
    }

    #[test]
    fn a_run_notably_off_its_usual_gets_a_note() {
        let dir = TempDir::new("durations-note");
        let mut durations = Durations::load(dir.path());
        for seconds in [40.0, 38.0, 36.0] {
            assert_eq!(durations.add(Kind::Build, "orders", "Orders", seconds, true), None);
            assert_eq!(durations.note(Kind::Build, "orders"), "", "no usual yet");
        }
        durations.add(Kind::Build, "orders", "Orders", 41.0, true);
        assert_eq!(durations.note(Kind::Build, "orders"), "", "within 15%");
        durations.add(Kind::Build, "orders", "Orders", 52.0, true);
        assert_eq!(durations.note(Kind::Build, "orders"), " · 33% slower than usual (39s)");
        durations.add(Kind::Build, "orders", "Orders", 20.0, true);
        assert_eq!(durations.note(Kind::Build, "orders"), " · 50% faster than usual (40s)");
        assert_eq!(durations.note(Kind::Start, "orders"), "", "another kind");
    }

    #[test]
    fn the_history_keeps_the_last_runs_and_survives_a_reload() {
        let dir = TempDir::new("durations-keep");
        let mut durations = Durations::load(dir.path());
        assert!(durations.is_empty());
        for n in 0..RUNS_KEPT + 5 {
            durations.add(Kind::Start, "shop", "Shop", n as f64, true);
        }
        durations.add(Kind::NpmCi, "shop", "Shop UI", 61.04, false);
        durations.add(Kind::Build, "orders", "Orders", 5.0, true);
        durations.add(Kind::Build, "api", "Api", 7.0, true);

        let loaded = Durations::load(dir.path());
        assert_eq!(loaded.problem, None);
        let starts = &loaded.item(Kind::Start, "shop").unwrap().runs;
        assert_eq!(starts.len(), RUNS_KEPT);
        assert_eq!(starts[0].sec, 5.0, "the oldest ones went");
        let npm = &loaded.item(Kind::NpmCi, "shop").unwrap();
        assert_eq!((npm.label.as_str(), npm.runs[0].sec, npm.runs[0].ok), ("Shop UI", 61.0, false));
        let groups: Vec<(Kind, Vec<&str>)> = loaded
            .grouped()
            .into_iter()
            .map(|(kind, items)| (kind, items.iter().map(|item| item.label.as_str()).collect()))
            .collect();
        assert_eq!(
            groups,
            [(Kind::Build, vec!["Api", "Orders"]), (Kind::Start, vec!["Shop"]), (Kind::NpmCi, vec!["Shop UI"])]
        );
    }

    #[test]
    fn entries_it_cant_read_are_written_back_as_they_were() {
        let dir = TempDir::new("durations-unknown");
        write(
            dir.join("history.json"),
            r#"[{"kind":"deploy","key":"unit","runs":[{"sec":3}]},
                {"kind":"build","key":"api","label":"Api","runs":[{"sec":4}]},
                {"kind":"start","key":"web","runs":[{"sec":"x"}]},
                {"kind":"build","key":"old","runs":[]}]"#,
        );
        let mut durations = Durations::load(dir.path());
        assert_eq!(durations.problem, None);
        assert_eq!(durations.grouped().len(), 1, "the readable items with runs");
        assert_eq!(durations.item(Kind::Build, "api").unwrap().runs[0].sec, 4.0);

        durations.add(Kind::Start, "web", "Web", 5.0, true); // replaces its unreadable entry
        let saved = json::read(&dir.join("history.json")).unwrap();
        let entries: Vec<(&str, &str)> = saved
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| (entry["kind"].as_str().unwrap(), entry["key"].as_str().unwrap()))
            .collect();
        assert_eq!(entries, [("build", "api"), ("build", "old"), ("start", "web"), ("deploy", "unit")]);
    }

    #[test]
    fn an_unreadable_history_is_moved_aside_and_a_new_one_starts() {
        let dir = TempDir::new("durations-corrupt");
        write(dir.join("history.json"), "{ not json");
        let mut durations = Durations::load(dir.path());
        assert!(
            durations.problem.as_deref().is_some_and(|why| why.contains("history.json.bad")),
            "{:?}",
            durations.problem
        );
        assert!(durations.is_empty());
        assert_eq!(std::fs::read_to_string(dir.join("history.json.bad")).unwrap(), "{ not json");
        durations.add(Kind::Build, "api", "Api", 4.0, true);
        assert_eq!(Durations::load(dir.path()).item(Kind::Build, "api").unwrap().runs.len(), 1);

        write(dir.join("history.json"), r#"{"an": "object"}"#);
        assert!(Durations::load(dir.path()).problem.is_some_and(|why| why.contains("it isn't a list")));
        assert!(Durations::load(&dir.join("missing")).problem.is_none());
    }

    #[test]
    fn a_history_it_cant_save_warns_once() {
        let dir = TempDir::new("durations-unwritable");
        let mut durations = Durations::load(&dir.join("missing-folder"));
        assert!(durations.add(Kind::Build, "api", "Api", 4.0, true).is_some_and(|why| why.contains("couldn't write")));
        assert_eq!(durations.add(Kind::Build, "api", "Api", 4.0, true), None, "said once");
        assert_eq!(durations.item(Kind::Build, "api").unwrap().runs.len(), 2, "still shown in D");
    }
}
