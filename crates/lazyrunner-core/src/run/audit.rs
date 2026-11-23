//! Outdated and vulnerable packages, per app (#79): `dotnet list package --vulnerable --include-transitive` and
//! `--outdated` for a .NET app, `npm audit` and `npm outdated` for an npm one - read from their JSON and merged into
//! one list per app: the vulnerable first (the worst first), then the outdated (a new major first). Kept in the state
//! folder (`audit.json`), so the audit view opens on the last scan. They ask the package feeds, so they run only when
//! asked, never at startup.
//!
//! Beside the apps, **the rest of the solution**: the projects of the repo's solution files that no app builds (a CI
//! pipeline restores them all, and fails on their vulnerable packages too) - scanned as one, its vulnerable packages
//! only, each with the projects it's in.

use std::cmp::Reverse;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::LazyLock;

use anyhow::{Result, bail};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::json::text as string;
use crate::profile::{App, AppKind, Health};
use crate::run::tools;

const FILE: &str = "audit.json";
/// The audit's entry for the solution's projects no app builds: its key, its name and its icons.
pub const REST: &str = "solution";
const REST_NAME: &str = "Rest of the solution";
const REST_ICON: &str = "▣";
const REST_NERD_ICON: &str = "\u{f1b2}";
/// How many folders down a solution file is looked for, and the folders it isn't looked for in.
const SOLUTION_DEPTH: usize = 3;
const NOT_SEARCHED: [&str; 7] = [".git", "node_modules", "bin", "obj", ".vs", "packages", "TestResults"];
const SOLUTION_KINDS: [&str; 2] = ["sln", "slnx"];

/// How bad an advisory is, the least first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Severity {
    Info,
    Low,
    Moderate,
    High,
    Critical,
}

impl Severity {
    /// The worst first.
    pub const ALL: [Self; 5] = [Self::Critical, Self::High, Self::Moderate, Self::Low, Self::Info];

    /// As npm and NuGet say it ("high", "High", "medium").
    fn parse(text: &str) -> Self {
        match text.to_ascii_lowercase().as_str() {
            "critical" => Self::Critical,
            "high" => Self::High,
            "moderate" | "medium" => Self::Moderate,
            "low" => Self::Low,
            _ => Self::Info,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Low => "low",
            Self::Moderate => "moderate",
            Self::High => "high",
            Self::Critical => "critical",
        }
    }
}

/// A known vulnerability in a package.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Advisory {
    /// Its id (GHSA-…), from its link.
    pub id: String,
    pub url: String,
    /// What it is (npm says, NuGet doesn't).
    pub title: String,
    pub severity: Severity,
    /// The versions it affects (npm says).
    pub range: String,
}

/// What fixes a package: `package` at `version` (a transitive one's fix can be the direct package that brings it in);
/// no version: `npm audit fix` works it out.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fix {
    pub package: String,
    pub version: String,
    /// A new major version: it may need changes in the app.
    pub major: bool,
}

/// A package that needs a look: vulnerable, outdated, or both.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Package {
    pub name: String,
    pub current: String,
    /// The version asked for (NuGet's requested, npm's wanted).
    pub wanted: Option<String>,
    pub latest: Option<String>,
    /// Referenced by the app itself, not only through another package.
    pub direct: bool,
    /// Its worst advisory's (npm says it for a package vulnerable through another one too).
    pub severity: Option<Severity>,
    pub advisories: Vec<Advisory>,
    pub fix: Option<Fix>,
    /// What it's vulnerable through (npm): other vulnerable packages it depends on.
    pub via: Vec<String>,
    /// The projects it's in, from the repo (the rest of the solution's scan says them).
    #[serde(default)]
    pub projects: Vec<String>,
}

impl Package {
    pub fn outdated(&self) -> bool {
        self.latest.as_ref().is_some_and(|latest| *latest != self.current)
    }

    /// Whether its latest version is a new major one.
    pub fn major_behind(&self) -> bool {
        self.latest.as_ref().is_some_and(|latest| major(latest) != major(&self.current))
    }
}

/// A package the scan before found vulnerable and this one doesn't: its version then, and now (none: it's gone).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fixed {
    pub name: String,
    pub from: String,
    pub to: Option<String>,
}

/// One app's last scan.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppAudit {
    /// When (Unix seconds).
    pub at: i64,
    pub packages: Vec<Package>,
    /// Why it couldn't be scanned (the packages are then the scan before).
    pub error: Option<String>,
    /// What the scan before found vulnerable that this one doesn't.
    #[serde(default)]
    pub fixed: Vec<Fixed>,
    /// The packages with something the scan before didn't have (they're new, or have a new advisory), and the new
    /// advisories (their ids).
    #[serde(default)]
    pub new: Vec<String>,
    #[serde(default)]
    pub new_advisories: Vec<String>,
}

impl AppAudit {
    /// Its worst advisory.
    pub fn worst(&self) -> Option<Severity> {
        self.packages.iter().filter_map(|package| package.severity).max()
    }

    /// How many of its packages are this bad.
    pub fn count(&self, severity: Severity) -> usize {
        self.packages.iter().filter(|package| package.severity == Some(severity)).count()
    }
}

/// The apps' last scans, by key.
pub type Audits = BTreeMap<String, AppAudit>;

