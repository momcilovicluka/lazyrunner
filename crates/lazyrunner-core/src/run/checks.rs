//! The startup checklist's checks: lazyrunner's version and update, the tools, the ports, the database server, the
//! frontend packages, the profile and the branch. Each check runs on its own (each in a thread of its own, side by side) and says ok, warn, fail
//! or skip, with a detail.

use std::io::Read;
use std::net::{TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::Controller;
use super::net::Listening;
use super::secrets;
use super::tools;
use crate::profile::{App, AppKind, Source};
use crate::update;

const SQL_TIMEOUT: Duration = Duration::from_secs(2);
const SQL_PORT: u16 = 1433;
/// A tool that hasn't printed its version by then is hung (a broken install, a prompt nobody sees).
const TOOL_TIMEOUT: Duration = Duration::from_secs(10);
const TOOL_POLL: Duration = Duration::from_millis(20);
/// The session's row (#85).
pub const SESSION_CHECK: &str = "Session";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Pending,
    Running,
    Ok,
    Warn,
    Fail,
    Skip,
}

pub type Outcome = (Status, String);

/// Says what a running check is doing (the update's download progress), shown on its line.
pub type Report<'a> = &'a mut dyn FnMut(&str);

pub struct Check {
    pub name: String,
    pub kind: Kind,
    /// The language of the command app it's about: its icon in the checklist.
    pub language: Option<&'static str>,
    pub run: Box<dyn FnOnce(Report) -> Outcome + Send>,
    /// The apps needn't wait for it at startup (the update check): it may finish after the checklist closes.
    pub background: bool,
}

/// What a check looks at: where the checklist groups it, and what fixes what it finds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    /// The lazyrunner that runs.
    Version,
    Update,
    /// The audit view's last scans: the packages with known vulnerabilities.
    Audit,
    Tool,
    Ports,
    Database,
    /// An npm app's packages, by its key.
    Packages(String),
    Project,
    Branch,
    /// The session kept when lazyrunner last closed (#85), and how old it is.
    Session(crate::run::session::Freshness),
}

/// The checklist's groups, in its order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Group {
    Environment,
    Project,
    Database,
    Packages,
    Update,
}

impl Kind {
    pub fn group(&self) -> Group {
        match self {
            Self::Version | Self::Tool => Group::Environment,
            Self::Project | Self::Ports | Self::Branch | Self::Session(_) => Group::Project,
            Self::Database => Group::Database,
            Self::Packages(_) | Self::Audit => Group::Packages,
            Self::Update => Group::Update,
        }
    }

    /// Where it comes in its group: lazyrunner before the tools, the profile before the ports and the branch.
    pub fn rank(&self) -> u8 {
        match self {
            Self::Version => 0,
            Self::Tool => 1,
            Self::Project => 2,
            Self::Ports => 3,
            Self::Branch => 4,
            Self::Session(_) => 5,
            Self::Database => 5,
            Self::Packages(_) => 6,
            Self::Audit => 7,
            Self::Update => 7,
        }
    }
}

impl Group {
    pub fn name(self) -> &'static str {
        match self {
            Self::Environment => "Environment",
            Self::Project => "Project",
            Self::Database => "Database",
            Self::Packages => "Packages",
            Self::Update => "Update",
        }
    }
}

fn check(kind: Kind, name: impl Into<String>, run: impl FnOnce() -> Outcome + Send + 'static) -> Check {
    Check { name: name.into(), kind, language: None, run: Box::new(move |_: Report| run()), background: false }
}

/// The first line a tool prints for `args`, when it runs and succeeds.
/// The line of a tool's version output that has the version: the first with a digit (Gradle's starts with a rule).
pub fn version_line(output: &str) -> Option<String> {
    output.lines().map(str::trim).find(|line| line.chars().any(|c| c.is_ascii_digit())).map(str::to_string)
}

