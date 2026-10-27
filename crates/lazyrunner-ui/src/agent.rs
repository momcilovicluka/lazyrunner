//! What an agent asks the open lazyrunner (`lazyrunner mcp` passes it on): `{"agent": {"v": 1, "op": "...", "args":
//! {...}, "client": "...", "repo": "..."}}` on the control channel. Questions are answered at once and change nothing;
//! actions run as the keys would - while an app builds, only what the keys allow then - and the log says who did what.
//! An action from a folder lazyrunner doesn't run is refused, since building one checkout while the agent edits
//! another misleads it.

use std::path::Path;

use lazyrunner_core::changelog::VERSION;
use lazyrunner_core::config::Toggle;
use lazyrunner_core::output::Hue;
use lazyrunner_core::run::requests::{Endpoint, Request, db_command_ms, endpoints};
use lazyrunner_core::run::suites::{Outcome, Suite};
use lazyrunner_core::run::{Command, Controller, Origin};
use serde_json::{Value, json};

use crate::screen::Ui;
use crate::screen::copy::{self, Pick};

/// The agent protocol this lazyrunner speaks; `lazyrunner mcp` sends it with every request.
pub const PROTOCOL: u64 = 1;
const LOG_LINES: usize = 100;
const MAX_LOG_LINES: usize = 500;
/// The newest lines of lazyrunner's own log in the status.
const STATUS_LOG: usize = 10;
/// A suite's failures shown at most, and the lines of each one's message and stack.
const FAILURES_SHOWN: usize = 30;
const FAILURE_LINES: usize = 15;
/// What of an agent's own text (its name, a test's name) reaches the log at most.
const CLIENT_CHARS: usize = 40;
const TEST_NAME_CHARS: usize = 300;
const NO_MIGRATIONS: &str = "no EF Core migrations found in the .NET apps' projects";
/// Requests (or endpoints) in an answer, by default and at most.
const REQUESTS: usize = 50;
const MAX_REQUESTS: usize = 500;
/// A request's log lines in get_request, at most (the newest).
const REQUEST_LOG_LINES: usize = 300;
const NO_REQUESTS: &str = "No requests yet: they come from ASP.NET's request log of the .NET apps lazyrunner started. \
                           An app that logs only somewhere else (Serilog to Seq, say) needs \"args\" in its profile to \
                           log to the console (README, \"Requests\").";

