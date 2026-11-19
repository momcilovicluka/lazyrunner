//! Code coverage per test suite (#33), when the `coverage` setting is on: a suite's full run collects it - .NET with
//! coverlet (`--collect "XPlat Code Coverage"`, a Cobertura file), jest and Nx with `--coverage` (its
//! `coverage-summary.json`), vitest with `--coverage.enabled` (the same summary) - into the suite's own folder under
//! the tests' folder. Each suite's last full runs' coverage is kept in coverage.json there, so its change since the run
//! before - the suite's and each file's - outlasts a restart.

use std::collections::HashMap;
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::suites::Runner;

pub const FILE: &str = "coverage.json";
/// The folder under the tests' folder where each suite's run writes its coverage, in a folder of its own.
pub const RUNS_FOLDER: &str = "coverage";
/// Full runs kept per suite: a file's trend is drawn from them.
pub const RUNS_KEPT: usize = 8;
/// What coverlet writes, and jest's and vitest's summary.
const COBERTURA: &str = "coverage.cobertura.xml";
const SUMMARY: &str = "coverage-summary.json";
/// The data collector `dotnet test` asks coverlet for, and its package.
const COLLECTOR: &str = "XPlat Code Coverage";
/// How deep the folder coverlet writes into (a GUID one) is looked through.
const SEARCH_DEPTH: usize = 3;
/// A file's change smaller than this (in points) isn't a change.
const LEAST_CHANGE: f64 = 0.05;

/// A file's coverage in a run: its path (as the report names it, made whole), the share of its lines covered, and
/// (Cobertura only) its first line not covered.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FileCoverage {
    pub path: String,
    pub percent: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_uncovered: Option<u32>,
}

/// A run's coverage: the share of the suite's lines covered, and each file's.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Coverage {
    pub percent: f64,
    pub files: Vec<FileCoverage>,
}

/// A file whose coverage changed since the full run before: its coverage now, by how much (in points), and where
/// its first line not covered is.
#[derive(Clone, Debug, PartialEq)]
pub struct FileChange {
    pub path: String,
    pub percent: f64,
    pub change: f64,
    pub first_uncovered: Option<u32>,
}

/// What a suite knows about collecting its coverage while lazyrunner runs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct State {
    /// A .NET test project references coverlet's collector (itself or through a Directory.*.props).
    pub coverlet: bool,
    /// The run going on collects coverage.
    pub collecting: bool,
    /// When a run's coverage was last read.
    pub read_at: Option<Instant>,
}

/// Whether a suite's run can collect coverage: .NET only with coverlet referenced.
pub fn collects(runner: &Runner, coverlet: bool) -> bool {
    coverlet || !runner.backend()
}

/// The folder a suite's run writes its coverage into (`file_name`: the suite's own, as its results are named).
pub fn run_dir(tests_dir: &Path, file_name: &str) -> PathBuf {
    tests_dir.join(RUNS_FOLDER).join(file_name)
}

/// What `dotnet test` adds to collect coverage.
pub fn dotnet_args() -> Vec<OsString> {
    vec!["--collect".into(), COLLECTOR.into()]
}

/// What jest (or `nx test`, which passes them on) adds to write its coverage summary into `dir`.
pub fn jest_args(dir: &Path) -> Vec<OsString> {
    let dir = format!("--coverageDirectory={}", dir.display());
    vec!["--coverage".into(), "--coverageReporters=json-summary".into(), dir.into()]
}

/// What vitest adds to write its coverage summary into `dir`.
pub fn vitest_args(dir: &Path) -> Vec<OsString> {
    let dir = format!("--coverage.reportsDirectory={}", dir.display());
    vec!["--coverage.enabled".into(), "--coverage.reporter=json-summary".into(), dir.into()]
}

/// Whether a .NET test project references coverlet's collector: in itself, or in a Directory.Build.props or a
/// Directory.Packages.props (a `GlobalPackageReference`) between it and the repo's root.
pub fn references_coverlet(project: &Path, root: &Path) -> bool {
    static REFERENCE: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r#"(?i)<(?:Global)?PackageReference\s[^>]*\b(?:Include|Update)\s*=\s*"coverlet\.collector""#)
            .expect("a valid pattern")
    });
    let names = |text: String| REFERENCE.is_match(&text);
    if std::fs::read_to_string(project).is_ok_and(names) {
        return true;
    }
    let mut folder = project.parent();
    while let Some(dir) = folder {
        let props = ["Directory.Build.props", "Directory.Packages.props"];
        if props.iter().any(|name| std::fs::read_to_string(dir.join(name)).is_ok_and(names)) {
            return true;
        }
        if dir == root {
            break;
        }
        folder = dir.parent();
    }
    false
}

