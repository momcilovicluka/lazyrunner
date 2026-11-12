//! What an agent asks the open lazyrunner (`lazyrunner mcp` passes it on): `{"agent": {"v": 1, "op": "...", "args":
//! {...}, "client": "...", "repo": "..."}}` on the control channel. Questions are answered at once and change nothing;
//! actions run as the keys would - while an app builds, only what the keys allow then - and the log says who did what.
//! An action from a folder lazyrunner doesn't run is refused, since building one checkout while the agent edits
//! another misleads it.

use std::path::Path;

use lazyrunner_core::changelog::VERSION;
use lazyrunner_core::config::Toggle;
use lazyrunner_core::output::Hue;
use lazyrunner_core::run::impact::Impact;
use lazyrunner_core::run::requests::{Endpoint, Request, endpoints};
use lazyrunner_core::run::suites::{Outcome, Suite};
use lazyrunner_core::run::waterfall;
use lazyrunner_core::run::{Command, Controller, Origin};
use serde_json::{Value, json};

use crate::screen::Ui;
use crate::screen::copy::{self, Pick};

mod drive;
mod views;

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
        "audit" => Ok(audit(controller)),
        "ports" => Ok(ports(controller)),
        "stack" => {
            controller.find_configured_calls();
            Ok(stack(controller))
        }
        "dependencies" => dependencies(controller, args),
        "requests" => requests(controller, args),
        "request" => one_request(controller, args),
        "actions" => Ok(drive::actions(ui, controller)),
        "durations" => Ok(views::durations(controller, args)),
        "health" => Ok(views::health(controller)),
        "timeline" => views::timeline(ui, controller, args),
        "sheet" => views::sheet(ui, controller, args),
        "checks" => Ok(views::checks(ui, controller, args)),
        "marks" => Ok(views::marks(controller)),
        "changelog" => Ok(views::changelog(args)),
        "build_map" => views::build(ui, controller, args),
        op @ ("start" | "stop" | "restart" | "build" | "run_tests" | "stop_tests" | "check_migrations"
        | "run_action" | "press_keys" | "switch_database" | "install_packages" | "apply_migrations"
        | "kill_port_holder" | "set_setting" | "run_task") => {
            in_repo(controller, request["repo"].as_str()).and_then(|()| act(ui, controller, op, args, &client))
        }
        other => Err(("unknown_op", format!("no op '{}'", clean(other, CLIENT_CHARS)))),
    };
    if let Ok(result) = &answer {
        let op = request["op"].as_str().unwrap_or_default();
        crate::screen::follow::agent_call(ui, controller, op, args, result, &client);
    }
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

