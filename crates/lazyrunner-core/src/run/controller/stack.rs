//! The stack map's data (K, #51): the edges between the apps - the calls they made by the port they went to (a .NET
//! app's HttpClient log, an npm app's Node hook), what the profile's `dependsOn` and an npm app's own config declare,
//! and for a caller whose calls aren't seen what the called app served - and the apps that use the database, with how
//! many commands they ran on it.

use std::ffi::OsString;
use std::time::Instant;

use super::Controller;
use crate::profile::AppKind;
use crate::run::calls::{self, Edge, WINDOW};
use crate::run::requests::Request;
use crate::run::tools::ServerCommand;

const NODE_OPTIONS: &str = "NODE_OPTIONS";
/// Packages that say a .NET app talks to a database.
const DATABASE_PACKAGES: [&str; 6] = [
    "EntityFrameworkCore",
    "Microsoft.Data.SqlClient",
    "System.Data.SqlClient",
    "Npgsql",
    "MySqlConnector",
    "Oracle.ManagedDataAccess",
];

/// Whether a project, or one it references, references a database's packages.
fn uses_database(project: &std::path::Path) -> bool {
    crate::run::build::projects(project).iter().any(|file| {
        std::fs::read_to_string(file).is_ok_and(|text| DATABASE_PACKAGES.iter().any(|name| text.contains(name)))
    })
}

/// An app's last minute of requests served, or calls made: how many, their 95th percentile, the failed ones (a server
/// error, or no answer), and the slowest with its time.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Minute {
    pub count: usize,
    pub p95_ms: Option<f64>,
    pub failed: usize,
    pub slowest: Option<(String, f64)>,
}

/// The database's last minute: its commands, their time together, the slowest, and the requests that look like an
/// N+1.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Database {
    pub commands: usize,
    pub total_ms: f64,
    pub slowest_ms: Option<f64>,
    pub repeated: usize,
}

impl Controller {
    /// An npm app starts with the Node hook that writes a line for each call it makes (`--require` added to its
    /// `NODE_OPTIONS`); as it was when the hook's file can't be written.
    pub(super) fn with_call_hook(&self, key: &str, mut command: ServerCommand) -> ServerCommand {
        if !matches!(self.app(key).map(|app| &app.kind), Some(AppKind::Npm { .. })) {
            return command;
        }
        let file = self.state_dir.join(calls::NODE_HOOK_FILE);
        let current = std::fs::read_to_string(&file).is_ok_and(|text| text == calls::NODE_HOOK);
        let written = current
            || std::fs::create_dir_all(&self.state_dir).and_then(|()| std::fs::write(&file, calls::NODE_HOOK)).is_ok();
        if !written {
            return command;
        }
        let name = OsString::from(NODE_OPTIONS);
        let before = command
            .spec
            .env
            .iter()
            .rev()
            .find(|(each, _)| each.eq_ignore_ascii_case(&name))
            .map(|(_, value)| value.clone())
            .or_else(|| std::env::var_os(NODE_OPTIONS))
            .filter(|value| !value.is_empty());
        // Forward slashes: NODE_OPTIONS reads a backslash in quotes as an escape.
        let require = format!("--require \"{}\"", file.to_string_lossy().replace('\\', "/"));
        let value = before.map_or_else(|| require.clone(), |before| format!("{} {require}", before.to_string_lossy()));
        command.spec.env.retain(|(each, _)| !each.eq_ignore_ascii_case(&name));
        command.spec.env.push((name, value.into()));
        command
    }

    /// Reads each npm app's config for the other apps' ports it names, and each .NET app's projects for a database's
    /// packages (as the stack map opens, as the agent asks).
    pub fn find_configured_calls(&mut self) {
        self.database_apps = self
            .apps
            .iter()
            .filter(|app| match &app.kind {
                AppKind::Dotnet { project, .. } => uses_database(&self.root.join(project)),
                AppKind::Npm { .. } | AppKind::Command(_) => false,
            })
            .map(|app| app.key.clone())
            .collect();
        let ports: Vec<(String, u16)> =
            self.apps.iter().filter_map(|app| Some((app.key.clone(), self.port(&app.key)?))).collect();
        let mut found = Vec::new();
        for app in &self.apps {
            // A .NET app's calls are in its log; its settings name ports for other reasons too (links, CORS).
            let AppKind::Npm { dir, .. } = &app.kind else { continue };
            let dir = self.root.join(dir);
            for (port, file) in calls::configured(&dir) {
                if let Some((to, _)) = ports.iter().find(|(key, known)| *known == port && *key != app.key) {
                    found.push((app.key.clone(), to.clone(), file));
                }
            }
        }
        self.configured_calls = found;
    }

    /// Who calls whom: declared first (`dependsOn`, then the apps' config), then as the calls came.
    pub fn stack_edges(&self) -> Vec<Edge> {
        let ports: Vec<(String, Option<u16>)> =
            self.apps.iter().map(|app| (app.key.clone(), self.port(&app.key))).collect();
        let key_of =
            |wanted: &str| self.apps.iter().find(|app| app.key.eq_ignore_ascii_case(wanted)).map(|app| &app.key);
        let mut declared: Vec<(String, String, String)> = self
            .apps
            .iter()
            .flat_map(|app| {
                let to = app.depends_on.iter().filter_map(|to| key_of(to));
                to.map(|to| (app.key.clone(), to.clone(), "dependsOn".to_string()))
            })
            .collect();
        declared.extend(self.configured_calls.iter().cloned());
        let mut edges = calls::edges(&self.calls, &ports, &declared, (self.now)());
        for edge in edges.iter_mut().filter(|edge| edge.seen == 0 && !self.logs_calls(&edge.from)) {
            self.served(edge);
        }
        edges
    }