/// What changed in `scan` since the scan before (`before`'s packages): the packages fixed, those with something new,
/// and the new advisories.
pub fn compare(before: &[Package], scan: &mut AppAudit) {
    let now = |name: &str| scan.packages.iter().find(|package| package.name == name);
    let fixed: Vec<Fixed> = before
        .iter()
        .filter(|old| old.severity.is_some() && now(&old.name).is_none_or(|package| package.severity.is_none()))
        .map(|old| {
            let to = now(&old.name).map(|package| package.current.clone()).filter(|to| *to != old.current);
            Fixed { name: old.name.clone(), from: old.current.clone(), to }
        })
        .collect();
    let known: HashSet<&str> =
        before.iter().flat_map(|package| package.advisories.iter().map(|advisory| advisory.id.as_str())).collect();
    let new_advisories: Vec<String> = scan
        .packages
        .iter()
        .flat_map(|package| &package.advisories)
        .filter(|advisory| !known.contains(advisory.id.as_str()))
        .map(|advisory| advisory.id.clone())
        .collect();
    let new: Vec<String> = scan
        .packages
        .iter()
        .filter(|package| {
            before.iter().all(|old| old.name != package.name)
                || package.advisories.iter().any(|advisory| new_advisories.contains(&advisory.id))
        })
        .map(|package| package.name.clone())
        .collect();
    (scan.fixed, scan.new, scan.new_advisories) = (fixed, new, new_advisories);
}

/// How long ago Unix time `at` was: `just now`, `5 min ago`, `2 h ago`, `3 d ago`.
pub fn age(at: i64) -> String {
    let seconds = (jiff::Timestamp::now().as_second() - at).max(0);
    match seconds {
        0..=59 => "just now".into(),
        60..=3_599 => format!("{} min ago", seconds / 60),
        3_600..=86_399 => format!("{} h ago", seconds / 3_600),
        _ => format!("{} d ago", seconds / 86_400),
    }
}

/// The scans kept in `state_dir`.
pub fn load(state_dir: &Path) -> Audits {
    let saved = crate::json::read(&state_dir.join(FILE));
    saved.and_then(|value| serde_json::from_value(value).ok()).unwrap_or_default()
}

pub fn save(state_dir: &Path, audits: &Audits) -> Result<()> {
    crate::config::write(&state_dir.join(FILE), &serde_json::to_value(audits)?)
}

/// What a scan found, the worst first: `1 critical · 2 high · 5 outdated`, or `up to date`.
pub fn summary(packages: &[Package]) -> String {
    let mut parts = Vec::new();
    for severity in Severity::ALL {
        let count = packages.iter().filter(|package| package.severity == Some(severity)).count();
        if count > 0 {
            parts.push(format!("{count} {}", severity.name()));
        }
    }
    let outdated = packages.iter().filter(|package| package.severity.is_none() && package.outdated()).count();
    if outdated > 0 {
        parts.push(format!("{outdated} outdated"));
    }
    if parts.is_empty() { "up to date".into() } else { parts.join(" · ") }
}

/// Scans `app`: runs what lists its packages, and merges what they say.
pub fn scan(root: &Path, app: &App) -> Result<Vec<Package>> {
    match &app.kind {
        AppKind::Dotnet { project, .. } => {
            let path = root.join(project);
            let run = |extra: &[&str]| {
                let mut command = Command::new(tools::dotnet());
                command.arg("list").arg(&path).arg("package").args(extra).args(["--format", "json"]);
                output(gently(command))
            };
            // One after the other: each dotnet starts an MSBuild worker per core to read the projects.
            let vulnerable = run(&["--vulnerable", "--include-transitive"])?;
            let outdated = run(&["--outdated", "--include-transitive"])?;
            dotnet(&vulnerable, &outdated)
        }
        AppKind::Npm { dir, .. } => {
            let folder = root.join(dir);
            if !folder.join("package-lock.json").is_file() {
                bail!("npm audit needs a package-lock.json (npm install makes it)");
            }
            let (program, launcher) = tools::npm_launcher()?;
            let run = |args: &[&str]| {
                let mut command = Command::new(&program);
                command.args(&launcher).args(args).current_dir(&folder);
                output(gently(command))
            };
            let (audit, outdated) = both(|| run(&["audit", "--json"]), || run(&["outdated", "--json"]));
            npm(&audit?, &outdated?, |name| installed(&folder, name))
        }
        AppKind::Command(_) => Ok(Vec::new()),
    }
}

/// The repo's solution files (`.sln`, `.slnx`), the shallowest first, at most [`SOLUTION_DEPTH`] folders down - not in
/// build output, packages or the tools' folders.
pub fn solutions(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut folders = VecDeque::from([(root.to_path_buf(), 0)]);
    while let Some((folder, depth)) = folders.pop_front() {
        let Ok(entries) = std::fs::read_dir(&folder) else { continue };
        let mut entries: Vec<std::fs::DirEntry> = entries.flatten().collect();
        entries.sort_by_key(std::fs::DirEntry::file_name);
        for entry in entries {
            let (path, name) = (entry.path(), entry.file_name());
            let Ok(kind) = entry.file_type() else { continue };
            let skipped = NOT_SEARCHED.iter().any(|skip| name.eq_ignore_ascii_case(skip));
            if kind.is_dir() && depth < SOLUTION_DEPTH && !skipped {
                folders.push_back((path, depth + 1));
            } else if kind.is_file()
                && path.extension().is_some_and(|ext| SOLUTION_KINDS.iter().any(|kind| ext.eq_ignore_ascii_case(kind)))
            {
                found.push(path);
            }
        }
    }
    found
}

/// The audit's entry for the solution's projects no app builds, when the repo has a solution and a .NET app: a .NET
/// "app" whose project is the first solution.
pub fn rest(root: &Path, apps: &[App]) -> Option<App> {
    if !apps.iter().any(App::is_dotnet) {
        return None;
    }
    let solution = solutions(root).into_iter().next()?;
    let project = solution.strip_prefix(root).unwrap_or(&solution).to_string_lossy().replace('\\', "/");
    Some(rest_entry(project))
}