type Refusal = (&'static str, String);
type Answer = Result<Value, Refusal>;

pub fn handle(ui: &mut Ui, controller: &mut Controller, request: &Value) -> Value {
    if request["v"].as_u64() != Some(PROTOCOL) {
        let why = format!(
            "this lazyrunner ({VERSION}) speaks agent protocol {PROTOCOL} - ask the user to restart lazyrunner (q, \
             then lazyrunner) so it matches lazyrunner mcp"
        );
        return refusal("protocol_mismatch", &why);
    }
    let args = &request["args"];
    let client = clean(request["client"].as_str().unwrap_or_default(), CLIENT_CHARS);
    let client = if client.is_empty() { "agent".to_string() } else { client };
    let answer = match request["op"].as_str().unwrap_or_default() {
        "status" => Ok(status(ui, controller)),
        "logs" => logs(controller, args),
        "errors" => errors(controller, args),
        "tests" => tests(controller, args),
        "migrations" => Ok(migrations(controller)),
        "requests" => requests(controller, args),
        "request" => one_request(controller, args),
        op @ ("start" | "stop" | "restart" | "build" | "run_tests" | "stop_tests" | "check_migrations") => {
            in_repo(controller, request["repo"].as_str()).and_then(|()| act(ui, controller, op, args, &client))
        }
        other => Err(("unknown_op", format!("no op '{}'", clean(other, CLIENT_CHARS)))),
    };
    match answer {
        Ok(result) => json!({ "ok": true, "result": result }),
        Err((code, why)) => refusal(code, &why),
    }
}

fn refusal(code: &str, why: &str) -> Value {
    json!({ "ok": false, "code": code, "error": why })
}

/// An agent's text without control characters (they'd break the screen and the log file), at most `cells` long.
fn clean(text: &str, cells: usize) -> String {
    text.chars().filter(|c| !c.is_control()).take(cells).collect::<String>().trim().to_string()
}

/// What the user is looking at.
fn ui_block(ui: &Ui, controller: &Controller) -> Value {
    json!({
        "view": ui.overlay.as_ref().map_or("columns", |overlay| overlay.kind()),
        "focus": controller.apps.get(ui.focus).map(|app| app.key.clone()),
        "zoom": ui.zoom,
    })
}

fn status(ui: &Ui, controller: &Controller) -> Value {
    let apps: Vec<Value> = controller
        .apps
        .iter()
        .zip(&controller.states)
        .map(|(app, state)| {
            let port = controller.port(&app.key);
            json!({
                "key": app.key,
                "name": app.name,
                "kind": if app.traits().builds { "dotnet" } else { "npm" },
                "status": state.status.name(),
                "up": state.is_up(),
                "port": port,
                "url": port.map(|port| format!("http://localhost:{port}")),
                "errors": state.errors,
                "warnings": state.warnings,
                "lines": state.output.total(),
                "build_errors": controller.failed_build_errors.get(&app.key).map(Vec::len),
            })
        })
        .collect();
    let getting_ready: Vec<String> = controller
        .getting_ready()
        .iter()
        .filter(|key| controller.state(key).is_some_and(|state| state.is_running() && !state.is_up()))
        .cloned()
        .collect();
    let totals = controller.test_totals();
    json!({
        "version": VERSION,
        "project": controller.project.name,
        "root": controller.root.display().to_string(),
        "branch": controller.branch,
        "database": controller.db_label,
        "busy": {
            "build": controller.building.as_ref().map(|job| job.key.clone()),
            "build_queue": controller.build_queue,
            "rebuild_pending": controller.rebuild_pending(),
            "getting_ready": getting_ready,
            "npm": controller.npm_ci.as_ref().map(|job| job.key.clone()),
            "tests": controller.tests_active(),
            "migrations": controller.migrations_active(),
        },
        "apps": apps,
        "tests": {
            "passed": totals.passed,
            "failed": totals.failed,
            "skipped": totals.skipped,
            "did_not_run": totals.errors,
            "suites_with_results": totals.ran,
        },
        "settings": { "restartOnCodeChange": controller.setting(Toggle::RestartOnCodeChange) },
        "ui": ui_block(ui, controller),
        "log": controller.log.iter().rev().take(STATUS_LOG).rev().map(|entry| entry.text.clone()).collect::<Vec<_>>(),
    })
}

/// The app a question or action names (`"app"`), by its key or its name.
fn app_index(controller: &Controller, args: &Value) -> Result<usize, Refusal> {
    let Some(wanted) = args["app"].as_str() else {
        return Err(("bad_args", format!("which app? (\"app\": one of {})", controller.keys().join(", "))));
    };
    controller
        .apps
        .iter()
        .position(|app| app.key.eq_ignore_ascii_case(wanted) || app.name.eq_ignore_ascii_case(wanted))
        .ok_or_else(|| {
            let wanted = clean(wanted, CLIENT_CHARS);
            ("bad_args", format!("no app '{wanted}' here (the apps: {})", controller.keys().join(", ")))
        })
}

/// An app's output, each line with its number: the newest `limit` lines after `since`, those matching `grep`
/// (ignoring case), errors only when asked. `next` is the number to pass as `since` for what comes after;
/// `truncated` says older matching lines after `since` didn't fit. A `since` past the newest line (lazyrunner opened
/// again since) starts over (`reset`).
fn logs(controller: &Controller, args: &Value) -> Answer {
    let index = app_index(controller, args)?;
    let output = &controller.states[index].output;
    let total = output.total();
    let oldest = total + 1 - output.len() as u64;
    let asked = args["since"].as_u64().unwrap_or(0);
    let reset = asked > total;
    let since = if reset { 0 } else { asked };
    let limit = args["limit"].as_u64().map_or(LOG_LINES, |limit| (limit as usize).clamp(1, MAX_LOG_LINES));
    let grep = args["grep"].as_str().map(str::to_lowercase);
    let errors_only = args["errors_only"].as_bool().unwrap_or(false);
    let mut lines: Vec<Value> = output
        .newest_first(0)
        .zip((oldest..=total).rev())
        .take_while(|(_, seq)| *seq > since)
        .filter(|(line, _)| !errors_only || copy::is_error(line))
        .filter(|(line, _)| grep.as_ref().is_none_or(|grep| line.text.to_lowercase().contains(grep)))
        .take(limit + 1)
        .map(|(line, seq)| json!({ "seq": seq, "text": line.text }))
        .collect();
    let truncated = lines.len() > limit;
    lines.truncate(limit);
    lines.reverse();
    Ok(json!({
        "app": controller.apps[index].key,
        "lines": lines,
        "next": total,
        "truncated": truncated,
        "older_lines_dropped": since.saturating_add(1) < oldest,
        "reset": reset,
    }))
}

/// An app's last error with its stack (a failed build's errors, with full paths), or every error, warning or both.
fn errors(controller: &Controller, args: &Value) -> Answer {
    let index = app_index(controller, args)?;
    let pick = match args["what"].as_str().unwrap_or("last") {
        "last" => Pick::LastError,
        "errors" => Pick::Errors,
        "warnings" => Pick::Warnings,
        "both" => Pick::Both,
        other => {
            let other = clean(other, CLIENT_CHARS);
            return Err(("bad_args", format!("\"what\" is last, errors, warnings or both, not '{other}'")));
        }
    };
    let (lines, entries) = copy::gather_app(controller, index, pick);
    Ok(json!({ "app": controller.apps[index].key, "entries": entries, "text": lines.join("\n") }))
}

/// Milliseconds to one decimal.
fn ms(ms: f64) -> f64 {
    (ms * 10.0).round() / 10.0
}

/// A request in a list: `id` is what get_request takes.
fn request_row(request: &Request) -> Value {
    json!({
        "id": request.seq,
        "at": request.at.format("%Y-%m-%dT%H:%M:%S%:z").to_string(),
        "app": request.app,
        "method": request.method,
        "path": request.path,
        "endpoint": request.endpoint,
        "status": request.status,
        "ms": ms(request.ms),
    })
}

fn endpoint_row(endpoint: &Endpoint) -> Value {
    json!({
        "app": endpoint.app,
        "method": endpoint.method,
        "endpoint": endpoint.endpoint,
        "count": endpoint.count,
        "errors": endpoint.errors,
        "avg_ms": ms(endpoint.avg),
        "p50_ms": ms(endpoint.p50),
        "p95_ms": ms(endpoint.p95),
        "max_ms": ms(endpoint.max),
        "total_ms": ms(endpoint.total),
    })
}

/// The requests the .NET apps served (the requests view): newest first, or slowest; after `since` (a request's id),
/// of one app, errors only (400 and up), with `path` in them (ignoring case) - or, with `endpoints`, summed up per
/// endpoint, the most time first. `next` is the id to pass as `since` for what comes after.
fn requests(controller: &Controller, args: &Value) -> Answer {
    let app = match &args["app"] {
        Value::Null => None,
        Value::String(_) => Some(controller.apps[app_index(controller, args)?].key.clone()),
        _ => return Err(("bad_args", "\"app\" is an app's key".into())),
    };
    let slowest = match args["sort"].as_str().unwrap_or("newest") {
        "newest" => false,
        "slowest" => true,
        other => {
            let other = clean(other, CLIENT_CHARS);
            return Err(("bad_args", format!("\"sort\" is newest or slowest, not '{other}'")));
        }
    };
    let since = args["since"].as_u64().unwrap_or(0);
    let errors_only = args["errors_only"].as_bool().unwrap_or(false);
    let path = args["path"].as_str().map(str::to_lowercase);
    let limit = args["limit"].as_u64().map_or(REQUESTS, |limit| (limit as usize).clamp(1, MAX_REQUESTS));
    let mut list: Vec<&Request> = controller
        .requests
        .items
        .iter()
        .rev()
        .filter(|request| request.seq > since)
        .filter(|request| app.as_ref().is_none_or(|app| *app == request.app))
        .filter(|request| !errors_only || request.status >= 400)
        .filter(|request| path.as_ref().is_none_or(|path| request.path.to_lowercase().contains(path)))
        .collect();
    let mut answer = json!({ "next": controller.requests.seq });
    if controller.requests.items.is_empty() {
        answer["hint"] = json!(NO_REQUESTS);
    }
    if args["endpoints"].as_bool().unwrap_or(false) {
        let mut summed = endpoints(list.into_iter());
        summed.sort_by(|a, b| b.total.total_cmp(&a.total));
        answer["matching"] = json!(summed.len());
        answer["endpoints"] = summed.iter().take(limit).map(endpoint_row).collect();
        return Ok(answer);
    }
    if slowest {
        list.sort_by(|a, b| b.ms.total_cmp(&a.ms));
    }
    answer["matching"] = json!(list.len());
    answer["requests"] = list.iter().take(limit).map(|request| request_row(request)).collect();
    Ok(answer)
}

/// One request in full: what it sent and returned, its endpoint's times to compare with, the log lines its app wrote
/// while it ran and the database commands among them (the requests view's details).
fn one_request(controller: &Controller, args: &Value) -> Answer {
    let Some(id) = args["id"].as_u64() else {
        return Err(("bad_args", "which request? (\"id\", from get_requests)".into()));
    };
    let Some(request) = controller.requests.items.iter().find(|request| request.seq == id) else {
        return Err(("not_found", format!("no request {id}: lazyrunner keeps the newest ones while it runs")));
    };
    let mut answer = request_row(request);
    answer["protocol"] = json!(request.proto);
    answer["sent"] = json!({ "content_type": request.sent_content, "length": request.sent_length });
    answer["returned"] = json!({ "content_type": request.content, "length": request.length });
    let same = controller.requests.items.iter().filter(|other| {
        other.app == request.app && other.method == request.method && other.endpoint == request.endpoint
    });
    if let Some(usual) = endpoints(same).pop() {
        let mut stats = endpoint_row(&usual);
        stats["slower_than_p95"] = json!(request.ms > usual.p95);
        answer["endpoint_stats"] = stats;
    }
    let Some((first, last)) = request.logs else {
        answer["log"] = json!({
            "known": false,
            "why": "its app logs no \"Request starting\" line for it (Serilog's request log says only when one finished)",
        });
        return Ok(answer);
    };
    let output = controller.state(&request.app).map(|state| &state.output);
    let kept: Vec<_> = output.map_or_else(Vec::new, |output| output.numbered(first, last).collect());
    let oldest = output.map_or(first, |output| output.total() + 1 - output.len() as u64).max(first);
    let times: Vec<f64> = kept.iter().filter_map(|line| db_command_ms(&line.text)).collect();
    let total: f64 = times.iter().sum();
    let lines: Vec<Value> = (oldest..)
        .zip(kept.iter())
        .skip(kept.len().saturating_sub(REQUEST_LOG_LINES))
        .map(|(seq, line)| json!({ "seq": seq, "text": line.text }))
        .collect();
    answer["log"] = json!({
        "known": true,
        "count": (last + 1).saturating_sub(first),
        "kept": kept.len(),
        "errors": kept.iter().filter(|line| copy::is_error(line)).count(),
        "possibly_mixed": request.mixed,
        "lines": lines,
    });
    let share = if request.ms > 0.0 { (total / request.ms * 100.0).round() / 100.0 } else { 0.0 };
    answer["database"] = json!({
        "commands": times.len(),
        "total_ms": ms(total),
        "slowest_ms": ms(times.iter().copied().fold(0.0, f64::max)),
        "share_of_request": share,
    });
    Ok(answer)
}

/// Starts finding the test suites (reading the projects), the first time they're asked for.
fn find_suites(controller: &mut Controller) {
    if controller.suites.is_none() && !controller.finding_suites() {
        controller.find_suites();
    }
}

/// The suites, or one suite's results with its failures.
fn tests(controller: &mut Controller, args: &Value) -> Answer {
    find_suites(controller);
    let Some(suites) = &controller.suites else {
        return Ok(json!({ "finding": true, "hint": "lazyrunner is finding the test suites - ask again in a moment" }));
    };
    let Some(wanted) = args["suite"].as_str() else {
        return Ok(json!({ "suites": suites.iter().map(suite_summary).collect::<Vec<_>>() }));
    };
    let suite = find_suite(suites, wanted)?;
    let first = |text: &str| text.lines().take(FAILURE_LINES).collect::<Vec<_>>().join("\n");
    let failures: Vec<Value> = suite
        .tests
        .iter()
        .filter(|test| test.outcome == Outcome::Failed)
        .take(FAILURES_SHOWN)
        .map(|test| {
            json!({ "name": test.name, "ms": test.ms.round(), "message": first(&test.message), "stack": first(&test.stack) })
        })
        .collect();
    let mut summary = suite_summary(suite);
    summary["failures"] = json!(failures);
    summary["output"] = json!(suite.output.iter().rev().take(FAILURE_LINES).rev().collect::<Vec<_>>());
    Ok(summary)
}

fn suite_summary(suite: &Suite) -> Value {
    json!({
        "key": suite.key,
        "name": suite.name,
        "app": suite.app,
        "status": format!("{:?}", suite.status).to_lowercase(),
        "passed": suite.passed,
        "failed": suite.failed,
        "skipped": suite.skipped,
        "seconds": suite.seconds.round(),
        "last_run": suite.last_run.map(|at| at.to_rfc3339()),
        "untested": suite.untested,
        "needs_packages": suite.needs_packages,
        "note": suite.note,
    })
}

fn find_suite<'a>(suites: &'a [Suite], wanted: &str) -> Result<&'a Suite, Refusal> {
    let found =
        suites.iter().find(|suite| suite.key.eq_ignore_ascii_case(wanted) || suite.name.eq_ignore_ascii_case(wanted));
    found.ok_or_else(|| ("bad_args", format!("no suite '{}' (list_tests names them)", clean(wanted, CLIENT_CHARS))))
}