fn tool_version(program: &PathBuf, args: &[&str]) -> Option<String> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < TOOL_TIMEOUT => std::thread::sleep(TOOL_POLL),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    let mut output = String::new();
    child.stdout.take()?.read_to_string(&mut output).ok()?;
    let mut errors = String::new();
    if let Some(mut stderr) = child.stderr.take() {
        let _ = stderr.read_to_string(&mut errors);
    }
    // `java -version` (Java 8) says it on stderr.
    status.success().then(|| version_line(&output).or_else(|| version_line(&errors)))?
}

fn version_of(tool: &str) -> Option<String> {
    tool_version(&tools::find_on_path(tool)?, &["--version"])
}

fn npm_version() -> Option<String> {
    let (program, prefix) = tools::npm_launcher().ok()?;
    let mut args: Vec<String> = prefix.iter().map(|arg| arg.to_string_lossy().into_owned()).collect();
    args.push("--version".into());
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    tool_version(&program, &args)
}

/// The tools' versions - git, the .NET SDK, Node.js, npm - for a bug report; `None`: not found.
pub fn tool_versions() -> Vec<(&'static str, Option<String>)> {
    vec![
        ("git", version_of("git").map(|version| version.trim_start_matches("git version ").to_string())),
        (".NET SDK", version_of("dotnet")),
        ("Node.js", version_of("node")),
        ("npm", npm_version()),
    ]
}

/// The checks for this project, ready to run.
pub fn checks(controller: &Controller) -> Vec<Check> {
    let mut checks =
        vec![check(Kind::Version, "lazyrunner", || (Status::Ok, format!("v{}", crate::changelog::VERSION)))];
    let config = controller.config.clone();
    let state_dir = controller.paths().state_dir.clone();
    checks.push(Check {
        name: "lazyrunner".into(),
        kind: Kind::Update,
        language: None,
        run: Box::new(move |report: Report| {
            let outcome = update::update(&config, &state_dir, false, &mut update::Progress::new(report));
            crate::contributors::refresh(&config, &state_dir);
            let status = match outcome.status {
                update::Status::Ok => Status::Ok,
                update::Status::Warn => Status::Warn,
                update::Status::Skip => Status::Skip,
            };
            (status, outcome.detail)
        }),
        background: true,
    });
    checks.push(check(Kind::Tool, "git", || match version_of("git") {
        Some(version) => (Status::Ok, version.trim_start_matches("git version ").to_string()),
        None => (Status::Fail, "not found on PATH".into()),
    }));
    if controller.apps.iter().any(App::is_dotnet) {
        checks.push(check(Kind::Tool, ".NET SDK", || match version_of("dotnet") {
            Some(version) => (Status::Ok, version),
            None => (Status::Fail, "dotnet not found on PATH".into()),
        }));
        checks.push(check(Kind::Tool, "dotnet-trace", || match crate::run::profile::tool() {
            Some(path) => (Status::Ok, tool_version(&path, &["--version"]).unwrap_or_else(|| "installed".into())),
            None => (
                Status::Skip,
                format!(
                    "not installed - Alt+p profiles an app with it; f installs it ({})",
                    crate::run::profile::INSTALL
                ),
            ),
        }));
    }
    if controller.apps.iter().any(|app| matches!(app.kind, AppKind::Npm { .. })) {
        checks.push(check(Kind::Tool, "Node.js / npm", || {
            let Some(node) = version_of("node") else { return (Status::Fail, "node not found on PATH".into()) };
            (Status::Ok, format!("node {node} · npm {}", npm_version().unwrap_or_else(|| "?".into())))
        }));
    }
    for app in controller.apps.iter().filter(|app| app.command().is_some_and(|command| command.compose.is_none())) {
        checks.push(Check { language: crate::apptypes::language(app), ..command_check(controller, app) });
    }
    if controller.apps.iter().any(|app| app.command().is_some_and(|command| command.compose.is_some())) {
        checks.push(check(Kind::Tool, DOCKER_CHECK, || match crate::run::compose::engine_version() {
            Ok(version) => (Status::Ok, format!("engine {version}")),
            Err(error) => (Status::Fail, error),
        }));
    }
    checks.push(ports_check(controller));
    if controller.apps.iter().any(|app| app.traits().uses_secrets) {
        checks.push(database_check(controller));
    }
    let installs = |app: &&App| app.command().is_some_and(|command| !command.install.is_empty());
    for app in controller.apps.iter().filter(|app| matches!(app.kind, AppKind::Npm { .. }) || installs(app)) {
        checks.push(packages_check(controller, &app.key));
    }
    if controller.apps.iter().any(|app| app.command().is_none()) {
        checks.push(audit_check(controller));
    }
    checks.push(project_check(controller));
    let mut branch = controller.branch.clone();
    for app in controller.apps.iter().filter(|app| app.traits().variants) {
        if let Some(mode) = controller.mode(&app.key) {
            branch.push_str(&format!(" · {}: {} ({})", app.name.to_lowercase(), mode.label, mode.script));
        }
    }
    // Opened on another branch than the session it closed on: what changed for running since (#104).
    let since = controller.resume.as_ref().map(|snapshot| snapshot.branch.clone());
    let since = since.filter(|from| !from.is_empty() && *from != controller.branch);
    let (root, apps, to) = (controller.root.clone(), controller.apps.clone(), controller.branch.clone());
    checks.push(check(Kind::Branch, "Branch", move || {
        let Some(from) = since else { return (Status::Ok, branch) };
        let changes = crate::run::branch_changes::between(&root, &from, &to, &apps);
        if changes.is_empty() {
            return (Status::Ok, branch);
        }
        let shorts: Vec<&str> = changes.iter().map(|change| change.short.as_str()).collect();
        (Status::Ok, format!("{branch} · since {from}: {} (Alt+g once open)", shorts.join(", ")))
    }));
    if let Some(snapshot) = &controller.resume {
        let now = chrono::Local::now().fixed_offset();
        let (summary, freshness) = (snapshot.summary(&controller.branch, now), snapshot.freshness(now));
        let count = controller.resume_choices.len();
        let summary = if count > 1 { format!("{summary} · 1 of {count} ([ ] another)") } else { summary };
        checks.push(check(Kind::Session(freshness), SESSION_CHECK, move || (Status::Ok, summary)));
    }
    checks
}