fn rest_entry(project: String) -> App {
    App {
        key: REST.into(),
        name: REST_NAME.into(),
        icon: Some(REST_ICON.into()),
        nerd_icon: Some(REST_NERD_ICON.into()),
        kind: AppKind::Dotnet { project, launch_profile: String::new(), swagger: None, args: Vec::new() },
        tests: Vec::new(),
        migrations: None,
        env: Vec::new(),
        depends_on: Vec::new(),
        health: Health::default(),
        restart_on_crash: false,
    }
}

/// The projects the .NET `apps` build - their own and every one they reference - as the scans compare them.
pub fn reached(root: &Path, apps: &[App]) -> HashSet<String> {
    let own = apps.iter().filter_map(|app| match &app.kind {
        AppKind::Dotnet { project, .. } => Some(root.join(project)),
        AppKind::Npm { .. } | AppKind::Command(_) => None,
    });
    own.flat_map(|project| crate::run::build::projects(&project)).map(|path| project_key(&path)).collect()
}

/// The projects a solution file lists (`.sln` lines, `.slnx` elements), as the scans compare them; none when it can't
/// be read.
fn solution_projects(solution: &Path) -> HashSet<String> {
    static SLN: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?im)^Project\("[^"]*"\)\s*=\s*"[^"]*",\s*"([^"]+\.\w+proj)""#).expect("a valid pattern")
    });
    static SLNX: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"(?i)<Project\s+Path="([^"]+\.\w+proj)""#).expect("a valid pattern"));
    let Ok(text) = crate::json::read_text(solution) else { return HashSet::new() };
    let dir = solution.parent().unwrap_or(Path::new(""));
    SLN.captures_iter(&text)
        .chain(SLNX.captures_iter(&text))
        .map(|found| project_key(&crate::run::build::normalize(&dir.join(crate::profile::native(&found[1])))))
        .collect()
}

/// A project's path as the scans compare them: forward slashes, lower case.
fn project_key(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/").to_lowercase()
}

/// Scans the projects of the repo's solutions that `reached` leaves out (no app builds them): their vulnerable
/// packages, each with the projects it's in - a solution whose projects the apps or the solutions before it cover is
/// skipped. `dotnet list` leaves out a project that was never restored.
pub fn scan_rest(root: &Path, reached: &HashSet<String>) -> Result<Vec<Package>> {
    let all = solutions(root);
    if all.is_empty() {
        bail!("there's no solution file (.sln, .slnx) in the repo");
    }
    let (mut found, mut covered) = (Vec::new(), reached.clone());
    for solution in &all {
        let listed = solution_projects(solution);
        if !listed.is_empty() && listed.is_subset(&covered) {
            continue;
        }
        covered.extend(listed);
        let run = |extra: &[&str]| {
            let mut command = Command::new(tools::dotnet());
            command.arg("list").arg(solution).arg("package").args(extra).args(["--format", "json"]);
            output(gently(command))
        };
        let vulnerable = rest_of(&run(&["--vulnerable", "--include-transitive"])?, reached)?;
        if dotnet_packages(&vulnerable).is_empty() {
            continue;
        }
        let outdated = rest_of(&run(&["--outdated", "--include-transitive"])?, reached)?;
        let held = holders(&vulnerable, root);
        for mut package in dotnet(&vulnerable.to_string(), &outdated.to_string())? {
            if package.severity.is_none() {
                continue;
            }
            package.projects = held.get(&package.name.to_lowercase()).cloned().unwrap_or_default();
            add(&mut found, package);
        }
    }
    Ok(sorted(found))
}

/// A solution's `dotnet list package` report without the projects in `reached` (those an app builds) and without its
/// problems (a project never restored); it fails only when it lists no project at all.
fn rest_of(text: &str, reached: &HashSet<String>) -> Result<Value> {
    let mut report: Value = serde_json::from_str(text)?;
    let listed = report["projects"].as_array().is_some_and(|projects| !projects.is_empty());
    let problems = report["problems"].as_array().into_iter().flatten();
    if !listed
        && let Some(problem) =
            problems.into_iter().find(|problem| string(&problem["level"]).eq_ignore_ascii_case("error"))
    {
        bail!("{}", string(&problem["text"]));
    }
    if let Some(projects) = report["projects"].as_array_mut() {
        projects.retain(|project| !reached.contains(&project_key(Path::new(&string(&project["path"])))));
    }
    report["problems"] = Value::Array(Vec::new());
    Ok(report)
}

/// Which projects of `report` hold each package (by its name in lower case), from the repo.
fn holders(report: &Value, root: &Path) -> HashMap<String, Vec<String>> {
    let prefix = format!("{}/", project_key(root).trim_end_matches('/'));
    let mut held: HashMap<String, Vec<String>> = HashMap::new();
    for project in report["projects"].as_array().into_iter().flatten() {
        let path = string(&project["path"]).replace('\\', "/");
        let inside = project_key(Path::new(&path)).starts_with(&prefix);
        let shown = if inside { path.get(prefix.len()..).unwrap_or(&path).to_string() } else { path.clone() };
        for framework in project["frameworks"].as_array().into_iter().flatten() {
            let lists = [&framework["topLevelPackages"], &framework["transitivePackages"]];
            for item in lists.into_iter().filter_map(|list| list.as_array()).flatten() {
                let names = held.entry(string(&item["id"]).to_lowercase()).or_default();
                if !names.contains(&shown) {
                    names.push(shown.clone());
                }
            }
        }
    }
    held
}

/// `command`, to run in the background without weighing on the machine: below normal priority on Windows (what it
/// starts inherits it - MSBuild's workers, one per core), so the apps and lazyrunner stay responsive; and without
/// MSBuild keeping its workers alive after it, which would hold gigabytes for minutes.
fn gently(mut command: Command) -> Command {
    command.env("MSBUILDDISABLENODEREUSE", "1");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x0000_4000;
        command.creation_flags(BELOW_NORMAL_PRIORITY_CLASS | tools::CREATE_NO_WINDOW);
    }
    command
}

