//! Compose apps (#56) in the controller: their services polled while they run - which makes them up, and what their
//! title shows - and stopped when the app stops, a start of it waiting for that stop.

use std::collections::HashMap;
use std::process::Child;
use std::time::{Duration, Instant};

use super::Controller;
use crate::output::Hue;
use crate::run::Event;
use crate::run::compose::{self, Service, State};

/// How often a running compose app's services are asked for.
const POLL_EVERY: Duration = Duration::from_secs(2);

/// The compose apps' services, the polls on their way, and the stops still running.
#[derive(Default)]
pub struct Composed {
    /// Each app's services as last polled, with the run (generation) they're of.
    services: HashMap<String, (u64, Vec<Service>)>,
    polling: Vec<String>,
    /// The run whose poll failed, said once.
    failed: HashMap<String, u64>,
    due: Option<Instant>,
    stopping: Vec<(String, Child)>,
    /// The ports each app's services publish, as its compose file says; the apps asked about.
    ports: HashMap<String, Vec<u16>>,
    asked: Vec<String>,
}

impl Composed {
    /// The stops still running (those done are let go).
    fn stops_running(&mut self) -> Vec<String> {
        self.stopping.retain_mut(|(_, child)| matches!(child.try_wait(), Ok(None)));
        self.stopping.iter().map(|(key, _)| key.clone()).collect()
    }
}

impl Controller {
    /// A running compose app's services as last polled.
    pub fn compose_services(&self, key: &str) -> Option<&[Service]> {
        let index = self.index(key)?;
        let (generation, services) = self.composed.services.get(key)?;
        (*generation == self.states[index].generation && self.states[index].is_running()).then_some(services.as_slice())
    }

    /// The compose app running a database service (postgres, SQL Server...): its index and that service.
    pub fn database_service(&self) -> Option<(usize, &Service)> {
        self.apps.iter().enumerate().find_map(|(index, app)| {
            let services = self.compose_services(&app.key)?;
            Some((index, services.iter().find(|service| compose::is_database(&service.image))?))
        })
    }

    /// Whether an app is a compose app.
    pub fn is_compose(&self, key: &str) -> bool {
        self.app(key).and_then(|app| app.command()).is_some_and(|command| command.compose.is_some())
    }

    /// The ports an app needs: its own, or a compose app's services' (as its compose file says).
    pub fn app_ports(&self, key: &str) -> Vec<u16> {
        match self.composed.ports.get(key) {
            Some(ports) => ports.clone(),
            None => self.port(key).into_iter().collect(),
        }
    }

    /// Whether a compose app's port is held by Docker itself (its own services, running from before): not another's.
    pub fn docker_holds(&self, key: &str, port: u16) -> bool {
        self.is_compose(key)
            && self.listening.ports.get(&port).is_some_and(|holder| compose::is_docker_process(&holder.name))
    }

    /// The env a compose app's docker compose gets: its profile's, then this machine's overrides (the start sheet).
    fn compose_env(&self, app: &crate::profile::App) -> Vec<(String, String)> {
        let overrides = self.env_overrides(&app.key).iter().map(|found| (found.name.clone(), found.value.clone()));
        app.env.iter().cloned().chain(overrides).collect()
    }

    /// The ports a compose file says its services publish, once read.
    pub(super) fn compose_ports_arrived(&mut self, key: String, ports: Result<Vec<u16>, String>) {
        let ports = ports.unwrap_or_else(|error| {
            self.pane(&key, &format!("docker compose config: {error}"), Hue::DarkGray);
            Vec::new()
        });
        self.composed.ports.insert(key, ports);
    }

    /// Whether a compose app's services are up.
    pub(super) fn compose_up(&self, key: &str) -> bool {
        self.compose_services(key).is_some_and(compose::is_up)
    }

