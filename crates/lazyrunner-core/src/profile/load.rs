//! Finding a folder's profile, reading it, applying `extends`, checking it, and turning it into a [`Project`].

use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow, bail};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use super::{App, AppKind, ProfileDirs, Project, REPO_PROFILE_FILE, Source, Variant, check, native};
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
    let problems = check::validate(&data, in_repo);
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
        root: None,
        source: Source::Given,
        file_hash: String::new(),
        hash: String::new(),
    }
}

fn to_app(app: &Value) -> App {
    let optional = |name: &str| truthy(field(app, name)).then(|| text(field(app, name)));
    let kind = if text(field(app, "type")).eq_ignore_ascii_case("dotnet") {
        AppKind::Dotnet {
            project: native(&text(field(app, "project"))),
            launch_profile: text(field(app, "launchProfile")),
            swagger: optional("swagger"),
        }
    } else {
        let variants = list(field(app, "variants")).into_iter().map(to_variant).collect();
        AppKind::Npm { dir: native(&text(field(app, "dir"))), variants }
    };
    let tests = list(field(app, "tests")).into_iter().map(text).collect();
    App { key: text(field(app, "key")), name: text(field(app, "name")), icon: optional("icon"), kind, tests }
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
