//! Test suites (T): the project's .NET test projects (`dotnet test`: NUnit, xUnit and MSTest alike), and its npm
//! apps' jest or vitest tests (an Nx workspace's projects one by one), and what their last run found. Each run
//! writes a TRX or jest JSON file to the project's state folder, so the last results are still there after a
//! restart.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::Instant;

use serde_json::Value;

use super::child::Child;
use crate::json;
use crate::output::strip_ansi;
use crate::profile::{App, AppKind};

/// Lines of a run's output a suite keeps to show (its log file has all of them).
pub const MAX_OUTPUT: usize = 400;
/// Folders never looked in for tests: build output, packages, tools' own.
const SKIPPED: [&str; 15] = [
    ".git",
    "node_modules",
    "bin",
    "obj",
    "dist",
    "target",
    ".vs",
    ".idea",
    ".angular",
    ".nx",
    ".nuget",
    "packages",
    "TestResults",
    "coverage",
    "wwwroot",
];
/// How a test file that failed as a whole (it doesn't compile) is named among its tests.
const WHOLE_FILE: &str = " (whole file)";
/// How many folder entries a search for test projects reads at most.
const SEARCH_BUDGET: usize = 200_000;
/// What a .NET test project references.
const TEST_SDK: &str = "Microsoft.NET.Test.Sdk";
/// What a test file is written in: `*.spec.ts`, `*.test.jsx`, ... (not `tsconfig.spec.json`).
const CODE: [&str; 8] = ["ts", "tsx", "js", "jsx", "mjs", "cjs", "mts", "cts"];
/// npm's default `test` script: there are no tests.
const NO_TESTS: &str = "no test specified";

/// What runs a suite, and how its results come back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Runner {
    /// `dotnet test` on a test project; results as TRX.
    Dotnet { project: PathBuf },
    /// An npm app's `test` script, running jest or vitest; results as jest's JSON (vitest writes the same).
    Npm { dir: PathBuf, vitest: bool },
    /// `nx test <project>` in an Nx workspace (jest); results as jest's JSON.
    Nx { workspace: PathBuf, project: String },
}

impl Runner {
    /// The tests view's group for it.
    pub fn group(&self) -> &'static str {
        match self {
            Runner::Dotnet { .. } => ".NET · dotnet test",
            Runner::Npm { vitest: false, .. } => "npm · jest",
            Runner::Npm { vitest: true, .. } => "npm · vitest",
            Runner::Nx { .. } => "Nx · nx test",
        }
    }

    /// A backend suite (.NET), not a frontend one (npm, Nx).
    pub fn backend(&self) -> bool {
        matches!(self, Runner::Dotnet { .. })
    }

    /// The npm folder it runs from (npm ci there rewrites what it needs).
    pub fn npm_dir(&self) -> Option<&Path> {
        match self {
            Runner::Npm { dir, .. } => Some(dir),
            Runner::Nx { workspace, .. } => Some(workspace),
            Runner::Dotnet { .. } => None,
        }
    }

    fn extension(&self) -> &'static str {
        if matches!(self, Runner::Dotnet { .. }) { "trx" } else { "json" }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Passed,
    Failed,
    Skipped,
}

/// One test's result.
#[derive(Clone, Debug, PartialEq)]
pub struct TestResult {
    pub name: String,
    pub outcome: Outcome,
    pub ms: f64,
    pub message: String,
    pub stack: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// Not run (no results from an earlier run either).
    Idle,
    Queued,
    Running,
    Passed,
    Failed,
    /// It didn't run: its build failed, or it left no results.
    Error,
    /// Stopped (s); the results are the run's before.
    Cancelled,
}

impl std::fmt::Debug for Suite {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Suite").field("key", &self.key).field("status", &self.status).finish_non_exhaustive()
    }
}

