//! Calls between the apps (#51): HttpClient logs every call a .NET app makes ("Start processing HTTP request GET
//! http://localhost:47202/api/invoices", then "End processing HTTP request after 12.9ms - 200"), lazyrunner turns that
//! log on for the apps it starts, and the url's port says which app was called. The npm apps it starts load a Node
//! hook that writes the same for their calls (a dev server's proxy to an API). Until an app's calls are seen, the
//! profile's `dependsOn` and an npm app's own config (its proxy, an environment file naming another app's port)
//! declare them. That's the stack map's edges.

use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use regex::Regex;

/// What `dotnet run` passes an app so HttpClient logs each call it makes.
pub const LOG_CALLS: &str = "--Logging:LogLevel:System.Net.Http.HttpClient=Information";
/// Calls kept, the newest, and for how long.
const KEPT: usize = 2000;
const KEPT_FOR: Duration = Duration::from_secs(600);
/// Calls an app has waiting for their answer, at most; one with none after this long had no answer.
const PENDING: usize = 256;
pub const NO_ANSWER: Duration = Duration::from_secs(30);
/// The edges' figures are over the last minute.
pub const WINDOW: Duration = Duration::from_secs(60);

fn pattern(regex: &str) -> Regex {
    Regex::new(regex).expect("a valid pattern")
}

static START: LazyLock<Regex> =
    LazyLock::new(|| pattern(r"Start processing HTTP request (?P<method>[A-Z]+) (?P<url>\S+)"));
static END: LazyLock<Regex> =
    LazyLock::new(|| pattern(r"End processing HTTP request after (?P<ms>[\d.,]+) ?ms - (?P<status>\w+)"));
/// HttpClient's own scope on its entries: "=> HTTP GET http://localhost:47202/api/invoices".
static SCOPE: LazyLock<Regex> = LazyLock::new(|| pattern(r"=> HTTP (?P<method>[A-Z]+) (?P<url>\S+)"));
/// The Node hook's line for a call: "lazyrunner-call GET http://localhost:5000/api/x 200 34" (its time in ms), or
/// `error` for its status when it had no answer.
static HOOKED: LazyLock<Regex> = LazyLock::new(|| {
    pattern(r"lazyrunner-call (?P<method>[A-Z]+) (?P<url>\S+) (?P<status>\d{3}|error) (?P<ms>\d+)\s*$")
});

/// What the npm apps lazyrunner starts load (`NODE_OPTIONS=--require`): it writes a line for each call the app's
/// Node processes make to a port on this machine - a dev server's proxy to an API - through Node's diagnostics
/// channels.
pub const NODE_HOOK: &str = include_str!("call-hook.cjs");
/// Its file in the state folder.
pub const NODE_HOOK_FILE: &str = "call-hook.cjs";

/// A call one app made.
#[derive(Clone, Debug, PartialEq)]
pub struct Call {
    /// The calling app's key.
    pub from: String,
    pub method: String,
    pub url: String,
    /// When it started.
    pub at: Instant,
    /// How long it took and its status; neither when it had no answer.
    pub ms: Option<f64>,
    pub status: Option<u16>,
}

impl Call {
    /// A server error, or no answer at all.
    pub fn failed(&self) -> bool {
        self.status.is_none_or(|status| status >= 500)
    }
}

/// An app's calls on their way: started, waiting for their end line.
#[derive(Debug, Default)]
pub struct Outgoing {
    pending: VecDeque<(String, String, Instant)>,
    /// The call the next end line belongs to, as its scope line said.
    scope: Option<String>,
    done: Vec<Call>,
}

impl Outgoing {
    /// An entry's scope line (hidden from the column): HttpClient's names the call its message is about.
    pub fn scope_line(&mut self, text: &str) {
        self.scope = SCOPE.captures(text).map(|found| found["url"].to_string());
    }