/// What an app gets on this machine on top of what it's started with (the start sheet), secrets masked.
fn env_overrides(controller: &Controller, key: &str) -> Value {
    let overrides = controller.env_overrides(key).iter().map(|found| {
        let secret = lazyrunner_core::run::secrets::looks_secret(&found.name, &found.value);
        (found.name.clone(), json!(if secret { "***" } else { found.value.as_str() }))
    });
    Value::Object(overrides.collect())
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
                "kind": app.traits().name,
                "status": state.status.name(),
                "up": state.is_up(),
                "port": port,
                "url": port.map(|port| format!("http://localhost:{port}")),
                "errors": state.errors,
                "warnings": state.warnings,
                "lines": state.output.total(),
                "build_errors": controller.failed_build_errors.get(&app.key).map(Vec::len),
                "env_overrides": env_overrides(controller, &app.key),
                "services": controller.compose_services(&app.key).map(|services| {
                    services
                        .iter()
                        .map(|service| json!({ "name": service.name, "state": service.detail, "ports": service.ports }))
                        .collect::<Vec<_>>()
                }),
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
        "tasks": tasks(controller),
        "alerts": views::alerts(controller),
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

/// The profile's tasks: each its key and name, whether it runs, and how its last run went.
fn tasks(controller: &Controller) -> Vec<Value> {
    let tenths = |seconds: f64| (seconds * 10.0).round() / 10.0;
    controller
        .tasks()
        .iter()
        .map(|task| {
            let last = controller
                .task_result(&task.key)
                .map(|result| json!({ "ok": result.ok, "seconds": tenths(result.seconds), "error": result.error }));
            let running = controller.task_started(&task.key).is_some();
            json!({ "key": task.key, "name": task.name, "running": running, "last": last })
        })
        .collect()
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

/// An app's output as one text, each line after its number: the newest `limit` lines after `since`, those matching
/// `grep` (ignoring case), errors only when asked. Not an object per line: an agent pays for every character, and
/// `{"seq":…,"text":…}` made a log half again as long. `next` is the number to pass as `since` for what comes
/// after; `truncated` says older matching lines after `since` didn't fit. A `since` past the newest line (lazyrunner
/// opened again since) starts over (`reset`).
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
    let mut lines: Vec<String> = output
        .newest_first(0)
        .zip((oldest..=total).rev())
        .take_while(|(_, seq)| *seq > since)
        .filter(|(line, _)| !errors_only || copy::is_error(line))
        .filter(|(line, _)| grep.as_ref().is_none_or(|grep| line.text.to_lowercase().contains(grep)))
        .take(limit + 1)
        .map(|(line, seq)| format!("{seq} {}", line.text))
        .collect();
    let truncated = lines.len() > limit;
    lines.truncate(limit);
    lines.reverse();
    Ok(json!({
        "app": controller.apps[index].key,
        "count": lines.len(),
        "text": lines.join("\n"),
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

/// A request in a list: `id` is what get_request takes; `n_plus_one` (the times one query ran) when it looks like one.
fn request_row(request: &Request) -> Value {
    let mut row = json!({
        "id": request.seq,
        "at": request.at.format("%Y-%m-%dT%H:%M:%S%:z").to_string(),
        "app": request.app,
        "method": request.method,
        "path": request.path,
        "endpoint": request.endpoint,
        "status": request.status,
        "ms": ms(request.ms),
    });
    if let Some(repeated) = request.db.as_ref().and_then(|db| db.repeated.as_ref()) {
        row["n_plus_one"] = json!(repeated.count);
    }
    row
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
        "n_plus_one_requests": endpoint.n_plus_one,
    })
}

/// The requests the .NET apps served (the requests view): newest first, or slowest; after `since` (a request's id),
/// of one app, errors only (400 and up), with `path` in them (ignoring case), of one `endpoint` (and `method`) - or,
/// with `endpoints`, summed up per
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
    let endpoint = args["endpoint"].as_str();
    let method = args["method"].as_str();
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
        .filter(|request| endpoint.is_none_or(|endpoint| request.endpoint.eq_ignore_ascii_case(endpoint)))
        .filter(|request| method.is_none_or(|method| request.method.eq_ignore_ascii_case(method)))
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
    if let Some(fall) = controller.state(&request.app).and_then(|state| waterfall::of(request, &state.output)) {
        answer["waterfall"] = views::waterfall(&fall);
    }
    if let Some(seq) = args["baseline"].as_u64() {
        let Some(baseline) = controller.requests.items.iter().find(|request| request.seq == seq) else {
            return Err(("not_found", format!("no request {seq} to compare with: lazyrunner keeps the newest ones")));
        };
        answer["compare"] = views::compare(request, baseline);
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
    let lines: Vec<String> = (oldest..)
        .zip(kept.iter())
        .skip(kept.len().saturating_sub(REQUEST_LOG_LINES))
        .map(|(seq, line)| format!("{seq} {}", line.text))
        .collect();
    answer["log"] = json!({
        "known": true,
        "count": (last + 1).saturating_sub(first),
        "kept": kept.len(),
        "errors": kept.iter().filter(|line| copy::is_error(line)).count(),
        "possibly_mixed": request.mixed,
        "text": lines.join("\n"),
    });
    let db = request.db.clone().unwrap_or_default();
    let share = if request.ms > 0.0 { (db.total_ms / request.ms * 100.0).round() / 100.0 } else { 0.0 };
    let repeated = db.repeated.map(|repeated| {
        json!({
            "sql": repeated.sql,
            "count": repeated.count,
            "total_ms": ms(repeated.total_ms),
            "hint": "the same query over and over - an N+1: a loop loading one row at a time; Include() or one query \
                     for all of them fixes it",
        })
    });
    answer["database"] = json!({
        "commands": db.commands,
        "total_ms": ms(db.total_ms),
        "slowest_ms": ms(db.slowest_ms),
        "share_of_request": share,
        "repeated": repeated,
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
    if controller.suites.is_some() {
        controller.assess_now();
    }
    let Some(suites) = &controller.suites else {
        return Ok(json!({ "finding": true, "hint": "lazyrunner is finding the test suites - ask again in a moment" }));
    };
    let Some(wanted) = args["suite"].as_str() else {
        let summaries: Vec<Value> = suites.iter().map(|suite| suite_summary(controller, suite)).collect();
        return Ok(json!({ "suites": summaries }));
    };
    let key = find_suite(suites, wanted)?.key.clone();
    controller.causes_now(&key);
    let Some(suite) = controller.suite(&key) else { return Err(("not_found", format!("no suite {key}"))) };
    let history = controller.test_history.suite(&suite.key);
    let first = |text: &str| text.lines().take(FAILURE_LINES).collect::<Vec<_>>().join("\n");
    let failures: Vec<Value> = suite
        .tests
        .iter()
        .filter(|test| test.outcome == Outcome::Failed)
        .take(FAILURES_SHOWN)
        .map(|test| {
            let story = history.map(|history| history.story(&test.name)).unwrap_or_default();
            let flaky = story.flaky.map(|(failures, runs)| format!("failed {failures} of its last {runs} runs"));
            json!({
                "name": test.name,
                "ms": test.ms.round(),
                "message": first(&test.message),
                "stack": first(&test.stack),
                "new": story.new,
                "flaky": flaky,
                "likely_cause": likely_cause(controller, &key, &test.name),
            })
        })
        .collect();
    let tally = history.map(|history| history.tally(&suite.tests)).unwrap_or_default();
    let mut summary = suite_summary(controller, suite);
    summary["failures"] = json!(failures);
    summary["new_failures"] = json!(tally.new);
    summary["flaky"] = json!(tally.flaky);
    summary["output"] = json!(suite.output.iter().rev().take(FAILURE_LINES).rev().collect::<Vec<_>>());
    Ok(summary)
}

fn suite_summary(controller: &Controller, suite: &Suite) -> Value {
    let (affected, why) = affected(controller, &suite.key);
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
        "affected": affected,
        "affected_why": why,
    })
}

/// What changed since a failed test last passed (#34): when that was, and each changed source with why it may be the
/// cause, the most likely first; null when it never passed in the runs kept or git can't say.
fn likely_cause(controller: &Controller, key: &str, test: &str) -> Value {
    let Some(found) = controller.likely_cause(key, test) else { return Value::Null };
    let files: Vec<Value> = found
        .causes
        .iter()
        .map(|cause| json!({ "file": cause.file, "why": crate::screen::suite_cause::lead(&cause.lead) }))
        .collect();
    json!({ "since_passed_at": found.passed_at, "changed": files })
}

/// Whether what changed since a suite's last whole run affects it (#78), and why: each reason as
/// "Orders.Tests ← Orders.Core ← src/Orders.Core/OrderService.cs (+2 more)", or why it can't be told (it runs with
/// the affected ones).
fn affected(controller: &Controller, key: &str) -> (bool, Vec<String>) {
    match controller.impact(key) {
        None | Some(Impact::Unchanged) => (false, Vec::new()),
        Some(Impact::Unknown(reason)) => (true, vec![format!("can't tell: {reason}")]),
        Some(Impact::Affected(whys)) => {
            let told = whys
                .iter()
                .map(|why| {
                    let mut hops =
                        if why.chain.is_empty() { vec!["every .NET suite".to_string()] } else { why.chain.clone() };
                    hops.extend(why.files.first().cloned());
                    let more =
                        if why.files.len() > 1 { format!(" (+{} more)", why.files.len() - 1) } else { String::new() };
                    format!("{}{more}", hops.join(" ← "))
                })
                .collect();
            (true, told)
        }
    }
}

fn find_suite<'a>(suites: &'a [Suite], wanted: &str) -> Result<&'a Suite, Refusal> {
    let found =
        suites.iter().find(|suite| suite.key.eq_ignore_ascii_case(wanted) || suite.name.eq_ignore_ascii_case(wanted));
    found.ok_or_else(|| ("bad_args", format!("no suite '{}' (list_tests names them)", clean(wanted, CLIENT_CHARS))))
}

/// The DbContexts as the migrations view has them - found the first time, never checked from here: each one's last
/// check, the pending migrations.
/// The port map's rows: each app's port, who holds it and for how long, and who held it over the last minute (a
/// character every two seconds, oldest first: `.` free, `o` its own app, `x` something else, `r` reserved); and the
/// ranges Windows reserved, as last read.
fn ports(controller: &Controller) -> Value {
    use lazyrunner_core::run::ports::Holding;
    let map = controller.port_map();
    let now = std::time::Instant::now();
    let rows: Vec<Value> = map
        .rows
        .iter()
        .map(|row| {
            let history: String = row
                .cells
                .iter()
                .map(|holding| match holding {
                    Holding::Free => '.',
                    Holding::Own => 'o',
                    Holding::Other => 'x',
                    Holding::Reserved => 'r',
                })
                .collect();
            let holder = row
                .holder
                .as_ref()
                .map(|holder| json!({ "pid": holder.pid, "name": holder.name, "command": holder.command }));
            json!({
                "app": row.key,
                "name": controller.app(&row.key).map(|app| app.name.clone()),
                "port": row.port,
                "state": row.holding.name(),
                "holder": holder,
                "forSeconds": now.saturating_duration_since(row.since).as_secs(),
                "history": history,
            })
        })
        .collect();
    let (wanted, conflicts, reserved) = map.summary();
    let ranges: Vec<String> = map.reserved.iter().map(|(first, last)| format!("{first}-{last}")).collect();
    json!({
        "ports": rows,
        "summary": { "wanted": wanted, "conflicts": conflicts, "reserved": reserved },
        "reservedRanges": ranges,
    })
}

/// The code map (#89): with no `part`, the projects - each with how many it depends on and how many use it - and
/// the cycles; with one (a project, namespace or folder, by name), what it depends on or what uses it (`direction`:
/// `depends_on`, `used_by`, or both), as far as `depth` (all without it), each with the way to it.
fn dependencies(controller: &mut Controller, args: &Value) -> Answer {
    use lazyrunner_core::run::code::STACK;
    if controller.code_map().is_none() {
        controller.read_code_now();
    }
    let Some(map) = controller.code_map() else { return Err(("not_found", "the code couldn't be read".into())) };
    let root = controller.root.clone();
    let path = |at: usize| {
        let path = &map.parts[at].path;
        path.strip_prefix(&root).unwrap_or(path).display().to_string().replace('\\', "/")
    };
    let named = |parts: &[usize]| parts.iter().map(|&at| map.parts[at].name.clone()).collect::<Vec<_>>();
    let breaks = |at: Option<usize>| -> Vec<Value> {
        let wanted = |from: usize, to: usize| at.is_none_or(|at| from == at || to == at);
        let breaks = map.breaks.iter().filter(|broken| wanted(broken.from, broken.to));
        breaks
            .map(|broken| {
                json!({ "from": map.parts[broken.from].name, "to": map.parts[broken.to].name, "why": broken.why })
            })
            .collect()
    };
    let Some(wanted) = args["part"].as_str() else {
        let (parts, links) = map.level(None);
        let (apps, _) = map.level(Some(STACK));
        let apps: Vec<Value> = apps
            .iter()
            .filter_map(|&at| map.parts[at].app.as_deref().map(|key| (at, key)))
            .map(|(at, key)| {
                let shares: serde_json::Map<String, Value> = map
                    .level(Some(STACK))
                    .0
                    .iter()
                    .filter_map(|&other| map.parts[other].app.as_deref().map(|other_key| (other, other_key)))
                    .filter(|&(_, other_key)| other_key != key)
                    .map(|(other, other_key)| (map.parts[other].name.clone(), json!(map.shared_projects(key, other_key))))
                    .filter(|(_, count)| count != &json!(0))
                    .collect();
                json!({ "name": map.parts[at].name, "key": key, "projects": named(&map.app_projects(key)), "shares": shares })
            })
            .collect();
        let projects: Vec<Value> = parts
            .iter()
            .map(|&at| {
                let part = &map.parts[at];
                let mut project = json!({
                    "name": part.name,
                    "kind": part.kind.name(),
                    "dotnet": matches!(part.stack, lazyrunner_core::run::code::Stack::Dotnet),
                    "path": path(at),
                    "dependsOn": map.reach(at, true, false).len(),
                    "usedBy": map.reach(at, false, false).len(),
                });
                if !part.tags.is_empty() {
                    project["tags"] = json!(part.tags);
                }
                project
            })
            .collect();
        let cycles: Vec<Vec<String>> = map.cycles(&parts, &links).iter().map(|cycle| named(cycle)).collect();
        return Ok(json!({ "apps": apps, "projects": projects, "cycles": cycles, "breaks": breaks(None) }));
    };
    let Some(at) = map.named(wanted).or_else(|| map.by_file(wanted)) else {
        let wanted = clean(wanted, CLIENT_CHARS);
        return Err(("not_found", format!("no part '{wanted}' - get_dependencies without a part lists the projects")));
    };
    let depth = args["depth"].as_u64().map(|depth| depth as usize);
    let direction = args["direction"].as_str().unwrap_or("both");
    let walk = |forward: bool| -> Vec<Value> {
        let mut found: Vec<(usize, usize)> = Vec::new();
        let mut frontier = vec![at];
        let mut far = 0;
        while !frontier.is_empty() && depth.is_none_or(|depth| far < depth) {
            far += 1;
            let mut next = Vec::new();
            for &here in &frontier {
                for there in map.reach(here, forward, false) {
                    if there != at && !found.iter().any(|(seen, _)| *seen == there) {
                        found.push((there, far));
                        next.push(there);
                    }
                }
            }
            frontier = next;
        }
        found
            .into_iter()
            .map(|(there, far)| {
                let way = if forward { map.way(at, there) } else { map.way(there, at) };
                let way: Vec<String> =
                    way.unwrap_or_default().iter().map(|&step| map.parts[step].name.clone()).collect();
                json!({ "name": map.parts[there].name, "kind": map.parts[there].kind.name(), "depth": far, "way": way })
            })
            .collect()
    };
    let part = &map.parts[at];
    let mut answer = json!({ "name": part.name, "kind": part.kind.name(), "path": path(at), "files": part.files });
    if direction != "used_by" {
        answer["dependsOn"] = json!(walk(true));
    }
    if direction != "depends_on" {
        answer["usedBy"] = json!(walk(false));
    }
    if let Some(app) = part.app.as_deref().filter(|_| part.inside.as_deref() == Some(STACK)) {
        answer["projects"] = json!(named(&map.app_projects(app)));
    }
    let own: Vec<usize> =
        (0..map.parts.len()).filter(|&inner| map.parts[inner].inside.as_ref() == Some(&part.id)).collect();
    if !own.is_empty() {
        answer["parts"] = json!(named(&own));
    }
    if !part.tags.is_empty() {
        answer["tags"] = json!(part.tags);
    }
    let broken = breaks(Some(at));
    if !broken.is_empty() {
        answer["breaks"] = json!(broken);
    }
    let implicit: Vec<usize> = map
        .links
        .iter()
        .filter(|link| link.implicit && (link.from == at || link.to == at))
        .map(|link| if link.from == at { link.to } else { link.from })
        .collect();
    if !implicit.is_empty() {
        answer["linkedByNxAlone"] = json!(named(&implicit));
    }
    Ok(answer)
}

/// The failed calls an agent sees, the newest.
const FAILED_CALLS_SHOWN: usize = 10;

/// The stack map (K, #51): each app's state and port; who calls whom - from the calls the .NET apps made (their
/// HttpClient log, by the port a call went to), the profile's dependsOn and the apps' own config (a dev server's proxy,
/// an environment file) - with each edge's last minute (for a caller that can't log its calls, what the called app
/// served); the apps that use the database and its commands; and the newest failed calls, with what came back.
fn stack(controller: &Controller) -> Value {
    let now = std::time::Instant::now();
    let apps: Vec<Value> = controller
        .apps
        .iter()
        .zip(&controller.states)
        .map(|(app, state)| {
            json!({
                "app": app.key,
                "name": app.name,
                "state": state.status.name(),
                "up": state.is_up(),
                "port": controller.port(&app.key),
                "dependsOn": app.depends_on,
            })
        })
        .collect();
    let edges: Vec<Value> = controller
        .stack_edges()
        .iter()
        .map(|edge| {
            json!({
                "from": edge.from,
                "to": edge.to,
                "declaredIn": edge.declared_in,
                "fromWhatItServed": edge.served,
                "seen": edge.seen,
                "callsLastMinute": edge.calls,
                "perSecond": (edge.per_second * 100.0).round() / 100.0,
                "p95Ms": edge.p95_ms.map(|ms| ms.round()),
                "failedLastMinute": edge.failed,
                "lastFailedSecondsAgo": edge.last_failed.map(|at| now.saturating_duration_since(at).as_secs()),
            })
        })
        .collect();
    let failed: Vec<Value> = controller
        .calls
        .items
        .iter()
        .rev()
        .filter(|call| call.failed())
        .take(FAILED_CALLS_SHOWN)
        .map(|call| {
            json!({
                "from": call.from,
                "method": call.method,
                "url": call.url,
                "status": call.status,
                "ms": call.ms.map(|ms| ms.round()),
                "noAnswer": call.status.is_none(),
                "secondsAgo": now.saturating_duration_since(call.at).as_secs(),
            })
        })
        .collect();
    let users = controller.database_users();
    let commands: Vec<Value> = users
        .iter()
        .map(|key| {
            let seen = controller.database_seen(key);
            json!({ "app": key, "commandsLastMinute": controller.database_commands(key), "commandsSeen": seen })
        })
        .collect();
    let database = (!users.is_empty()).then(|| json!({ "label": controller.db_label, "usedBy": commands }));
    json!({ "apps": apps, "edges": edges, "database": database, "failedCalls": failed })
}

/// The audit view's last scans, per app: what each found, and the command that fixes each package.
fn audit(controller: &Controller) -> Value {
    use lazyrunner_core::run::audit::{self, Severity};
    let apps: Vec<Value> = controller
        .apps
        .iter()
        .map(|app| {
            let scan = controller.audits().get(&app.key);
            let folder = audit::folder(&controller.root, app).display().to_string();
            let packages: Vec<Value> = scan.map_or_else(Vec::new, |scan| {
                scan.packages
                    .iter()
                    .map(|package| {
                        let advisories: Vec<Value> = package
                            .advisories
                            .iter()
                            .map(|advisory| {
                                let severity = advisory.severity.name();
                                json!({ "id": advisory.id, "title": advisory.title, "severity": severity, "url": advisory.url })
                            })
                            .collect();
                        json!({
                            "name": package.name,
                            "current": package.current,
                            "latest": package.latest,
                            "direct": package.direct,
                            "severity": package.severity.map(Severity::name),
                            "new": scan.new.contains(&package.name),
                            "advisories": advisories,
                            "fix": audit::command(app, package),
                            "in": folder,
                        })
                    })
                    .collect()
            });
            json!({
                "app": app.key,
                "name": app.name,
                "scanned": scan.map(|scan| audit::age(scan.at)),
                "error": scan.and_then(|scan| scan.error.clone()),
                "summary": scan.map(|scan| audit::summary(&scan.packages)),
                "fixed": scan.map(|scan| scan.fixed.iter().map(|fixed| fixed.name.clone()).collect::<Vec<_>>()),
                "packages": packages,
            })
        })
        .collect();
    let installing = controller.audit_installing().map(|(_, command)| command);
    let mut answer = json!({ "apps": apps, "scanning": controller.auditing(), "installing": installing });
    if controller.audits().is_empty() {
        answer["hint"] = json!(
            "Nothing is scanned yet: the scans ask the package feeds, so they run when the user asks (A, then R)."
        );
    }
    answer
}

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
        "run_action" => drive::run_action(ui, controller, args)?,
        "press_keys" => drive::press_keys(ui, controller, args)?,
        "switch_database" => drive::switch_database(controller, args)?,
        "install_packages" => drive::install_packages(controller, args)?,
        "apply_migrations" => drive::apply_migrations(controller, args)?,
        "kill_port_holder" => drive::kill_port_holder(controller, args)?,
        "set_setting" => drive::set_setting(controller, args)?,
        "run_task" => drive::run_task(controller, args)?,
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
    if args["affected"].as_bool().unwrap_or(false) {
        controller.run_affected();
        if !controller.tests_active() {
            return Err(("nothing_to_run", "nothing changed since the suites' last runs - list_tests says why".into()));
        }
        return Ok(format!("run the affected suites: {}", controller.affected().join(", ")));
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