pub struct Suite {
    pub key: String,
    pub name: String,
    pub runner: Runner,
    /// The app it belongs to (its key).
    pub app: Option<String>,
    /// An Nx project with a test target but no test files: nothing to run.
    pub untested: bool,
    /// Why it can't run for want of packages (no node_modules, no Nx in them), when it can't.
    pub needs_packages: Option<String>,
    pub status: Status,
    /// What it was before it was queued: a stop puts it back.
    pub before: Status,
    /// Why it waits, or what went wrong.
    pub note: String,
    pub tests: Vec<TestResult>,
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
    pub seconds: f64,
    pub last_run: Option<chrono::DateTime<chrono::Local>>,
    /// The output worth showing (passed tests are only counted), the newest last.
    pub output: VecDeque<String>,
    /// Tests the current run has finished.
    pub done: usize,
    pub started: Option<Instant>,
    /// A .NET suite's project is built (it runs with --no-build).
    pub built: bool,
    /// The run is some tests only: the failed ones (f, F), or one (picked with l).
    pub failed_only: bool,
    /// The tests such a run asked for.
    pub rerun: Vec<String>,
    pub(crate) job: u64,
    pub(crate) child: Option<Child>,
    /// All of the run's output, for its log file.
    pub(crate) lines: Vec<String>,
}

impl Suite {
    pub fn new(key: impl Into<String>, name: impl Into<String>, runner: Runner) -> Self {
        Self {
            key: key.into(),
            name: name.into(),
            runner,
            app: None,
            untested: false,
            needs_packages: None,
            status: Status::Idle,
            before: Status::Idle,
            note: String::new(),
            tests: Vec::new(),
            passed: 0,
            failed: 0,
            skipped: 0,
            seconds: 0.0,
            last_run: None,
            output: VecDeque::new(),
            done: 0,
            started: None,
            built: false,
            failed_only: false,
            rerun: Vec::new(),
            job: 0,
            child: None,
            lines: Vec::new(),
        }
    }

    pub fn total(&self) -> usize {
        self.tests.len()
    }

    pub fn is_active(&self) -> bool {
        matches!(self.status, Status::Queued | Status::Running)
    }

    /// The last full run's results.
    pub fn results_file(&self, dir: &Path) -> PathBuf {
        dir.join(format!("{}.{}", self.file_name(), self.runner.extension()))
    }

    /// A run of the failed tests only writes its own file, merged into the full run's results.
    pub fn rerun_file(&self, dir: &Path) -> PathBuf {
        dir.join(format!("{}.rerun.{}", self.file_name(), self.runner.extension()))
    }

    pub fn log_file(&self, dir: &Path) -> PathBuf {
        dir.join(format!("{}.log", self.file_name()))
    }

    fn file_name(&self) -> String {
        self.key.chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { '_' }).collect()
    }

    pub fn set_tests(&mut self, tests: Vec<TestResult>) {
        let count = |outcome| tests.iter().filter(|test| test.outcome == outcome).count();
        (self.passed, self.failed, self.skipped) =
            (count(Outcome::Passed), count(Outcome::Failed), count(Outcome::Skipped));
        self.tests = tests;
    }

    /// The last full run's results, if there is one (after a restart).
    pub fn load_last(&mut self, dir: &Path) {
        let file = self.results_file(dir);
        let Ok((tests, seconds)) = read_results(&self.runner, &file) else { return };
        self.set_tests(tests);
        self.seconds = seconds.unwrap_or_default();
        self.status = if self.failed > 0 { Status::Failed } else { Status::Passed };
        self.last_run = std::fs::metadata(&file).and_then(|meta| meta.modified()).ok().map(Into::into);
    }

    /// A run of the `requested` tests only: their results replace the last ones (jest and vitest also list the
    /// file's other tests, as skipped: those keep theirs). A file that failed as a whole and now loads drops out; a
    /// new failure joins. Returns how many of the requested tests ran.
    pub fn merge(&mut self, fresh: Vec<TestResult>, requested: &[String]) -> usize {
        let mut ran = 0;
        let mut merged = Vec::new();
        for test in std::mem::take(&mut self.tests) {
            if !requested.contains(&test.name) {
                merged.push(test);
                continue;
            }
            match fresh.iter().find(|new| new.name == test.name) {
                Some(new) => {
                    ran += 1;
                    merged.push(new.clone());
                }
                None if test.name.ends_with(WHOLE_FILE) => {}
                None => merged.push(test),
            }
        }
        let new_failures: Vec<TestResult> = fresh
            .into_iter()
            .filter(|new| new.outcome == Outcome::Failed && !merged.iter().any(|test| test.name == new.name))
            .collect();
        merged.extend(new_failures);
        self.set_tests(merged);
        ran
    }

    /// A line of the running suite's output: kept for its log, counted when it is a finished test, shown unless
    /// it is a passed test (there are hundreds) or a build warning.
    pub(crate) fn receive(&mut self, line: &str) {
        let text = strip_ansi(line);
        self.lines.push(text.clone());
        let trimmed = text.trim_start();
        if trimmed.starts_with("Passed ") || trimmed.starts_with("✓ ") {
            self.done += 1;
            return;
        }
        if ["Failed ", "Skipped ", "PASS ", "FAIL ", "× ", "↓ "].iter().any(|start| trimmed.starts_with(start)) {
            self.done += 1;
        }
        if trimmed.is_empty() || text.contains(": warning ") {
            return;
        }
        self.output.push_back(text);
        while self.output.len() > MAX_OUTPUT {
            self.output.pop_front();
        }
    }
}