fn ports_check(controller: &Controller) -> Check {
    let wanted: Vec<(u16, bool)> = controller
        .apps
        .iter()
        .filter_map(|app| Some((controller.port(&app.key)?, controller.is_running(&app.key))))
        .collect();
    check(Kind::Ports, "Ports", move || ports_report(&wanted, &Listening::now(), &super::net::reserved_ranges()))
}

/// The apps' ports (and whether each app runs): free, held by another process (! frees those at start), or reserved
/// by Windows - which no app can listen on.
fn ports_report(wanted: &[(u16, bool)], listening: &Listening, reserved: &[super::net::PortRange]) -> Outcome {
    let mut taken = Vec::new();
    let mut blocked = Vec::new();
    for &(port, _) in wanted.iter().filter(|(_, running)| !running) {
        if let Some(owner) = listening.ports.get(&port) {
            taken.push(format!("{port} used by {}", owner.name));
        } else if let Some((first, last)) = super::net::reserved(port, reserved) {
            blocked.push(format!(
                "{port} reserved by Windows ({first}-{last}) - net stop winnat, then net start winnat, in an \
                 elevated terminal frees it"
            ));
        }
    }
    if !blocked.is_empty() {
        return (Status::Fail, blocked.into_iter().chain(taken).collect::<Vec<_>>().join("; "));
    }
    if !taken.is_empty() {
        return (Status::Warn, taken.join("; "));
    }
    let ports: Vec<String> = wanted.iter().map(|(port, _)| port.to_string()).collect();
    (Status::Ok, format!("{} free", ports.join(", ")))
}

/// Where a SQL Server listens: host and port, or why it isn't checked.
pub fn sql_endpoint(server: &str) -> Result<(String, u16), String> {
    let value = ["tcp:", "np:", "lpc:"].iter().find_map(|prefix| server.strip_prefix(prefix)).unwrap_or(server);
    if value.to_lowercase().starts_with("(localdb)") {
        return Err("LocalDB - not checked".into());
    }
    let (host, port) = match value.rsplit_once(',') {
        Some((host, port)) if port.trim().parse::<u16>().is_ok() => {
            (host.trim(), port.trim().parse().unwrap_or(SQL_PORT))
        }
        _ => (value, SQL_PORT),
    };
    if host.contains('\\') {
        return Err(format!("named instance ({host}) - its port is dynamic, not checked"));
    }
    let host = if matches!(host, "." | "(local)") { "localhost" } else { host };
    Ok((host.to_string(), port))
}

