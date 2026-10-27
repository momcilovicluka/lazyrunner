//! `lazyrunner mcp`: an MCP server on stdin and stdout (JSON-RPC 2.0, one message per line) for an agent such as
//! Claude Code. It keeps nothing of its own: each tool asks the open lazyrunner over its control channel
//! (lazyrunner_ui::agent), so what the agent does happens in the lazyrunner the user watches. stdout carries only the
//! protocol.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::io;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use lazyrunner_core::changelog::VERSION;
use lazyrunner_core::ipc;
use lazyrunner_core::paths::Paths;
use lazyrunner_ui::agent::PROTOCOL;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc::{self, UnboundedSender};
use tokio::task::{AbortHandle, JoinSet};

use crate::profile_cli::install_dir;

/// The MCP versions this server speaks, newest first; a client asking for another gets the newest.
const MCP_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];
/// How long lazyrunner has to answer one request (its UI paused by a selection in its window doesn't answer).
const ASK_TIMEOUT: Duration = Duration::from_secs(10);
const WAIT_POLL: Duration = Duration::from_millis(500);
const WAIT_DEFAULT_SECS: u64 = 120;
const WAIT_MAX_SECS: u64 = 600;
/// What a tool answers at most; more is cut, with a note to ask for less.
const MAX_ANSWER_CHARS: usize = 60_000;
const WAITS: [&str; 4] = ["ready", "build_done", "tests_done", "migrations_checked"];
const NOT_OPEN: &str = "not_running: lazyrunner isn't open - ask the user to open it in the project's folder (run \
                        lazyrunner in a terminal there), then try again.";
const ELEVATED: &str = "elevated: lazyrunner runs as Administrator and this agent doesn't, and Windows doesn't let a \
                        program that isn't elevated drive one that is - ask the user to quit lazyrunner (q) and open \
                        it again in a terminal that isn't \"Run as administrator\"";
const NOT_ANSWERING: &str = "not_answering: lazyrunner didn't answer in 10 s - it may be paused (a selection in its \
                             window); ask the user to look at it";
const INSTRUCTIONS: &str = "lazyrunner runs this project's apps (.NET APIs, npm frontends) in a terminal the user \
watches, and builds, tests and checks migrations for them. While it's open, use its tools instead of running dotnet \
or npm yourself: what you do shows in its log. Start every turn with get_status - the user may have changed things \
since. Use wait instead of calling get_status in a loop. Keep get_logs small with since, grep or errors_only; after \
a failed build or a crash, get_errors has the error with its stack. For a slow or failing endpoint, get_requests \
(errors_only, sort slowest, endpoints) and get_request show what the app logged and queried while serving it. Tell \
the user before stopping or restarting apps \
they may be using. With restartOnCodeChange on (get_status settings), your own edits rebuild the running APIs by \
themselves a moment after the last save: wait for build_done instead of calling build.";

/// How the server reaches the open lazyrunner (the tests use a fake).
pub trait Channel: Send + Sync + 'static {
    fn ask(&self, body: Value) -> impl Future<Output = io::Result<Value>> + Send;
}

/// The control channel of the lazyrunner open on this machine.
struct ControlChannel(String);

impl Channel for ControlChannel {
    fn ask(&self, body: Value) -> impl Future<Output = io::Result<Value>> + Send {
        let name = self.0.clone();
        async move {
            match tokio::time::timeout(ASK_TIMEOUT, ipc::request(&name, &body)).await {
                Ok(answer) => answer,
                Err(_) => Err(io::Error::new(io::ErrorKind::TimedOut, "no answer")),
            }
        }
    }
}

pub struct Server<C> {
    channel: C,
    /// The agent's folder (where the client started this server): actions elsewhere are refused.
    repo: Option<String>,
    /// The client's name, from `initialize`: the log names it.
    client: Mutex<String>,
}

/// `lazyrunner mcp` (`ipc`: another control channel, for the tests).
pub fn run(ipc: Option<String>) -> Result<()> {
    let name = ipc.unwrap_or_else(|| crate::start::control_name(&Paths::from_env(&install_dir())));
    let repo = std::env::current_dir().ok().map(|dir| dir.display().to_string());
    let server = Arc::new(Server::new(ControlChannel(name), repo));
    tokio::runtime::Builder::new_multi_thread().enable_all().build()?.block_on(serve(server))
}