/// Runs `first` and `second` side by side (they wait on the package feeds), and waits for both.
fn both(
    first: impl FnOnce() -> Result<String> + Send,
    second: impl FnOnce() -> Result<String>,
) -> (Result<String>, Result<String>) {
    std::thread::scope(|scope| {
        let first = scope.spawn(first);
        let second = second();
        (first.join().unwrap_or_else(|_| Err(anyhow::anyhow!("it stopped unexpectedly"))), second)
    })
}

/// What a tool printed (its JSON) whatever its exit code - npm's audit and outdated exit with 1 when they find
/// something - and nothing as `{}`; else what it said went wrong.
fn output(mut command: Command) -> Result<String> {
    let done = command.stdin(Stdio::null()).output()?;
    let printed = String::from_utf8_lossy(&done.stdout).into_owned();
    if printed.trim_start().starts_with('{') {
        return Ok(printed);
    }
    if printed.trim().is_empty() && done.status.success() {
        return Ok("{}".into());
    }
    let said = String::from_utf8_lossy(&done.stderr).into_owned();
    let line = said.lines().chain(printed.lines()).map(str::trim).find(|line| !line.is_empty());
    bail!("{}", line.unwrap_or("it printed nothing"))
}

/// A .NET app's packages from `dotnet list package --vulnerable --include-transitive` and `--outdated
/// --include-transitive` (`--format json`): the vulnerable ones, and the direct ones outdated (a transitive one only
/// when it's vulnerable too: its fix is a direct reference to its latest version).
pub fn dotnet(vulnerable: &str, outdated: &str) -> Result<Vec<Package>> {
    let (vulnerable, outdated) = (dotnet_report(vulnerable)?, dotnet_report(outdated)?);
    let mut found: Vec<Package> = Vec::new();
    for (direct, item) in dotnet_packages(&vulnerable) {
        let advisories: Vec<Advisory> = item["vulnerabilities"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|said| {
                let url = string(&said["advisoryurl"]);
                let severity = Severity::parse(&string(&said["severity"]));
                Advisory { id: advisory_id(&url), url, title: String::new(), severity, range: String::new() }
            })
            .collect();
        let severity = advisories.iter().map(|advisory| advisory.severity).max();
        let (current, wanted) = (string(&item["resolvedVersion"]), optional(&item["requestedVersion"]));
        let package = Package { name: string(&item["id"]), current, wanted, direct, severity, advisories, ..blank() };
        add(&mut found, package);
    }
    for (direct, item) in dotnet_packages(&outdated) {
        let (name, latest) = (string(&item["id"]), optional(&item["latestVersion"]));
        match found.iter_mut().find(|package| package.name.eq_ignore_ascii_case(&name)) {
            Some(package) => package.latest = latest,
            None if direct => {
                let (current, wanted) = (string(&item["resolvedVersion"]), optional(&item["requestedVersion"]));
                found.push(Package { name, current, wanted, latest, direct, ..blank() });
            }
            None => {}
        }
    }
    for package in found.iter_mut().filter(|package| package.severity.is_some()) {
        let major = package.major_behind();
        package.fix = package.latest.clone().map(|version| Fix { package: package.name.clone(), version, major });
    }
    Ok(sorted(found))
}

/// A `dotnet list package` report, or what it said went wrong.
fn dotnet_report(text: &str) -> Result<Value> {
    let report: Value = serde_json::from_str(text)?;
    let problems = report["problems"].as_array().into_iter().flatten();
    if let Some(problem) = problems.into_iter().find(|problem| string(&problem["level"]).eq_ignore_ascii_case("error"))
    {
        bail!("{}", string(&problem["text"]));
    }
    Ok(report)
}

/// Every package in a report, of every project and framework: whether it's a direct one, and what's said of it.
fn dotnet_packages(report: &Value) -> Vec<(bool, &Value)> {
    let projects = report["projects"].as_array().into_iter().flatten();
    let frameworks = projects.flat_map(|project| project["frameworks"].as_array().into_iter().flatten());
    frameworks
        .flat_map(|framework| {
            let direct = framework["topLevelPackages"].as_array().into_iter().flatten().map(|item| (true, item));
            let transitive = framework["transitivePackages"].as_array().into_iter().flatten().map(|item| (false, item));
            direct.chain(transitive)
        })
        .collect()
}

/// Adds `package`, or what's new about it when another project or framework listed it already.
fn add(found: &mut Vec<Package>, package: Package) {
    let Some(known) = found.iter_mut().find(|other| other.name.eq_ignore_ascii_case(&package.name)) else {
        found.push(package);
        return;
    };
    known.direct |= package.direct;
    known.severity = known.severity.max(package.severity);
    known.latest = known.latest.take().or(package.latest);
    known.fix = known.fix.take().or(package.fix);
    for project in package.projects {
        if !known.projects.contains(&project) {
            known.projects.push(project);
        }
    }
    for advisory in package.advisories {
        if !known.advisories.iter().any(|other| other.id == advisory.id) {
            known.advisories.push(advisory);
        }
    }
}

