//! `lazyrunner profile`: check a profile (its fields, the schema, and the checkout it is for) and propose one for a
//! checkout (init); plus the field reference in projects/README.md, made from profile.schema.json. Written so a
//! person and an LLM can both write a profile with them.

pub mod init;
pub mod reference;

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use anyhow::{Result, anyhow};
use regex::Regex;
use serde::Serialize;
use serde_json::{Value, json};

use crate::apptypes;
use crate::json::{field, list, truthy};
use crate::profile::{self, Found, ProfileDirs, Project, REPO_PROFILE_FILE, Source};

/// profile.schema.json, as shipped.
pub const SCHEMA: &str = include_str!("../../../../projects/profile.schema.json");

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
    Info,
}

/// One thing `profile check` found: `at` is the field it is about (empty when the whole profile).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Problem {
    pub level: Severity,
    pub at: String,
    pub message: String,
}

impl Problem {
    pub fn new(level: Severity, at: impl Into<String>, message: impl Into<String>) -> Self {
        Self { level, at: at.into(), message: message.into() }
    }
}

/// How a line of output looks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Accent,
    Ok,
    Error,
    Warning,
    Dim,
    Plain,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Note {
    pub text: String,
    pub tone: Tone,
}

fn note(text: impl Into<String>, tone: Tone) -> Note {
    Note { text: text.into(), tone }
}

/// What a `profile` command printed and returned: `data` (JSON) is the output proper; `notes` are for the person.
#[derive(Debug, Default)]
pub struct Outcome {
    pub code: i32,
    pub data: Option<String>,
    pub notes: Vec<Note>,
}

/// Which profile a check is about: the file given, else the one lazyrunner would use here.
pub fn check_target(path: Option<&Path>, here: &Path, dirs: &ProfileDirs) -> Result<Found> {
    let Some(path) = path else {
        return profile::find_project_profile(here, dirs)?.ok_or_else(|| {
            anyhow!(
                "lazyrunner: no profile applies here - give a file: lazyrunner profile check <file>, or write one: lazyrunner profile init"
            )
        });
    };
    let full = std::path::absolute(here.join(path))?;
    if !full.exists() {
        return Err(anyhow!("lazyrunner: {} doesn't exist", full.display()));
    }
    if full.file_name().is_some_and(|name| name == REPO_PROFILE_FILE) {
        let root = full.parent().map(Path::to_path_buf);
        return Ok(Found { path: full, source: Source::Repo, root });
    }
    let data = profile::read_profile_data(&full)?;
    if truthy(field(&data, "extends")) || !truthy(field(&data, "detect")) {
        return Ok(Found { path: full, source: Source::Repo, root: Some(here.to_path_buf()) }); // a draft of a repo's profile
    }
    let marker = list(field(field(&data, "detect"), "anyFile")).first().and_then(|m| m.as_str()).map(str::to_string);
    let root = here.ancestors().find(|dir| marker.as_ref().is_some_and(|marker| dir.join(marker).exists()));
    Ok(Found { path: full, source: Source::Given, root: root.map(Path::to_path_buf) })
}

/// Every problem with a profile, plus the profile when it loads. The checkout checks need the target's root.
pub fn problems(target: &Found, dirs: &ProfileDirs) -> (Option<Project>, Vec<Problem>) {
    let project = match profile::resolve(target, dirs) {
        Ok(project) => project,
        Err(error) => return (None, problems_from_message(&error.to_string())),
    };
    let mut problems = schema_problems(&target.path);
    match &target.root {
        None => problems.push(Problem::new(
            Severity::Info,
            "detect",
            format!(
                "no folder from here up has {}, so the checkout wasn't checked - run it inside the repo",
                project.repo_marker
            ),
        )),
        Some(root) => problems.extend(checkout_problems(&project, root)),
    }
    (Some(project), problems)
}

static PROBLEM_COUNT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^lazyrunner: .* has (a problem|\d+ problems):$").expect("a valid pattern"));
static FIELD_AT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([\w$]+(\[\d+\])?( \(\w+\))?(\.\w+(\[\d+\])?)*) ").expect("a valid pattern"));