/// Answers each message from stdin as it comes (a long wait doesn't hold up the others), one writer for stdout. A
/// cancelled request stops; once stdin closes (the client is gone), whatever still runs stops and the server ends.
async fn serve<C: Channel>(server: Arc<Server<C>>) -> Result<()> {
    let (out, mut outgoing) = mpsc::unbounded_channel::<Value>();
    let writer = tokio::spawn(async move {
        let mut stdout = tokio::io::stdout();
        while let Some(message) = outgoing.recv().await {
            let line = format!("{message}\n");
            if stdout.write_all(line.as_bytes()).await.is_err() || stdout.flush().await.is_err() {
                break;
            }
        }
    });
    let running: Arc<Mutex<HashMap<String, AbortHandle>>> = Arc::default();
    let mut tasks = JoinSet::new();
    let mut input = BufReader::new(tokio::io::stdin());
    let mut bytes = Vec::new();
    loop {
        bytes.clear();
        if input.read_until(b'\n', &mut bytes).await? == 0 {
            break;
        }
        // Bytes that aren't UTF-8 make a line that isn't JSON: it gets an error, not the end of the server.
        let line = String::from_utf8_lossy(&bytes).trim().to_string();
        if line.is_empty() {
            continue;
        }
        let message = serde_json::from_str::<Value>(&line).unwrap_or(Value::Null);
        if message["method"] == "notifications/cancelled" {
            let request = message["params"]["requestId"].to_string();
            if let Some(task) = running.lock().ok().and_then(|mut running| running.remove(&request)) {
                task.abort();
            }
            continue;
        }
        let id = message.get("id").map(Value::to_string);
        let (server, out, done) = (server.clone(), out.clone(), running.clone());
        let finished = id.clone();
        // Held while the task is registered, so a quick one can't finish before it is.
        let mut registry = running.lock().ok();
        let task = tasks.spawn(async move {
            if let Some(answer) = server.respond(&line, &out).await {
                let _ = out.send(answer);
            }
            if let (Some(id), Ok(mut running)) = (finished, done.lock()) {
                running.remove(&id);
            }
        });
        if let (Some(id), Some(registry)) = (id, registry.as_mut()) {
            registry.insert(id, task);
        }
        drop(registry);
        while tasks.try_join_next().is_some() {}
    }
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
    drop(out);
    let _ = writer.await;
    Ok(())
}

fn rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// A tool's answer: text, an error when `failed`; cut when it is too long.
fn tool_result(text: String, failed: bool) -> Value {
    let text = match text.char_indices().nth(MAX_ANSWER_CHARS) {
        Some((cut, _)) => format!(
            "{}\n… cut at {MAX_ANSWER_CHARS} characters - ask for less (since, limit, grep, a suite)",
            &text[..cut]
        ),
        None => text,
    };
    json!({ "content": [{ "type": "text", "text": text }], "isError": failed })
}

fn tool(name: &str, description: &str, properties: Value, required: &[&str], read_only: bool) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": { "type": "object", "properties": properties, "required": required, "additionalProperties": false },
        "annotations": { "readOnlyHint": read_only, "destructiveHint": false, "openWorldHint": false },
    })
}

/// Why lazyrunner couldn't be asked: not open, not answering, run as Administrator while this isn't (Windows keeps
/// a program that isn't elevated from driving one that is), or the channel failing (it may be closing).
fn unreachable(error: &io::Error) -> String {
    match error.kind() {
        io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused => NOT_OPEN.into(),
        io::ErrorKind::TimedOut => NOT_ANSWERING.into(),
        io::ErrorKind::PermissionDenied => ELEVATED.into(),
        _ => format!("channel_error: lazyrunner's control channel failed ({error}) - it may be closing; try again"),
    }
}

impl<C: Channel> Server<C> {
    pub fn new(channel: C, repo: Option<String>) -> Self {
        Self { channel, repo, client: Mutex::new("agent".into()) }
    }

    /// The answer to one message; none for a notification or a response.
    pub async fn respond(&self, line: &str, out: &UnboundedSender<Value>) -> Option<Value> {
        let message: Value = match serde_json::from_str(line) {
            Ok(message) => message,
            Err(error) => return Some(rpc_error(Value::Null, -32700, &format!("not JSON: {error}"))),
        };
        let id = message.get("id").cloned()?;
        let Some(method) = message["method"].as_str() else {
            let is_response = message.get("result").is_some() || message.get("error").is_some();
            return (!is_response).then(|| rpc_error(id, -32600, "not a request: it has no method"));
        };
        let params = &message["params"];
        let result = match method {
            "initialize" => self.initialize(params),
            "ping" => json!({}),
            "tools/list" => json!({ "tools": self.tools().await }),
            "tools/call" => match self.call(params, out).await {
                Some(result) => result,
                None => {
                    let name = params["name"].as_str().unwrap_or_default();
                    return Some(rpc_error(id, -32602, &format!("no tool '{name}'")));
                }
            },
            method => return Some(rpc_error(id, -32601, &format!("no method '{method}'"))),
        };
        Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
    }