/// An npm app's packages from `npm audit --json` and `npm outdated --json`; `installed` says a package's version in
/// node_modules (npm audit doesn't).
pub fn npm(audit: &str, outdated: &str, installed: impl Fn(&str) -> Option<String>) -> Result<Vec<Package>> {
    let audit: Value = serde_json::from_str(audit)?;
    if let Some(error) = audit.get("error") {
        let summary = string(&error["summary"]);
        bail!("{}", summary.lines().next().filter(|line| !line.is_empty()).unwrap_or("npm audit failed"));
    }
    let outdated: Value = serde_json::from_str(outdated)?;
    let mut found = Vec::new();
    for (name, item) in audit["vulnerabilities"].as_object().into_iter().flatten() {
        let (mut advisories, mut via) = (Vec::new(), Vec::new());
        for cause in item["via"].as_array().into_iter().flatten() {
            match cause.as_str() {
                Some(other) => via.push(other.to_string()),
                None => {
                    let url = string(&cause["url"]);
                    let (title, range) = (string(&cause["title"]), string(&cause["range"]));
                    let severity = Severity::parse(&string(&cause["severity"]));
                    advisories.push(Advisory { id: advisory_id(&url), url, title, severity, range });
                }
            }
        }
        let fix = match &item["fixAvailable"] {
            Value::Object(_) => Some(Fix {
                package: string(&item["fixAvailable"]["name"]),
                version: string(&item["fixAvailable"]["version"]),
                major: item["fixAvailable"]["isSemVerMajor"].as_bool().unwrap_or(false),
            }),
            Value::Bool(true) => Some(Fix { package: name.clone(), version: String::new(), major: false }),
            _ => None,
        };
        let listed = &outdated[name.as_str()];
        found.push(Package {
            name: name.clone(),
            current: installed(name).or_else(|| optional(&listed["current"])).unwrap_or_default(),
            wanted: optional(&listed["wanted"]),
            latest: optional(&listed["latest"]),
            direct: item["isDirect"].as_bool().unwrap_or(false),
            severity: Some(Severity::parse(&string(&item["severity"]))),
            advisories,
            fix,
            via,
            projects: Vec::new(),
        });
    }
    for (name, item) in outdated.as_object().into_iter().flatten() {
        if found.iter().any(|package| package.name == *name) {
            continue;
        }
        let current = optional(&item["current"]).unwrap_or_else(|| "not installed".into());
        let (wanted, latest) = (optional(&item["wanted"]), optional(&item["latest"]));
        found.push(Package { name: name.clone(), current, wanted, latest, direct: true, ..blank() });
    }
    Ok(sorted(found))
}

/// The version of `name` in `folder`'s node_modules.
fn installed(folder: &Path, name: &str) -> Option<String> {
    let manifest = crate::json::read(&folder.join("node_modules").join(name).join("package.json"))?;
    optional(&manifest["version"])
}

/// What fixes `package` in `app`, run in [`folder`]: its fix, else an upgrade to its latest version (for the rest of
/// the solution, in each project it's in).
pub fn command(app: &App, package: &Package) -> Option<String> {
    let each: Vec<String> = fix_args(app, package).iter().map(|(dotnet, args)| shown(*dotnet, args)).collect();
    (!each.is_empty()).then(|| each.join(" && "))
}

/// A command as it's run: the program, then its arguments (quoted when they hold a space).
fn shown(dotnet: bool, args: &[String]) -> String {
    let args: Vec<String> =
        args.iter().map(|arg| if arg.contains(' ') { format!("\"{arg}\"") } else { arg.clone() }).collect();
    format!("{} {}", if dotnet { "dotnet" } else { "npm" }, args.join(" "))
}

/// The package and version that fix `package`: its fix's, else its latest when it's outdated.
fn target(package: &Package) -> Option<(&str, &str)> {
    match (&package.fix, &package.latest) {
        (Some(fix), _) => Some((fix.package.as_str(), fix.version.as_str())),
        (None, Some(latest)) if package.outdated() => Some((package.name.as_str(), latest.as_str())),
        _ => None,
    }
}

/// The fixes of `packages` as they run, one after another: an npm app's in one `npm install` (and one `npm audit
/// fix` for those with no fixed version known), a .NET app's a `dotnet add package` each - each package once, the
/// first (the worst) one's version.
pub fn fixes(app: &App, packages: &[Package]) -> Vec<(bool, Vec<String>)> {
    let mut seen: Vec<&str> = Vec::new();
    let mut each = Vec::new();
    for package in packages {
        let Some((name, _)) = target(package) else { continue };
        if seen.contains(&name) {
            continue;
        }
        seen.push(name);
        each.extend(fix_args(app, package));
    }
    if !matches!(app.kind, AppKind::Npm { .. }) {
        return each;
    }
    let (fix, install): (Vec<_>, Vec<_>) = each.into_iter().partition(|(_, args)| args[0] == "audit");
    let mut out = Vec::new();
    if !install.is_empty() {
        let mut args = vec!["install".to_string()];
        args.extend(install.into_iter().filter_map(|(_, args)| args.into_iter().nth(1)));
        out.push((false, args));
    }
    if !fix.is_empty() {
        out.push((false, vec!["audit".into(), "fix".into()]));
    }
    out
}

/// The commands [`fixes`] runs, as they read.
pub fn commands(app: &App, packages: &[Package]) -> Vec<String> {
    fixes(app, packages).iter().map(|(dotnet, args)| shown(*dotnet, args)).collect()
}

