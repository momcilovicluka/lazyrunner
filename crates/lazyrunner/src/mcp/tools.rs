//! The tools `lazyrunner mcp` offers: their names, what they do and their arguments, as an agent reads them.

use serde_json::{Value, json};

use super::{WAIT_MAX_SECS, WAITS};

/// The views `get_view` reads, each the lazyrunner op that answers it.
pub const VIEWS: [&str; 6] = ["changelog", "durations", "ports", "audit", "checks", "snapshot"];
/// The `database` tool's ops, each with the lazyrunner op that does it.
pub const DATABASE_OPS: [(&str, &str); 6] = [
    ("add", "add_database"),
    ("copy", "copy_database"),
    ("backup", "backup_database"),
    ("export", "export_database"),
    ("import", "import_database"),
    ("files", "database_files"),
];
/// Tools merged into another (#112), with the call that does it now: an agent or a permission rule may still name them.
const RENAMED: [(&str, &str); 12] = [
    ("get_changelog", "get_view with view \"changelog\""),
    ("get_durations", "get_view with view \"durations\""),
    ("get_ports", "get_view with view \"ports\""),
    ("get_audit", "get_view with view \"audit\""),
    ("get_checks", "get_view with view \"checks\""),
    ("get_snapshot", "get_view with view \"snapshot\""),
    ("add_database", "database with op \"add\""),
    ("copy_database", "database with op \"copy\""),
    ("backup_database", "database with op \"backup\""),
    ("export_database", "database with op \"export\""),
    ("import_database", "database with op \"import\""),
    ("database_files", "database with op \"files\""),
];

/// What a tool that was merged into another is called now, as an error an agent reads.
pub fn renamed(name: &str) -> Option<String> {
    RENAMED
        .iter()
        .find(|(old, _)| *old == name)
        .map(|(old, new)| format!("renamed: {old} is now {new} - call that instead"))
}

