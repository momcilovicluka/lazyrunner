//! Docker Compose (#56): a compose app's services as `docker compose ps` reports them, whether they're up, and
//! stopping them when the app stops - its `docker compose up` only attaches to them.

use std::path::Path;
use std::process::{Child, Command, Output, Stdio};
use std::sync::{LazyLock, mpsc};
use std::time::Duration;

use regex::Regex;
use serde_json::Value;

use super::tools::find_on_path;
use crate::json::{field, list, text};
use crate::profile::{COMPOSE_CONFIG, COMPOSE_PS, COMPOSE_STOP, Compose, DOCKER};

/// How long `docker compose ps` and `docker version` may take.
const TIMEOUT: Duration = Duration::from_secs(10);
const PS_OPTIONS: [&str; 3] = ["--all", "--format", "json"];
const CONFIG_OPTIONS: [&str; 2] = ["--format", "json"];
/// The most ports one published range (`8000-8010`) counts for.
const MOST_IN_RANGE: u16 = 64;
/// Docker's own processes: what holds a published port while a container runs - never killed for a port.
const DOCKER_PROCESSES: [&str; 5] = ["docker", "com.docker", "wslrelay", "vpnkit", "rootlesskit"];

/// Whether a process holding a port is Docker's own (its backend, its proxy).
pub fn is_docker_process(name: &str) -> bool {
    let name = name.to_lowercase();
    DOCKER_PROCESSES.iter().any(|docker| name.starts_with(docker))
}

/// The ports `docker compose config --format json` says the services publish (`"published": "5432"`, a range, a
/// number), in order, once each.
pub fn parse_config_ports(output: &str) -> Vec<u16> {
    let Ok(config) = serde_json::from_str::<Value>(output) else { return Vec::new() };
    let services = field(&config, "services").as_object().cloned().unwrap_or_default();
    let mut ports: Vec<u16> = services
        .values()
        .flat_map(|service| list(field(service, "ports")).into_iter().cloned().collect::<Vec<_>>())
        .flat_map(|port| {
            let published = match field(&port, "published") {
                Value::Number(number) => number.to_string(),
                other => text(other),
            };
            let (from, to) = published.split_once('-').unwrap_or((&published, &published));
            match (from.trim().parse::<u16>(), to.trim().parse::<u16>()) {
                (Ok(from), Ok(to)) if from > 0 && from <= to => {
                    (from..=to.min(from.saturating_add(MOST_IN_RANGE))).collect()
                }
                _ => Vec::new(),
            }
        })
        .collect();
    ports.sort_unstable();
    ports.dedup();
    ports
}

/// The ports the app's services publish, as its compose file says.
pub fn config_ports(dir: &Path, compose: &Compose, env: &[(String, String)]) -> Result<Vec<u16>, String> {
    let output = output(docker(dir, env, &compose.args(COMPOSE_CONFIG, &CONFIG_OPTIONS))?)?;
    if !output.status.success() {
        return Err(failure(&output));
    }
    Ok(parse_config_ports(&String::from_utf8_lossy(&output.stdout)))
}
const RUNNING: &str = "running";
const CREATED: &str = "created";
const EXITED: &str = "exited";
const HEALTHY: &str = "healthy";
const UNHEALTHY: &str = "unhealthy";

/// How a service is doing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum State {
    /// Running, and healthy when it has a healthcheck.
    Up,
    #[default]
    /// Created, or running while its healthcheck hasn't passed yet.
    Starting,
    /// Unhealthy, restarting, or exited with an error.
    Down,
    /// Exited fine: a one-off service (migrations, a seed) that did its job.
    Done,
}

/// A compose service: its name, how it's doing (and Docker's words for it), the ports it publishes, and its image.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Service {
    pub name: String,
    pub state: State,
    pub detail: String,
    pub ports: Vec<u16>,
    pub image: String,
}

/// Images of a database: a service running one is the database the stack map draws.
const DATABASE_IMAGES: [&str; 10] = [
    "postgres",
    "postgis",
    "timescale",
    "mysql",
    "mariadb",
    "mssql",
    "sql-server",
    "azure-sql-edge",
    "mongo",
    "cockroach",
];

/// Whether an image is a database's (`postgres:16`, `mcr.microsoft.com/mssql/server:2022-latest`).
pub fn is_database(image: &str) -> bool {
    let image = image.to_lowercase();
    let name = image.split(['@', ':']).next().unwrap_or_default();
    DATABASE_IMAGES.iter().any(|database| name.split('/').any(|part| part.starts_with(database)))
}