/// The loader's message, one problem per line, each with the field it starts with.
fn problems_from_message(message: &str) -> Vec<Problem> {
    let mut problems = Vec::new();
    for line in message.lines() {
        let text = line.trim_matches([' ', '-']);
        if text.is_empty() || PROBLEM_COUNT.is_match(text) {
            continue;
        }
        let text = text.strip_prefix("lazyrunner: ").unwrap_or(text);
        let at = FIELD_AT.captures(text).map_or("", |found| found.get(1).map_or("", |m| m.as_str()));
        problems.push(Problem::new(Severity::Error, at, text[at.len()..].trim()));
    }
    problems
}

static VALIDATOR: LazyLock<jsonschema::Validator> = LazyLock::new(|| {
    let schema: Value = serde_json::from_str(SCHEMA).expect("profile.schema.json is JSON");
    jsonschema::validator_for(&schema).expect("profile.schema.json is a valid schema")
});

/// What profile.schema.json says about the file (nothing when it passes or isn't JSON).
pub fn schema_problems(path: &Path) -> Vec<Problem> {
    let Ok(data) = profile::read_profile_data(path) else { return Vec::new() };
    schema_errors(&data).into_iter().map(|message| Problem::new(Severity::Error, "schema", message)).collect()
}

pub fn schema_errors(data: &Value) -> Vec<String> {
    VALIDATOR
        .iter_errors(data)
        .map(|error| {
            let at = error.instance_path().to_string();
            if at.is_empty() { error.to_string() } else { format!("{error} at '{at}'") }
        })
        .collect()
}

/// The checkout: the marker and watch folder exist, each app is set up, and no two apps want one port.
pub fn checkout_problems(project: &Project, root: &Path) -> Vec<Problem> {
    let mut problems = Vec::new();
    if project.source != Source::Repo && !root.join(&project.repo_marker).exists() {
        let message = format!("{} is not in {}", project.repo_marker, root.display());
        problems.push(Problem::new(Severity::Error, "detect.anyFile", message));
    }
    if !root.join(&project.source_dir).exists() {
        let message =
            format!("{} is not a folder in {}, so changes are never noticed", project.source_dir, root.display());
        problems.push(Problem::new(Severity::Warning, "watch", message));
    }
    let mut ports: Vec<(u16, Vec<&str>)> = Vec::new();
    for (index, app) in project.apps.iter().enumerate() {
        problems.extend(apptypes::setup_problems(root, app, &format!("apps[{index}] ({})", app.key), &project.apps));
        if let Some(port) = apptypes::port(root, app, &project.apps) {
            match ports.iter_mut().find(|(used, _)| *used == port) {
                Some((_, names)) => names.push(&app.name),
                None => ports.push((port, vec![&app.name])),
            }
        }
    }
    for (port, names) in ports.iter().filter(|(_, names)| names.len() > 1) {
        let message =
            format!("{} both use port {port} on this branch - only one can listen on it", names.join(" and "));
        problems.push(Problem::new(Severity::Error, "apps", message));
    }
    problems
}

fn plural(count: usize, word: &str) -> String {
    format!("{count} {word}{}", if count == 1 { "" } else { "s" })
}