    /// One of the app's output lines: a call that starts, or the end of one.
    pub fn line(&mut self, from: &str, text: &str, now: Instant) {
        if let Some(found) = START.captures(text) {
            self.scope = None;
            self.pending.push_back((found["method"].to_string(), found["url"].to_string(), now));
            if self.pending.len() > PENDING {
                self.pending.pop_front();
            }
            return;
        }
        let Some(found) = END.captures(text) else { return };
        let scope = self.scope.take();
        let at = scope.and_then(|url| self.pending.iter().position(|(_, pending, _)| *pending == url)).unwrap_or(0);
        let Some((method, url, started)) = self.pending.remove(at) else { return };
        self.done.push(Call {
            from: from.to_string(),
            method,
            url,
            at: started,
            ms: found["ms"].replace(',', ".").parse().ok(),
            status: status(&found["status"]),
        });
    }

    /// A line of the Node hook's, a call that ended: true when it was one (it stays out of the column).
    pub fn hook_line(&mut self, from: &str, text: &str, now: Instant) -> bool {
        if !text.contains("lazyrunner-call ") {
            return false;
        }
        let Some(found) = HOOKED.captures(text) else { return false };
        let ms: f64 = found["ms"].parse().unwrap_or(0.0);
        let answered = &found["status"] != "error";
        self.done.push(Call {
            from: from.to_string(),
            method: found["method"].to_string(),
            url: found["url"].to_string(),
            at: now.checked_sub(Duration::from_secs_f64(ms / 1000.0)).unwrap_or(now),
            ms: answered.then_some(ms),
            status: found["status"].parse().ok(),
        });
        true
    }

    /// The calls that waited too long: they had no answer.
    pub fn expire(&mut self, from: &str, now: Instant) {
        while let Some((method, url, at)) = self.pending.front().filter(|(.., at)| now.duration_since(*at) >= NO_ANSWER)
        {
            let call = Call {
                from: from.to_string(),
                method: method.clone(),
                url: url.clone(),
                at: *at,
                ms: None,
                status: None,
            };
            self.done.push(call);
            self.pending.pop_front();
        }
    }

    /// The calls finished since last asked.
    pub fn take(&mut self) -> Vec<Call> {
        std::mem::take(&mut self.done)
    }

    /// Forgets what's on its way (the app stopped).
    pub fn clear(&mut self) {
        self.pending.clear();
        self.scope = None;
    }
}

/// A status as HttpClient writes it: `200`, or by name on older .NETs (`OK`).
fn status(text: &str) -> Option<u16> {
    if let Ok(code) = text.parse() {
        return Some(code);
    }
    let named = [
        ("OK", 200),
        ("Created", 201),
        ("Accepted", 202),
        ("NoContent", 204),
        ("MovedPermanently", 301),
        ("Found", 302),
        ("NotModified", 304),
        ("BadRequest", 400),
        ("Unauthorized", 401),
        ("Forbidden", 403),
        ("NotFound", 404),
        ("Conflict", 409),
        ("UnprocessableEntity", 422),
        ("TooManyRequests", 429),
        ("InternalServerError", 500),
        ("NotImplemented", 501),
        ("BadGateway", 502),
        ("ServiceUnavailable", 503),
        ("GatewayTimeout", 504),
    ];
    named.iter().find(|(name, _)| *name == text).map(|(_, code)| *code)
}

/// The port of a url on this machine (`localhost`, `127.0.0.1`, `[::1]`); `None` for anywhere else.
pub fn local_port(url: &str) -> Option<u16> {
    let (scheme, rest) = url.split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority.rsplit('@').next()?;
    let (host, port) = match authority.rsplit_once(':').filter(|(host, _)| !host.ends_with(':')) {
        Some((host, port)) if !port.contains(']') => (host, port.parse().ok()?),
        _ => (authority, if scheme.eq_ignore_ascii_case("https") { 443 } else { 80 }),
    };
    let host = host.to_lowercase();
    ["localhost", "127.0.0.1", "[::1]", "0.0.0.0"].contains(&host.as_str()).then_some(port)
}