// ---------------------------------------------------------------- finding them

/// The project's suites: .NET projects referencing the test SDK, npm apps whose `test` script runs jest or vitest,
/// and the Nx projects with tests of an npm app that is an Nx workspace. By kind, then name.
pub fn discover(root: &Path, apps: &[App]) -> Vec<Suite> {
    let mut suites = Vec::new();
    walk(root, |path| {
        if path.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("csproj"))
            && std::fs::read_to_string(path).is_ok_and(|text| text.contains(TEST_SDK))
        {
            let name = path.file_stem().map_or(String::new(), |stem| stem.to_string_lossy().into_owned());
            let relative = path.strip_prefix(root).unwrap_or(path).to_string_lossy().replace('\\', "/");
            suites.push(Suite::new(format!("dotnet:{relative}"), name, Runner::Dotnet { project: path.to_path_buf() }));
        }
    });
    suites.sort_by(|a, b| a.name.cmp(&b.name));
    for app in apps {
        let AppKind::Npm { dir, .. } = &app.kind else { continue };
        let folder = root.join(dir);
        let found = if folder.join("nx.json").is_file() {
            nx_suites(&folder)
        } else {
            npm_suite(app, &folder).into_iter().collect()
        };
        for suite in found {
            if !suites.iter().any(|known| known.key == suite.key) {
                suites.push(suite);
            }
        }
    }
    for suite in &mut suites {
        suite.app = owner(root, apps, suite);
    }
    suites
}

/// The app a suite belongs to: the one whose `"tests"` name it, else the .NET app its project references, else the
/// npm app whose folder it runs in.
fn owner(root: &Path, apps: &[App], suite: &Suite) -> Option<String> {
    let listed = apps.iter().find(|app| app.tests.iter().any(|test| answers_to(root, suite, test)));
    let found = listed.or_else(|| match &suite.runner {
        Runner::Dotnet { project } => {
            let referenced = references(project);
            apps.iter().find(|app| match &app.kind {
                AppKind::Dotnet { project: own, .. } => referenced.contains(&same(&root.join(own))),
                AppKind::Npm { .. } => false,
            })
        }
        Runner::Npm { dir: folder, .. } | Runner::Nx { workspace: folder, .. } => {
            apps.iter().find(|app| match &app.kind {
                AppKind::Npm { dir, .. } => same(&root.join(dir)) == same(folder),
                AppKind::Dotnet { .. } => false,
            })
        }
    });
    found.map(|app| app.key.clone())
}

/// Whether a `"tests"` entry names the suite: by its name (a .NET test project's, an Nx project's), or by its
/// project's or folder's path from the repo root.
fn answers_to(root: &Path, suite: &Suite, entry: &str) -> bool {
    let entry = entry.trim().replace('\\', "/");
    let path = match &suite.runner {
        Runner::Dotnet { project } => Some(project),
        Runner::Npm { dir, .. } => Some(dir),
        Runner::Nx { .. } => None,
    };
    let relative =
        path.and_then(|path| path.strip_prefix(root).ok()).map(|path| path.to_string_lossy().replace('\\', "/"));
    suite.name.eq_ignore_ascii_case(&entry) || relative.is_some_and(|relative| relative.eq_ignore_ascii_case(&entry))
}

/// The projects a .csproj references (`same` paths).
fn references(project: &Path) -> Vec<String> {
    static REFERENCE: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r#"<ProjectReference\s[^>]*Include\s*=\s*"([^"]+)""#).expect("a valid pattern")
    });
    let Ok(text) = std::fs::read_to_string(project) else { return Vec::new() };
    let dir = project.parent().unwrap_or(Path::new(""));
    REFERENCE.captures_iter(&text).map(|found| same(&dir.join(found[1].replace('\\', "/")))).collect()
}