/// Every tool; `apps` names the open project's apps in the descriptions (": orders, shop"), empty when lazyrunner
/// isn't open.
pub fn list(apps: &str) -> Vec<Value> {
    let app = json!({ "type": "string", "description": format!("An app's key (get_status lists them{apps})") });
    let some_apps = json!({
        "app": { "type": "string", "description": format!("An app's key (get_status lists them{apps}), or \"all\" (the default)") },
    });
    let database = json!({ "type": "string", "description": "A database's name or label (the one in use by default)" });
    vec![
        tool(
            "get_status",
            "What lazyrunner runs now: each app's state, port, url and errors; what's busy; the test totals; the \
             settings; what the user looks at; its newest log lines. Call it first in every turn.",
            json!({}),
            &[],
            true,
        ),
        tool(
            "get_logs",
            "An app's output, each line after its number: the newest `limit` (100, 500 at most) after `since` (the \
             last answer's `next`), matching `grep` (any case) or errors only. `parsed`: its log entries instead \
             (level, time, category, message, EF Core SQL).",
            json!({
                "app": app,
                "since": { "type": "integer", "minimum": 0 },
                "limit": { "type": "integer", "minimum": 1, "maximum": 500 },
                "grep": { "type": "string" },
                "errors_only": { "type": "boolean" },
                "parsed": { "type": "boolean" },
            }),
            &["app"],
            true,
        ),
        tool(
            "get_errors",
            "An app's last error with its stack (after a failed build, its errors with full paths), or every error, \
             warning or both.",
            json!({ "app": app, "what": { "type": "string", "enum": ["last", "errors", "warnings", "both"] } }),
            &["app"],
            true,
        ),
        tool(
            "get_requests",
            "The HTTP requests the .NET apps served, newest or slowest first: id, method, path, endpoint, status, ms. \
             Narrow by `since` (the last answer's `next`), app, errors only (400+), `path`, `endpoint` (and \
             `method`); `limit` 50 (500 at most). `endpoints`: per endpoint instead (count, errors, p50, p95, max).",
            json!({
                "app": app,
                "since": { "type": "integer", "minimum": 0 },
                "limit": { "type": "integer", "minimum": 1, "maximum": 500 },
                "errors_only": { "type": "boolean" },
                "path": { "type": "string" },
                "endpoint": { "type": "string", "description": "e.g. /api/orders/{id}" },
                "method": { "type": "string" },
                "sort": { "type": "string", "enum": ["newest", "slowest"] },
                "endpoints": { "type": "boolean" },
            }),
            &[],
            true,
        ),
        tool(
            "get_request",
            "One request in full, by its id from get_requests: what it sent and returned, its endpoint's p50 and \
             p95, its log lines and EF Core commands (an N+1 as `repeated`), and its waterfall. `baseline`: another \
             request's id, the two compared.",
            json!({ "id": { "type": "integer", "minimum": 1 }, "baseline": { "type": "integer", "minimum": 1 } }),
            &["id"],
            true,
        ),
        destructive(tool(
            "replay_request",
            "Sends a request from get_requests again to its app on localhost, with `headers` (an Authorization the \
             user gives you) and `body` the log lacks; answers the new id, status, ms and what differs. Any but GET, \
             HEAD, OPTIONS repeats its writes: needs confirm: true - ask the user first.",
            json!({
                "id": { "type": "integer", "minimum": 1 },
                "headers": { "type": "object", "additionalProperties": { "type": "string" } },
                "body": { "type": "string" },
                "confirm": { "type": "boolean" },
            }),
            &["id"],
            false,
        )),
        tool(
            "get_screen",
            "A fallback - use the dedicated tools first: lazyrunner's screen as the user sees it now, as text.",
            json!({}),
            &[],
            true,
        ),
        tool(
            "start_apps",
            "Starts an app or every app (building what changed first). Then wait for ready.",
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
            "Builds an app or every app that builds, without starting it (a running one stops first). Then wait for \
             build_done; get_errors when it failed.",
            some_apps,
            &[],
            false,
        ),
        tool(
            "wait",
            "Waits until `for` is done and says how it went; ready: no build or npm install runs and every started app \
             answers; request, error: the next to come, filtered as in get_requests - ask the user to reproduce it. \
             timeout_sec 120 (600 max).",
            json!({
                "for": { "type": "string", "enum": WAITS },
                "timeout_sec": { "type": "integer", "minimum": 1, "maximum": WAIT_MAX_SECS },
                "app": { "type": "string" },
                "method": { "type": "string" },
                "path": { "type": "string" },
                "endpoint": { "type": "string" },
                "errors_only": { "type": "boolean" },
            }),
            &["for"],
            true,
        ),
        tool(
            "list_tests",
            "The test suites (.NET, npm and Nx) with their last results, and whether what changed since their last \
             whole run affects them (affected, affected_why).",
            json!({}),
            &[],
            true,
        ),
        tool(
            "get_test_results",
            "A suite's last results: its failed tests with messages, stacks and likely causes (sources changed since \
             it last passed), its last output lines, and its code coverage when that setting is on.",
            json!({ "suite": { "type": "string", "description": "A suite's key or name (list_tests)" } }),
            &["suite"],
            true,
        ),
        tool(
            "run_tests",
            "Runs every suite, one suite, its failed tests, one test, or the suites what changed affects. Then wait \
             for tests_done.",
            json!({
                "suite": { "type": "string", "description": "A suite's key or name (list_tests); all without it" },
                "failed_only": { "type": "boolean" },
                "test": { "type": "string", "description": "One test's full name; needs its suite" },
                "affected": { "type": "boolean" },
            }),
            &[],
            false,
        ),
        tool("stop_tests", "Stops the tests that run or wait.", json!({}), &[], false),
        tool(
            "list_actions",
            "A fallback - use the dedicated tools first: every action as the command palette lists it, and every \
             setting with its state.",
            json!({}),
            &[],
            true,
        ),
        destructive(tool(
            "run_action",
            "Runs an action by its key from list_actions, as the user's key would, on `app` when it acts on the \
             focused app. While an app builds, only while_building ones run. needs_confirm ones need confirm: true - \
             ask the user first.",
            json!({
                "key": { "type": "string", "description": "e.g. \"D\", \"i c\", \", r\"" },
                "app": app.clone(),
                "confirm": { "type": "boolean" },
            }),
            &["key"],
            false,
        )),
        destructive(tool(
            "press_keys",
            "A fallback - use the dedicated tools first: presses keys in lazyrunner, space-separated as its key \
             reference (?) names them (\"g g\", \"Ctrl+o\", \"Esc\"); answers with the screen after. q, Q, Ctrl+C, \
             L, !, Ctrl+L and Ctrl+R need confirm: true.",
            json!({ "keys": { "type": "string" }, "confirm": { "type": "boolean" } }),
            &["keys"],
            false,
        )),
        destructive(tool(
            "switch_database",
            "Makes a SQL connection string in the APIs' secrets.json the one in use (as d does), by its database or \
             label; the running APIs restart on it. Needs confirm: true - ask the user first. Without name, lists \
             them.",
            json!({
                "name": { "type": "string" },
                "key": { "type": "string", "description": "Picks between ones of one name, e.g. ConnectionStrings:Default" },
                "confirm": { "type": "boolean" },
            }),
            &[],
            false,
        )),
        tool(
            "database",
            "Database work that drops nothing, by `op`, as the database screen (d) does it: add - a connection \
             string in secrets.json copying `from` with database `name`, not in use (switch: true makes it the one \
             in use, restarting the APIs: needs confirm - ask the user first); copy - `database` to a new one; \
             backup - a COPY_ONLY .bak in the server's backup folder; export - a .bacpac; import - `file` as a new \
             database; files - the backup files, sizes and the job running. copy, backup, export and import run in \
             the background.",
            json!({
                "op": { "type": "string", "enum": DATABASE_OPS.map(|(op, _)| op) },
                "database": { "type": "string", "description": "copy, backup, export: which (name or label; the one in use by default); import: one on the target server" },
                "name": { "type": "string", "description": "add, copy, import: the new database's name" },
                "from": { "type": "string", "description": "add: the connection string to copy (database or label; the one in use by default)" },
                "key": { "type": "string", "description": "add: picks between ones of one name, e.g. ConnectionStrings:Default" },
                "switch": { "type": "boolean" },
                "confirm": { "type": "boolean" },
                "file": { "type": "string", "description": "import: a file's name or path (files lists them)" },
            }),
            &["op"],
            false,
        ),
        tool(
            "save_point",
            "Makes a save point of a database (s, S on the database screen): a SQL Server snapshot (the default, \
             made in a moment) or a slower COPY_ONLY backup; `label` names it. In the background: get_status's \
             save_points list it once it's made.",
            json!({
                "database": database.clone(),
                "kind": { "type": "string", "enum": ["snapshot", "backup"] },
                "label": { "type": "string" },
            }),
            &[],
            false,
        ),
        destructive(tool(
            "rewind",
            "Rewinds a database to a save point (r): its .NET apps stop, it's restored, they start again; what \
             changed since is lost, other connections are cut off, its other snapshots dropped. Needs confirm: true - \
             ask the user first. Then wait for ready.",
            json!({
                "database": database.clone(),
                "save_point": { "type": "string", "description": "Its label or id (the newest by default)" },
                "confirm": { "type": "boolean" },
            }),
            &[],
            false,
        )),
        tool(
            "database_changes",
            "What changed in a database since a save point, table by table: rows then and now, and the small tables \
             whose rows changed in place - exactly what an action touched.",
            json!({
                "database": database.clone(),
                "save_point": { "type": "string", "description": "Its label or id (the newest by default)" },
            }),
            &[],
            true,
        ),
        tool(
            "query_database",
            "One SELECT (or WITH ... SELECT) on a database, read-only: anything else is refused, and it runs in a \
             transaction that's rolled back. Answers columns, rows (arrays), count, more, ms; values cut at 500 \
             characters.",
            json!({
                "sql": { "type": "string" },
                "database": database.clone(),
                "max_rows": { "type": "integer", "minimum": 1, "maximum": 1000, "default": 100 },
            }),
            &["sql"],
            true,
        ),
        destructive(tool(
            "drop_database",
            "Drops a database (Del on the database screen): its save points, its connections cut off, its \
             connection strings removed from secrets.json. Never the one in use or one a running app is on. Its data \
             is gone for good - needs confirm: true; ask the user first.",
            json!({ "database": database, "confirm": { "type": "boolean" } }),
            &["database"],
            false,
        )),
        tool(
            "run_task",
            "Runs one of the profile's tasks (get_status lists them) in the background - a one-off command such as \
             seeding the database. Then wait for task_done.",
            json!({ "task": { "type": "string", "description": "The task's key or name" } }),
            &["task"],
            false,
        ),
        tool(
            "install_packages",
            "npm ci (clean, the default) or npm install (clean: false) for an npm app; a command app's own install. \
             Then wait for ready.",
            json!({ "app": app.clone(), "clean": { "type": "boolean" } }),
            &["app"],
            false,
        ),
        destructive(tool(
            "apply_migrations",
            "Applies a DbContext's pending migrations, or moves it to the migration `to` (id or name): back reverts \
             the newer ones and needs confirm: true. check_migrations first. Then wait for migrations_checked.",
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
            "Kills whatever holds a port an app needs, then starts the apps (as ! does). Needs confirm: true - ask \
             the user first.",
            json!({ "confirm": { "type": "boolean" } }),
            &[],
            false,
        )),
        tool(
            "open_in_editor",
            "Opens a file of the repo at a line and column in the user's editor (as O does), to show them code. \
             Needs confirm: true - ask the user first.",
            json!({
                "file": { "type": "string" },
                "line": { "type": "integer" },
                "column": { "type": "integer" },
                "confirm": { "type": "boolean" },
            }),
            &["file"],
            false,
        ),
        tool(
            "set_setting",
            "Turns a setting (list_actions and get_status show them) on or off; saved, as , does.",
            json!({ "setting": { "type": "string" }, "on": { "type": "boolean" } }),
            &["setting", "on"],
            false,
        ),
        tool(
            "get_migrations",
            "The EF Core DbContexts and their migrations as last checked against the database: when, applied, \
             pending. check_migrations checks them.",
            json!({}),
            &[],
            true,
        ),
        tool(
            "get_stack",
            "The stack map (K): which app calls which, with each edge's calls, p95 and failures over the last \
             minute; each app's state and port; the apps on the database; the newest failed calls.",
            json!({}),
            &[],
            true,
        ),
        tool(
            "profile_app",
            "Samples a running app's CPU for `seconds` (10) - a .NET app with dotnet-trace, an npm app with Node's \
             inspector - while you make the slow request. Then wait for profile_done and read get_profile.",
            json!({ "app": app.clone(), "seconds": { "type": "integer", "minimum": 3, "maximum": 120 } }),
            &["app"],
            false,
        ),
        tool(
            "get_profile",
            "An app's newest CPU capture (profile_app): its CPU time, the top frames by self and total time, the \
             split by namespace, and each top frame's change since the capture before.",
            json!({ "app": app.clone() }),
            &["app"],
            true,
        ),
        tool(
            "get_health",
            "The stack's health (K, Tab): each app's p95, 5xx rate, memory, CPU and memory growth over the last \
             minute against its targets, its breaches and how long they lasted, and the alerts.",
            json!({}),
            &[],
            true,
        ),
        tool(
            "get_dependencies",
            "The code map: what depends on what - .NET projects and namespaces, TypeScript and Nx projects and \
             folders, Angular modules and components. Without part: the apps, every project, the cycles and the \
             links a rule forbids. With part: what it depends on and what uses it, with depth and way.",
            json!({
                "part": { "type": "string", "description": "A project, namespace, folder, module, component or app by name, or a source file" },
                "direction": { "type": "string", "enum": ["depends_on", "used_by", "both"] },
                "depth": { "type": "integer", "minimum": 1, "description": "How far; all without it" },
            }),
            &[],
            true,
        ),
        tool(
            "get_timeline",
            "The timeline (~): every app's output in the order it came, each line with how long ago and its app - \
             what happened across the stack around a moment. `count` newest lines (100, at most 500).",
            json!({
                "app": { "type": "string" },
                "grep": { "type": "string" },
                "errors_only": { "type": "boolean" },
                "count": { "type": "integer", "minimum": 1, "maximum": 500 },
            }),
            &[],
            true,
        ),
        tool(
            "get_start_sheet",
            "An app's start sheet (x, e): its command line or launch profile, and its environment with where each \
             variable comes from; secrets masked. Without app, the focused one.",
            json!({ "app": { "type": "string" } }),
            &[],
            true,
        ),
        tool(
            "get_marks",
            "The lines the user marked (m), oldest first, with their app and line number - what they want you to \
             look at.",
            json!({}),
            &[],
            true,
        ),
        tool(
            "get_build",
            "A .NET app's build as a map (Alt+b): each project's state, its own time against its usual, and the \
             critical path. The running build, else the app's last; without app, the running or the newest.",
            json!({ "app": { "type": "string" } }),
            &[],
            true,
        ),
        tool(
            "get_view",
            "A read-only view, by `view`: changelog - the newest versions' changes and the running version; \
             durations (D) - builds, starts, installs, test suites and tasks against their usual times; ports (P) - \
             each app's port and who holds it; audit (A) - outdated and vulnerable packages as last scanned, with \
             fixes; checks - the startup checks; snapshot - this run, masked, as , A v writes it for a teammate.",
            json!({
                "view": { "type": "string", "enum": VIEWS },
                "count": { "type": "integer", "minimum": 1, "maximum": 20, "description": "changelog: how many versions (3)" },
                "kind": { "type": "string", "description": "durations: build, start, npm, test or task" },
                "rerun": { "type": "boolean", "description": "checks: run them again" },
            }),
            &["view"],
            true,
        ),
        tool(
            "check_migrations",
            "Checks one DbContext (`context`) against the database, or finds and checks them all. Then wait for \
             migrations_checked.",
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

/// A tool as `tools/list` says it, without what a client assumes anyway: no empty `required`, and no
/// `destructiveHint` on a read-only tool (MCP reads it only when `readOnlyHint` is false).
fn tool(name: &str, description: &str, properties: Value, required: &[&str], read_only: bool) -> Value {
    let mut schema = json!({ "type": "object", "properties": properties, "additionalProperties": false });
    if !required.is_empty() {
        schema["required"] = json!(required);
    }
    let mut annotations = json!({ "readOnlyHint": read_only, "openWorldHint": false });
    if !read_only {
        annotations["destructiveHint"] = json!(false);
    }
    json!({ "name": name, "description": description, "inputSchema": schema, "annotations": annotations })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What `tools/list` may weigh at most, in characters of its JSON: an agent pays for it in every session (#112).
    const BUDGET: usize = 22_349;

    #[test]
    fn the_tool_list_stays_within_its_budget() {
        let size = json!({ "tools": list("") }).to_string().chars().count();
        assert!(size <= BUDGET, "tools/list is {size} characters, over its budget of {BUDGET}");
    }

    #[test]
    fn a_merged_tool_says_what_it_is_now_and_no_listed_tool_is_one() {
        assert_eq!(
            renamed("get_ports").unwrap(),
            "renamed: get_ports is now get_view with view \"ports\" - call that instead"
        );
        assert_eq!(renamed("get_status"), None);
        let listed: Vec<Value> = list("").iter().map(|tool| tool["name"].clone()).collect();
        assert!(RENAMED.iter().all(|(old, _)| !listed.contains(&json!(old))));
    }
}