    fn initialize(&self, params: &Value) -> Value {
        if let Some(name) = params["clientInfo"]["name"].as_str().filter(|name| !name.is_empty())
            && let Ok(mut client) = self.client.lock()
        {
            *client = name.to_string();
        }
        let asked = params["protocolVersion"].as_str().unwrap_or_default();
        let version = MCP_VERSIONS.iter().find(|version| **version == asked).unwrap_or(&MCP_VERSIONS[0]);
        json!({
            "protocolVersion": version,
            "capabilities": { "tools": { "listChanged": false } },
            "serverInfo": { "name": "lazyrunner", "version": VERSION },
            "instructions": INSTRUCTIONS,
        })
    }

    /// Asks the open lazyrunner; its refusal (or its absence) is the error.
    async fn agent(&self, op: &str, args: Value) -> Result<Value, String> {
        let client = self.client.lock().map(|client| client.clone()).unwrap_or_default();
        let body = json!({ "agent": { "v": PROTOCOL, "op": op, "args": args, "client": client, "repo": self.repo } });
        let answer = self.channel.ask(body).await.map_err(|error| unreachable(&error))?;
        if answer["ok"] == true {
            return Ok(answer["result"].clone());
        }
        let why = answer["error"].as_str().unwrap_or("lazyrunner refused");
        match answer["code"].as_str() {
            Some(code) => Err(format!("{code}: {why}")),
            // Older than the agent channel: it doesn't know the request at all.
            None if why == "unknown request" => Err("protocol_mismatch: the open lazyrunner is older than \
                                                     lazyrunner mcp - ask the user to restart it (q, then lazyrunner)"
                .into()),
            None => Err(format!("refused: {why}")),
        }
    }