/// The calls the apps made, the newest kept.
#[derive(Debug, Default)]
pub struct Calls {
    pub items: VecDeque<Call>,
}

impl Calls {
    pub fn add(&mut self, calls: Vec<Call>, now: Instant) {
        self.items.extend(calls);
        while self.items.len() > KEPT || self.items.front().is_some_and(|call| now.duration_since(call.at) > KEPT_FOR) {
            self.items.pop_front();
        }
    }
}

/// One app calling another: where that's declared, and what it did over the last minute.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Edge {
    pub from: String,
    pub to: String,
    /// Where it's declared: `dependsOn`, or the caller's config that names the other's port (`proxy.conf.json`).
    pub declared_in: Option<String>,
    /// Its figures are the requests the called app served: the caller can't log its calls (a browser app, through
    /// its dev server's proxy).
    pub served: bool,
    /// Calls seen, in the calls kept.
    pub seen: usize,
    /// Over the last minute: calls, per second, the 95th percentile of their times, and the failed ones.
    pub calls: usize,
    pub per_second: f64,
    pub p95_ms: Option<f64>,
    pub failed: usize,
    /// When the last failed call started.
    pub last_failed: Option<Instant>,
}

impl Edge {
    /// A failure in the last minute.
    pub fn failing(&self) -> bool {
        self.failed > 0
    }
}

/// The edges between the apps: `ports` is each app's key and port, `declared` the pairs declared and where (the
/// first of a pair counts). A call to a port no app has isn't an edge.
pub fn edges(
    calls: &Calls,
    ports: &[(String, Option<u16>)],
    declared: &[(String, String, String)],
    now: Instant,
) -> Vec<Edge> {
    let by_port: HashMap<u16, &str> = ports.iter().filter_map(|(key, port)| Some(((*port)?, key.as_str()))).collect();
    let mut edges: Vec<Edge> = Vec::new();
    for (from, to, place) in declared {
        if from != to && !edges.iter().any(|edge| edge.from == *from && edge.to == *to) {
            let declared_in = Some(place.clone());
            edges.push(Edge { from: from.clone(), to: to.clone(), declared_in, ..Edge::default() });
        }
    }
    let mut times: HashMap<(String, String), Vec<f64>> = HashMap::new();
    for call in &calls.items {
        let Some(to) = local_port(&call.url).and_then(|port| by_port.get(&port)) else { continue };
        if *to == call.from {
            continue;
        }
        let at = match edges.iter().position(|edge| edge.from == call.from && edge.to == *to) {
            Some(at) => at,
            None => {
                edges.push(Edge { from: call.from.clone(), to: (*to).to_string(), ..Edge::default() });
                edges.len() - 1
            }
        };
        let edge = &mut edges[at];
        edge.seen += 1;
        if call.failed() {
            edge.last_failed = Some(edge.last_failed.map_or(call.at, |last| last.max(call.at)));
        }
        if now.saturating_duration_since(call.at) > WINDOW {
            continue;
        }
        edge.calls += 1;
        edge.failed += usize::from(call.failed());
        if let Some(ms) = call.ms {
            times.entry((edge.from.clone(), edge.to.clone())).or_default().push(ms);
        }
    }
    for edge in &mut edges {
        edge.per_second = edge.calls as f64 / WINDOW.as_secs_f64();
        edge.p95_ms = times.get_mut(&(edge.from.clone(), edge.to.clone())).and_then(|times| p95(times));
    }
    edges
}

/// The 95th percentile of some times.
pub fn p95(times: &mut [f64]) -> Option<f64> {
    if times.is_empty() {
        return None;
    }
    times.sort_by(f64::total_cmp);
    let at = ((times.len() as f64 * 0.95).ceil() as usize).clamp(1, times.len()) - 1;
    Some(times[at])
}

