//! Finding a folder's profile, reading it, applying `extends`, checking it, and turning it into a [`Project`].

use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow, bail};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use super::{App, AppKind, Migrations, ProfileDirs, Project, REPO_PROFILE_FILE, Ready, Source, Variant, check, native};
use crate::json::{self, field, list, text, truthy};

/// A profile lazyrunner knows besides a repo's own.
#[derive(Clone, Debug)]
pub struct Known {
    pub path: PathBuf,
    pub source: Source,
    pub data: Value,
}

/// The profile a folder belongs to, found but not loaded yet ([`resolve`] loads it).
#[derive(Clone, Debug)]
pub struct Found {
    pub path: PathBuf,
    pub source: Source,
    /// The repo root; `None` when not known (a file given on its own).
    pub root: Option<PathBuf>,
}

fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(|| path.display().to_string(), |name| name.to_string_lossy().into_owned())
}

/// A profile file as JSON; it must hold one object.
pub fn read_profile_data(path: &Path) -> Result<Value> {
    Ok(read_profile(path)?.1)
}

/// A profile file's text and its JSON, from one read (what is checked is what trust is given to).
fn read_profile(path: &Path) -> Result<(String, Value)> {
    let file = file_name(path);
    let text =
        json::read_text(path).map_err(|_| anyhow!("lazyrunner: project profile not found: {}", path.display()))?;
    let data: Value =
        serde_json::from_str(&text).map_err(|error| anyhow!("lazyrunner: {file} is not valid JSON - {error}"))?;
    if !data.is_object() {
        bail!("lazyrunner: {file} must hold one JSON object");
    }
    Ok((text, data))
}

/// The profiles lazyrunner knows besides a repo's own, yours first (one with the id of a bundled profile replaces
/// it), each folder in name order, plus the files it had to leave out and why.
pub fn known_profiles(dirs: &ProfileDirs) -> Result<(Vec<Known>, Vec<String>)> {
    if let Some(only) = &dirs.only {
        return Ok((
            vec![Known { path: only.clone(), source: Source::Given, data: read_profile_data(only)? }],
            Vec::new(),
        ));
    }
    let mut known: Vec<Known> = Vec::new();
    let mut warnings = Vec::new();
    for (source, dir) in [(Source::User, &dirs.user), (Source::Bundled, &dirs.bundled)] {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        let mut files: Vec<PathBuf> = entries
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| path.is_file())
            .filter(|path| {
                let name = file_name(path).to_lowercase();
                name.ends_with(".json") && !name.ends_with(".schema.json")
            })
            .collect();
        files.sort_by_key(|path| file_name(path).to_lowercase());
        for path in files {
            let data = match read_profile_data(&path) {
                Ok(data) => data,
                Err(error) => {
                    warnings.push(error.to_string());
                    continue;
                }
            };
            let id = text(field(&data, "id"));
            if known.iter().any(|other| text(field(&other.data, "id")).eq_ignore_ascii_case(&id)) {
                continue;
            }
            known.push(Known { path, source, data });
        }
    }
    Ok((known, warnings))
}

/// The project a folder belongs to: walking up from it, the first folder with a `.lazyrunner.json` or the marker
/// file of a known profile.
pub fn find_project_profile(start: &Path, dirs: &ProfileDirs) -> Result<Option<Found>> {
    let (known, _) = known_profiles(dirs)?;
    let mut dir = Some(start);
    while let Some(here) = dir {
        let repo_file = here.join(REPO_PROFILE_FILE);
        if dirs.only.is_none() && repo_file.exists() {
            return Ok(Some(Found { path: repo_file, source: Source::Repo, root: Some(here.to_path_buf()) }));
        }
        for candidate in &known {
            let marker = list(field(field(&candidate.data, "detect"), "anyFile")).first().and_then(|m| m.as_str());
            if let Some(marker) = marker.filter(|marker| !marker.is_empty())
                && here.join(marker).exists()
            {
                return Ok(Some(Found {
                    path: candidate.path.clone(),
                    source: candidate.source,
                    root: Some(here.to_path_buf()),
                }));
            }
        }
        dir = here.parent();
    }
    Ok(None)
}