    async fn tools(&self) -> Vec<Value> {
        // The apps in the description, not as an enum: it would be stale once lazyrunner opens another project.
        let keys: Vec<String> = match self.agent("status", json!({})).await {
            Ok(status) => status["apps"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|app| app["key"].as_str())
                .map(String::from)
                .collect(),
            Err(_) => Vec::new(),
        };
        let apps = if keys.is_empty() { String::new() } else { format!(": {}", keys.join(", ")) };
        let app = json!({ "type": "string", "description": format!("An app's key (get_status lists them{apps})") });
        let some_apps = json!({
            "app": { "type": "string", "description": format!("An app's key (get_status lists them{apps}), or \"all\" (the default)") },
        });
        vec![
            tool(
                "get_status",
                "What lazyrunner runs now: each app's state, port, url and error count; what's busy (a build, a \
                 rebuild after code changes, the apps getting ready, npm, tests, migrations); the test totals; the \
                 settings; what the user is looking at; lazyrunner's newest log lines. Call it first in every turn.",
                json!({}),
                &[],
                true,
            ),
            tool(
                "get_logs",
                "An app's output, each line numbered: the newest `limit` (100, at most 500) after `since` (pass the \
                 last answer's `next` to get only what's new; `truncated` says older matching lines didn't fit), \
                 those containing `grep` (ignoring case), or only the error lines.",
                json!({
                    "app": app,
                    "since": { "type": "integer", "minimum": 0 },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 500 },
                    "grep": { "type": "string" },
                    "errors_only": { "type": "boolean" },
                }),
                &["app"],
                true,
            ),
            tool(
                "get_errors",
                "An app's last error with its stack trace (after a failed build: the build's errors with their full \
                 paths), or every error, warning, or both, in order.",
                json!({ "app": app, "what": { "type": "string", "enum": ["last", "errors", "warnings", "both"] } }),
                &["app"],
                true,
            ),
            tool(
                "get_requests",
                "The HTTP requests the .NET apps served (lazyrunner's requests view, from ASP.NET's request log): \
                 id, method, path, endpoint, status and milliseconds, newest first or slowest first (sort), after \
                 `since` (pass the last answer's `next` for only the new ones), of one app, errors only (status 400 \
                 and up), with `path` in them, or of one `endpoint` as the endpoints list names it (and `method`); \
                 `limit` (50, at most 500). With endpoints: true, summed up per \
                 endpoint instead - count, errors, avg, p50, p95, max. `n_plus_one` marks a request that ran one \
                 query over and over (and n_plus_one_requests an endpoint's); get_request has one in full.",
                json!({
                    "app": app,
                    "since": { "type": "integer", "minimum": 0 },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 500 },
                    "errors_only": { "type": "boolean" },
                    "path": { "type": "string" },
                    "endpoint": { "type": "string", "description": "As endpoints: true names it, e.g. /api/orders/{id}" },
                    "method": { "type": "string" },
                    "sort": { "type": "string", "enum": ["newest", "slowest"] },
                    "endpoints": { "type": "boolean" },
                }),
                &[],
                true,
            ),
            tool(
                "get_request",
                "One request in full, by its id from get_requests: what it sent and returned, its endpoint's times \
                 to compare with (p50, p95, slower_than_p95), the log lines its app wrote while it ran \
                 (possibly_mixed: other requests ran meanwhile, so some lines may be theirs), and its EF Core \
                 database commands: how many, their total and the slowest, and `repeated` - the query it ran over \
                 and over (10 times or more), an N+1.",
                json!({ "id": { "type": "integer", "minimum": 1 } }),
                &["id"],
                true,
            ),
            tool("get_screen", "lazyrunner's screen as the user sees it now, as text.", json!({}), &[], true),
            tool(
                "start_apps",
                "Starts an app or every app (builds the .NET ones first when their code changed). Then wait for ready.",
                some_apps.clone(),
                &[],
                false,
            ),
            tool("stop_apps", "Stops an app or every app.", some_apps.clone(), &[], false),
            tool(
                "restart_apps",
                "Restarts an app or every app (rebuilding what changed). Then wait for ready.",
                some_apps.clone(),
                &[],
                false,
            ),
            tool(
                "build",
                "Builds an app or every .NET app without starting it (a running one stops first). Then wait for \
                 build_done and read get_errors when it failed.",
                some_apps,
                &[],
                false,
            ),
            tool(
                "wait",
                "Waits until something is done, then says how it went: ready (no build or rebuild, no npm install, \
                 every started app answers), build_done (the builds that ran while waiting), tests_done or \
                 migrations_checked. At most timeout_sec (120, at most 600); call it again when it timed out.",
                json!({
                    "for": { "type": "string", "enum": WAITS },
                    "timeout_sec": { "type": "integer", "minimum": 1, "maximum": WAIT_MAX_SECS },
                }),
                &["for"],
                true,
            ),
            tool(
                "list_tests",
                "The test suites (.NET test projects, npm and Nx test targets) with their last results.",
                json!({}),
                &[],
                true,
            ),
            tool(
                "get_test_results",
                "A suite's last results: its failed tests with their messages and stacks, and its last output lines.",
                json!({ "suite": { "type": "string", "description": "A suite's key or name (list_tests)" } }),
                &["suite"],
                true,
            ),
            tool(
                "run_tests",
                "Runs every suite, one suite, its failed tests only, or one test of a suite. Then wait for \
                 tests_done.",
                json!({
                    "suite": { "type": "string", "description": "A suite's key or name (list_tests); every suite without it" },
                    "failed_only": { "type": "boolean" },
                    "test": { "type": "string", "description": "One test's full name; needs its suite" },
                }),
                &[],
                false,
            ),
            tool("stop_tests", "Stops the tests that run or wait.", json!({}), &[], false),
            tool(
                "get_migrations",
                "The EF Core DbContexts and their migrations as last checked against the database the apps use: \
                 when, applied, pending. It doesn't check them (check_migrations does).",
                json!({}),
                &[],
                true,
            ),
            tool(
                "check_migrations",
                "Checks one DbContext against the database (`context`), or finds them again and checks them all. \
                 Then wait for migrations_checked.",
                json!({ "context": { "type": "string", "description": "A context's key or name (get_migrations)" } }),
                &[],
                false,
            ),
        ]
    }

    /// A tool's result; none for a tool this server doesn't have.
    async fn call(&self, params: &Value, out: &UnboundedSender<Value>) -> Option<Value> {
        let args = params.get("arguments").cloned().filter(Value::is_object).unwrap_or_else(|| json!({}));
        let outcome = match params["name"].as_str().unwrap_or_default() {
            "get_status" => self.agent("status", json!({})).await,
            "get_logs" => self.agent("logs", args).await,
            "get_errors" => self.agent("errors", args).await,
            "get_requests" => self.agent("requests", args).await,
            "get_request" if !args["id"].is_u64() => Err("bad_args: which request? (an id from get_requests)".into()),
            "get_request" => self.agent("request", args).await,
            "get_screen" => self.screen().await,
            "start_apps" => self.agent("start", args).await,
            "stop_apps" => self.agent("stop", args).await,
            "restart_apps" => self.agent("restart", args).await,
            "build" => self.agent("build", args).await,
            "wait" => self.wait(&args, &params["_meta"]["progressToken"], out).await,
            "list_tests" => self.agent("tests", json!({})).await,
            "get_test_results" if !args["suite"].is_string() => Err("bad_args: which suite? (list_tests)".into()),
            "get_test_results" => self.agent("tests", args).await,
            "run_tests" => self.agent("run_tests", args).await,
            "stop_tests" => self.agent("stop_tests", json!({})).await,
            "get_migrations" => self.agent("migrations", json!({})).await,
            "check_migrations" => self.agent("check_migrations", args).await,
            _ => return None,
        };
        Some(match outcome {
            Ok(Value::String(text)) => tool_result(text, false),
            Ok(value) => tool_result(serde_json::to_string_pretty(&value).unwrap_or_default(), false),
            Err(why) => tool_result(why, true),
        })
    }

    /// The frame the open lazyrunner draws next.
    async fn screen(&self) -> Result<Value, String> {
        let answer = self.channel.ask(json!({ "dump": true })).await.map_err(|error| unreachable(&error))?;
        answer["frame"].as_str().map(|frame| Value::String(frame.to_string())).ok_or_else(|| {
            format!("lazyrunner couldn't show its screen: {}", answer["error"].as_str().unwrap_or("no frame"))
        })
    }

    /// Asks for the status until the condition holds or the time is up (a poll lazyrunner didn't answer counts as
    /// not done yet); tells a client that asked for progress what it waits for, every poll.
    async fn wait(&self, args: &Value, progress: &Value, out: &UnboundedSender<Value>) -> Result<Value, String> {
        let condition = args["for"].as_str().filter(|condition| WAITS.contains(condition));
        let Some(condition) = condition else {
            return Err(format!("bad_args: \"for\" is one of {}", WAITS.join(", ")));
        };
        let limit = args["timeout_sec"].as_u64().unwrap_or(WAIT_DEFAULT_SECS).clamp(1, WAIT_MAX_SECS);
        let started = Instant::now();
        let mut built = HashSet::new();
        let mut polls = 0u64;
        loop {
            let status = match self.agent("status", json!({})).await {
                Ok(status) => Some(status),
                Err(why) if why == NOT_ANSWERING => None,
                Err(why) => return Err(why),
            };
            let waited = started.elapsed().as_secs();
            if let Some(status) = &status {
                built.extend(building_keys(status));
                if let Some(summary) = done(condition, status, &built) {
                    return Ok(json!({ "done": true, "waited_sec": waited, "summary": summary, "status": status }));
                }
            }
            if waited >= limit {
                let doing = status.as_ref().map_or("not answering".to_string(), busy);
                let summary = format!("still {doing} after {waited}s - call wait again, or look at get_status");
                return Ok(json!({ "done": false, "waited_sec": waited, "summary": summary, "status": status }));
            }
            if !progress.is_null() {
                polls += 1;
                let message = status.as_ref().map_or("lazyrunner isn't answering".to_string(), busy);
                let params = json!({ "progressToken": progress, "progress": polls, "message": message });
                let _ = out.send(json!({ "jsonrpc": "2.0", "method": "notifications/progress", "params": params }));
            }
            tokio::time::sleep(WAIT_POLL).await;
        }
    }
}