fn database_check(controller: &Controller) -> Check {
    let path = secrets::secrets_path(&controller.root, &controller.apps);
    check(Kind::Database, "Database", move || {
        let Some(path) = path else {
            return (Status::Skip, "the APIs keep no user secrets - nothing to check".into());
        };
        let Ok(text) = std::fs::read_to_string(path) else {
            return (Status::Warn, "no secrets.json - the APIs use appsettings".into());
        };
        let active = secrets::groups(&text)
            .into_iter()
            .next()
            .and_then(|(_, entries)| entries.into_iter().find(|entry| entry.active));
        let Some(active) = active else { return (Status::Warn, "no active connection string in secrets.json".into()) };
        let (host, port) = match sql_endpoint(&active.server) {
            Ok(endpoint) => endpoint,
            Err(why) => return (Status::Skip, format!("{} · {why}", active.label)),
        };
        let started = Instant::now();
        let answers = (host.as_str(), port).to_socket_addrs().ok().is_some_and(|mut addresses| {
            addresses.any(|address| TcpStream::connect_timeout(&address, SQL_TIMEOUT).is_ok())
        });
        if answers {
            (Status::Ok, format!("{} · port {port} answers ({} ms)", active.label, started.elapsed().as_millis()))
        } else {
            (Status::Warn, format!("{} · nothing answers on port {port}", active.label))
        }
    })
}

/// What the audit view's last scans found (no network: it scans only when asked): a warning while an app has a high
/// or critical advisory.
fn audit_check(controller: &Controller) -> Check {
    let scans: Vec<(String, crate::run::audit::AppAudit)> = controller
        .apps
        .iter()
        .filter_map(|app| Some((app.name.clone(), controller.audits().get(&app.key)?.clone())))
        .collect();
    check(Kind::Audit, "Vulnerabilities", move || audit_report(&scans))
}

fn audit_report(scans: &[(String, crate::run::audit::AppAudit)]) -> Outcome {
    use crate::run::audit::{Severity, age};
    let Some(oldest) = scans.iter().map(|(_, scan)| scan.at).min() else {
        return (Status::Skip, "not scanned - A, then R scans every app's packages".into());
    };
    let bad: Vec<String> = scans
        .iter()
        .filter_map(|(name, scan)| {
            let worst = scan.worst().filter(|worst| *worst >= Severity::High)?;
            Some(format!("{name}: {} {}", scan.count(worst), worst.name()))
        })
        .collect();
    if bad.is_empty() {
        (Status::Ok, format!("nothing high or critical (scanned {})", age(oldest)))
    } else {
        (Status::Warn, format!("{} - O shows them", bad.join(" · ")))
    }
}

/// A command app's programs - its tools (with their versions), what it runs, builds and installs with - each found in
/// its `path` folders, on PATH or in its folder.
/// How to get a tool a command app needs, when it's missing (on Windows, which lazyrunner's releases are for).
const HOW_TO_GET: [(&str, &str); 10] = [
    ("java", "winget install Microsoft.OpenJDK.21"),
    ("mvn", "the project's ./mvnw, or maven.apache.org/download"),
    ("gradle", "the project's ./gradlew, or gradle.org/install"),
    ("python", "winget install Python.Python.3.12"),
    ("uv", "winget install astral-sh.uv"),
    ("go", "winget install GoLang.Go"),
    ("cargo", "winget install Rustlang.Rustup"),
    ("node", "winget install OpenJS.NodeJS.LTS"),
    ("npm", "winget install OpenJS.NodeJS.LTS"),
    ("docker", "winget install Docker.DockerDesktop"),
];
/// The version in what a tool says about itself: `Python 3.12.1`, `go version go1.23.1 windows/amd64`.
static VERSION: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| regex::Regex::new(r"\d+\.\d+(\.\d+)?").expect("a valid pattern"));