/// A port on this machine in a config: `http://localhost:5000`, `127.0.0.1:5000`.
static PORT: LazyLock<Regex> =
    LazyLock::new(|| pattern(r"(?i)\b(?:localhost|127\.0\.0\.1|0\.0\.0\.0|\[::1\]):(?P<port>\d{2,5})\b"));
/// Folders a config isn't in.
const NOT_CONFIG: [&str; 12] =
    ["node_modules", "dist", "bin", "obj", ".angular", ".git", "build", "coverage", ".next", "out", "wwwroot", ".nx"];
/// How deep under an app's folder its config is looked for, and the biggest file read.
const CONFIG_DEPTH: usize = 5;
const CONFIG_BYTES: u64 = 256 * 1024;

/// A file that says where an npm app's calls go: a dev server's proxy, a bundler's config, an environment file.
fn is_config(name: &str) -> bool {
    let name = name.to_lowercase();
    let script = [".json", ".js", ".cjs", ".mjs", ".ts"].iter().any(|ending| name.ends_with(ending));
    (name.starts_with("proxy") && script)
        || ["vite.config.", "webpack.config.", "vue.config.", "next.config."]
            .iter()
            .any(|start| name.starts_with(start))
        || name == ".env"
        || name.starts_with(".env.")
        || (name.starts_with("environment") && name.ends_with(".ts"))
}