fn names(status: &Value, keep: impl Fn(&Value) -> bool) -> Vec<&str> {
    status["apps"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|app| keep(app))
        .filter_map(|app| app["name"].as_str())
        .collect()
}

/// The apps that build, or wait to, in this status.
fn building_keys(status: &Value) -> Vec<String> {
    let busy = &status["busy"];
    let queued = busy["build_queue"].as_array().into_iter().flatten();
    std::iter::once(&busy["build"]).chain(queued).filter_map(Value::as_str).map(String::from).collect()
}

/// A build runs or waits, or a rebuild after code changes is about to start.
fn building(status: &Value) -> bool {
    !building_keys(status).is_empty() || status["busy"]["rebuild_pending"] == true
}

/// How the wait ended, when the condition holds; `built`: the apps seen building while waiting.
fn done(condition: &str, status: &Value, built: &HashSet<String>) -> Option<String> {
    let busy = &status["busy"];
    match condition {
        "build_done" if !building(status) => Some(build_summary(status, built)),
        "ready"
            if !building(status)
                && busy["npm"].is_null()
                && busy["getting_ready"].as_array().is_none_or(Vec::is_empty) =>
        {
            let up = names(status, |app| app["up"] == true);
            let down = names(status, |app| app["status"] == "exited" || app["status"] == "error");
            let mut summary =
                if up.is_empty() { "nothing is up".to_string() } else { format!("up: {}", up.join(", ")) };
            if !down.is_empty() {
                summary.push_str(&format!("; stopped with an error: {} (get_errors)", down.join(", ")));
            }
            Some(summary)
        }
        "tests_done" if busy["tests"] != true => {
            let tests = &status["tests"];
            Some(format!("tests: {} passed, {} failed, {} skipped", tests["passed"], tests["failed"], tests["skipped"]))
        }
        "migrations_checked" if busy["migrations"] != true => Some("migrations checked (get_migrations)".into()),
        _ => None,
    }
}