/// A found profile, checked and turned into a [`Project`]. A broken profile is one error naming the file and every
/// problem in it.
pub fn resolve(found: &Found, dirs: &ProfileDirs) -> Result<Project> {
    let file = file_name(&found.path);
    let (raw, mut data) = read_profile(&found.path)?;
    let in_repo = found.source == Source::Repo;
    if in_repo && truthy(field(&data, "extends")) {
        let extends = text(field(&data, "extends"));
        let (known, _) = known_profiles(dirs)?;
        let Some(base) = known.iter().find(|known| text(field(&known.data, "id")).eq_ignore_ascii_case(&extends))
        else {
            bail!(
                "lazyrunner: {file} extends '{extends}', which is not a known profile (in {} or {})",
                dirs.bundled.display(),
                dirs.user.display()
            );
        };
        data = merge(&base.data, &data);
    }
    let (data, mut problems) = super::preset::apply_all(&data, found.root.as_deref(), dirs);
    problems.extend(check::validate(&data, in_repo));
    if !problems.is_empty() {
        let count = if problems.len() == 1 { "a problem".to_string() } else { format!("{} problems", problems.len()) };
        bail!("lazyrunner: {file} has {count}:\n  - {}", problems.join("\n  - "));
    }
    let mut project = to_project(&data, &found.path);
    if in_repo {
        project.repo_marker = REPO_PROFILE_FILE.to_string();
    }
    project.root = found.root.clone();
    project.source = found.source;
    project.file_hash = text_hash(&raw);
    project.hash = text_hash(&json::sorted(&data));
    Ok(project)
}

/// One profile file, without looking for it.
pub fn import(path: &Path, dirs: &ProfileDirs) -> Result<Project> {
    resolve(&Found { path: path.to_path_buf(), source: Source::Given, root: None }, dirs)
}

/// A repo's changes on top of the profile it extends: each top-level field replaces the base's, except apps, which
/// merge by key (an app's fields replace the base app's; a new key adds an app at the end).
pub fn merge(base: &Value, changes: &Value) -> Value {
    let mut merged: Map<String, Value> = base.as_object().cloned().unwrap_or_default();
    for (key, value) in changes.as_object().into_iter().flatten() {
        if !matches!(key.as_str(), "apps" | "extends" | "$schema") {
            merged.insert(key.clone(), value.clone());
        }
    }
    if truthy(field(changes, "apps")) {
        let mut apps: Vec<Value> = list(field(base, "apps")).into_iter().cloned().collect();
        for app in list(field(changes, "apps")) {
            let key = text(field(app, "key"));
            match apps.iter_mut().find(|existing| text(field(existing, "key")).eq_ignore_ascii_case(&key)) {
                Some(Value::Object(existing)) => {
                    for (name, value) in app.as_object().into_iter().flatten() {
                        existing.insert(name.clone(), value.clone());
                    }
                }
                _ => apps.push(app.clone()),
            }
        }
        merged.insert("apps".into(), Value::Array(apps));
    }
    Value::Object(merged)
}

/// SHA-256 of the text with Windows line endings made Unix ones, as upper-case hex.
pub fn text_hash(text: &str) -> String {
    let digest = Sha256::digest(text.replace("\r\n", "\n").as_bytes());
    digest.iter().map(|byte| format!("{byte:02X}")).collect()
}

/// The checked profile as a [`Project`] (root, source and hashes are filled in by [`resolve`]).
fn to_project(data: &Value, path: &Path) -> Project {
    let apps = list(field(data, "apps")).into_iter().map(to_app).collect();
    Project {
        id: text(field(data, "id")),
        name: text(field(data, "name")),
        path: path.to_path_buf(),
        repo_marker: native(&list(field(field(data, "detect"), "anyFile")).first().map_or(String::new(), |m| text(m))),
        source_dir: native(&list(field(data, "watch")).first().map_or(String::new(), |w| text(w))),
        apps,
        layers: to_layers(field(data, "layers")),
        tasks: to_tasks(field(data, "tasks")),
        root: None,
        source: Source::Given,
        file_hash: String::new(),
        hash: String::new(),
    }
}

