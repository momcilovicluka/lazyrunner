//! The startup checklist's checks: the tools, the ports, the database server, the frontend packages, the project
//! and the branch. Each check runs on its own (each in a thread of its own, side by side) and says ok, warn, fail
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
use crate::profile::{AppKind, Source};
use crate::update;

const SQL_TIMEOUT: Duration = Duration::from_secs(2);
const SQL_PORT: u16 = 1433;
/// A tool that hasn't printed its version by then is hung (a broken install, a prompt nobody sees).
const TOOL_TIMEOUT: Duration = Duration::from_secs(10);
const TOOL_POLL: Duration = Duration::from_millis(20);

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
    pub run: Box<dyn FnOnce(Report) -> Outcome + Send>,
    /// The apps needn't wait for it at startup (the update check): it may finish after the checklist closes.
    pub background: bool,
}

fn check(name: impl Into<String>, run: impl FnOnce() -> Outcome + Send + 'static) -> Check {
    Check { name: name.into(), run: Box::new(move |_: Report| run()), background: false }
}

/// The first line a tool prints for `args`, when it runs and succeeds.
fn tool_version(program: &PathBuf, args: &[&str]) -> Option<String> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
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
    status
        .success()
        .then_some(output)?
        .lines()
        .next()
        .map(|line| line.trim().to_string())
        .filter(|line| !line.is_empty())
}

fn version_of(tool: &str) -> Option<String> {
    tool_version(&tools::find_on_path(tool)?, &["--version"])
}

/// The checks for this project, ready to run.
pub fn checks(controller: &Controller) -> Vec<Check> {
    let mut checks = Vec::new();
    let config = controller.config.clone();
    let state_dir = controller.paths().state_dir.clone();
    checks.push(Check {
        name: "lazyrunner".into(),
        run: Box::new(move |report: Report| {
            let outcome = update::update(&config, &state_dir, false, &mut update::Progress::new(report));
            let status = match outcome.status {
                update::Status::Ok => Status::Ok,
                update::Status::Warn => Status::Warn,
                update::Status::Skip => Status::Skip,
            };
            (status, outcome.detail)
        }),
        background: true,
    });
    checks.push(check("git", || match version_of("git") {
        Some(version) => (Status::Ok, version.trim_start_matches("git version ").to_string()),
        None => (Status::Fail, "not found on PATH".into()),
    }));
    if controller.apps.iter().any(|app| app.traits().builds) {
        checks.push(check(".NET SDK", || match version_of("dotnet") {
            Some(version) => (Status::Ok, version),
            None => (Status::Fail, "dotnet not found on PATH".into()),
        }));
    }
    if controller.apps.iter().any(|app| app.traits().installs) {
        checks.push(check("Node.js / npm", || {
            let Some(node) = version_of("node") else { return (Status::Fail, "node not found on PATH".into()) };
            let npm = tools::npm_launcher().ok().and_then(|(program, prefix)| {
                let mut args: Vec<String> = prefix.iter().map(|arg| arg.to_string_lossy().into_owned()).collect();
                args.push("--version".into());
                let args: Vec<&str> = args.iter().map(String::as_str).collect();
                tool_version(&program, &args)
            });
            (Status::Ok, format!("node {node} · npm {}", npm.unwrap_or_else(|| "?".into())))
        }));
    }
    checks.push(ports_check(controller));
    if controller.apps.iter().any(|app| app.traits().uses_secrets) {
        checks.push(database_check(controller));
    }
    for app in controller.apps.iter().filter(|app| app.traits().installs) {
        checks.push(packages_check(controller, &app.key));
    }
    checks.push(project_check(controller));
    let mut branch = controller.branch.clone();
    for app in controller.apps.iter().filter(|app| app.traits().variants) {
        if let Some(mode) = controller.mode(&app.key) {
            branch.push_str(&format!(" · {}: {} ({})", app.name.to_lowercase(), mode.label, mode.script));
        }
    }
    checks.push(check("Branch", move || (Status::Ok, branch)));
    checks
}

fn ports_check(controller: &Controller) -> Check {
    let wanted: Vec<(u16, bool)> = controller
        .apps
        .iter()
        .filter_map(|app| Some((controller.port(&app.key)?, controller.is_running(&app.key))))
        .collect();
    check("Ports", move || {
        let listening = Listening::now();
        let taken: Vec<String> = wanted
            .iter()
            .filter(|(port, running)| !running && listening.contains(*port))
            .map(|(port, _)| format!("{port} used by {}", listening.ports[port].name))
            .collect();
        if taken.is_empty() {
            let ports: Vec<String> = wanted.iter().map(|(port, _)| port.to_string()).collect();
            (Status::Ok, format!("{} free", ports.join(", ")))
        } else {
            (Status::Warn, taken.join("; "))
        }
    })
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
    check("Database", move || {
        let Some(text) = path.and_then(|path| std::fs::read_to_string(path).ok()) else {
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

fn packages_check(controller: &Controller, key: &str) -> Check {
    let app = controller.app(key).expect("a known app");
    let name = format!("{} packages", app.name);
    let AppKind::Npm { dir, .. } = &app.kind else { return check(name, || (Status::Skip, String::new())) };
    let folder = controller.root.join(dir);
    let dir = dir.clone();
    let in_sync = controller.lock_in_sync(key);
    check(name, move || {
        if !folder.join("node_modules").exists() {
            return (
                Status::Fail,
                format!("{} is missing - press i (npm ci)", crate::profile::join(&dir, "node_modules")),
            );
        }
        match in_sync {
            None => (Status::Ok, "installed (no npm ci from lazyrunner yet to compare with)".into()),
            Some(false) => (Status::Warn, "package-lock.json changed since the last npm ci - press i".into()),
            Some(true) => (Status::Ok, "in sync with package-lock.json".into()),
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
    check("Project", move || {
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

    #[test]
    fn sql_servers_resolve_to_a_host_and_port() {
        assert_eq!(sql_endpoint("tcp:db.example.test,1444"), Ok(("db.example.test".into(), 1444)));
        assert_eq!(sql_endpoint("."), Ok(("localhost".into(), 1433)));
        assert!(sql_endpoint("(localdb)\\MSSQLLocalDB").is_err());
        assert!(sql_endpoint("server\\SQLEXPRESS").unwrap_err().starts_with("named instance"));
    }
}