/// The builds that ran while waiting, and how they went; failures from before are said as such.
fn build_summary(status: &Value, built: &HashSet<String>) -> String {
    let apps: Vec<&Value> = status["apps"].as_array().into_iter().flatten().collect();
    let failed = |app: &&Value| {
        app["build_errors"]
            .as_u64()
            .map(|errors| format!("{} ({errors} errors)", app["name"].as_str().unwrap_or_default()))
    };
    let ran = |app: &&&Value| app["key"].as_str().is_some_and(|key| built.contains(key));
    let failures: Vec<String> = apps.iter().filter(ran).filter_map(failed).collect();
    let fine: Vec<&str> = apps
        .iter()
        .filter(ran)
        .filter(|app| app["build_errors"].is_null())
        .filter_map(|app| app["name"].as_str())
        .collect();
    let earlier: Vec<String> = apps.iter().filter(|app| !ran(app)).filter_map(failed).collect();
    let mut said = Vec::new();
    if built.is_empty() {
        said.push("no build ran while waiting".to_string());
    }
    if !fine.is_empty() {
        said.push(format!("built: {}", fine.join(", ")));
    }
    if !failures.is_empty() {
        said.push(format!("failed: {} (get_errors)", failures.join(", ")));
    }
    if !earlier.is_empty() {
        said.push(format!("still failing from an earlier build: {}", earlier.join(", ")));
    }
    said.join("; ")
}