/// The DbContexts as the migrations view has them - found the first time, never checked from here: each one's last
/// check, the pending migrations.
fn migrations(controller: &mut Controller) -> Value {
    if !controller.find_migrations() {
        return json!({ "contexts": [], "hint": NO_MIGRATIONS });
    }
    let contexts: Vec<Value> = controller
        .contexts
        .iter()
        .flatten()
        .map(|context| {
            json!({
                "key": context.key(),
                "app": context.app,
                "name": context.name,
                "status": format!("{:?}", context.status).to_lowercase(),
                "checked": context.checked(),
                "database": (!context.db.is_empty()).then(|| context.db.clone()),
                "applied": context.items.iter().filter(|item| item.applied == Some(true)).count(),
                "pending": context.pending().iter().map(|item| item.id.clone()).collect::<Vec<_>>(),
                "error": (!context.error.is_empty()).then(|| context.error.clone()),
            })
        })
        .collect();
    json!({ "contexts": contexts, "busy": controller.migrations_active() })
}

/// Whether the agent works in the checkout lazyrunner runs (or a folder in it).
fn in_repo(controller: &Controller, repo: Option<&str>) -> Result<(), Refusal> {
    let Some(repo) = repo else { return Ok(()) };
    let real = |path: &Path| std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let (repo, root) = (real(Path::new(repo)), real(&controller.root));
    if repo.starts_with(&root) {
        return Ok(());
    }
    let why = format!(
        "lazyrunner runs {} but you work in {} - ask the user to run lazyrunner in your folder (the open one \
         switches to it)",
        shown(&root),
        shown(&repo)
    );
    Err(("wrong_repo", why))
}