/// `"tasks": [{ "key": "seed", "name": "Seed the database", "run": "python seed.py" }]`.
fn to_tasks(tasks: &Value) -> Vec<super::Task> {
    let env = |task: &Value| {
        let vars = field(task, "env").as_object();
        vars.map(|vars| {
            vars.iter().filter_map(|(name, value)| Some((name.clone(), value.as_str()?.to_string()))).collect()
        })
        .unwrap_or_default()
    };
    list(tasks)
        .into_iter()
        .map(|task| super::Task {
            key: text(field(task, "key")),
            name: text(field(task, "name")),
            run: words(field(task, "run")),
            cwd: native(&text(field(task, "cwd"))),
            env: env(task),
        })
        .collect()
}

/// `"layers": { "Orders.Core": { "mustNotReference": ["Orders.Api"] } }`.
fn to_layers(layers: &Value) -> Vec<super::Layer> {
    let Some(layers) = layers.as_object() else { return Vec::new() };
    layers
        .iter()
        .map(|(parts, rule)| super::Layer {
            parts: parts.clone(),
            must_not_reference: list(field(rule, "mustNotReference")).into_iter().map(text).collect(),
        })
        .collect()
}

/// A command: a list of arguments as written, or text split into them (see [`split`]).
pub fn words(value: &Value) -> Vec<String> {
    match value.as_array() {
        Some(arguments) => arguments.iter().map(text).collect(),
        None => split(&text(value)),
    }
}

/// Text split into arguments at spaces; a "quoted part" stays one argument, without its quotes.
pub fn split(line: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let (mut quoted, mut started) = (false, false);
    for c in line.chars() {
        match c {
            '"' => (quoted, started) = (!quoted, true),
            c if c.is_whitespace() && !quoted => {
                if started {
                    words.push(std::mem::take(&mut word));
                    started = false;
                }
            }
            c => {
                word.push(c);
                started = true;
            }
        }
    }
    if started {
        words.push(word);
    }
    words
}

fn to_command(app: &Value) -> super::Command {
    let port_of = |value: &Value| value.as_u64().and_then(|port| u16::try_from(port).ok());
    let port = port_of(field(app, "port"));
    let ready = field(app, "ready");
    let ready = match port_of(field(ready, "port")) {
        Some(port) => Ready::Port(port),
        None if truthy(field(ready, "line")) && truthy(field(ready, "orPort")) => {
            Ready::LineOrPort(text(field(ready, "line")))
        }
        None if truthy(field(ready, "line")) => Ready::Line(text(field(ready, "line"))),
        None if truthy(field(ready, "http")) => Ready::Http(text(field(ready, "http"))),
        None => port.map_or(Ready::Started, Ready::Port),
    };
    let texts = |name: &str| list(field(app, name)).into_iter().map(text).collect();
    super::Command {
        run: words(field(app, "run")),
        cwd: native(&text(field(app, "cwd"))),
        port,
        build: words(field(app, "build")),
        install: words(field(app, "install")),
        setup: words(field(app, "setup")),
        install_from: texts("installFrom"),
        ready,
        watch: texts("watch"),
        errors: texts("errors"),
        warnings: texts("warnings"),
        frames: truthy(field(app, "frames")).then(|| text(field(app, "frames"))),
        preset: truthy(field(app, "preset")).then(|| text(field(app, "preset"))),
        path: texts("path"),
        tools: texts("tools"),
        compose: None,
    }
}

/// An error line in a compose app's output, and a warning line.
const COMPOSE_ERRORS: &str = r"(?i)\b(error|fatal|panic|exception)\b";
const COMPOSE_WARNINGS: &str = r"(?i)\bwarn(ing)?\b";

/// A compose app: a command app whose run is `docker compose up` of its services, up once they are.
fn to_compose(app: &Value) -> super::Command {
    let compose = super::Compose {
        file: truthy(field(app, "file")).then(|| native(&text(field(app, "file")))),
        services: list(field(app, "services")).into_iter().map(text).collect(),
    };
    let mut run = vec![super::DOCKER.to_string()];
    run.extend(compose.args(super::COMPOSE_UP, &[]));
    super::Command {
        run,
        cwd: native(&text(field(app, "cwd"))),
        port: None,
        build: Vec::new(),
        install: Vec::new(),
        setup: Vec::new(),
        install_from: Vec::new(),
        ready: Ready::Services,
        watch: Vec::new(),
        errors: vec![COMPOSE_ERRORS.into()],
        warnings: vec![COMPOSE_WARNINGS.into()],
        frames: None,
        preset: None,
        path: Vec::new(),
        tools: vec![super::DOCKER.into()],
        compose: Some(compose),
    }
}