/// The coverage a run wrote into `dir`: coverlet's Cobertura file for .NET, the summary for jest and vitest.
pub fn collect(runner: &Runner, dir: &Path) -> Result<Coverage, String> {
    let file = match runner {
        Runner::Dotnet { .. } => find(dir, COBERTURA, SEARCH_DEPTH)
            .ok_or_else(|| format!("no {COBERTURA} in {} - is coverlet.collector restored?", dir.display()))?,
        Runner::Npm { .. } | Runner::Nx { .. } => dir.join(SUMMARY),
    };
    let text = std::fs::read_to_string(&file).map_err(|error| format!("can't read {} ({error})", file.display()))?;
    match runner {
        Runner::Dotnet { .. } => read_cobertura(&text),
        Runner::Npm { .. } | Runner::Nx { .. } => read_summary(&text),
    }
}

/// The newest file called `name` in `dir` or its folders, `depth` deep.
fn find(dir: &Path, name: &str, depth: usize) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    let mut found: Vec<PathBuf> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() && depth > 0 {
            found.extend(find(&path, name, depth - 1));
        } else if entry.file_name().eq_ignore_ascii_case(name) {
            found.push(path);
        }
    }
    let modified = |path: &PathBuf| std::fs::metadata(path).and_then(|meta| meta.modified()).ok();
    found.into_iter().max_by_key(modified)
}

/// A Cobertura report (coverlet's): the run's line rate, and each file's lines - a file's classes (partial ones, the
/// compiler's) together, a line counted once and covered when any of them hit it.
pub fn read_cobertura(text: &str) -> Result<Coverage, String> {
    let text = text.trim_start_matches('\u{feff}');
    let document = roxmltree::Document::parse(text).map_err(|error| format!("not a Cobertura file ({error})"))?;
    let named = |node: &roxmltree::Node, name: &str| node.is_element() && node.tag_name().name() == name;
    let root = document.root_element();
    if !named(&root, "coverage") {
        return Err("not a Cobertura file (no <coverage>)".into());
    }
    let source = document.descendants().find(|node| named(node, "source")).and_then(|node| node.text());
    let mut lines: HashMap<String, HashMap<u32, bool>> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for class in document.descendants().filter(|node| named(node, "class")) {
        let Some(name) = class.attribute("filename") else { continue };
        let path = whole(source, name);
        if !lines.contains_key(&path) {
            order.push(path.clone());
        }
        let file = lines.entry(path).or_default();
        let own = class.children().filter(|node| named(node, "lines")).flat_map(|node| node.children());
        for line in own.filter(|node| named(node, "line")) {
            let Some(number) = line.attribute("number").and_then(|number| number.parse::<u32>().ok()) else {
                continue;
            };
            let hit = line.attribute("hits").and_then(|hits| hits.parse::<u64>().ok()).is_some_and(|hits| hits > 0);
            *file.entry(number).or_default() |= hit;
        }
    }
    let files: Vec<FileCoverage> = order
        .into_iter()
        .filter_map(|path| {
            let file = lines.remove(&path)?;
            let covered = file.values().filter(|hit| **hit).count();
            let first_uncovered = file.iter().filter(|(_, hit)| !**hit).map(|(number, _)| *number).min();
            Some(FileCoverage { path, percent: share(covered, file.len()), first_uncovered })
        })
        .collect();
    let rate = root.attribute("line-rate").and_then(|rate| rate.parse::<f64>().ok());
    let percent = rate.map_or_else(|| overall(&files), |rate| rate * 100.0);
    Ok(Coverage { percent, files })
}

/// A Cobertura file's name made whole with its report's source folder, unless it already is.
fn whole(source: Option<&str>, name: &str) -> String {
    match source.map(str::trim).filter(|source| !source.is_empty()) {
        Some(source) if !Path::new(name).is_absolute() => Path::new(source).join(name).display().to_string(),
        _ => name.to_string(),
    }
}