/// A tool as a capsule: `python 3.12.1`, else what it said.
pub fn capsule(program: &str, said: &str) -> String {
    VERSION.find(said).map_or_else(|| said.to_string(), |version| format!("{program} {}", version.as_str()))
}

/// The tools missing, each with how to get it when lazyrunner knows.
pub fn missing_tools(missing: &[&str]) -> String {
    let get = |tool: &str| HOW_TO_GET.iter().find(|(name, _)| name.eq_ignore_ascii_case(tool)).map(|(_, how)| *how);
    let each: Vec<String> = missing
        .iter()
        .map(|tool| get(tool).map_or_else(|| tool.to_string(), |how| format!("{tool} ({how})")))
        .collect();
    each.join(", ")
}

/// The checklist's row for Docker, which runs the compose apps.
pub const DOCKER_CHECK: &str = "Docker";

fn command_check(controller: &Controller, app: &App) -> Check {
    let command = app.command().expect("a command app").clone();
    let (dir, folders) = tools::command_folders(&controller.root, &command);
    let mut programs: Vec<String> = command.tools.clone();
    for words in [&command.run, &command.build, &command.install] {
        if let Some(program) = words.first().filter(|program| !programs.contains(program)) {
            programs.push(program.clone());
        }
    }
    check(Kind::Tool, app.name.clone(), move || {
        let found = |program: &String| tools::program_path(&dir, &folders, program);
        let missing: Vec<&str> =
            programs.iter().filter(|program| found(program).is_none()).map(String::as_str).collect();
        if !missing.is_empty() {
            let hint = crate::apptypes::missing_path(&command, &folders);
            return (
                Status::Fail,
                format!("{} not found on PATH or in {}{hint}", missing_tools(&missing), dir.display()),
            );
        }
        let shown: Vec<String> = programs
            .iter()
            .map(|program| match command.tools.contains(program) {
                true => found(program)
                    .and_then(|path| tool_version(&path, &["--version"]).or_else(|| tool_version(&path, &["version"])))
                    .map_or(program.clone(), |said| capsule(program, &said)),
                false => program.clone(),
            })
            .collect();
        (Status::Ok, shown.join(" · "))
    })
}

fn packages_check(controller: &Controller, key: &str) -> Check {
    let app = controller.app(key).expect("a known app");
    let name = format!("{} packages", app.name);
    let kind = Kind::Packages(key.to_string());
    if let Some(command) = app.command() {
        return command_packages_check(controller, key, command, kind, name);
    }
    let AppKind::Npm { dir, .. } = &app.kind else { return check(kind, name, || (Status::Skip, String::new())) };
    let folder = controller.root.join(dir);
    let dir = dir.clone();
    let in_sync = controller.lock_in_sync(key);
    check(kind, name, move || {
        if !tools::has_packages(&folder) {
            return (
                Status::Fail,
                format!("{} is missing - press i (npm ci)", crate::profile::join(&dir, "node_modules")),
            );
        }
        if !folder.join("node_modules").exists() {
            return (Status::Ok, "nothing to install - package.json lists no packages".into());
        }
        match in_sync {
            None => (Status::Ok, "installed (no npm ci from lazyrunner yet to compare with)".into()),
            Some(false) => (Status::Warn, "package-lock.json changed since the last npm ci - press i".into()),
            Some(true) => (Status::Ok, "in sync with package-lock.json".into()),
        }
    })
}