/// A path without Windows' `\\?\` prefix.
fn shown(path: &Path) -> String {
    let text = path.display().to_string();
    text.strip_prefix(r"\\?\").map_or(text.clone(), str::to_string)
}

fn act(ui: &mut Ui, controller: &mut Controller, op: &str, args: &Value, client: &str) -> Answer {
    let what = match op {
        "start" | "stop" | "restart" | "build" => app_action(controller, op, args)?,
        "run_tests" => run_tests(controller, args)?,
        "stop_tests" => {
            controller.stop_tests(false);
            "stop the tests".into()
        }
        _ => check_migrations(controller, args)?,
    };
    // Said once it's done: a refused action leaves no line.
    controller.add_log(format!("▸ {client}: {what}"), Hue::Magenta);
    Ok(json!({ "accepted": true, "doing": what, "ui": ui_block(ui, controller) }))
}

/// start, stop, restart or build an app (`"app"`), or every app (`"all"`, or no `"app"`). While an app builds, only
/// starting one app and stopping are allowed, as with the keys.
fn app_action(controller: &mut Controller, op: &str, args: &Value) -> Result<String, Refusal> {
    let key = match &args["app"] {
        Value::Null => None,
        Value::String(app) if app.eq_ignore_ascii_case("all") => None,
        Value::String(_) => Some(controller.apps[app_index(controller, args)?].key.clone()),
        _ => return Err(("bad_args", "\"app\" is an app's key, or \"all\"".into())),
    };
    let allowed_mid_build = op == "stop" || (op == "start" && key.is_some());
    if let Some(job) = controller.building.as_ref().filter(|_| !allowed_mid_build) {
        let name = controller.app(&job.key).map_or(job.key.clone(), |app| app.name.clone());
        return Err(("busy", format!("{name} is building - wait for build_done, then try again")));
    }
    let command = match (op, key.clone()) {
        ("start", None) => Command::StartAll,
        ("start", Some(key)) => Command::Start(key),
        ("stop", None) => Command::StopAll,
        ("stop", Some(key)) => Command::Stop(key),
        ("restart", None) => Command::RestartAll,
        ("restart", Some(key)) => Command::Restart(key),
        (_, None) => Command::BuildAll,
        (_, Some(key)) => Command::Build(key),
    };
    let target = key.map_or_else(|| "every app".to_string(), |key| controller.names(&[key]));
    controller.execute(command, Origin::Agent).map_err(|why| ("refused", why))?;
    Ok(format!("{op} {target}"))
}

/// Every suite, one (`"suite"`), the failed tests only (`"failed_only"`), or one test of a suite (`"test"`).
fn run_tests(controller: &mut Controller, args: &Value) -> Result<String, Refusal> {
    find_suites(controller);
    let Some(suites) = &controller.suites else {
        return Err(("busy", "lazyrunner is finding the test suites - try again in a moment".into()));
    };
    if controller.tests_active() {
        return Err(("busy", "tests are running - wait for tests_done, or stop_tests".into()));
    }
    let failed_only = args["failed_only"].as_bool().unwrap_or(false);
    let picked: Vec<String> = match args["suite"].as_str() {
        Some(wanted) => vec![find_suite(suites, wanted)?.key.clone()],
        None => suites.iter().filter(|suite| !suite.untested).map(|suite| suite.key.clone()).collect(),
    };
    let what = if let Some(test) = args["test"].as_str() {
        let (Some(_), [key]) = (args["suite"].as_str(), picked.as_slice()) else {
            return Err(("bad_args", "a single test needs its \"suite\" too".into()));
        };
        let key = key.clone();
        controller.run_test(&key, test);
        format!("run {} ({key})", clean(test, TEST_NAME_CHARS))
    } else {
        controller.run_tests(&picked, failed_only);
        let which = if args["suite"].is_string() { picked.join(", ") } else { "every suite".into() };
        format!("run the tests: {which}{}", if failed_only { " (failed only)" } else { "" })
    };
    if !controller.tests_active() {
        // Nothing queued: no suite to run, packages missing, no such test or nothing failed. The log says why.
        let why = controller.log.back().map_or(String::new(), |entry| format!(" ({})", entry.text));
        return Err(("nothing_to_run", format!("no tests started{why} - list_tests shows the suites")));
    }
    Ok(what)
}

/// Checks one context against the database (`"context"`), or finds them again and checks them all.
fn check_migrations(controller: &mut Controller, args: &Value) -> Result<String, Refusal> {
    let Some(wanted) = args["context"].as_str() else {
        if !controller.reload_migrations() {
            return Err(("not_found", NO_MIGRATIONS.into()));
        }
        return Ok("check every migration context".into());
    };
    if !controller.find_migrations() {
        return Err(("not_found", NO_MIGRATIONS.into()));
    }
    let key = controller
        .contexts
        .iter()
        .flatten()
        .find(|context| context.key().eq_ignore_ascii_case(wanted) || context.name.eq_ignore_ascii_case(wanted))
        .map(|context| context.key())
        .ok_or_else(|| {
            let wanted = clean(wanted, CLIENT_CHARS);
            ("bad_args", format!("no migration context '{wanted}' (get_migrations names them)"))
        })?;
    controller.list_migrations(Some(&key));
    Ok(format!("check {key}"))
}
