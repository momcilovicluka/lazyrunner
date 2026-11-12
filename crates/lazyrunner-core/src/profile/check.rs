//! Every problem in a profile, each naming the field it is about (`apps[2].port` ...). The messages say what to
//! write, so a person or an LLM can fix the file from them alone.

use std::collections::HashMap;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

use super::{REPO_PROFILE_FILE, SCHEMA_VERSION};
use crate::json::{field, is_integer, keys, list, text, truthy};

/// The fields each part of a profile can have (profile.schema.json has the same): anything else is most likely a
/// typo, which lazyrunner would otherwise ignore without a word.
pub struct Fields {
    pub top: &'static [&'static str],
    pub detect: &'static [&'static str],
    pub dotnet: &'static [&'static str],
    pub npm: &'static [&'static str],
    pub command: &'static [&'static str],
    pub compose: &'static [&'static str],
    pub command_ready: &'static [&'static str],
    pub preset: &'static [&'static str],
    pub task: &'static [&'static str],
    pub variant: &'static [&'static str],
    pub when: &'static [&'static str],
    pub ready: &'static [&'static str],
    pub migrations: &'static [&'static str],
    pub health: &'static [&'static str],
}

pub const FIELDS: Fields = Fields {
    top: &[
        "$schema",
        "schemaVersion",
        "extends",
        "id",
        "name",
        "detect",
        "watch",
        "apps",
        "layers",
        "presets",
        "tasks",
    ],
    detect: &["anyFile"],
    dotnet: &[
        "key",
        "name",
        "icon",
        "nerdIcon",
        "type",
        "project",
        "launchProfile",
        "swagger",
        "args",
        "env",
        "tests",
        "migrations",
        "dependsOn",
        "health",
    ],
    npm: &["key", "name", "icon", "nerdIcon", "type", "dir", "variants", "env", "tests", "dependsOn", "health"],
    command: &[
        "key",
        "name",
        "icon",
        "nerdIcon",
        "type",
        "run",
        "cwd",
        "port",
        "build",
        "install",
        "setup",
        "installFrom",
        "ready",
        "watch",
        "errors",
        "warnings",
        "frames",
        "preset",
        "path",
        "tools",
        "env",
        "dependsOn",
        "health",
    ],
    compose: &["key", "name", "icon", "nerdIcon", "type", "file", "cwd", "services", "env", "dependsOn"],
    command_ready: &["port", "line", "http", "orPort"],
    preset: &[
        "id",
        "name",
        "nerdIcon",
        "detect",
        "tools",
        "path",
        "env",
        "run",
        "build",
        "install",
        "setup",
        "installFrom",
        "ready",
        "watch",
        "errors",
        "warnings",
        "frames",
        "options",
    ],
    task: &["key", "name", "run", "cwd", "env"],
    variant: &["label", "when", "script", "port", "servedBy", "ready"],
    when: &["packageScript"],
    ready: &["file"],
    migrations: &["project", "contexts"],
    health: &["p95Ms", "errorRate", "memoryMb", "cpuPercent", "path", "expectedStatus"],
};

impl Fields {
    pub fn all(&self) -> Vec<&'static str> {
        let mut all: Vec<&str> = [
            self.top,
            self.detect,
            self.dotnet,
            self.npm,
            self.command,
            self.compose,
            self.command_ready,
            self.preset,
            self.task,
            self.variant,
            self.when,
            self.ready,
            self.migrations,
            self.health,
        ]
        .concat();
        all.sort_unstable();
        all.dedup();
        all
    }
}