/// A path as it compares with another: `..` and `.` resolved, one kind of slash, any case.
fn same(path: &Path) -> String {
    let mut parts: Vec<String> = Vec::new();
    for part in path.components() {
        match part {
            std::path::Component::ParentDir => {
                parts.pop();
            }
            std::path::Component::CurDir => {}
            other => parts.push(other.as_os_str().to_string_lossy().to_lowercase()),
        }
    }
    parts.join("/")
}

/// An npm app's tests, when its `test` script runs jest or vitest (other runners leave no results to read).
fn npm_suite(app: &App, folder: &Path) -> Option<Suite> {
    let package = json::read(&folder.join("package.json"))?;
    let script = package["scripts"]["test"].as_str().filter(|script| !script.contains(NO_TESTS))?;
    let depends = |name: &str| ["dependencies", "devDependencies"].iter().any(|deps| package[deps].get(name).is_some());
    let vitest = script.contains("vitest") || (!script.contains("jest") && depends("vitest"));
    if !vitest && !script.contains("jest") && !depends("jest") {
        return None;
    }
    let key = format!("npm:{}", folder.display());
    Some(Suite::new(key, app.name.clone(), Runner::Npm { dir: folder.to_path_buf(), vitest }))
}

/// An Nx workspace's projects that have a test target (or a jest config) and test files.
fn nx_suites(workspace: &Path) -> Vec<Suite> {
    let mut projects = Vec::new();
    let mut specs = Vec::new();
    walk(workspace, |path| {
        let name = path.file_name().map_or(String::new(), |name| name.to_string_lossy().into_owned());
        if name == "project.json" {
            projects.extend(path.parent().map(Path::to_path_buf));
        } else if [".spec.", ".test."].iter().any(|part| name.contains(part))
            && path.extension().is_some_and(|extension| CODE.iter().any(|code| extension.eq_ignore_ascii_case(code)))
        {
            specs.push(path.to_path_buf());
        }
    });
    let mut suites: Vec<Suite> = projects
        .into_iter()
        .filter_map(|dir| {
            let project = json::read(&dir.join("project.json"))?;
            let name = project["name"].as_str()?.to_string();
            let jest_config =
                ["ts", "js", "mjs", "cjs"].iter().any(|ext| dir.join(format!("jest.config.{ext}")).is_file());
            (project["targets"].get("test").is_some() || jest_config).then(|| {
                let key = format!("nx:{}:{name}", workspace.display());
                let runner = Runner::Nx { workspace: workspace.to_path_buf(), project: name.clone() };
                let mut suite = Suite::new(key, name, runner);
                suite.untested = !specs.iter().any(|spec| spec.starts_with(&dir));
                suite
            })
        })
        .collect();
    suites.sort_by(|a, b| a.name.cmp(&b.name));
    suites
}

/// Every file under `root`, breadth first, not into links, build output or packages, and only so far.
fn walk(root: &Path, mut visit: impl FnMut(&Path)) {
    let mut budget = SEARCH_BUDGET;
    let mut folders = VecDeque::from([root.to_path_buf()]);
    while let Some(folder) = folders.pop_front() {
        let Ok(entries) = std::fs::read_dir(&folder) else { continue };
        for entry in entries.flatten() {
            if budget == 0 {
                return;
            }
            budget -= 1;
            let Ok(kind) = entry.file_type() else { continue };
            if kind.is_dir() {
                if !SKIPPED.iter().any(|skipped| entry.file_name().eq_ignore_ascii_case(skipped)) {
                    folders.push_back(entry.path());
                }
            } else if kind.is_file() {
                visit(&entry.path());
            }
        }
    }
}

// ---------------------------------------------------------------- results

/// The tests in a results file, and how long the run took when the file says.
pub fn read_results(runner: &Runner, path: &Path) -> Result<(Vec<TestResult>, Option<f64>), String> {
    let text = std::fs::read_to_string(path).map_err(|error| format!("can't read {} ({error})", path.display()))?;
    match runner {
        Runner::Dotnet { .. } => read_trx(&text),
        Runner::Npm { .. } | Runner::Nx { .. } => read_jest(&text),
    }
}