    /// Whether an app's calls are in its log: a .NET app's are.
    fn logs_calls(&self, key: &str) -> bool {
        self.app(key).is_some_and(|app| matches!(app.kind, AppKind::Dotnet { .. }))
    }

    /// The requests an app served over the last minute, each with how long ago.
    fn served_recently(&self, key: &str) -> Vec<(&Request, std::time::Duration)> {
        let now = chrono::Local::now();
        self.requests
            .items
            .iter()
            .filter(|request| request.app == key)
            .filter_map(|request| Some((request, (now - request.at).to_std().ok()?)))
            .filter(|(_, age)| *age <= WINDOW)
            .collect()
    }

    /// What an app served over the last minute: how many requests, their 95th percentile, the server errors among
    /// them, and the slowest ("GET /api/orders/{id}" and its time).
    pub fn served_last_minute(&self, key: &str) -> Minute {
        let recent = self.served_recently(key);
        let mut times: Vec<f64> = recent.iter().map(|(request, _)| request.ms).collect();
        let slowest = recent
            .iter()
            .max_by(|a, b| a.0.ms.total_cmp(&b.0.ms))
            .map(|(request, _)| (format!("{} {}", request.method, request.endpoint), request.ms));
        Minute {
            count: recent.len(),
            p95_ms: calls::p95(&mut times),
            failed: recent.iter().filter(|(request, _)| request.status >= 500).count(),
            slowest,
        }
    }

    /// The calls an app made to the others over the last minute (an npm app's through its Node hook): how many, their
    /// 95th percentile, and the failed ones.
    pub fn calls_last_minute(&self, key: &str) -> Minute {
        let now = (self.now)();
        let recent: Vec<&calls::Call> = self
            .calls
            .items
            .iter()
            .filter(|call| call.from == key && now.saturating_duration_since(call.at) <= WINDOW)
            .collect();
        let mut times: Vec<f64> = recent.iter().filter_map(|call| call.ms).collect();
        Minute {
            count: recent.len(),
            p95_ms: calls::p95(&mut times),
            failed: recent.iter().filter(|call| call.failed()).count(),
            slowest: None,
        }
    }

    /// The database over the last minute, all its users together: its commands, their time, the slowest, and the
    /// requests that ran one query over and over (an N+1, it looks like).
    pub fn database_minute(&self) -> Database {
        let summaries: Vec<&crate::run::requests::DbSummary> = self
            .database_users()
            .iter()
            .flat_map(|key| self.served_recently(key))
            .filter_map(|(request, _)| request.db.as_ref())
            .collect();
        Database {
            commands: summaries.iter().map(|db| db.commands).sum(),
            total_ms: summaries.iter().map(|db| db.total_ms).sum(),
            slowest_ms: summaries.iter().map(|db| db.slowest_ms).max_by(f64::total_cmp),
            repeated: summaries.iter().filter(|db| db.repeated.is_some()).count(),
        }
    }

    /// For a caller whose calls aren't seen: what the called app served, as the edge's figures.
    fn served(&self, edge: &mut Edge) {
        let all = self.requests.items.iter().filter(|request| request.app == edge.to).count();
        if all == 0 {
            return;
        }
        let recent = self.served_recently(&edge.to);
        let mut times: Vec<f64> = recent.iter().map(|(request, _)| request.ms).collect();
        let failed: Vec<std::time::Duration> =
            recent.iter().filter(|(request, _)| request.status >= 500).map(|(_, age)| *age).collect();
        let now = Instant::now();
        edge.served = true;
        edge.seen = all;
        edge.calls = recent.len();
        edge.per_second = recent.len() as f64 / WINDOW.as_secs_f64();
        edge.p95_ms = calls::p95(&mut times);
        edge.failed = failed.len();
        edge.last_failed = failed.iter().min().and_then(|age| now.checked_sub(*age));
    }

    /// The apps that use the database: their projects reference its packages, they have migrations, or their
    /// requests ran database commands.
    pub fn database_users(&self) -> Vec<String> {
        self.apps
            .iter()
            .filter(|app| {
                app.migrations.is_some() || self.database_apps.contains(&app.key) || self.database_seen(&app.key)
            })
            .map(|app| app.key.clone())
            .collect()
    }

    /// Whether an app's requests ran database commands, in the requests kept.
    pub fn database_seen(&self, key: &str) -> bool {
        self.requests.items.iter().any(|request| request.app == key && request.db.is_some())
    }

    /// The database commands an app's requests ran over the last minute.
    pub fn database_commands(&self, key: &str) -> usize {
        self.served_recently(key).iter().filter_map(|(request, _)| request.db.as_ref()).map(|db| db.commands).sum()
    }

    /// The calls that waited too long for an answer had none.
    pub(super) fn expire_calls(&mut self, now: Instant) {
        let mut ended = Vec::new();
        for state in &mut self.states {
            state.outgoing.expire(&state.key, now);
            ended.extend(state.outgoing.take());
        }
        if !ended.is_empty() {
            self.calls.add(ended, now);
        }
    }
}