/// jest's `coverage-summary.json` (vitest writes the same): its total's lines, and each file's.
pub fn read_summary(text: &str) -> Result<Coverage, String> {
    let summary: Value = serde_json::from_str(text).map_err(|error| format!("not a coverage summary ({error})"))?;
    let entries = summary.as_object().ok_or("not a coverage summary (not an object)")?;
    let lines = |entry: &Value| {
        let lines = &entry["lines"];
        let count = |name: &str| lines[name].as_u64().unwrap_or(0) as usize;
        (count("covered"), count("total"))
    };
    let total = entries.get("total").ok_or("no total in the coverage summary")?;
    let (covered, all) = lines(total);
    let files = entries
        .iter()
        .filter(|(name, _)| *name != "total")
        .map(|(path, entry)| {
            let (covered, all) = lines(entry);
            FileCoverage { path: path.clone(), percent: share(covered, all), first_uncovered: None }
        })
        .collect();
    Ok(Coverage { percent: share(covered, all), files })
}

/// `part` of `all` in percent; all of nothing is all of it.
fn share(part: usize, all: usize) -> f64 {
    if all == 0 { 100.0 } else { part as f64 * 100.0 / all as f64 }
}

/// The files' coverage together, when a report doesn't say: their average.
fn overall(files: &[FileCoverage]) -> f64 {
    if files.is_empty() { 0.0 } else { files.iter().map(|file| file.percent).sum::<f64>() / files.len() as f64 }
}

/// A suite's last full runs' coverage, oldest first.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SuiteCoverage {
    pub runs: Vec<Coverage>,
}

impl SuiteCoverage {
    /// The last full run's coverage.
    pub fn last(&self) -> Option<&Coverage> {
        self.runs.last()
    }

    /// The full run's before the last.
    pub fn before(&self) -> Option<&Coverage> {
        self.runs.len().checked_sub(2).and_then(|at| self.runs.get(at))
    }

    /// The suite's change since the full run before, in points.
    pub fn change(&self) -> Option<f64> {
        Some(self.last()?.percent - self.before()?.percent)
    }

    /// The files whose coverage changed most since the full run before, the drops first (the biggest first), then
    /// the gains; at most `most`. A file new in the last run, or gone from it, didn't change.
    pub fn changed_files(&self, most: usize) -> Vec<FileChange> {
        let (Some(last), Some(before)) = (self.last(), self.before()) else { return Vec::new() };
        let was: HashMap<&str, f64> = before.files.iter().map(|file| (file.path.as_str(), file.percent)).collect();
        let mut changed: Vec<FileChange> = last
            .files
            .iter()
            .filter_map(|file| {
                let change = file.percent - was.get(file.path.as_str())?;
                (change.abs() >= LEAST_CHANGE).then(|| FileChange {
                    path: file.path.clone(),
                    percent: file.percent,
                    change,
                    first_uncovered: file.first_uncovered,
                })
            })
            .collect();
        changed.sort_by(|a, b| {
            let drop = |change: &FileChange| change.change >= 0.0;
            drop(a).cmp(&drop(b)).then_with(|| b.change.abs().total_cmp(&a.change.abs()))
        });
        changed.truncate(most);
        changed
    }

    /// A file's coverage in the runs kept that had it, oldest first.
    pub fn trend(&self, path: &str) -> Vec<f64> {
        let files = self.runs.iter().map(|run| run.files.iter().find(|file| file.path == path));
        files.flatten().map(|file| file.percent).collect()
    }

    fn record(&mut self, coverage: Coverage) {
        self.runs.push(coverage);
        let extra = self.runs.len().saturating_sub(RUNS_KEPT);
        self.runs.drain(..extra);
    }
}

/// Every suite's coverage, by its key (coverage.json in the tests' folder).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CoverageHistory(pub HashMap<String, SuiteCoverage>);

impl CoverageHistory {
    /// The coverage kept in the tests' folder `dir`; none when there's no file or it doesn't read.
    pub fn load(dir: &Path) -> Self {
        crate::json::read(&dir.join(FILE)).and_then(|value| serde_json::from_value(value).ok()).unwrap_or_default()
    }

    pub fn save(&self, dir: &Path) -> io::Result<()> {
        let text = serde_json::to_string(self).map_err(io::Error::other)?;
        std::fs::create_dir_all(dir)?;
        std::fs::write(dir.join(FILE), text)
    }