    /// Asks for the running compose apps' services, every [`POLL_EVERY`], in the background.
    pub(super) fn tick_compose(&mut self, now: Instant) {
        if self.composed.due.is_some_and(|due| now < due) {
            return;
        }
        self.composed.due = Some(now + POLL_EVERY);
        for app in &self.apps {
            let Some(setup) = app.command().and_then(|command| command.compose.clone()) else { continue };
            if self.composed.asked.contains(&app.key) {
                continue;
            }
            self.composed.asked.push(app.key.clone());
            let (key, events, env) = (app.key.clone(), self.events.clone(), self.compose_env(app));
            let dir = self.root.join(app.command().map_or("", |command| command.cwd.as_str()));
            tokio::task::spawn_blocking(move || {
                let ports = compose::config_ports(&dir, &setup, &env);
                let _ = events.send(Event::ComposePorts { key, ports });
            });
        }
        for (index, app) in self.apps.iter().enumerate() {
            let Some(command) = app.command() else { continue };
            let Some(setup) = command.compose.clone() else { continue };
            if !self.states[index].is_running() || self.composed.polling.contains(&app.key) {
                continue;
            }
            self.composed.polling.push(app.key.clone());
            let (key, generation, events) = (app.key.clone(), self.states[index].generation, self.events.clone());
            let (dir, env) = (self.root.join(&command.cwd), self.compose_env(app));
            tokio::task::spawn_blocking(move || {
                let services = compose::ps(&dir, &setup, &env);
                let _ = events.send(Event::ComposeServices { key, generation, services });
            });
        }
    }

    /// A poll's answer: the services kept, a change of one said in the app's column, and why a poll failed said once.
    pub(super) fn compose_arrived(&mut self, key: String, generation: u64, services: Result<Vec<Service>, String>) {
        self.composed.polling.retain(|polling| *polling != key);
        if self.state(&key).is_none_or(|state| state.generation != generation || !state.is_running()) {
            return;
        }
        let services = match services {
            Ok(services) => services,
            Err(error) => {
                if self.composed.failed.insert(key.clone(), generation) != Some(generation) {
                    self.pane(&key, &format!("docker compose ps: {error}"), Hue::DarkGray);
                }
                return;
            }
        };
        let before = self.compose_services(&key).map(<[Service]>::to_vec);
        if let Some(before) = before {
            for service in &services {
                let was = before.iter().find(|old| old.name == service.name).map(|old| old.state);
                if was.is_some_and(|was| was != service.state) {
                    let line = format!("{} {}: {}", symbol(service.state), service.name, service.detail);
                    self.pane(&key, &line, hue(service.state));
                }
            }
        }
        self.composed.services.insert(key, (generation, services));
        self.watch_states();
    }

    /// Stops a compose app's services - its `up` only attached to them.
    pub(super) fn stop_compose(&mut self, key: &str) {
        let Some(app) = self.app(key) else { return };
        let Some(command) = app.command() else { return };
        let Some(setup) = &command.compose else { return };
        let stopped = compose::stop(&self.root.join(&command.cwd), setup, &self.compose_env(app));
        self.composed.services.remove(key);
        match stopped {
            Ok(child) => self.composed.stopping.push((key.to_string(), child)),
            Err(error) => self.pane(key, &format!("✗ docker compose stop: {error}"), Hue::Red),
        }
    }

    /// A start of compose apps still being stopped waits on the tick: their `up` would attach to services on their
    /// way down.
    pub(super) fn wait_for_compose_stops(&mut self, only: Option<&[String]>, force_build: bool) -> bool {
        let waiting: Vec<String> = self
            .composed
            .stops_running()
            .into_iter()
            .filter(|key| only.is_none_or(|only| only.contains(key)))
            .collect();
        if waiting.is_empty() {
            return false;
        }
        if !self.retrying_start {
            self.add_log(format!("Waiting for {} to stop…", self.names(&waiting)), Hue::DarkGray);
        }
        self.park_start(only, force_build);
        true
    }
}

/// How a service shows: in the title, and in a line when it changes.
pub fn symbol(state: State) -> &'static str {
    match state {
        State::Up => "●",
        State::Starting => "◌",
        State::Down => "✗",
        State::Done => "✓",
    }
}

