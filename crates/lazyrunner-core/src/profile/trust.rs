//! Trusting a repo's own profile. It says what lazyrunner runs, and a clone or a branch can bring one you haven't
//! seen, so it is used only once you trust it, and again after its file changes: the trust store keeps the hash you
//! trusted per file. Bundled profiles and your own come with lazyrunner or from you.

use std::io::{self, Write};
use std::path::Path;

use serde_json::{Map, Value};

use super::{AppKind, Project, Source};

fn store_key(project: &Project) -> String {
    project.path.display().to_string()
}

pub fn is_trusted(project: &Project, store: &Path) -> bool {
    if project.source != Source::Repo {
        return true;
    }
    read_store(store).get(&store_key(project)).and_then(Value::as_str) == Some(project.file_hash.as_str())
}

/// Trusts the profile as it is now. A store that's there but can't be read is left alone (an error), so the other
/// trusted files aren't forgotten.
pub fn approve(project: &Project, store: &Path) -> io::Result<()> {
    let mut trusted = match crate::json::read_for_update(store)? {
        Some(Value::Object(trusted)) => trusted,
        None => Map::new(),
        Some(_) => return Err(io::Error::new(io::ErrorKind::InvalidData, "the trust store must hold one JSON object")),
    };
    trusted.insert(store_key(project), Value::String(project.file_hash.clone()));
    crate::config::write(store, &Value::Object(trusted)).map_err(io::Error::other)
}

/// The trusted files and their hashes; a missing or broken store trusts nothing.
pub fn read_store(store: &Path) -> Map<String, Value> {
    match crate::json::read(store) {
        Some(Value::Object(trusted)) => trusted,
        _ => Map::new(),
    }
}

/// What a profile runs, one line per app, for the trust question.
pub fn summary(project: &Project) -> Vec<String> {
    project
        .apps
        .iter()
        .map(|app| match &app.kind {
            AppKind::Npm { dir, variants } => {
                let scripts: Vec<&str> = variants.iter().map(|variant| variant.script.as_str()).collect();
                format!("  {}: npm run {} in {dir}", app.name, scripts.join(" or "))
            }
            AppKind::Dotnet { project, launch_profile, .. } => {
                format!("  {}: dotnet build + run {project} (launch profile {launch_profile})", app.name)
            }
        })
        .collect()
}

/// Asks before using a repo profile that isn't trusted yet, or changed since; `ask` shows the question and returns
/// the answer. True when it may run.
pub fn confirm(
    project: &Project,
    store: &Path,
    out: &mut dyn Write,
    ask: &mut dyn FnMut(&str) -> String,
) -> io::Result<bool> {
    if is_trusted(project, store) {
        return Ok(true);
    }
    let state = if read_store(store).contains_key(&store_key(project)) {
        "changed since you trusted it"
    } else {
        "is this repo's own profile for lazyrunner"
    };
    writeln!(out, "{} {state}. {} runs:", project.path.display(), project.name)?;
    for line in summary(project) {
        writeln!(out, "{line}")?;
    }
    let answer = ask("Trust it and run these? [y/N]");
    if !matches!(answer.trim().to_lowercase().as_str(), "y" | "yes") {
        writeln!(out, "Not trusted - lazyrunner did not start.")?;
        return Ok(false);
    }
    approve(project, store)?;
    Ok(true)
}