/// An app's `"health"`: its targets are positive numbers (the 5xx rate a share, CPU a percent), its path starts with
/// /, its status is an HTTP one.
fn check_health(health: &Value, at: &str, problems: &mut Vec<String>) {
    if !health.is_object() {
        problems.push(format!("{at} must be an object, e.g. {{ \"p95Ms\": 500, \"path\": \"/healthz\" }}"));
        return;
    }
    known_fields(health, FIELDS.health, at, problems);
    for (name, most) in [("p95Ms", f64::MAX), ("memoryMb", f64::MAX), ("cpuPercent", 100.0), ("errorRate", 1.0)] {
        let value = field(health, name);
        if !value.is_null() && !value.as_f64().is_some_and(|value| value > 0.0 && value <= most) {
            let range =
                if most == f64::MAX { "a number above 0".to_string() } else { format!("above 0, at most {most}") };
            problems.push(format!("{at}.{name} must be {range}"));
        }
    }
    let path = field(health, "path");
    if !path.is_null() && !text(path).starts_with('/') {
        problems.push(format!("{at}.path must start with /, e.g. '/healthz'"));
    }
    let status = field(health, "expectedStatus");
    if !status.is_null() && !status.as_u64().is_some_and(|code| (100..=599).contains(&code)) {
        problems.push(format!("{at}.expectedStatus must be an HTTP status, e.g. 200"));
    }
}

fn pattern(regex: &str) -> Regex {
    Regex::new(regex).expect("a valid pattern")
}

// Case doesn't matter in these, as in the PowerShell version (the schema is stricter).
static ID: LazyLock<Regex> = LazyLock::new(|| pattern(r"(?i)^[a-z0-9][a-z0-9-]*$"));
static KEY: LazyLock<Regex> = LazyLock::new(|| pattern(r"(?i)^[a-z][a-z0-9]*$"));
static CSPROJ: LazyLock<Regex> = LazyLock::new(|| pattern(r"(?i)\.csproj$"));