/// `docker compose ps --format json`'s services: a JSON object per line (Compose 2.21 on), or one JSON array.
pub fn parse(output: &str) -> Vec<Service> {
    let output = output.trim();
    let items: Vec<Value> = if output.starts_with('[') {
        serde_json::from_str::<Value>(output).map(|all| list(&all).into_iter().cloned().collect()).unwrap_or_default()
    } else {
        output.lines().filter_map(|line| serde_json::from_str(line.trim()).ok()).collect()
    };
    let mut services: Vec<Service> = items.iter().map(service).collect();
    services.sort_by(|a, b| a.name.cmp(&b.name));
    services
}

fn service(item: &Value) -> Service {
    let state = text(field(item, "State")).to_lowercase();
    let health = text(field(item, "Health")).to_lowercase();
    let code = field(item, "ExitCode").as_i64().unwrap_or_default();
    let name = [text(field(item, "Service")), text(field(item, "Name"))]
        .into_iter()
        .find(|name| !name.is_empty())
        .unwrap_or_default();
    let how = match (state.as_str(), health.as_str()) {
        (RUNNING, "" | HEALTHY) => State::Up,
        (RUNNING, UNHEALTHY) => State::Down,
        (RUNNING | CREATED, _) => State::Starting,
        (EXITED, _) if code == 0 => State::Done,
        _ => State::Down,
    };
    let detail = match () {
        _ if state == EXITED => format!("{state} ({code})"),
        _ if health.is_empty() => state,
        _ => format!("{state} ({health})"),
    };
    let mut ports: Vec<u16> = list(field(item, "Publishers"))
        .into_iter()
        .filter_map(|publisher| field(publisher, "PublishedPort").as_u64())
        .filter_map(|port| u16::try_from(port).ok())
        .filter(|port| *port != 0)
        .collect();
    ports.sort_unstable();
    ports.dedup();
    Service { name, state: how, detail, ports, image: text(field(item, "Image")) }
}

/// Compose's prefix on a service's line: `db-1  | `, the service's name and its replica.
static PREFIX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([A-Za-z0-9][A-Za-z0-9_.-]*?)(?:-\d+)?\s+\| ?").expect("a valid pattern"));

/// A line `docker compose up` printed for a service: the service, and what it said.
pub fn split_line(text: &str) -> Option<(&str, &str)> {
    let found = PREFIX.captures(text)?;
    let whole = found.get(0)?;
    Some((found.get(1)?.as_str(), &text[whole.end()..]))
}

/// Up: there are services, and each is up or did its job.
pub fn is_up(services: &[Service]) -> bool {
    !services.is_empty() && services.iter().all(|service| matches!(service.state, State::Up | State::Done))
}

/// `docker` with these arguments in `dir`, with `env` on top of lazyrunner's own.
fn docker(dir: &Path, env: &[(String, String)], args: &[String]) -> Result<Command, String> {
    let program = find_on_path(DOCKER).ok_or_else(|| "docker was not found on PATH - install Docker".to_string())?;
    let mut command = Command::new(program);
    command.args(args).current_dir(dir).envs(env.iter().map(|(name, value)| (name, value))).stdin(Stdio::null());
    Ok(command)
}

/// Runs `command` and waits for it, [`TIMEOUT`] at most.
fn output(mut command: Command) -> Result<Output, String> {
    let child = command.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|error| error.to_string())?;
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(child.wait_with_output());
    });
    match receiver.recv_timeout(TIMEOUT) {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(error)) => Err(error.to_string()),
        Err(_) => Err(format!("no answer in {}s", TIMEOUT.as_secs())),
    }
}

/// The first line Docker wrote to stderr, or its exit code.
fn failure(output: &Output) -> String {
    let error = String::from_utf8_lossy(&output.stderr);
    error.lines().map(str::trim).find(|line| !line.is_empty()).map_or_else(
        || format!("exit code {}", output.status.code().map_or("-".into(), |code| code.to_string())),
        str::to_string,
    )
}

/// The app's services now.
pub fn ps(dir: &Path, compose: &Compose, env: &[(String, String)]) -> Result<Vec<Service>, String> {
    let output = output(docker(dir, env, &compose.args(COMPOSE_PS, &PS_OPTIONS))?)?;
    if !output.status.success() {
        return Err(failure(&output));
    }
    Ok(parse(&String::from_utf8_lossy(&output.stdout)))
}

/// Starts stopping the app's services; it goes on after lazyrunner quits.
pub fn stop(dir: &Path, compose: &Compose, env: &[(String, String)]) -> Result<Child, String> {
    let mut command = docker(dir, env, &compose.args(COMPOSE_STOP, &[]))?;
    command.stdout(Stdio::null()).stderr(Stdio::null()).spawn().map_err(|error| error.to_string())
}