/// A TRX file (`dotnet test --logger trx`): each result named Namespace.Class.Method.
fn read_trx(text: &str) -> Result<(Vec<TestResult>, Option<f64>), String> {
    let text = text.trim_start_matches('\u{feff}');
    let document = roxmltree::Document::parse(text).map_err(|error| format!("not a TRX file ({error})"))?;
    let named = |node: roxmltree::Node, name: &str| node.is_element() && node.tag_name().name() == name;
    let classes: HashMap<&str, &str> = document
        .descendants()
        .filter(|node| named(*node, "UnitTest"))
        .filter_map(|test| {
            let method = test.children().find(|child| named(*child, "TestMethod"))?;
            Some((test.attribute("id")?, method.attribute("className")?))
        })
        .collect();
    let text_of = |node: roxmltree::Node, path: &[&str]| -> String {
        let mut node = Some(node);
        for name in path {
            node = node.and_then(|node| node.children().find(|child| named(*child, name)));
        }
        node.and_then(|node| node.text()).unwrap_or_default().trim().to_string()
    };
    let tests = document
        .descendants()
        .filter(|node| named(*node, "UnitTestResult"))
        .map(|result| {
            let name = result.attribute("testName").unwrap_or_default();
            let class = result.attribute("testId").and_then(|id| classes.get(id));
            let qualified =
                class.is_some_and(|class| name.strip_prefix(class).is_some_and(|rest| rest.starts_with('.')));
            let name = match class {
                Some(class) if !qualified => format!("{class}.{name}"),
                _ => name.to_string(),
            };
            let outcome = match result.attribute("outcome") {
                Some("Passed") => Outcome::Passed,
                Some("Failed" | "Error" | "Timeout" | "Aborted") => Outcome::Failed,
                _ => Outcome::Skipped,
            };
            TestResult {
                name,
                outcome,
                ms: result.attribute("duration").map_or(0.0, |duration| span_seconds(duration) * 1000.0),
                message: text_of(result, &["Output", "ErrorInfo", "Message"]),
                stack: text_of(result, &["Output", "ErrorInfo", "StackTrace"]),
            }
        })
        .collect();
    let times = document.descendants().find(|node| named(*node, "Times"));
    let time = |name| {
        times.and_then(|times| times.attribute(name)).and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok())
    };
    let seconds = match (time("start"), time("finish")) {
        (Some(start), Some(finish)) => Some((finish - start).num_milliseconds() as f64 / 1000.0),
        _ => None,
    };
    Ok((tests, seconds))
}

/// "00:01:02.5000000" in seconds.
fn span_seconds(span: &str) -> f64 {
    span.split(':').fold(0.0, |total, part| total * 60.0 + part.parse::<f64>().unwrap_or(0.0))
}

/// jest's `--json` output (vitest's json reporter writes the same): each test by its full name; a file that failed
/// as a whole (it doesn't compile, an import is missing) is one failed test.
fn read_jest(text: &str) -> Result<(Vec<TestResult>, Option<f64>), String> {
    let results: Value = serde_json::from_str(text).map_err(|error| format!("not jest's JSON ({error})"))?;
    let files = results["testResults"].as_array().ok_or("no testResults in jest's JSON")?;
    let mut tests = Vec::new();
    for file in files {
        let assertions = file["assertionResults"].as_array().map_or(&[][..], Vec::as_slice);
        let file_message = strip_ansi(file["message"].as_str().unwrap_or_default());
        if assertions.is_empty() && !file_message.trim().is_empty() {
            let path = file["name"].as_str().unwrap_or_default();
            let name = Path::new(path).file_name().map_or(path.to_string(), |name| name.to_string_lossy().into_owned());
            let (message, stack) = split_stack(&file_message);
            tests.push(TestResult {
                name: format!("{name}{WHOLE_FILE}"),
                outcome: Outcome::Failed,
                ms: 0.0,
                message,
                stack,
            });
            continue;
        }
        for assertion in assertions {
            let outcome = match assertion["status"].as_str() {
                Some("passed") => Outcome::Passed,
                Some("failed") => Outcome::Failed,
                _ => Outcome::Skipped,
            };
            let failures: Vec<&str> = assertion["failureMessages"]
                .as_array()
                .map_or_else(Vec::new, |messages| messages.iter().filter_map(Value::as_str).collect());
            let (message, stack) = split_stack(&strip_ansi(&failures.join("\n")));
            let name = assertion["fullName"].as_str().or_else(|| assertion["title"].as_str()).unwrap_or_default();
            let ms = assertion["duration"].as_f64().unwrap_or(0.0);
            tests.push(TestResult { name: name.to_string(), outcome, ms, message, stack });
        }
    }
    let start = results["startTime"].as_f64();
    let end = files.iter().filter_map(|file| file["endTime"].as_f64()).reduce(f64::max);
    let seconds = start.zip(end).map(|(start, end)| (end - start).max(0.0) / 1000.0);
    Ok((tests, seconds))
}