/// Every problem in a profile; empty when it is fine. A repo's `.lazyrunner.json` needs no detect (the file itself
/// marks the repo).
pub fn validate(data: &Value, in_repo: bool) -> Vec<String> {
    let mut problems = Vec::new();
    let version = field(data, "schemaVersion");
    if !is_integer(version) {
        return vec!["schemaVersion is required: 1".into()];
    }
    let version = version.as_i64().unwrap_or(i64::MAX);
    if version > SCHEMA_VERSION {
        return vec![format!(
            "schemaVersion {version} needs a newer lazyrunner (this one reads profiles up to version {SCHEMA_VERSION})"
        )];
    }
    if version < 1 {
        return vec!["schemaVersion must be 1".into()];
    }
    known_fields(data, FIELDS.top, "", &mut problems);
    if truthy(field(data, "extends")) && !in_repo {
        problems.push(format!("extends is only for a repo's {REPO_PROFILE_FILE}"));
    }
    if !ID.is_match(&text(field(data, "id"))) {
        problems.push("id is required: a short lowercase name such as 'shop'".into());
    }
    if text(field(data, "name")).trim().is_empty() {
        problems.push("name is required".into());
    }
    check_detect(data, in_repo, &mut problems);
    let watch = list(field(data, "watch"));
    if !truthy(field(data, "watch")) {
        problems.push(r#"watch is required: the folder whose changes need a rebuild, e.g. ["services"]"#.into());
    } else if watch.len() != 1 {
        problems.push(format!("watch: one folder for now (it has {})", watch.len()));
    } else {
        relative_path(watch[0], "watch[0]", &mut problems);
    }
    if !truthy(field(data, "apps")) {
        problems.push("apps is required: at least one app".into());
        return problems;
    }
    let apps = list(field(data, "apps"));
    let mut keys: HashMap<String, &Value> = HashMap::new();
    for (index, app) in apps.iter().enumerate() {
        check_app(app, index, &mut keys, &mut problems);
    }
    let npm = apps.iter().filter(|app| app.is_object() && text(field(app, "type")).eq_ignore_ascii_case("npm"));
    for variant in npm.flat_map(|app| list(field(app, "variants"))) {
        if !variant.is_object() || !truthy(field(variant, "servedBy")) {
            continue;
        }
        let served_by = text(field(variant, "servedBy"));
        let server = keys.get(&served_by.to_lowercase());
        if !server.is_some_and(|server| text(field(server, "type")).eq_ignore_ascii_case("dotnet")) {
            problems.push(format!("servedBy '{served_by}' must be the key of a dotnet app"));
        }
    }
    for app in apps.iter().filter(|app| app.is_object()) {
        check_depends_on(app, &keys, &mut problems);
    }
    check_layers(field(data, "layers"), &mut problems);
    check_presets(field(data, "presets"), &mut problems);
    check_tasks(field(data, "tasks"), &mut problems);
    problems
}

/// `layers`: each a project's name (or a pattern) and the names it must not reference.
fn check_layers(layers: &Value, problems: &mut Vec<String>) {
    if layers.is_null() {
        return;
    }
    let Some(layers) = layers.as_object() else {
        problems.push("layers must be an object: a project's name and what it must not reference".into());
        return;
    };
    for (parts, rule) in layers {
        let names = field(rule, "mustNotReference");
        let all_text = names.as_array().is_some_and(|names| !names.is_empty() && names.iter().all(Value::is_string));
        if !rule.is_object() || !all_text {
            problems.push(format!("layers.{parts}.mustNotReference must list project names, e.g. [\"Orders.Api\"]"));
        }
    }
}

/// `dependsOn`: other apps' keys.
fn check_depends_on(app: &Value, keys: &HashMap<String, &Value>, problems: &mut Vec<String>) {
    let depends_on = field(app, "dependsOn");
    if depends_on.is_null() {
        return;
    }
    let key = text(field(app, "key"));
    let Some(called) = depends_on.as_array().filter(|called| called.iter().all(Value::is_string)) else {
        problems.push(format!("{key}.dependsOn must list app keys, e.g. [\"billing\"]"));
        return;
    };
    for called in called.iter().map(text) {
        if called.eq_ignore_ascii_case(&key) {
            problems.push(format!("{key}.dependsOn names the app itself"));
        } else if !keys.contains_key(&called.to_lowercase()) {
            problems.push(format!("{key}.dependsOn: '{called}' isn't an app's key"));
        }
    }
}

fn check_detect(data: &Value, in_repo: bool, problems: &mut Vec<String>) {
    let detect = field(data, "detect");
    if in_repo && detect.is_null() {
        return;
    }
    if !detect.is_object() || !truthy(field(detect, "anyFile")) {
        problems.push(r#"detect.anyFile is required: the file that marks the repo, e.g. ["Shop.sln"]"#.into());
        return;
    }
    known_fields(detect, FIELDS.detect, "detect", problems);
    let markers = list(field(detect, "anyFile"));
    if markers.len() != 1 {
        problems.push(format!("detect.anyFile: one file for now (it has {})", markers.len()));
    } else {
        relative_path(markers[0], "detect.anyFile[0]", problems);
    }
}

fn check_app<'a>(app: &'a Value, index: usize, keys: &mut HashMap<String, &'a Value>, problems: &mut Vec<String>) {
    let mut at = format!("apps[{index}]");
    if !app.is_object() {
        problems.push(format!("{at} must be an object"));
        return;
    }
    let key = text(field(app, "key"));
    if !KEY.is_match(&key) {
        problems.push(format!("{at}.key is required: lowercase letters and digits, e.g. orders"));
    } else if let std::collections::hash_map::Entry::Vacant(free) = keys.entry(key.to_lowercase()) {
        free.insert(app);
    } else {
        problems.push(format!("{at}.key '{key}' is used twice"));
    }
    if truthy(field(app, "key")) {
        at = format!("{at} ({key})");
    }
    if text(field(app, "name")).trim().is_empty() {
        problems.push(format!("{at}.name is required"));
    }
    for name in ["icon", "nerdIcon"] {
        let icon = field(app, name);
        if !icon.is_null() && icon.as_str().is_none_or(|icon| icon.encode_utf16().count() != 1) {
            problems.push(format!("{at}.{name} must be one character (it fills one cell of the title bar)"));
        }
    }
    let env = field(app, "env");
    if !env.is_null() && !env.as_object().is_some_and(|vars| vars.values().all(Value::is_string)) {
        problems
            .push(format!("{at}.env must name text values, e.g. {{ \"ASPNETCORE_ENVIRONMENT\": \"Development\" }}"));
    }
    let health = field(app, "health");
    if !health.is_null() {
        check_health(health, &format!("{at}.health"), problems);
    }
    let kind = field(app, "type").as_str().map(str::to_lowercase);
    match kind.as_deref() {
        Some("dotnet") => {
            known_fields(app, FIELDS.dotnet, &at, problems);
            let project = field(app, "project");
            if !truthy(project) {
                problems.push(format!("{at}.project is required: the .csproj to run"));
            } else {
                relative_path(project, &format!("{at}.project"), problems);
                if !CSPROJ.is_match(&text(project)) {
                    problems.push(format!("{at}.project must be a .csproj file"));
                }
            }
            if text(field(app, "launchProfile")).trim().is_empty() {
                problems
                    .push(format!("{at}.launchProfile is required: a profile from its Properties/launchSettings.json"));
            }
            let swagger = field(app, "swagger");
            if !swagger.is_null() && !text(swagger).starts_with('/') {
                problems.push(format!("{at}.swagger must start with /, e.g. '/swagger'"));
            }
            let args = field(app, "args");
            let all_text = |args: &Vec<Value>| args.iter().all(|arg| arg.as_str().is_some_and(|arg| !arg.is_empty()));
            if !args.is_null() && !args.as_array().is_some_and(all_text) {
                problems.push(format!("{at}.args must be a list of arguments, e.g. [\"--Seq:ApiKey=\"]"));
            }
            let setup = field(app, "migrations");
            if !setup.is_null() {
                known_fields(setup, FIELDS.migrations, &format!("{at}.migrations"), problems);
                let project = field(setup, "project");
                if !project.is_null() {
                    relative_path(project, &format!("{at}.migrations.project"), problems);
                    if !CSPROJ.is_match(&text(project)) {
                        problems.push(format!("{at}.migrations.project must be a .csproj file"));
                    }
                }
            }
        }
        Some("npm") => {
            known_fields(app, FIELDS.npm, &at, problems);
            check_npm_app(app, &at, problems);
        }
        Some(super::TYPE_COMMAND) => {
            known_fields(app, FIELDS.command, &at, problems);
            check_command_app(app, &at, problems);
        }
        Some(super::TYPE_COMPOSE) => {
            known_fields(app, FIELDS.compose, &at, problems);
            check_compose_app(app, &at, problems);
        }
        _ => {
            let kind = text(field(app, "type"));
            problems.push(format!("{at}.type must be 'dotnet', 'npm', 'command' or 'compose' (it is '{kind}')"));
        }
    }
}

fn is_port(value: &Value) -> bool {
    is_integer(value) && value.as_i64().is_some_and(|port| (1..=65535).contains(&port))
}

/// A list of text, none of it empty.
fn texts(value: &Value) -> bool {
    value.as_array().is_some_and(|items| items.iter().all(|item| item.as_str().is_some_and(|item| !item.is_empty())))
}

/// A compose app: its file and folder are paths in the repo, its services names.
fn check_compose_app(app: &Value, at: &str, problems: &mut Vec<String>) {
    for name in ["file", "cwd"] {
        let path = field(app, name);
        if !path.is_null() {
            relative_path(path, &format!("{at}.{name}"), problems);
        }
    }
    let services = field(app, "services");
    if !services.is_null() && !texts(services) {
        problems.push(format!("{at}.services must be a list of the compose file's services, e.g. [\"db\", \"redis\"]"));
    }
}

/// What a command's text looks like, in messages.
const COMMAND_EXAMPLE: &str = "\"uvicorn main:app --port {port}\"";

/// A command app: what it runs (text, or a list of arguments, or from its preset), where, its port - which `{port}`
/// needs - and the rest as [`check_command_fields`] says.
fn check_command_app(app: &Value, at: &str, problems: &mut Vec<String>) {
    let preset = text(field(app, "preset"));
    if field(app, "run").is_null() {
        problems.push(if preset.is_empty() {
            format!("{at}.run must be a command: text, or a list of arguments, e.g. {COMMAND_EXAMPLE}")
        } else {
            format!("{at}.run is required: the {preset} preset has no run for this folder - say what runs it")
        });
    }
    if !field(app, "preset").is_null() && preset.trim().is_empty() {
        problems.push(format!("{at}.preset must name a preset, e.g. 'python'"));
    }
    let cwd = field(app, "cwd");
    if !cwd.is_null() {
        relative_path(cwd, &format!("{at}.cwd"), problems);
    }
    let port = field(app, "port");
    if !port.is_null() && !is_port(port) {
        problems.push(format!("{at}.port must be a port number (1-65535)"));
    }
    let commands = ["run", "build", "install"].map(|name| super::load::words(field(app, name)).join(" "));
    let env = field(app, "env").as_object().into_iter().flatten().filter_map(|(_, value)| value.as_str());
    let uses_port = commands.iter().map(String::as_str).chain(env).any(|text| text.contains(super::PORT_PLACEHOLDER));
    if uses_port && port.is_null() && field(field(app, "ready"), "port").is_null() {
        problems.push(format!("{at} uses {} but has no port: add \"port\", e.g. 8000", super::PORT_PLACEHOLDER));
    }
    check_command_fields(app, at, true, problems);
}

/// A command app's fields as written, or a preset's (`app` false: its ready can't know a port yet): commands that are
/// commands, its paths, lists that are lists, patterns that are patterns, a ready that says one thing.
fn check_command_fields(app: &Value, at: &str, is_app: bool, problems: &mut Vec<String>) {
    for name in ["run", "build", "install", "setup"] {
        let value = field(app, name);
        if value.is_null() || (name == "setup" && value.as_array().is_some_and(Vec::is_empty)) {
            continue; // an empty setup: none, over a later option's
        }
        let command = (value.is_string() || texts(value)) && !super::load::words(value).is_empty();
        if !command {
            problems
                .push(format!("{at}.{name} must be a command: text, or a list of arguments, e.g. {COMMAND_EXAMPLE}"));
        }
    }
    check_ready(app, at, is_app, problems);
    for name in ["path", "tools", "installFrom"] {
        let value = field(app, name);
        if !value.is_null() && !texts(value) {
            problems.push(format!("{at}.{name} must be a list of text, e.g. [\".venv/Scripts\"]"));
        }
    }
    for (index, folder) in list(field(app, "path")).into_iter().enumerate() {
        relative_path(folder, &format!("{at}.path[{index}]"), problems);
    }
    for (index, file) in list(field(app, "installFrom")).into_iter().enumerate() {
        relative_path(file, &format!("{at}.installFrom[{index}]"), problems);
    }
    for name in ["watch", "errors", "warnings"] {
        let value = field(app, name);
        if !value.is_null() && !texts(value) {
            problems.push(format!("{at}.{name} must be a list of text, e.g. [\"**/*.py\"]"));
        }
    }
    for name in ["errors", "warnings"] {
        for (index, item) in list(field(app, name)).into_iter().enumerate() {
            if let Some(Err(error)) = item.as_str().map(Regex::new) {
                problems.push(format!("{at}.{name}[{index}] must be a regular expression: {error}"));
            }
        }
    }
    let frames = field(app, "frames");
    if frames.is_null() {
        return;
    }
    let groups = |pattern: &Regex| {
        ["file", "line"].iter().all(|group| pattern.capture_names().flatten().any(|name| name == *group))
    };
    match frames.as_str().map(Regex::new) {
        Some(Ok(pattern)) if groups(&pattern) => {}
        Some(Ok(_)) => problems.push(format!("{at}.frames needs a (?P<file>...) and a (?P<line>...) group")),
        Some(Err(error)) => problems.push(format!("{at}.frames must be a regular expression: {error}")),
        None => problems.push(format!("{at}.frames must be a regular expression with file and line groups")),
    }
}

/// The profile's tasks (`"tasks"`): each with a unique key, a name and a command to run; its folder and env as an app's.
fn check_tasks(tasks: &Value, problems: &mut Vec<String>) {
    if tasks.is_null() {
        return;
    }
    let Some(tasks) = tasks.as_array() else {
        problems.push("tasks must be a list, e.g. [{ \"key\": \"seed\", \"name\": \"Seed the database\", \"run\": \"python seed.py\" }]".into());
        return;
    };
    let mut keys = std::collections::HashSet::new();
    for (index, task) in tasks.iter().enumerate() {
        let mut at = format!("tasks[{index}]");
        if !task.is_object() {
            problems.push(format!("{at} must be an object: its key, name and run"));
            continue;
        }
        let key = text(field(task, "key"));
        if !KEY.is_match(&key) {
            problems.push(format!("{at}.key is required: lowercase letters and digits, e.g. seed"));
        } else if !keys.insert(key.to_lowercase()) {
            problems.push(format!("{at}.key '{key}' is used twice"));
        } else {
            at = format!("{at} ({key})");
        }
        known_fields(task, FIELDS.task, &at, problems);
        if text(field(task, "name")).trim().is_empty() {
            problems.push(format!("{at}.name is required: what the palette calls it"));
        }
        let run = field(task, "run");
        if !((run.is_string() || texts(run)) && !super::load::words(run).is_empty()) {
            problems.push(format!("{at}.run must be a command: text, or a list of arguments, e.g. \"python seed.py\""));
        }
        let cwd = field(task, "cwd");
        if !cwd.is_null() {
            relative_path(cwd, &format!("{at}.cwd"), problems);
        }
        let env = field(task, "env");
        if !env.is_null() && !env.as_object().is_some_and(|vars| vars.values().all(Value::is_string)) {
            problems.push(format!("{at}.env must name text values, e.g. {{ \"APP_ENV\": \"dev\" }}"));
        }
    }
}

/// The profile's own presets (`"presets"`): each a preset, as [`check_preset`] says.
fn check_presets(presets: &Value, problems: &mut Vec<String>) {
    if presets.is_null() {
        return;
    }
    let Some(presets) = presets.as_object() else {
        problems.push("presets must be an object: each preset by its name, e.g. { \"python\": { ... } }".into());
        return;
    };
    for (name, preset) in presets {
        check_preset(preset, &format!("presets.{name}"), problems);
    }
}

/// A preset: the fields it fills in, as a command app's would be, and its options - each those fields and when it
/// holds (a file in the app's folder, a program on PATH).
pub(crate) fn check_preset(preset: &Value, at: &str, problems: &mut Vec<String>) {
    if !preset.is_object() {
        problems.push(format!("{at} must be an object: the fields it fills in"));
        return;
    }
    known_fields(preset, FIELDS.preset, at, problems);
    check_command_fields(preset, at, false, problems);
    let option_fields: Vec<&str> = super::preset::FILLED.iter().copied().chain(["when"]).collect();
    for (index, option) in list(field(preset, "options")).into_iter().enumerate() {
        let oat = format!("{at}.options[{index}]");
        if !option.is_object() {
            problems.push(format!("{oat} must be an object: when it holds, and what it fills in"));
            continue;
        }
        known_fields(option, &option_fields, &oat, problems);
        let when = field(option, "when");
        if when.is_object() {
            known_fields(when, &super::preset::WHEN, &format!("{oat}.when"), problems);
        }
        check_command_fields(option, &oat, false, problems);
    }
}

/// A command app's `ready`: one of a port, a line of its output (a pattern) or an HTTP path on its port.
fn check_ready(app: &Value, at: &str, is_app: bool, problems: &mut Vec<String>) {
    let ready = field(app, "ready");
    if ready.is_null() {
        return;
    }
    let rat = format!("{at}.ready");
    let shapes = "{ \"port\": 8000 }, { \"line\": \"Uvicorn running on\" } or { \"http\": \"/health\" }";
    if !ready.is_object() {
        problems.push(format!("{rat} must be an object: {shapes}"));
        return;
    }
    known_fields(ready, FIELDS.command_ready, &rat, problems);
    if ["port", "line", "http"].iter().filter(|name| !field(ready, name).is_null()).count() != 1 {
        problems.push(format!("{rat} needs one of port, line or http: {shapes}"));
    }
    let port = field(ready, "port");
    if !port.is_null() && !is_port(port) {
        problems.push(format!("{rat}.port must be a port number (1-65535)"));
    }
    let line = field(ready, "line");
    if !line.is_null() {
        match line.as_str().filter(|line| !line.is_empty()).map(Regex::new) {
            Some(Ok(_)) => {}
            Some(Err(error)) => problems.push(format!("{rat}.line must be a regular expression: {error}")),
            None => problems.push(format!("{rat}.line must be text: what its output says once it's up")),
        }
    }
    let or_port = field(ready, "orPort");
    if !or_port.is_null() && (!or_port.is_boolean() || line.is_null()) {
        problems.push(format!("{rat}.orPort goes with line: true, up once the line shows or the port listens"));
    }
    let http = field(ready, "http");
    if !http.is_null() && !text(http).starts_with('/') {
        problems.push(format!("{rat}.http must be a path starting with /, e.g. '/health'"));
    }
    if is_app && !http.is_null() && field(app, "port").is_null() {
        problems.push(format!("{rat}.http needs the app's port: the path is asked of it"));
    }
}

/// An npm app: its folder and one or more ways to run, the first whose `when.packageScript` package.json has: a
/// dev server on a port, or a watch build that a dotnet app serves. A variant without when always applies, so only
/// the last one can leave it out.
fn check_npm_app(app: &Value, at: &str, problems: &mut Vec<String>) {
    let dir = field(app, "dir");
    if !truthy(dir) {
        problems.push(format!("{at}.dir is required: the folder with package.json"));
    } else {
        relative_path(dir, &format!("{at}.dir"), problems);
    }
    if !truthy(field(app, "variants")) {
        problems.push(format!("{at}.variants is required: how it runs (a dev server or a watch build)"));
        return;
    }
    let variants = list(field(app, "variants"));
    for (index, variant) in variants.iter().enumerate() {
        let vat = format!("{at}.variants[{index}]");
        if !variant.is_object() {
            problems.push(format!("{vat} must be an object"));
            continue;
        }
        known_fields(variant, FIELDS.variant, &vat, problems);
        let (when, ready) = (field(variant, "when"), field(variant, "ready"));
        if when.is_object() {
            known_fields(when, FIELDS.when, &format!("{vat}.when"), problems);
        }
        if ready.is_object() {
            known_fields(ready, FIELDS.ready, &format!("{vat}.ready"), problems);
        }
        if !when.is_null() && text(field(when, "packageScript")).trim().is_empty() {
            problems.push(format!("{vat}.when.packageScript is required: the package.json script that selects it"));
        } else if when.is_null() && index + 1 < variants.len() {
            problems.push(format!("{vat} has no when, so it always applies: only the last variant can leave it out"));
        }
        if text(field(variant, "script")).trim().is_empty() {
            problems.push(format!("{vat}.script is required: the npm script to run"));
        }
        let port = field(variant, "port");
        let (has_port, has_server) = (!port.is_null(), truthy(field(variant, "servedBy")));
        if has_port == has_server {
            let which = if has_port { "both" } else { "neither" };
            problems.push(format!("{vat} needs either port (a dev server) or servedBy (a watch build), not {which}"));
        }
        if has_port && (!is_integer(port) || port.as_i64().is_none_or(|port| !(1..=65535).contains(&port))) {
            problems.push(format!("{vat}.port must be a port number (1-65535)"));
        }
        if has_server {
            let file = field(ready, "file");
            if !truthy(file) {
                problems.push(format!("{vat}.ready.file is required: the file the build writes when it is done"));
            } else {
                relative_path(file, &format!("{vat}.ready.file"), problems);
            }
        }
    }
}

fn relative_path(value: &Value, at: &str, problems: &mut Vec<String>) {
    let Some(path) = value.as_str().filter(|path| !path.trim().is_empty()) else {
        problems.push(format!("{at} must be a path"));
        return;
    };
    if path.contains(':') || path.starts_with(['\\', '/']) {
        problems.push(format!("{at} must be relative to the repo root (it is '{path}')"));
    }
}

/// Fields the object has that this part of a profile can't: each with the field it most likely means.
pub fn known_fields(object: &Value, known: &[&str], at: &str, problems: &mut Vec<String>) {
    for name in keys(object).into_iter().filter(|name| !known.contains(name)) {
        // A field one or two typos away (one for short names) is most likely the one meant.
        let closest = known.iter().min_by_key(|candidate| edit_distance(name, candidate)).copied().unwrap_or_default();
        let limit = if name.chars().count() >= 4 { 2 } else { 1 };
        let hint = if edit_distance(name, closest) <= limit {
            format!("did you mean {closest}?")
        } else {
            format!("it can have: {}", known.join(", "))
        };
        let prefix = if at.is_empty() { String::new() } else { format!("{at}.") };
        problems.push(format!("{prefix}{name} is not a profile field - {hint}"));
    }
}

/// How many letters to change, add or drop to get from one word to the other (case doesn't count).
pub fn edit_distance(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.to_lowercase().chars().collect(), b.to_lowercase().chars().collect());
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut current = vec![i + 1; b.len() + 1];
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            current[j + 1] = (previous[j + 1] + 1).min(current[j] + 1).min(previous[j] + cost);
        }
        previous = current;
    }
    previous[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edit_distance_counts_letters_ignoring_case() {
        assert_eq!(edit_distance("swager", "swagger"), 1);
        assert_eq!(edit_distance("Prot", "port"), 2);
        assert_eq!(edit_distance("", "abc"), 3);
        assert_eq!(edit_distance("Name", "name"), 0);
    }

    #[test]
    fn an_apps_health_holds_positive_targets_a_path_and_an_http_status() {
        let mut problems = Vec::new();
        let good = serde_json::json!({ "p95Ms": 500, "errorRate": 0.02, "path": "/healthz", "expectedStatus": 200 });
        check_health(&good, "apps[0].health", &mut problems);
        assert!(problems.is_empty(), "{problems:?}");
        let bad = serde_json::json!({ "p95Ms": 0, "errorRate": 2, "cpuPercent": 150, "path": "healthz", "expectedStatus": 42, "p59Ms": 1 });
        check_health(&bad, "apps[0].health", &mut problems);
        for said in [
            "p95Ms must be",
            "errorRate must be",
            "cpuPercent must be",
            "path must start with /",
            "expectedStatus",
            "p59Ms",
        ] {
            assert!(problems.iter().any(|problem| problem.contains(said)), "{said}: {problems:?}");
        }
    }

    #[test]
    fn every_field_once_and_sorted() {
        let all = FIELDS.all();
        assert!(all.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(all.contains(&"servedBy") && all.contains(&"packageScript"));
    }
}