/// The ports on this machine an app's config names, each with the first file that does (by its name).
pub fn configured(dir: &Path) -> Vec<(u16, String)> {
    fn look(dir: &Path, depth: usize, found: &mut Vec<(u16, String)>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if path.is_dir() {
                if depth < CONFIG_DEPTH && !NOT_CONFIG.iter().any(|skip| name.eq_ignore_ascii_case(skip)) {
                    look(&path, depth + 1, found);
                }
                continue;
            }
            let small = entry.metadata().is_ok_and(|meta| meta.len() <= CONFIG_BYTES);
            let Some(text) = (small && is_config(&name)).then(|| std::fs::read_to_string(&path).ok()).flatten() else {
                continue;
            };
            for port in PORT.captures_iter(&text).filter_map(|found| found["port"].parse().ok()) {
                if !found.iter().any(|(known, _)| *known == port) {
                    found.push((port, name.clone()));
                }
            }
        }
    }
    let mut found = Vec::new();
    look(dir, 0, &mut found);
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_call_is_its_start_line_and_its_end_line_matched_by_its_scope() {
        let now = Instant::now();
        let mut outgoing = Outgoing::default();
        outgoing.line("orders", "      Start processing HTTP request GET http://localhost:47202/api/invoices", now);
        outgoing.line("orders", "      Start processing HTTP request POST http://localhost:47203/api/mail", now);
        outgoing.scope_line("      => SpanId:1 => HTTP POST http://localhost:47203/api/mail");
        outgoing.line("orders", "      End processing HTTP request after 12.9056ms - 202", now);
        outgoing.line("orders", "      End processing HTTP request after 3,5ms - InternalServerError", now);
        let calls = outgoing.take();
        assert_eq!(calls.len(), 2);
        assert_eq!(
            (calls[0].url.as_str(), calls[0].status, calls[0].ms),
            ("http://localhost:47203/api/mail", Some(202), Some(12.9056))
        );
        assert_eq!(
            (calls[1].url.as_str(), calls[1].status, calls[1].ms),
            ("http://localhost:47202/api/invoices", Some(500), Some(3.5))
        );
        assert!(calls[1].failed() && !calls[0].failed());

        outgoing.line("orders", "Start processing HTTP request GET http://localhost:47202/api/slow", now);
        outgoing.expire("orders", now + Duration::from_secs(5));
        assert!(outgoing.take().is_empty(), "still waiting");
        outgoing.expire("orders", now + NO_ANSWER);
        let lost = outgoing.take();
        assert!(lost.len() == 1 && lost[0].status.is_none() && lost[0].failed(), "no answer: {lost:?}");
    }

    #[test]
    fn an_apps_config_names_the_ports_its_calls_go_to() {
        use crate::testing::{TempDir, write};
        let temp = TempDir::new("calls-configured");
        let app = temp.join("shop");
        write(app.join("proxy.conf.json"), r#"{ "/api": { "target": "http://localhost:47201", "secure": false } }"#);
        write(
            app.join("src/environments/environment.ts"),
            "export const environment = { admin: 'http://127.0.0.1:47202/' };",
        );
        write(app.join("node_modules/lib/proxy.conf.json"), r#"{ "target": "http://localhost:9999" }"#);
        write(app.join("README.md"), "Runs on http://localhost:4200");
        let mut found = configured(&app);
        found.sort();
        assert_eq!(found, [(47201, "proxy.conf.json".to_string()), (47202, "environment.ts".to_string())]);
    }

    #[test]
    fn the_node_hooks_lines_are_calls() {
        let now = Instant::now();
        let mut outgoing = Outgoing::default();
        assert!(outgoing.hook_line("spa", "lazyrunner-call GET http://localhost:5000/api/meetings?x=1 200 34", now));
        assert!(outgoing.hook_line("spa", "lazyrunner-call POST http://localhost:5000/api/x error 2", now));
        assert!(!outgoing.hook_line("spa", "Compiled successfully.", now));
        let calls = outgoing.take();
        assert_eq!((calls[0].status, calls[0].ms, calls[0].from.as_str()), (Some(200), Some(34.0), "spa"));
        assert!(calls[1].failed() && calls[1].ms.is_none(), "no answer");
        assert_eq!(local_port(&calls[0].url), Some(5000));
    }

    #[test]
    fn a_url_on_this_machine_has_a_port() {
        assert_eq!(local_port("http://localhost:47202/api/x?y=1"), Some(47202));
        assert_eq!(local_port("https://127.0.0.1/api"), Some(443));
        assert_eq!(local_port("http://[::1]:5000/"), Some(5000));
        assert_eq!(local_port("https://api.example.com:8443/x"), None);
    }

    #[test]
    fn the_edges_are_the_calls_by_the_port_they_went_to_and_what_the_profile_declares() {
        let now = Instant::now();
        let call = |from: &str, url: &str, ms: Option<f64>, status: Option<u16>, ago: u64| Call {
            from: from.into(),
            method: "GET".into(),
            url: url.into(),
            at: now - Duration::from_secs(ago),
            ms,
            status,
        };
        let mut calls = Calls::default();
        calls.add(
            vec![
                call("orders", "http://localhost:47202/a", Some(10.0), Some(200), 5),
                call("orders", "http://localhost:47202/b", Some(30.0), Some(200), 4),
                call("orders", "http://localhost:47202/c", None, None, 3),
                call("orders", "http://localhost:47202/d", Some(20.0), Some(200), 120),
                call("orders", "http://localhost:9999/elsewhere", Some(1.0), Some(200), 1),
                call("orders", "http://localhost:47201/self", Some(1.0), Some(200), 1),
            ],
            now,
        );
        let ports =
            [("orders".to_string(), Some(47201)), ("billing".to_string(), Some(47202)), ("shop".to_string(), None)];
        let declared = [
            ("shop".to_string(), "orders".to_string(), "dependsOn".to_string()),
            ("shop".to_string(), "orders".to_string(), "proxy.conf.json".to_string()),
        ];
        let edges = edges(&calls, &ports, &declared, now);
        assert_eq!(edges.len(), 2, "{edges:?}");
        assert_eq!((edges[0].declared_in.as_deref(), edges[0].seen), (Some("dependsOn"), 0), "declared, not seen yet");
        let billing = &edges[1];
        assert_eq!(
            (billing.from.as_str(), billing.to.as_str(), billing.seen, billing.calls),
            ("orders", "billing", 4, 3)
        );
        assert_eq!((billing.failed, billing.p95_ms), (1, Some(30.0)));
        assert!(billing.failing() && (billing.per_second - 0.05).abs() < 1e-9);
    }
}