/// Docker's engine version, or why it can't be asked: not installed, or not running.
pub fn engine_version() -> Result<String, String> {
    let args = ["version", "--format", "{{.Server.Version}}"].map(String::from);
    let output = output(docker(Path::new("."), &[], &args)?)?;
    let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !output.status.success() || version.is_empty() {
        let start = if cfg!(windows) { "start Docker Desktop" } else { "start the Docker daemon" };
        return Err(format!("Docker isn't running - {start} ({})", failure(&output)));
    }
    Ok(version)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINES: &str = r#"{"Name":"shop-db-1","Service":"db","Image":"postgres:16","State":"running","Health":"healthy","ExitCode":0,"Publishers":[{"URL":"0.0.0.0","TargetPort":5432,"PublishedPort":5432,"Protocol":"tcp"},{"URL":"::","TargetPort":5432,"PublishedPort":5432,"Protocol":"tcp"}]}
{"Name":"shop-cache-1","Service":"cache","State":"running","Health":"starting","ExitCode":0,"Publishers":[{"URL":"","TargetPort":6379,"PublishedPort":0,"Protocol":"tcp"}]}
{"Name":"shop-migrate-1","Service":"migrate","State":"exited","Health":"","ExitCode":0,"Publishers":null}
"#;

    #[test]
    fn ps_lines_give_each_service_how_it_does_and_its_ports() {
        let services = parse(LINES);
        let names: Vec<&str> = services.iter().map(|service| service.name.as_str()).collect();
        assert_eq!(names, ["cache", "db", "migrate"], "by name");
        assert_eq!((services[1].state, services[1].detail.as_str()), (State::Up, "running (healthy)"));
        assert_eq!((services[1].ports.as_slice(), services[1].image.as_str()), ([5432].as_slice(), "postgres:16"));
        assert_eq!((services[0].state, services[0].ports.len()), (State::Starting, 0), "unpublished");
        assert_eq!((services[2].state, services[2].detail.as_str()), (State::Done, "exited (0)"));
        assert!(!is_up(&services), "the cache isn't healthy yet");
        assert!(is_up(&services[1..]), "a one-off that did its job counts");
    }

    #[test]
    fn the_config_says_which_ports_the_services_publish() {
        let config = r#"{"name":"shop","services":{
            "db":{"image":"postgres:16","ports":[{"mode":"ingress","target":5432,"published":"5432","protocol":"tcp"}]},
            "web":{"ports":[{"target":80,"published":8080},{"target":81,"published":"9000-9002"},{"target":82}]},
            "worker":{}}}"#;
        assert_eq!(parse_config_ports(config), [5432, 8080, 9000, 9001, 9002]);
        assert!(parse_config_ports("not json").is_empty());
        assert!(is_docker_process("com.docker.backend.exe") && is_docker_process("wslrelay.exe"));
        assert!(!is_docker_process("postgres.exe"));
    }

    #[test]
    fn a_services_line_is_split_from_composes_prefix() {
        assert_eq!(split_line("db-1     | listening on 5432"), Some(("db", "listening on 5432")));
        assert_eq!(split_line("my-queue-2 |ready"), Some(("my-queue", "ready")));
        assert_eq!(split_line("custom_name  | x | y"), Some(("custom_name", "x | y")));
        assert_eq!(split_line("Attaching to cache-1, db-1"), None);
        assert_eq!(split_line("db-1 exited with code 0"), None);
    }

    #[test]
    fn a_database_image_is_known_by_its_name() {
        for image in ["postgres:16", "mcr.microsoft.com/mssql/server:2022-latest", "bitnami/mariadb", "mongo@sha256:1"]
        {
            assert!(is_database(image), "{image}");
        }
        for image in ["redis:7", "nginx", "rabbitmq:3-management", "mcr.microsoft.com/azure-storage/azurite"] {
            assert!(!is_database(image), "{image}");
        }
    }

    #[test]
    fn an_older_compose_answers_with_an_array_and_failures_are_down() {
        let array = r#"[{"Name":"a-web-1","Service":"web","State":"exited","ExitCode":1},
            {"Name":"a-api-1","Service":"api","State":"running","Health":"unhealthy"},
            {"Name":"a-q-1","Service":"q","State":"restarting"},
            {"Name":"a-w-1","Service":"w","State":"created"}]"#;
        let services = parse(array);
        let states: Vec<State> = services.iter().map(|service| service.state).collect();
        assert_eq!(states, [State::Down, State::Down, State::Starting, State::Down], "api, q, w, web");
        assert_eq!(services[3].detail, "exited (1)");
        assert!(parse("").is_empty() && !is_up(&[]), "nothing running isn't up");
    }

    #[test]
    fn compose_commands_name_the_file_and_the_services() {
        let compose = Compose { file: Some("ops/compose.yaml".into()), services: vec!["db".into(), "cache".into()] };
        assert_eq!(
            compose.args(COMPOSE_PS, &PS_OPTIONS).join(" "),
            "compose --ansi never -f ops/compose.yaml ps --all --format json db cache"
        );
        let all = Compose { file: None, services: Vec::new() };
        assert_eq!(all.args(COMPOSE_STOP, &[]).join(" "), "compose --ansi never stop");
    }
}
