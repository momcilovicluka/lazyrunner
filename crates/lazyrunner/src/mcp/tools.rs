//! The tools `lazyrunner mcp` offers: their names, what they do and their arguments, as an agent reads them.

use serde_json::{Value, json};

use super::{WAIT_MAX_SECS, WAITS};

/// Every tool; `apps` names the open project's apps in the descriptions (": orders, shop"), empty when lazyrunner
/// isn't open.
pub fn list(apps: &str) -> Vec<Value> {
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
        tool(
            "get_screen",
            "lazyrunner's screen as the user sees it now, as text. In follow mode (F) it shows what you read last.",
            json!({}),
            &[],
            true,
        ),
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
            "list_actions",
            "Everything lazyrunner can do: every action as its command palette lists it (key, label, hint, \
             group, while_building: it runs while an app builds, focused_app: it acts on the focused app, \
             needs_confirm) and every setting with its state. run_action runs an action, set_setting sets a \
             setting.",
            json!({}),
            &[],
            true,
        ),
        destructive(tool(
            "run_action",
            "Runs an action by its key from list_actions, as the user's key would - on `app` (focused first) \
             when it acts on the focused app. While an app builds, only the while_building ones run. One with \
             needs_confirm (quit, kill a port's holder, clear the output, reopen lazyrunner, the git tool) needs \
             confirm: true - ask the user first. One that opens a box or a view leaves it on the user's screen: \
             press_keys Esc closes it.",
            json!({
                "key": { "type": "string", "description": "The action's key from list_actions, e.g. \"D\", \"i c\", \", r\"" },
                "app": app.clone(),
                "confirm": { "type": "boolean" },
            }),
            &["key"],
            false,
        )),
        destructive(tool(
            "press_keys",
            "Presses keys in the open lazyrunner, for what no tool covers - named as its key reference (?) \
             shows them, space-separated: \"g g\", \"Ctrl+o\", \"Alt+Left\", \"Esc\", \"Enter\", \"Space\". \
             Answers with the screen after. Refused while the user types in a prompt; q, Q, Ctrl+C, L, !, \
             Ctrl+L and Ctrl+R need confirm: true.",
            json!({ "keys": { "type": "string" }, "confirm": { "type": "boolean" } }),
            &["keys"],
            false,
        )),
        destructive(tool(
            "switch_database",
            "Makes a SQL connection string in the APIs' secrets.json the active one (as d does), by its \
             database or label; `key` picks between connection strings of one name. The running APIs restart \
             on it. Needs confirm: true - ask the user first. Without a name, the answer lists them.",
            json!({
                "name": { "type": "string" },
                "key": { "type": "string", "description": "The connection string's key, e.g. ConnectionStrings:Default" },
                "confirm": { "type": "boolean" },
            }),
            &[],
            false,
        )),
        tool(
            "install_packages",
            "npm ci (clean: true, the default) or npm install (clean: false) for an npm app. Then wait for ready.",
            json!({ "app": app.clone(), "clean": { "type": "boolean" } }),
            &["app"],
            false,
        ),
        destructive(tool(
            "apply_migrations",
            "Applies a DbContext's pending migrations, or moves it to the migration `to` (its id or name): \
             forward applies up to it, back reverts the newer ones through their Down() and needs confirm: \
             true. The context must be checked first (check_migrations). Then wait for migrations_checked.",
            json!({
                "context": { "type": "string", "description": "A context's key or name (get_migrations)" },
                "to": { "type": "string" },
                "confirm": { "type": "boolean" },
            }),
            &["context"],
            false,
        )),
        destructive(tool(
            "kill_port_holder",
            "Kills whatever holds a port an app needs, then starts the apps (as ! does). Needs confirm: true - \
             ask the user first.",
            json!({ "confirm": { "type": "boolean" } }),
            &[],
            false,
        )),
        tool(
            "set_setting",
            "Turns a setting (list_actions and get_status show them) on or off; saved, as , does.",
            json!({ "setting": { "type": "string" }, "on": { "type": "boolean" } }),
            &["setting", "on"],
            false,
        ),
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

/// A tool that can take something away from the user (with confirm).
fn destructive(mut tool: Value) -> Value {
    tool["annotations"]["destructiveHint"] = json!(true);
    tool
}

fn tool(name: &str, description: &str, properties: Value, required: &[&str], read_only: bool) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": { "type": "object", "properties": properties, "required": required, "additionalProperties": false },
        "annotations": { "readOnlyHint": read_only, "destructiveHint": false, "openWorldHint": false },
    })
}