/// A failure's message and its stack (the `at …` lines).
fn split_stack(text: &str) -> (String, String) {
    let (stack, message): (Vec<&str>, Vec<&str>) = text.lines().partition(|line| line.trim_start().starts_with("at "));
    (message.join("\n").trim().to_string(), stack.join("\n").trim().to_string())
}

// ---------------------------------------------------------------- running them

/// The `--filter` that runs these .NET tests: exact names (running Adds doesn't also run Adds_two); a parameterised
/// case (`Method(1, 2)`) runs with all of its method's cases.
pub fn dotnet_filter(names: &[String]) -> String {
    let mut parts: Vec<String> = Vec::new();
    for name in names {
        let (method, exact) = match name.split_once('(') {
            Some((method, _)) => (method, false),
            None => (name.as_str(), true),
        };
        let mut escaped = String::new();
        for c in method.chars() {
            if "\\()&|=!~".contains(c) {
                escaped.push('\\');
            }
            escaped.push(c);
        }
        let part = format!("FullyQualifiedName{}{escaped}", if exact { "=" } else { "~" });
        if !parts.contains(&part) {
            parts.push(part);
        }
    }
    parts.join("|")
}

/// The `--testNamePattern` (jest) or `-t` (vitest) that runs exactly these tests by their full names.
pub fn name_pattern(names: &[String]) -> String {
    let names: Vec<String> = names.iter().map(|name| regex::escape(name)).collect();
    format!("^(?:{})$", names.join("|"))
}