pub fn hue(state: State) -> Hue {
    match state {
        State::Up => Hue::Green,
        State::Starting => Hue::Yellow,
        State::Down => Hue::Red,
        State::Done => Hue::DarkGray,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tokio::sync::mpsc;

    use super::*;
    use crate::paths::Paths;
    use crate::profile::{ProfileDirs, import};
    use crate::run::app::Status;
    use crate::run::controller::Setup;
    use crate::testing::TempDir;

    /// A controller with one compose app, Infra (its db and cache), running its first run - no Docker needed.
    fn running(temp: &TempDir) -> Controller {
        let repo = temp.join("repo");
        crate::testing::write(repo.join("compose.yaml"), "services: {}\n");
        let profile = json!({
            "schemaVersion": 1, "id": "infra", "name": "Infra", "detect": { "anyFile": ["compose.yaml"] },
            "watch": ["."], "apps": [{ "key": "infra", "name": "Infra", "type": "compose", "services": ["db", "cache"] }]
        });
        let file = temp.join("profile.json");
        crate::config::write(&file, &profile).unwrap();
        let profiles =
            ProfileDirs { user: temp.join("profiles"), bundled: temp.join("bundled"), only: Some(file.clone()) };
        let paths = Paths { config_file: temp.join("config.json"), state_dir: temp.join("state"), profiles };
        crate::config::write(&paths.config_file, &json!({})).unwrap();
        let project = import(&file, &paths.profiles).unwrap();
        let (events, _) = mpsc::unbounded_channel();
        let mut controller = Controller::new(Setup { project, root: repo, paths, events, guard: None }).unwrap();
        let now = (controller.now)();
        let state = &mut controller.states[0];
        (state.status, state.generation, state.started_at) = (Status::Running, 1, Some(now));
        controller
    }

    fn service(name: &str, state: State, detail: &str) -> Service {
        Service { name: name.into(), state, detail: detail.into(), ..Service::default() }
    }

    fn said(controller: &Controller) -> Vec<String> {
        controller.states[0].output.newest_first(0).map(|line| line.text.clone()).collect()
    }

    #[tokio::test]
    async fn a_compose_app_is_up_once_its_services_are_and_says_when_one_changes() {
        let temp = TempDir::new("compose-up");
        let mut controller = running(&temp);
        let starting =
            vec![service("cache", State::Starting, "running (starting)"), service("db", State::Up, "running")];
        controller.compose_arrived("infra".into(), 1, Ok(starting));
        assert!(!controller.states[0].is_up(), "the cache isn't healthy yet");
        assert_eq!(controller.compose_services("infra").map(<[Service]>::len), Some(2));
        let up = vec![service("cache", State::Up, "running (healthy)"), service("db", State::Up, "running")];
        controller.compose_arrived("infra".into(), 1, Ok(up));
        assert!(controller.states[0].is_up(), "{:?}", said(&controller));
        assert!(said(&controller).contains(&"● cache: running (healthy)".to_string()), "{:?}", said(&controller));
        assert!(!said(&controller).iter().any(|line| line.contains("db:")), "db didn't change");
        controller.compose_arrived("infra".into(), 0, Ok(Vec::new()));
        assert_eq!(controller.compose_services("infra").map(<[Service]>::len), Some(2), "an earlier run's is dropped");
        for _ in 0..2 {
            controller.compose_arrived("infra".into(), 1, Err("Cannot connect to the Docker daemon".into()));
        }
        let failed = said(&controller).iter().filter(|line| line.contains("docker compose ps: Cannot connect")).count();
        assert_eq!(failed, 1, "said once a run");
    }

    #[tokio::test]
    async fn a_port_its_services_publish_held_by_another_program_is_a_conflict() {
        let temp = TempDir::new("compose-ports");
        let mut controller = running(&temp);
        controller.states[0].status = Status::Stopped;
        let held = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = held.local_addr().unwrap().port();
        controller.compose_ports_arrived("infra".into(), Ok(vec![port]));
        assert_eq!(controller.app_ports("infra"), [port]);
        controller.start(None, false);
        let log: Vec<String> = controller.log.iter().map(|entry| entry.text.clone()).collect();
        assert!(controller.conflicts.iter().any(|conflict| conflict.port == port), "{log:?}");
        assert!(!controller.is_running("infra"), "it doesn't start onto a taken port");
    }

    #[tokio::test]
    async fn a_start_waits_while_its_services_are_still_stopping() {
        let temp = TempDir::new("compose-stop");
        let mut controller = running(&temp);
        controller.states[0].status = Status::Stopped;
        let (program, args) =
            if cfg!(windows) { ("cmd", ["/C", "ping -n 30 127.0.0.1 >nul"]) } else { ("sh", ["-c", "sleep 30"]) };
        let stop = std::process::Command::new(program).args(args).spawn().unwrap();
        controller.composed.stopping.push(("infra".into(), stop));
        controller.start(None, false);
        assert!(controller.waiting_start.is_some() && !controller.is_running("infra"), "it waits");
        let log = controller.log.back().map(|entry| entry.text.clone()).unwrap_or_default();
        assert_eq!(log, "Waiting for Infra to stop…");
        let (_, mut stop) = controller.composed.stopping.pop().unwrap();
        stop.kill().unwrap();
        stop.wait().unwrap();
        assert!(!controller.wait_for_compose_stops(None, false), "not once it's stopped");
    }
}