/// What runs `package`'s fix: dotnet (true) or npm, with these arguments - for the rest of the solution, a `dotnet
/// add` in each project it's in.
fn fix_args(app: &App, package: &Package) -> Vec<(bool, Vec<String>)> {
    let Some((name, version)) = target(package) else { return Vec::new() };
    let add = |file: &str| (true, ["add", file, "package", name, "--version", version].map(String::from).to_vec());
    match &app.kind {
        AppKind::Dotnet { .. } if app.key == REST => package.projects.iter().map(|project| add(project)).collect(),
        AppKind::Dotnet { project, .. } => {
            let file =
                Path::new(project).file_name().map_or_else(String::new, |file| file.to_string_lossy().into_owned());
            vec![add(&file)]
        }
        AppKind::Npm { .. } if version.is_empty() => vec![(false, vec!["audit".into(), "fix".into()])],
        AppKind::Npm { .. } => vec![(false, vec!["install".into(), format!("{name}@{version}")])],
        AppKind::Command(_) => Vec::new(),
    }
}

/// Runs the fixes of `packages` in their app's folder, one after another ([`fixes`]), and waits; what went wrong when
/// one didn't work (and how many didn't).
pub fn install(root: &Path, app: &App, packages: &[Package]) -> Result<()> {
    let each = fixes(app, packages);
    if each.is_empty() {
        bail!("there's nothing to install");
    }
    let failed: Vec<String> = each
        .iter()
        .filter_map(|(dotnet, args)| run(root, app, *dotnet, args).err().map(|error| error.to_string()))
        .collect();
    match failed.first() {
        None => Ok(()),
        Some(error) if each.len() == 1 => bail!("{error}"),
        Some(error) => bail!("{} of {} failed - {error}", failed.len(), each.len()),
    }
}

/// Runs one fix in its app's folder, and waits; what it said went wrong when it didn't work.
fn run(root: &Path, app: &App, dotnet: bool, args: &[String]) -> Result<()> {
    let mut command = if dotnet {
        Command::new(tools::dotnet())
    } else {
        let (program, launcher) = tools::npm_launcher()?;
        let mut command = Command::new(program);
        command.args(launcher);
        command
    };
    command.args(args).current_dir(folder(root, app)).stdin(Stdio::null());
    let done = gently(command).output()?;
    if done.status.success() {
        return Ok(());
    }
    let said = format!("{}\n{}", String::from_utf8_lossy(&done.stderr), String::from_utf8_lossy(&done.stdout));
    let lines: Vec<&str> = said.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
    let line = lines.iter().find(|line| line.to_lowercase().contains("error")).or(lines.last());
    bail!("{}", line.copied().unwrap_or("it failed"))
}

/// Where an app's [`command`] runs: a .NET app's project folder, an npm app's, a command app's; the repo for the rest
/// of the solution.
pub fn folder(root: &Path, app: &App) -> PathBuf {
    match &app.kind {
        AppKind::Dotnet { .. } if app.key == REST => root.to_path_buf(),
        AppKind::Dotnet { project, .. } => {
            root.join(project).parent().map_or_else(|| root.to_path_buf(), Path::to_path_buf)
        }
        AppKind::Npm { dir, .. } => root.join(dir),
        AppKind::Command(command) => root.join(&command.cwd),
    }
}

/// The vulnerable first (the worst first), then the outdated (a new major first), by name within.
fn sorted(mut found: Vec<Package>) -> Vec<Package> {
    found.sort_by_cached_key(|package| {
        (
            Reverse(package.severity),
            Reverse(package.outdated()),
            Reverse(package.major_behind()),
            package.name.to_lowercase(),
        )
    });
    found
}

fn blank() -> Package {
    Package {
        name: String::new(),
        current: String::new(),
        wanted: None,
        latest: None,
        direct: false,
        severity: None,
        advisories: Vec::new(),
        fix: None,
        via: Vec::new(),
        projects: Vec::new(),
    }
}

fn major(version: &str) -> &str {
    version.split(['.', '-', '+']).next().unwrap_or(version)
}

fn advisory_id(url: &str) -> String {
    url.trim_end_matches('/').rsplit('/').next().unwrap_or_default().to_string()
}