/// The MSBuild project that builds these test projects in one run, so the projects they share build once.
pub fn build_project(projects: &[&Path]) -> String {
    let escape = |path: &Path| {
        path.display()
            .to_string()
            .replace('&', "&amp;")
            .replace('"', "&quot;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    };
    let items: String =
        projects.iter().map(|project| format!("    <TestProject Include=\"{}\" />\n", escape(project))).collect();
    format!(
        "<Project DefaultTargets=\"Build\">\n  <!-- Written by lazyrunner: its test projects, built in one MSBuild run. -->\n  <ItemGroup>\n{items}  </ItemGroup>\n  <Target Name=\"Restore\"><MSBuild Projects=\"@(TestProject)\" Targets=\"Restore\" BuildInParallel=\"true\" /></Target>\n  <Target Name=\"Build\"><MSBuild Projects=\"@(TestProject)\" Targets=\"Build\" BuildInParallel=\"true\" /></Target>\n</Project>\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{TempDir, write};

    const TRX: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<TestRun xmlns="http://microsoft.com/schemas/VisualStudio/TeamTest/2010">
  <Times creation="2026-10-22T10:00:00.0000000+02:00" start="2026-10-22T10:00:01.0000000+02:00" finish="2026-10-22T10:00:04.5000000+02:00" />
  <Results>
    <UnitTestResult testId="a" testName="Adds" outcome="Passed" duration="00:00:00.0120000" />
    <UnitTestResult testId="b" testName="Shop.Tests.CartTests.Totals(2, 3)" outcome="Failed" duration="00:00:01.2500000">
      <Output><ErrorInfo><Message>Expected 5 but was 6</Message><StackTrace>   at Shop.Tests.CartTests.Totals(Int32 a, Int32 b) in C:\src\CartTests.cs:line 42</StackTrace></ErrorInfo></Output>
    </UnitTestResult>
    <UnitTestResult testId="c" testName="Later" outcome="NotExecuted" />
  </Results>
  <TestDefinitions>
    <UnitTest id="a" name="Adds"><TestMethod className="Shop.Tests.CartTests" name="Adds" /></UnitTest>
    <UnitTest id="b" name="Totals"><TestMethod className="Shop.Tests.CartTests" name="Totals" /></UnitTest>
  </TestDefinitions>
</TestRun>"#;

    fn test(name: &str, outcome: Outcome) -> TestResult {
        TestResult { name: name.into(), outcome, ms: 0.0, message: String::new(), stack: String::new() }
    }

    #[test]
    fn a_trx_file_gives_each_test_by_its_full_name() {
        let (tests, seconds) = read_trx(TRX).unwrap();
        assert_eq!(seconds, Some(3.5));
        let names: Vec<&str> = tests.iter().map(|test| test.name.as_str()).collect();
        assert_eq!(names, ["Shop.Tests.CartTests.Adds", "Shop.Tests.CartTests.Totals(2, 3)", "Later"]);
        let outcomes: Vec<Outcome> = tests.iter().map(|test| test.outcome).collect();
        assert_eq!(outcomes, [Outcome::Passed, Outcome::Failed, Outcome::Skipped]);
        assert_eq!(tests[1].ms, 1250.0);
        assert_eq!(tests[1].message, "Expected 5 but was 6");
        assert!(tests[1].stack.ends_with("CartTests.cs:line 42"));
    }

    #[test]
    fn jest_json_gives_each_test_and_a_file_that_failed_as_a_whole() {
        let json = r#"{ "startTime": 1000, "testResults": [
            { "name": "/w/src/cart.spec.ts", "endTime": 3500, "message": "", "assertionResults": [
                { "fullName": "cart totals", "status": "passed", "duration": 4 },
                { "fullName": "cart rounds", "status": "failed", "duration": 9,
                  "failureMessages": ["\u001b[31mExpected: 2\u001b[39m\nReceived: 3\n    at Object.<anonymous> (/w/src/cart.spec.ts:12:5)"] },
                { "fullName": "cart later", "status": "pending" } ] },
            { "name": "/w/src/broken.spec.ts", "endTime": 2000, "message": "Cannot find module './x'", "assertionResults": [] } ] }"#;
        let (tests, seconds) = read_jest(json).unwrap();
        assert_eq!(seconds, Some(2.5));
        assert_eq!(tests.len(), 4);
        assert_eq!(tests[1].message, "Expected: 2\nReceived: 3");
        assert_eq!(tests[1].stack, "at Object.<anonymous> (/w/src/cart.spec.ts:12:5)");
        assert_eq!(tests[2].outcome, Outcome::Skipped);
        assert_eq!((tests[3].name.as_str(), tests[3].outcome), ("broken.spec.ts (whole file)", Outcome::Failed));
    }

    #[test]
    fn a_run_of_some_tests_replaces_only_theirs() {
        let mut suite = Suite::new("npm:x", "X", Runner::Npm { dir: "x".into(), vitest: false });
        let broken = "x.spec.ts (whole file)";
        suite.set_tests(vec![
            test("a", Outcome::Passed),
            test("b", Outcome::Failed),
            test("c", Outcome::Failed),
            test(broken, Outcome::Failed),
        ]);
        let requested = ["b", "c", broken].map(String::from);
        // jest lists the file's other tests as skipped; the file that failed as a whole loads now.
        let fresh = vec![test("a", Outcome::Skipped), test("b", Outcome::Passed), test("c", Outcome::Skipped)];
        assert_eq!(suite.merge(fresh, &requested), 2);
        assert_eq!((suite.passed, suite.failed, suite.skipped, suite.total()), (2, 0, 1, 3));
        assert_eq!(suite.tests[0], test("a", Outcome::Passed), "not asked for: kept");
        assert_eq!(suite.tests[1], test("b", Outcome::Passed), "in its place");
        assert!(suite.merge(vec![test("d", Outcome::Failed)], &[]) == 0 && suite.failed == 1, "a new failure joins");
    }

    #[test]
    fn output_shows_what_matters_and_counts_finished_tests() {
        let mut suite = Suite::new("dotnet:x", "X", Runner::Dotnet { project: "x.csproj".into() });
        for line in
            ["  Passed Adds [12 ms]", "  Failed Totals [1 s]", "", "x.cs(1,1): warning CS1: y", "Total tests: 2"]
        {
            suite.receive(line);
        }
        assert_eq!(suite.done, 2);
        assert_eq!(suite.output, ["  Failed Totals [1 s]", "Total tests: 2"]);
        assert_eq!(suite.lines.len(), 5, "the log keeps everything");
    }

    #[test]
    fn filters_run_exactly_the_named_tests() {
        let names = ["Shop.Cart.Adds", "Shop.Cart.Totals(2, 3)", "Shop.Cart.Totals(1, 1)"].map(String::from);
        assert_eq!(dotnet_filter(&names), "FullyQualifiedName=Shop.Cart.Adds|FullyQualifiedName~Shop.Cart.Totals");
        assert_eq!(name_pattern(&["cart totals (2)".to_string()]), r"^(?:cart totals \(2\))$");
    }

    #[test]
    fn suites_are_found_outside_build_output_and_packages() {
        let temp = TempDir::new("suites-discover");
        let root = temp.path();
        let sdk = r#"<Project Sdk="Microsoft.NET.Sdk"><ItemGroup><PackageReference Include="Microsoft.NET.Test.Sdk" Version="17.0.0" /></ItemGroup></Project>"#;
        let tests = r#"<Project Sdk="Microsoft.NET.Sdk"><ItemGroup><PackageReference Include="Microsoft.NET.Test.Sdk" Version="17.0.0" /><ProjectReference Include="..\..\services\Shop\Shop.csproj" /></ItemGroup></Project>"#;
        write(root.join("tests/Shop.Tests/Shop.Tests.csproj"), tests);
        write(root.join("services/Shop/Shop.csproj"), r#"<Project Sdk="Microsoft.NET.Sdk.Web" />"#);
        write(root.join("node_modules/x/X.Tests.csproj"), sdk);
        write(root.join("web/shop/package.json"), r#"{ "scripts": { "test": "vitest run" } }"#);
        write(
            root.join("web/docs/package.json"),
            r#"{ "scripts": { "test": "echo \"Error: no test specified\" && exit 1" } }"#,
        );
        write(root.join("web/nx/nx.json"), "{}");
        write(root.join("web/nx/apps/admin/project.json"), r#"{ "name": "admin", "targets": { "test": {} } }"#);
        write(root.join("web/nx/apps/admin/src/a.spec.ts"), "");
        write(root.join("web/nx/libs/ui/project.json"), r#"{ "name": "ui", "targets": { "test": {} } }"#);
        write(root.join("web/nx/libs/ui/tsconfig.spec.json"), "{}"); // a config, not a test
        let npm = |key: &str, dir: &str| App {
            key: key.into(),
            name: key.into(),
            icon: None,
            nerd_icon: None,
            tests: Vec::new(),
            migrations: None,
            kind: AppKind::Npm { dir: dir.into(), variants: Vec::new() },
        };
        let api = App {
            key: "api".into(),
            name: "Api".into(),
            icon: None,
            nerd_icon: None,
            tests: Vec::new(),
            migrations: None,
            kind: AppKind::Dotnet {
                project: "services/Shop/Shop.csproj".into(),
                launch_profile: "Api".into(),
                swagger: None,
            },
        };
        let mut docs = npm("docs", "web/docs");
        docs.tests = vec!["Admin".into()]; // the profile says so, whatever the folder
        let apps = [api, npm("shop", "web/shop"), docs, npm("admin", "web/nx")];
        let suites = discover(root, &apps);
        let untested: Vec<&str> =
            suites.iter().filter(|suite| suite.untested).map(|suite| suite.name.as_str()).collect();
        assert_eq!(untested, ["ui"], "a test target, no test files");
        let found: Vec<(String, &str, Option<String>)> = suites
            .iter()
            .filter(|suite| !suite.untested)
            .map(|suite| (suite.name.clone(), suite.runner.group(), suite.app.clone()))
            .collect();
        let expected = [
            ("Shop.Tests", ".NET · dotnet test", "api"),
            ("shop", "npm · vitest", "shop"),
            ("admin", "Nx · nx test", "docs"),
        ];
        let expected = expected.map(|(name, group, app)| (name.to_string(), group, Some(app.to_string())));
        assert_eq!(found, expected, "no tests in ui, none in docs; Shop.Tests references the api's project");
    }
}