/// A command app's install: its own `path` folder (a `.venv`) there, and in sync with the files it installs from
/// (`installFrom`) as lazyrunner's last install left them.
fn command_packages_check(
    controller: &Controller,
    key: &str,
    command: &crate::profile::Command,
    kind: Kind,
    name: String,
) -> Check {
    let missing = tools::lacks_own_folders(&controller.root, command);
    let env = tools::own_folders(command)
        .first()
        .map(|folder| folder.split(['/', '\\']).next().unwrap_or(folder).to_string());
    let (install, files) = (command.install.join(" "), command.install_from.join(", "));
    let in_sync = controller.lock_in_sync(key);
    check(kind, name, move || {
        if let Some(env) = env.as_ref().filter(|_| missing) {
            return (Status::Fail, format!("{env} isn't there yet - press i ({install})"));
        }
        let place = env.as_ref().map_or(String::new(), |env| format!("{env} · "));
        match in_sync {
            None if files.is_empty() => (Status::Ok, format!("{place}i runs {install}")),
            None => (Status::Ok, format!("{place}no install from lazyrunner yet to compare {files} with")),
            Some(false) => (Status::Warn, format!("{place}{files} changed since the last install - press i")),
            Some(true) => (Status::Ok, format!("{place}in sync with {files}")),
        }
    })
}

fn project_check(controller: &Controller) -> Check {
    let project = &controller.project;
    let file_name = project.path.file_name().map_or(String::new(), |name| name.to_string_lossy().into_owned());
    let file = if project.source == Source::Repo {
        format!("this repo's {file_name}")
    } else {
        format!("{} profile {}", project.source.name(), project.path.display())
    };
    let (name, changed) = (project.name.clone(), controller.profile_changed);
    check(Kind::Project, "Profile", move || {
        if changed {
            (Status::Warn, format!("{name} from {file} - this branch changed it (, then p reloads)"))
        } else {
            (Status::Ok, format!("{name} from {file}"))
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run::net::Owner;

    #[test]
    fn a_tool_shows_as_a_capsule_and_a_missing_one_says_how_to_get_it() {
        assert_eq!(capsule("python", "Python 3.12.1"), "python 3.12.1");
        assert_eq!(capsule("go", "go version go1.23.1 windows/amd64"), "go 1.23.1");
        assert_eq!(capsule("uv", "uv 0.5.1 (f399a5271 2024-11-08)"), "uv 0.5.1");
        assert_eq!(capsule("tool", "no version here"), "no version here");
        assert_eq!(missing_tools(&["uv", "azurite"]), "uv (winget install astral-sh.uv), azurite");
        assert_eq!(
            version_line("\n------------\nGradle 8.5\n------------"),
            Some("Gradle 8.5".into()),
            "past its rule"
        );
        assert_eq!(capsule("java", "openjdk 21.0.1 2023-10-17"), "java 21.0.1");
        assert_eq!(capsule("mvn", "Apache Maven 3.9.6 (bc0240f3)"), "mvn 3.9.6");
    }

    #[test]
    fn a_reserved_port_fails_the_ports_check_and_a_busy_one_warns() {
        let mut listening = Listening::default();
        listening.ports.insert(47301, Owner { pid: 42, name: "node.exe".into() });
        let reserved = [(58121, 58220)];
        let (status, detail) = ports_report(&[(47201, false), (47202, false)], &listening, &reserved);
        assert_eq!((status, detail.as_str()), (Status::Ok, "47201, 47202 free"));
        let (status, detail) = ports_report(&[(47301, false)], &listening, &reserved);
        assert_eq!((status, detail.as_str()), (Status::Warn, "47301 used by node.exe"));
        let (status, detail) = ports_report(&[(58201, false), (47301, false)], &listening, &reserved);
        assert_eq!(status, Status::Fail);
        assert!(detail.starts_with("58201 reserved by Windows (58121-58220) - net stop winnat"), "{detail}");
        assert!(detail.ends_with("; 47301 used by node.exe"), "{detail}");
        let (status, _) = ports_report(&[(58201, true)], &listening, &reserved);
        assert_eq!(status, Status::Ok, "an app that runs has its port");
    }

    #[test]
    fn sql_servers_resolve_to_a_host_and_port() {
        assert_eq!(sql_endpoint("tcp:db.example.test,1444"), Ok(("db.example.test".into(), 1444)));
        assert_eq!(sql_endpoint("."), Ok(("localhost".into(), 1433)));
        assert!(sql_endpoint("(localdb)\\MSSQLLocalDB").is_err());
        assert!(sql_endpoint("server\\SQLEXPRESS").unwrap_err().starts_with("named instance"));
    }
}