/// `lazyrunner profile check [file] [--json]`.
pub fn check(path: Option<&Path>, as_json: bool, here: &Path, dirs: &ProfileDirs) -> Outcome {
    let target = match check_target(path, here, dirs) {
        Ok(target) => target,
        Err(error) => return Outcome { code: 1, data: None, notes: vec![note(error.to_string(), Tone::Error)] },
    };
    let (project, problems) = problems(&target, dirs);
    let errors = problems.iter().filter(|problem| problem.level == Severity::Error).count();
    let code = i32::from(errors > 0);
    if as_json {
        let report = json!({
            "file": target.path.display().to_string(),
            "source": target.source.name(),
            "root": target.root.as_ref().map(|root| root.display().to_string()),
            "ok": errors == 0,
            "problems": problems,
        });
        let data = serde_json::to_string_pretty(&report).unwrap_or_default();
        return Outcome { code, data: Some(data), notes: Vec::new() };
    }
    let mut notes = Vec::new();
    let place = target.root.as_ref().map_or(String::new(), |root| format!(" for {}", root.display()));
    notes.push(note(format!("{} ({} profile{place})", target.path.display(), target.source.name()), Tone::Accent));
    for problem in &problems {
        let (mark, tone) = match problem.level {
            Severity::Error => ("x", Tone::Error),
            Severity::Warning => ("!", Tone::Warning),
            Severity::Info => ("-", Tone::Dim),
        };
        let at = if problem.at.is_empty() { String::new() } else { format!("{}: ", problem.at) };
        notes.push(note(format!("  {mark} {at}{}", problem.message), tone));
    }
    let warnings = problems.iter().filter(|problem| problem.level == Severity::Warning).count();
    if errors > 0 {
        let also = if warnings > 0 { format!(", {}", plural(warnings, "warning")) } else { String::new() };
        notes.push(note(format!("{}{also} - fix and check again", plural(errors, "error")), Tone::Error));
        return Outcome { code, data: None, notes };
    }
    let with = if warnings > 0 { format!(" with {}", plural(warnings, "warning")) } else { String::new() };
    let apps = project.map_or(String::new(), |project| {
        let names: Vec<&str> = project.apps.iter().map(|app| app.name.as_str()).collect();
        format!(" - {} apps: {}", project.apps.len(), names.join(", "))
    });
    notes.push(note(format!("OK{with}{apps}"), Tone::Ok));
    Outcome { code, data: None, notes }
}

/// The usage, for `lazyrunner profile` without an action (code 0) or with an unknown one (code 1).
pub fn usage(known_action: bool) -> Outcome {
    let lines = [
        "lazyrunner profile check [file] [--json]   check a profile: its fields, and against the checkout it is for",
        "lazyrunner profile init [folder] [--save]  propose a .lazyrunner.json for a checkout (--save writes it)",
        "projects/README.md describes profiles and how to write one.",
    ];
    Outcome {
        code: i32::from(!known_action),
        data: None,
        notes: lines.iter().map(|line| note(*line, Tone::Plain)).collect(),
    }
}

/// Where `init` looks: the folder given, else the repo this folder is in (or this folder).
pub fn init_root(dir: Option<&Path>, here: &Path) -> Result<PathBuf> {
    let start = dir.map_or_else(|| here.to_path_buf(), |dir| here.join(dir));
    if !start.is_dir() {
        return Err(anyhow!("lazyrunner: {} is not a folder", start.display()));
    }
    let root = std::path::absolute(&start)?;
    if dir.is_some() {
        return Ok(root);
    }
    let top = std::process::Command::new("git").arg("-C").arg(&root).args(["rev-parse", "--show-toplevel"]).output();
    match top {
        Ok(output) if output.status.success() => {
            let top = String::from_utf8_lossy(&output.stdout).trim().to_string();
            Ok(if top.is_empty() { root } else { PathBuf::from(profile::native(&top)) })
        }
        _ => Ok(root),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loader_messages_become_problems_with_their_field() {
        let message = "lazyrunner: x.json has 3 problems:\n  - name is required\n  - apps[0] (orders).project is required: the .csproj to run\n  - detect.anyFile: one file for now (it has 2)";
        let problems = problems_from_message(message);
        let at: Vec<&str> = problems.iter().map(|problem| problem.at.as_str()).collect();
        assert_eq!(at, ["name", "apps[0] (orders).project", ""]);
        assert_eq!(problems[1].message, "is required: the .csproj to run");
        assert_eq!(problems[2].message, "detect.anyFile: one file for now (it has 2)");
    }

    #[test]
    fn the_schema_compiles_and_names_where_it_fails() {
        assert!(
            schema_errors(&json!({ "schemaVersion": 1, "id": "x", "name": "X", "watch": ["s"], "apps": [] }))
                .iter()
                .any(|e| e.contains("/apps"))
        );
    }
}