    pub fn suite(&self, key: &str) -> Option<&SuiteCoverage> {
        self.0.get(key)
    }

    /// A full run's coverage, the newest of the suite's.
    pub fn record(&mut self, key: &str, coverage: Coverage) {
        self.0.entry(key.to_string()).or_default().record(coverage);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{TempDir, write};

    const COBERTURA_SAMPLE: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<coverage line-rate="0.784" branch-rate="0.5" version="1.9" timestamp="1700000000">
  <sources><source>C:\src\shop\</source></sources>
  <packages>
    <package name="Orders" line-rate="0.784">
      <classes>
        <class name="Orders.Cart" filename="Orders\Cart.cs" line-rate="0.5">
          <lines>
            <line number="10" hits="3" branch="False" />
            <line number="11" hits="0" branch="False" />
            <line number="14" hits="0" branch="False" />
            <line number="12" hits="1" branch="False" />
          </lines>
        </class>
        <class name="Orders.Cart/&lt;Totals&gt;d__2" filename="Orders\Cart.cs" line-rate="1">
          <lines><line number="11" hits="2" branch="False" /></lines>
        </class>
        <class name="Orders.Pricing" filename="Orders\Pricing.cs" line-rate="1">
          <lines><line number="3" hits="1" branch="False" /></lines>
        </class>
      </classes>
    </package>
  </packages>
</coverage>"#;

    fn file(path: &str, percent: f64) -> FileCoverage {
        FileCoverage { path: path.into(), percent, first_uncovered: None }
    }

    #[test]
    fn a_cobertura_file_gives_the_runs_rate_and_each_files_lines_together() {
        let coverage = read_cobertura(COBERTURA_SAMPLE).unwrap();
        assert!((coverage.percent - 78.4).abs() < 1e-9, "{}", coverage.percent);
        let cart = Path::new(r"C:\src\shop\").join(r"Orders\Cart.cs").display().to_string();
        assert_eq!(coverage.files[0].path, cart, "made whole with its source folder");
        assert_eq!(coverage.files[0].percent, 75.0, "line 11 hit by the compiler's class: 3 of its 4 lines");
        assert_eq!(coverage.files[0].first_uncovered, Some(14));
        assert_eq!((coverage.files[1].percent, coverage.files[1].first_uncovered), (100.0, None));
        assert!(read_cobertura("<TestRun />").is_err(), "not coverage");
    }

    #[test]
    fn jests_summary_gives_the_totals_lines_and_each_files() {
        let summary = r#"{
            "total": { "lines": { "total": 200, "covered": 150, "skipped": 0, "pct": 75 }, "statements": {} },
            "/w/web/src/cart.ts": { "lines": { "total": 40, "covered": 30, "skipped": 0, "pct": 75 } },
            "/w/web/src/empty.ts": { "lines": { "total": 0, "covered": 0, "skipped": 0, "pct": "Unknown" } }
        }"#;
        let coverage = read_summary(summary).unwrap();
        assert_eq!(coverage.percent, 75.0);
        assert_eq!(coverage.files, [file("/w/web/src/cart.ts", 75.0), file("/w/web/src/empty.ts", 100.0)]);
        assert!(read_summary("[]").is_err());
    }

    #[test]
    fn a_suites_change_and_its_files_most_changed_drops_first() {
        let mut history = CoverageHistory::default();
        let run = |percent, files: &[(&str, f64)]| Coverage {
            percent,
            files: files.iter().map(|(path, percent)| file(path, *percent)).collect(),
        };
        history.record("shop", run(80.0, &[("a.ts", 90.0), ("b.ts", 50.0), ("c.ts", 70.0), ("d.ts", 10.0)]));
        assert_eq!(history.suite("shop").unwrap().change(), None, "one run: nothing to compare");
        history.record(
            "shop",
            run(78.8, &[("a.ts", 80.0), ("b.ts", 51.0), ("c.ts", 40.0), ("d.ts", 10.01), ("new.ts", 0.0)]),
        );
        let suite = history.suite("shop").unwrap();
        assert!((suite.change().unwrap() + 1.2).abs() < 1e-9);
        let changed: Vec<(String, f64)> =
            suite.changed_files(5).into_iter().map(|file| (file.path, file.change.round())).collect();
        let expected =
            [("c.ts", -30.0), ("a.ts", -10.0), ("b.ts", 1.0)].map(|(path, change)| (path.to_string(), change));
        assert_eq!(changed, expected, "d.ts barely moved, new.ts is new");
        assert_eq!(suite.changed_files(1).len(), 1);
        assert_eq!(suite.trend("a.ts"), [90.0, 80.0]);
        assert_eq!(suite.trend("new.ts"), [0.0]);
    }

    #[test]
    fn the_last_full_runs_coverage_outlasts_a_restart_and_old_runs_go() {
        let temp = TempDir::new("coverage-history");
        let mut history = CoverageHistory::default();
        for percent in 0..RUNS_KEPT + 2 {
            history.record("shop", Coverage { percent: percent as f64, files: vec![file("a.ts", 1.0)] });
        }
        let runs = &history.suite("shop").unwrap().runs;
        assert_eq!((runs.len(), runs[0].percent), (RUNS_KEPT, 2.0), "the two oldest went");
        history.save(temp.path()).unwrap();
        assert_eq!(CoverageHistory::load(temp.path()), history);
        assert_eq!(CoverageHistory::load(&temp.join("nowhere")), CoverageHistory::default());
    }

    #[test]
    fn coverlet_is_found_in_the_project_or_a_props_file_above_it() {
        let temp = TempDir::new("coverage-coverlet");
        let project = temp.join("tests/Orders.Tests/Orders.Tests.csproj");
        write(&project, r#"<Project><ItemGroup><PackageReference Include="xunit" /></ItemGroup></Project>"#);
        write(
            temp.join("Directory.Packages.props"),
            r#"<Project><PackageVersion Include="coverlet.collector" Version="6.0.2" /></Project>"#,
        );
        assert!(!references_coverlet(&project, temp.path()), "a version alone references nothing");
        write(
            temp.join("tests/Directory.Build.props"),
            r#"<Project><ItemGroup><PackageReference Include="coverlet.collector" /></ItemGroup></Project>"#,
        );
        assert!(references_coverlet(&project, temp.path()));
        let own = temp.join("Billing.Tests.csproj");
        write(&own, r#"<PackageReference Include="Coverlet.Collector" Version="6.0.2">"#);
        assert!(references_coverlet(&own, temp.path()), "in itself, any case");
    }

    #[test]
    fn a_run_collects_coverage_with_its_runners_flags() {
        let dotnet = Runner::Dotnet { project: "x.csproj".into() };
        assert!(!collects(&dotnet, false), "no coverlet: no coverage");
        assert!(collects(&dotnet, true));
        assert!(collects(&Runner::Npm { dir: "web".into(), vitest: false }, false));
        assert_eq!(dotnet_args(), ["--collect", "XPlat Code Coverage"]);
        let dir = Path::new("state").join("coverage");
        let flags = |args: Vec<OsString>| args.into_iter().map(|arg| arg.to_string_lossy().into_owned()).collect();
        let jest: Vec<String> = flags(jest_args(&dir));
        assert_eq!(jest[..2], ["--coverage", "--coverageReporters=json-summary"]);
        assert_eq!(jest[2], format!("--coverageDirectory={}", dir.display()));
        let vitest: Vec<String> = flags(vitest_args(&dir));
        assert_eq!(vitest[..2], ["--coverage.enabled", "--coverage.reporter=json-summary"]);
        assert_eq!(vitest[2], format!("--coverage.reportsDirectory={}", dir.display()));
    }

    #[test]
    fn a_runs_coverage_is_found_where_its_runner_wrote_it() {
        let temp = TempDir::new("coverage-collect");
        let dotnet = Runner::Dotnet { project: "x.csproj".into() };
        assert!(collect(&dotnet, temp.path()).is_err_and(|why| why.contains("no coverage.cobertura.xml")));
        write(temp.join("1b2c3d4e-guid/coverage.cobertura.xml"), COBERTURA_SAMPLE);
        assert_eq!(collect(&dotnet, temp.path()).map(|coverage| coverage.files.len()), Ok(2));
        let jest = Runner::Nx { workspace: "web".into(), project: "ui".into() };
        write(temp.join(SUMMARY), r#"{ "total": { "lines": { "total": 4, "covered": 1 } } }"#);
        assert_eq!(collect(&jest, temp.path()).map(|coverage| coverage.percent), Ok(25.0));
    }
}