/// What lazyrunner is busy with, for progress and a wait that timed out.
fn busy(status: &Value) -> String {
    let busy = &status["busy"];
    let name = |key: &Value| {
        let key = key.as_str().unwrap_or_default();
        names(status, |app| app["key"] == key).first().map_or(key.to_string(), |name| name.to_string())
    };
    let mut doing = Vec::new();
    if !busy["build"].is_null() {
        doing.push(format!("building {}", name(&busy["build"])));
    } else if busy["rebuild_pending"] == true {
        doing.push("about to rebuild after code changes".to_string());
    }
    if !busy["npm"].is_null() {
        doing.push(format!("installing {}'s packages", name(&busy["npm"])));
    }
    let waiting: Vec<String> = busy["getting_ready"].as_array().into_iter().flatten().map(name).collect();
    if !waiting.is_empty() {
        doing.push(format!("waiting for {} to answer", waiting.join(", ")));
    }
    if busy["tests"] == true {
        doing.push("running tests".into());
    }
    if busy["migrations"] == true {
        doing.push("checking migrations".into());
    }
    if doing.is_empty() { "busy".into() } else { doing.join(", ") }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    type Answer = Box<dyn Fn(&Value, usize) -> io::Result<Value> + Send + Sync>;

    /// A lazyrunner that answers with `answer` (the request, the number of the call) and records what it was asked.
    struct Fake {
        answer: Answer,
        asked: Mutex<Vec<Value>>,
        calls: AtomicUsize,
    }

    impl Channel for Fake {
        fn ask(&self, body: Value) -> impl Future<Output = io::Result<Value>> + Send {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            let answer = (self.answer)(&body, call);
            self.asked.lock().unwrap().push(body);
            async move { answer }
        }
    }

    fn server(answer: impl Fn(&Value, usize) -> io::Result<Value> + Send + Sync + 'static) -> Server<Fake> {
        let fake = Fake { answer: Box::new(answer), asked: Mutex::new(Vec::new()), calls: AtomicUsize::new(0) };
        Server::new(fake, Some("C:/work/shop".into()))
    }

    fn status(build: Option<&str>, orders_errors: Option<u64>) -> Value {
        json!({
            "apps": [
                { "key": "orders", "name": "Orders", "up": build.is_none(), "status": "running", "build_errors": orders_errors },
                { "key": "shop", "name": "Shop", "up": false, "status": "stopped", "build_errors": null },
            ],
            "busy": { "build": build, "build_queue": [], "rebuild_pending": false, "getting_ready": [], "npm": null,
                      "tests": false, "migrations": false },
            "tests": { "passed": 3, "failed": 0, "skipped": 1 },
        })
    }

    fn open(result: Value) -> io::Result<Value> {
        Ok(json!({ "ok": true, "result": result }))
    }

    async fn ask(server: &Server<Fake>, message: Value) -> (Option<Value>, Vec<Value>) {
        let (out, mut sent) = mpsc::unbounded_channel();
        let answer = server.respond(&message.to_string(), &out).await;
        drop(out);
        let mut notes = Vec::new();
        while let Some(note) = sent.recv().await {
            notes.push(note);
        }
        (answer, notes)
    }

    fn text(answer: &Value) -> String {
        answer["result"]["content"][0]["text"].as_str().unwrap_or_default().to_string()
    }

    fn call(id: u64, name: &str, arguments: Value) -> Value {
        json!({ "jsonrpc": "2.0", "id": id, "method": "tools/call", "params": { "name": name, "arguments": arguments } })
    }

    #[tokio::test]
    async fn initialize_picks_a_version_both_speak_and_names_the_client_in_what_it_asks() {
        let server = server(|_, _| open(status(None, None)));
        let hello = json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "protocolVersion": "2025-03-26", "clientInfo": { "name": "claude-code" } } });
        let (answer, _) = ask(&server, hello).await;
        let result = &answer.unwrap()["result"];
        assert_eq!(result["protocolVersion"], "2025-03-26");
        assert_eq!(result["serverInfo"]["name"], "lazyrunner");
        assert!(result["instructions"].as_str().unwrap().contains("get_status"));
        let future =
            json!({ "jsonrpc": "2.0", "id": 2, "method": "initialize", "params": { "protocolVersion": "2099-01-01" } });
        assert_eq!(ask(&server, future).await.0.unwrap()["result"]["protocolVersion"], MCP_VERSIONS[0]);

        let (answer, _) = ask(&server, call(3, "get_status", json!({}))).await;
        let answer = answer.unwrap();
        assert_eq!(answer["result"]["isError"], false);
        assert!(text(&answer).contains("\"orders\""));
        let asked = server.channel.asked.lock().unwrap().last().cloned().unwrap();
        assert_eq!(asked["agent"]["op"], "status");
        assert_eq!(asked["agent"]["v"], PROTOCOL);
        assert_eq!(asked["agent"]["client"], "claude-code");
        assert_eq!(asked["agent"]["repo"], "C:/work/shop");
    }

    #[tokio::test]
    async fn notifications_and_responses_get_no_answer_and_bad_messages_get_errors() {
        let server = server(|_, _| open(json!({})));
        let note = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
        assert_eq!(ask(&server, note).await.0, None);
        let response = json!({ "jsonrpc": "2.0", "id": 9, "result": {} });
        assert_eq!(ask(&server, response).await.0, None, "a client's answer to nothing we asked");
        let ping = json!({ "jsonrpc": "2.0", "id": "p", "method": "ping" });
        assert_eq!(ask(&server, ping).await.0.unwrap()["result"], json!({}));
        let unknown = json!({ "jsonrpc": "2.0", "id": 7, "method": "resources/list" });
        assert_eq!(ask(&server, unknown).await.0.unwrap()["error"]["code"], -32601);
        let no_method = json!({ "jsonrpc": "2.0", "id": 8 });
        assert_eq!(ask(&server, no_method).await.0.unwrap()["error"]["code"], -32600);
        assert_eq!(ask(&server, call(10, "fly", json!({}))).await.0.unwrap()["error"]["code"], -32602);
        let (out, _sent) = mpsc::unbounded_channel();
        assert_eq!(server.respond("{nope", &out).await.unwrap()["error"]["code"], -32700);
    }

    #[tokio::test]
    async fn the_tools_name_the_apps_and_each_way_of_not_getting_an_answer_says_what_it_is() {
        let list = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" });
        let (answer, _) = ask(&server(|_, _| open(status(None, None))), list.clone()).await;
        let tools = answer.unwrap()["result"]["tools"].as_array().unwrap().clone();
        let tool = |name: &str| tools.iter().find(|tool| tool["name"] == name).unwrap().clone();
        let app = &tool("start_apps")["inputSchema"]["properties"]["app"];
        assert!(app["enum"].is_null() && app["description"].as_str().unwrap().contains(": orders, shop)"), "{app}");
        assert_eq!(tool("get_logs")["annotations"]["readOnlyHint"], true);
        assert_eq!(tool("get_request")["annotations"]["readOnlyHint"], true);
        assert_eq!(tool("get_request")["inputSchema"]["required"], json!(["id"]));
        assert_eq!(tool("build")["annotations"]["readOnlyHint"], false);

        let with = |kind: io::ErrorKind| server(move |_, _| Err(io::Error::new(kind, "no")));
        let closed = with(io::ErrorKind::NotFound);
        let (answer, _) = ask(&closed, list).await;
        assert!(answer.unwrap()["result"]["tools"].as_array().is_some_and(|tools| tools.len() == 17));
        let status_call = call(2, "get_status", json!({}));
        let answer = ask(&closed, status_call.clone()).await.0.unwrap();
        assert_eq!(answer["result"]["isError"], true);
        assert!(text(&answer).starts_with("not_running: lazyrunner isn't open"));
        let paused = text(&ask(&with(io::ErrorKind::TimedOut), status_call.clone()).await.0.unwrap());
        assert!(paused.starts_with("not_answering:"), "{paused}");
        let broken = text(&ask(&with(io::ErrorKind::UnexpectedEof), status_call.clone()).await.0.unwrap());
        assert!(broken.starts_with("channel_error:"), "{broken}");
        let elevated = text(&ask(&with(io::ErrorKind::PermissionDenied), status_call.clone()).await.0.unwrap());
        assert!(elevated.starts_with("elevated: lazyrunner runs as Administrator"), "{elevated}");

        let refusing = server(|_, _| Ok(json!({ "ok": false, "code": "busy", "error": "Orders is building" })));
        assert_eq!(text(&ask(&refusing, status_call.clone()).await.0.unwrap()), "busy: Orders is building");
        let older = server(|_, _| Ok(json!({ "ok": false, "error": "unknown request" })));
        assert!(text(&ask(&older, status_call.clone()).await.0.unwrap()).starts_with("protocol_mismatch:"));
        let closing = server(|_, _| Ok(json!({ "ok": false, "error": "lazyrunner is closing" })));
        assert_eq!(text(&ask(&closing, status_call).await.0.unwrap()), "refused: lazyrunner is closing");
    }

    #[tokio::test]
    async fn wait_asks_until_the_build_is_done_and_says_what_was_built_with_progress_meanwhile() {
        // Orders builds for two polls and fails; Shop failed earlier and wasn't built since.
        let building = server(|_, call| {
            let mut answer = status(if call < 2 { Some("orders") } else { None }, (call >= 2).then_some(3));
            answer["apps"][1]["build_errors"] = json!(1);
            open(answer)
        });
        let wait = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": { "name": "wait", "arguments": { "for": "build_done" }, "_meta": { "progressToken": "t1" } } });
        let (answer, notes) = ask(&building, wait).await;
        let answer: Value = serde_json::from_str(&text(&answer.unwrap())).unwrap();
        assert_eq!(answer["done"], true);
        assert_eq!(
            answer["summary"],
            "failed: Orders (3 errors) (get_errors); still failing from an earlier build: Shop (1 errors)"
        );
        let progress: Vec<&Value> = notes.iter().map(|note| &note["params"]["progress"]).collect();
        assert_eq!(progress, [&json!(1), &json!(2)], "increasing, once per poll that wasn't done");
        assert_eq!(notes[0]["params"]["message"], "building Orders");

        let nothing = server(|_, _| open(status(None, None)));
        let answer = ask(&nothing, call(2, "wait", json!({ "for": "build_done" }))).await.0.unwrap();
        let answer: Value = serde_json::from_str(&text(&answer)).unwrap();
        assert_eq!(answer["summary"], "no build ran while waiting");

        let stuck = server(|_, _| open(status(Some("orders"), None)));
        let answer = ask(&stuck, call(3, "wait", json!({ "for": "build_done", "timeout_sec": 1 }))).await.0.unwrap();
        let answer: Value = serde_json::from_str(&text(&answer)).unwrap();
        assert_eq!(answer["done"], false);
        assert!(answer["summary"].as_str().unwrap().starts_with("still building Orders after 1s"));
        assert!(
            text(&ask(&stuck, call(4, "wait", json!({ "for": "lunch" }))).await.0.unwrap()).starts_with("bad_args:")
        );
    }

    #[test]
    fn ready_waits_for_npm_installs_and_rebuilds_after_code_changes() {
        let mut installing = status(None, None);
        installing["busy"]["npm"] = json!("shop");
        assert_eq!(done("ready", &installing, &HashSet::new()), None);
        assert_eq!(busy(&installing), "installing Shop's packages");
        let mut saved = status(None, None);
        saved["busy"]["rebuild_pending"] = json!(true);
        assert_eq!(done("ready", &saved, &HashSet::new()), None);
        assert_eq!(done("build_done", &saved, &HashSet::new()), None);
        assert_eq!(done("ready", &status(None, None), &HashSet::new()).unwrap(), "up: Orders");
    }

    #[test]
    fn a_long_answer_is_cut_with_a_note() {
        let long = "é".repeat(MAX_ANSWER_CHARS + 10);
        let cut = tool_result(long, false)["content"][0]["text"].as_str().unwrap().to_string();
        assert!(cut.ends_with("ask for less (since, limit, grep, a suite)"));
        assert_eq!(cut.chars().filter(|c| *c == 'é').count(), MAX_ANSWER_CHARS);
    }
}