fn optional(value: &Value) -> Option<String> {
    value.as_str().filter(|text| !text.is_empty()).map(str::to_string)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::testing::TempDir;

    #[test]
    fn the_rest_of_the_solution_is_its_projects_no_app_builds_each_package_with_its_projects() {
        let root = Path::new("C:/work/shop");
        let project = |path: &str| {
            let vulnerable = json!({ "severity": "High", "advisoryurl": "https://github.com/advisories/GHSA-1" });
            let package =
                json!({ "id": "Example.Mapper", "resolvedVersion": "16.1.1", "vulnerabilities": [vulnerable] });
            json!({ "path": path, "frameworks": [{ "framework": "net8.0", "topLevelPackages": [package] }] })
        };
        let not_restored = json!({ "level": "error", "text": "No assets file was found for Old.csproj" });
        let report = json!({ "version": 1, "problems": [not_restored], "projects": [
            project("C:\\work\\shop\\api\\Orders\\Orders.csproj"),
            project("C:\\work\\shop\\api\\Tracking\\Tracking.csproj"),
            project("C:\\work\\shop\\api\\Tracking.Domain\\Tracking.Domain.csproj"),
        ] });
        let reached = HashSet::from([project_key(Path::new("C:/work/shop/api/Orders/Orders.csproj"))]);
        let rest = rest_of(&report.to_string(), &reached).unwrap();
        assert_eq!(rest["projects"].as_array().map(Vec::len), Some(2), "the app's own project left out");
        let held = holders(&rest, root);
        let projects = ["api/Tracking/Tracking.csproj", "api/Tracking.Domain/Tracking.Domain.csproj"];
        assert_eq!(held["example.mapper"], projects, "from the repo, each once");
        let empty = json!({ "version": 1, "problems": [{ "level": "error", "text": "MSB1009" }], "projects": [] });
        assert!(rest_of(&empty.to_string(), &reached).is_err(), "nothing listed: what went wrong");

        let fix = Fix { package: "Example.Mapper".into(), version: "16.3.0".into(), major: false };
        let package = Package {
            name: "Example.Mapper".into(),
            severity: Some(Severity::High),
            fix: Some(fix),
            projects: projects.map(String::from).to_vec(),
            ..blank()
        };
        let entry = rest_entry("api/Shop.sln".into());
        let each = "dotnet add api/Tracking/Tracking.csproj package Example.Mapper --version 16.3.0 && dotnet add \
                    api/Tracking.Domain/Tracking.Domain.csproj package Example.Mapper --version 16.3.0";
        assert_eq!(command(&entry, &package).as_deref(), Some(each), "in each project it's in");
        assert_eq!(folder(root, &entry), root, "from the repo");
    }

    #[test]
    fn a_solutions_projects_are_read_from_its_file() {
        let temp = TempDir::new("audit-solution-projects");
        let sln = concat!(
            r#"Project("{FAE04EC0-301F-11D3-BF4B-00C04F79EFBC}") = "Tracking", "Tracking\Tracking.csproj", "{1}""#,
            "\n",
            r#"Project("{2150E333-8FDC-42A3-9474-1A3956D46DE8}") = "src", "src", "{2}""#,
            "\n",
        );
        crate::testing::write(temp.join("api/Shop.sln"), sln);
        crate::testing::write(
            temp.join("api/Shop.slnx"),
            "<Solution><Project Path=\"../tools/Tool/Tool.csproj\" /></Solution>",
        );
        let key = |path: &str| project_key(&temp.join(path));
        assert_eq!(solution_projects(&temp.join("api/Shop.sln")), HashSet::from([key("api/Tracking/Tracking.csproj")]));
        assert_eq!(solution_projects(&temp.join("api/Shop.slnx")), HashSet::from([key("tools/Tool/Tool.csproj")]));
        assert!(solution_projects(&temp.join("api/Gone.sln")).is_empty());
    }

    #[test]
    fn solutions_are_found_a_few_folders_down_but_not_in_build_output_or_packages() {
        let temp = TempDir::new("audit-solutions");
        for file in [
            "api/Shop.sln",
            "api/tools/Tools.slnx",
            "api/bin/Debug/Copy.sln",
            "node_modules/x/X.sln",
            "a/b/c/d/Deep.sln",
        ] {
            crate::testing::write(temp.join(file), "");
        }
        let found: Vec<String> = solutions(temp.path())
            .iter()
            .map(|path| path.strip_prefix(temp.path()).unwrap().to_string_lossy().replace('\\', "/"))
            .collect();
        assert_eq!(found, ["api/Shop.sln", "api/tools/Tools.slnx"]);
    }

    fn dotnet_app() -> App {
        let kind = AppKind::Dotnet {
            project: "services/Orders/Orders.csproj".into(),
            launch_profile: "Orders".into(),
            swagger: None,
            args: Vec::new(),
        };
        App {
            key: "orders".into(),
            name: "Orders".into(),
            icon: None,
            nerd_icon: None,
            kind,
            tests: Vec::new(),
            migrations: None,
            env: Vec::new(),
            depends_on: Vec::new(),
            health: crate::profile::Health::default(),
            restart_on_crash: false,
        }
    }

    fn report(top: Value, transitive: Value) -> String {
        let framework = json!({ "framework": "net10.0", "topLevelPackages": top, "transitivePackages": transitive });
        json!({ "version": 1, "projects": [{ "path": "Orders.csproj", "frameworks": [framework] }] }).to_string()
    }

    #[test]
    fn a_dotnet_apps_vulnerable_and_outdated_packages_merge_the_worst_first_with_their_fixes() {
        let advisory = |id: &str, severity: &str| json!({ "severity": severity, "advisoryurl": format!("https://github.com/advisories/{id}") });
        let vulnerable = report(
            json!([{ "id": "Example.Json", "requestedVersion": "12.0.1", "resolvedVersion": "12.0.1",
                     "vulnerabilities": [advisory("GHSA-aaaa-bbbb-cccc", "High")] }]),
            json!([{ "id": "Example.Text", "resolvedVersion": "8.0.0",
                     "vulnerabilities": [advisory("GHSA-dddd-eeee-ffff", "High"), advisory("GHSA-gggg-hhhh-iiii", "Moderate")] }]),
        );
        let outdated = report(
            json!([{ "id": "Example.Json", "requestedVersion": "12.0.1", "resolvedVersion": "12.0.1", "latestVersion": "13.0.4" },
                   { "id": "Example.Old", "requestedVersion": "1.0.0", "resolvedVersion": "1.0.0", "latestVersion": "1.2.0" }]),
            json!([{ "id": "Example.Text", "resolvedVersion": "8.0.0", "latestVersion": "10.0.1" },
                   { "id": "Example.Deep", "resolvedVersion": "2.0.0", "latestVersion": "3.0.0" }]),
        );
        let found = dotnet(&vulnerable, &outdated).unwrap();
        let names: Vec<&str> = found.iter().map(|package| package.name.as_str()).collect();
        assert_eq!(names, ["Example.Json", "Example.Text", "Example.Old"], "a transitive one only when vulnerable");
        assert_eq!(found[0].fix, Some(Fix { package: "Example.Json".into(), version: "13.0.4".into(), major: true }));
        assert!(!found[1].direct && found[1].advisories.len() == 2 && found[1].severity == Some(Severity::High));
        assert_eq!(found[1].advisories[0].id, "GHSA-dddd-eeee-ffff");
        assert_eq!((found[2].severity, found[2].fix.as_ref()), (None, None), "outdated only");
        let app = dotnet_app();
        assert_eq!(command(&app, &found[0]).unwrap(), "dotnet add Orders.csproj package Example.Json --version 13.0.4");
        assert_eq!(command(&app, &found[2]).unwrap(), "dotnet add Orders.csproj package Example.Old --version 1.2.0");
        let twice = [found[0].clone(), found[0].clone(), found[2].clone()];
        assert_eq!(commands(&app, &twice).len(), 2, "a .NET app's fixes run one by one, each package once");
        assert!(folder(Path::new("repo"), &app).ends_with("services/Orders"));
        assert_eq!(summary(&found), "2 high · 1 outdated");
        let failed = json!({ "version": 1, "problems": [{ "level": "error", "text": "No assets file was found" }] });
        assert_eq!(dotnet(&failed.to_string(), "{}").unwrap_err().to_string(), "No assets file was found");
    }

    #[test]
    fn an_npm_apps_audit_and_outdated_merge_with_what_fixes_each() {
        let audit = json!({ "auditReportVersion": 2, "vulnerabilities": {
            "lodash": { "name": "lodash", "severity": "high", "isDirect": true,
                "via": [{ "title": "Command Injection in lodash", "url": "https://github.com/advisories/GHSA-aaaa-bbbb-cccc",
                          "severity": "high", "range": "<4.17.21" }],
                "fixAvailable": { "name": "lodash", "version": "4.18.1", "isSemVerMajor": false } },
            "wrapper": { "name": "wrapper", "severity": "moderate", "isDirect": true, "via": ["lodash"], "fixAvailable": true }
        } });
        let outdated = json!({
            "lodash": { "current": "4.17.19", "wanted": "4.17.19", "latest": "4.18.1" },
            "left-pad": { "current": "1.1.0", "wanted": "1.1.0", "latest": "1.3.0" }
        });
        let installed = |name: &str| (name == "lodash").then(|| "4.17.19".to_string());
        let found = npm(&audit.to_string(), &outdated.to_string(), installed).unwrap();
        let names: Vec<&str> = found.iter().map(|package| package.name.as_str()).collect();
        assert_eq!(names, ["lodash", "wrapper", "left-pad"]);
        assert_eq!(found[0].advisories[0].title, "Command Injection in lodash");
        assert_eq!(found[1].via, ["lodash"]);
        let app = App { kind: AppKind::Npm { dir: "web".into(), variants: Vec::new() }, ..dotnet_app() };
        assert_eq!(command(&app, &found[0]).unwrap(), "npm install lodash@4.18.1");
        assert_eq!(command(&app, &found[1]).unwrap(), "npm audit fix", "fixable, no version given");
        assert_eq!(command(&app, &found[2]).unwrap(), "npm install left-pad@1.3.0");
        assert_eq!(
            commands(&app, &found),
            ["npm install lodash@4.18.1 left-pad@1.3.0", "npm audit fix"],
            "many fixes: one install, and one audit fix for those with no version"
        );
        let failed =
            json!({ "error": { "code": "ENOLOCK", "summary": "This command requires an existing lockfile." } });
        assert!(npm(&failed.to_string(), "{}", |_| None).unwrap_err().to_string().contains("existing lockfile"));
    }

    #[test]
    fn the_scans_are_kept_and_read_back() {
        let temp = TempDir::new("audit-saved");
        let package =
            Package { name: "lodash".into(), current: "4.17.19".into(), severity: Some(Severity::Critical), ..blank() };
        let scan = AppAudit { at: 1, packages: vec![package], ..AppAudit::default() };
        let audits = Audits::from([("web".to_string(), scan)]);
        save(&temp.join("state"), &audits).unwrap();
        assert_eq!(load(&temp.join("state")), audits);
        assert_eq!(audits["web"].worst(), Some(Severity::Critical));
        assert_eq!(audits["web"].count(Severity::Critical), 1);
        assert!(load(&temp.join("elsewhere")).is_empty());
    }

    #[test]
    fn a_scan_says_what_it_fixed_and_what_is_new_since_the_one_before() {
        let advisory = |id: &str| Advisory {
            id: id.into(),
            url: String::new(),
            title: String::new(),
            severity: Severity::High,
            range: String::new(),
        };
        let vulnerable = |name: &str, current: &str, ids: &[&str]| Package {
            name: name.into(),
            current: current.into(),
            severity: Some(Severity::High),
            advisories: ids.iter().map(|id| advisory(id)).collect(),
            ..blank()
        };
        let before = [vulnerable("lodash", "4.17.19", &["GHSA-1"]), vulnerable("minimist", "1.2.5", &["GHSA-2"])];
        let outdated = Package {
            latest: Some("5.0.0".into()),
            ..Package { name: "lodash".into(), current: "4.18.1".into(), ..blank() }
        };
        let mut scan = AppAudit {
            packages: vec![
                vulnerable("minimist", "1.2.5", &["GHSA-2", "GHSA-3"]),
                outdated,
                vulnerable("qs", "6.0.0", &["GHSA-4"]),
            ],
            ..AppAudit::default()
        };
        compare(&before, &mut scan);
        assert_eq!(scan.fixed, [Fixed { name: "lodash".into(), from: "4.17.19".into(), to: Some("4.18.1".into()) }]);
        assert_eq!(scan.new, ["minimist", "qs"], "a new advisory, a new package");
        assert_eq!(scan.new_advisories, ["GHSA-3", "GHSA-4"]);
        assert_eq!(age(jiff::Timestamp::now().as_second() - 7_200), "2 h ago");
    }
}