fn to_app(app: &Value) -> App {
    let optional = |name: &str| truthy(field(app, name)).then(|| text(field(app, name)));
    let kind = match text(field(app, "type")).to_lowercase().as_str() {
        super::TYPE_DOTNET => AppKind::Dotnet {
            project: native(&text(field(app, "project"))),
            launch_profile: text(field(app, "launchProfile")),
            swagger: optional("swagger"),
            args: list(field(app, "args")).into_iter().map(text).collect(),
        },
        super::TYPE_COMMAND => AppKind::Command(Box::new(to_command(app))),
        super::TYPE_COMPOSE => AppKind::Command(Box::new(to_compose(app))),
        _ => {
            let variants = list(field(app, "variants")).into_iter().map(to_variant).collect();
            AppKind::Npm { dir: native(&text(field(app, "dir"))), variants }
        }
    };
    let tests = list(field(app, "tests")).into_iter().map(text).collect();
    let depends_on = list(field(app, "dependsOn")).into_iter().map(text).collect();
    let env = field(app, "env")
        .as_object()
        .map(|vars| vars.iter().filter_map(|(name, value)| Some((name.clone(), value.as_str()?.to_string()))).collect())
        .unwrap_or_default();
    let health = to_health(field(app, "health"));
    let setup = field(app, "migrations");
    let migrations = setup.is_object().then(|| Migrations {
        project: truthy(field(setup, "project")).then(|| native(&text(field(setup, "project")))),
        contexts: list(field(setup, "contexts")).into_iter().map(text).collect(),
    });
    App {
        key: text(field(app, "key")),
        name: text(field(app, "name")),
        icon: optional("icon"),
        nerd_icon: optional("nerdIcon"),
        kind,
        tests,
        migrations,
        env,
        depends_on,
        health,
    }
}

/// An app's `"health"`: what it says, the defaults for the rest.
fn to_health(health: &Value) -> super::Health {
    let defaults = super::Health::default();
    let whole = |name: &str, default: u32| field(health, name).as_f64().map_or(default, |value| value.max(0.0) as u32);
    let rate = field(health, "errorRate").as_f64();
    super::Health {
        path: truthy(field(health, "path")).then(|| text(field(health, "path"))),
        expected_status: field(health, "expectedStatus").as_u64().and_then(|code| u16::try_from(code).ok()),
        p95_ms: whole("p95Ms", defaults.p95_ms),
        error_bp: rate.map_or(defaults.error_bp, |rate| (rate.clamp(0.0, 1.0) * 10_000.0).round() as u32),
        memory_mb: whole("memoryMb", defaults.memory_mb),
        cpu_percent: whole("cpuPercent", defaults.cpu_percent),
    }
}

fn to_variant(variant: &Value) -> Variant {
    let port = field(variant, "port").as_u64().and_then(|port| u16::try_from(port).ok());
    let serves = !field(variant, "port").is_null();
    let label = if truthy(field(variant, "label")) {
        text(field(variant, "label"))
    } else if serves {
        "dev server".into()
    } else {
        "watch build".into()
    };
    let when = field(field(variant, "when"), "packageScript");
    let ready = field(field(variant, "ready"), "file");
    Variant {
        label,
        when: truthy(when).then(|| text(when)),
        script: text(field(variant, "script")),
        port,
        served_by: truthy(field(variant, "servedBy")).then(|| text(field(variant, "servedBy"))),
        ready_file: truthy(ready).then(|| native(&text(ready))),
    }
}

#[cfg(test)]
mod health_tests {
    use super::*;

    #[test]
    fn an_apps_health_takes_what_it_says_and_the_defaults_for_the_rest() {
        let health = to_health(
            &serde_json::json!({ "p95Ms": 500, "errorRate": 0.02, "path": "/healthz", "expectedStatus": 200 }),
        );
        assert_eq!((health.p95_ms, health.error_bp, health.memory_mb, health.cpu_percent), (500, 200, 2048, 90));
        assert_eq!((health.path.as_deref(), health.expected_status), (Some("/healthz"), Some(200)));
        assert_eq!(to_health(&Value::Null), crate::profile::Health::default());
    }
}
