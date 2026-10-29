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
    pub variant: &'static [&'static str],
    pub when: &'static [&'static str],
    pub ready: &'static [&'static str],
    pub migrations: &'static [&'static str],
}

pub const FIELDS: Fields = Fields {
    top: &["$schema", "schemaVersion", "extends", "id", "name", "detect", "watch", "apps"],
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
    ],
    npm: &["key", "name", "icon", "nerdIcon", "type", "dir", "variants", "env", "tests"],
    variant: &["label", "when", "script", "port", "servedBy", "ready"],
    when: &["packageScript"],
    ready: &["file"],
    migrations: &["project", "contexts"],
};

impl Fields {
    pub fn all(&self) -> Vec<&'static str> {
        let mut all: Vec<&str> =
            [self.top, self.detect, self.dotnet, self.npm, self.variant, self.when, self.ready, self.migrations]
                .concat();
        all.sort_unstable();
        all.dedup();
        all
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
    problems
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
        _ => problems.push(format!("{at}.type must be 'dotnet' or 'npm' (it is '{}')", text(field(app, "type")))),
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
    fn every_field_once_and_sorted() {
        let all = FIELDS.all();
        assert!(all.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(all.contains(&"servedBy") && all.contains(&"packageScript"));
    }
}
