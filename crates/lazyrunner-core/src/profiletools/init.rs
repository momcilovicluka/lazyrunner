//! `lazyrunner profile init`: a first profile for a checkout, from its solutions, projects and package.json files,
//! to check and edit.

use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Map, Value, json};

use super::{Note, Outcome, Severity, Tone, init_root, note, problems};
use crate::apptypes::is_test_project;
use crate::json::{self, field, field_ci, keys, text, truthy};
use crate::profile::{Found, ProfileDirs, REPO_PROFILE_FILE, Source};

const SKIP_DIRS: [&str; 12] =
    ["node_modules", "bin", "obj", ".git", ".vs", ".idea", "dist", "out", "build", "packages", "artifacts", "coverage"];
const SCAN_DEPTH: usize = 6;
/// The ports dev tools use when a script names none.
const VITE_PORT: u16 = 5173;
const NG_PORT: u16 = 4200;
const WEBPACK_PORT: u16 = 8080;
const GUESSED_PORT: u16 = 3000;

fn file_name(path: &Path) -> String {
    path.file_name().map_or(String::new(), |name| name.to_string_lossy().into_owned())
}

/// The files init looks at (solutions, projects and package.json), breadth first in name order, skipping build
/// output, packages and the like.
pub fn scaffold_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut pending = VecDeque::from([(root.to_path_buf(), 0)]);
    while let Some((dir, level)) = pending.pop_front() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        // Links are left alone: one in an untrusted repo could point at the whole disk.
        let mut entries: Vec<(PathBuf, std::fs::FileType)> = entries
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| Some((entry.path(), entry.file_type().ok()?)))
            .filter(|(_, kind)| !kind.is_symlink())
            .collect();
        entries.sort_by_key(|(path, _)| file_name(path).to_lowercase());
        for (path, kind) in entries {
            let name = file_name(&path).to_lowercase();
            if kind.is_dir() {
                if !SKIP_DIRS.contains(&name.as_str()) && level < SCAN_DEPTH {
                    pending.push_back((path, level + 1));
                }
            } else if name.ends_with(".csproj") || name.ends_with(".sln") || name == "package.json" {
                files.push(path);
            }
        }
    }
    files
}

/// A key from a name: lowercase letters and digits, starting with a letter, unique among `taken`.
pub fn app_key(name: &str, taken: &mut HashSet<String>) -> String {
    let mut key: String =
        name.to_lowercase().chars().filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit()).collect();
    if !key.starts_with(|c: char| c.is_ascii_lowercase()) {
        key = format!("app{key}");
    }
    let mut unique = key.clone();
    let mut n = 2;
    while taken.contains(&unique) {
        unique = format!("{key}{n}");
        n += 1;
    }
    taken.insert(unique.clone());
    unique
}

/// The folder all these relative paths share ("." when none).
pub fn common_folder(paths: &[String]) -> String {
    let split = |path: &str| -> Vec<String> {
        path.split(['/', '\\']).filter(|part| !part.is_empty()).map(str::to_string).collect()
    };
    let Some(first) = paths.first() else { return ".".into() };
    let mut common = split(first);
    for path in paths {
        let parts = split(path);
        let same = common.iter().zip(&parts).take_while(|(a, b)| a == b).count();
        common.truncate(same);
    }
    if common.is_empty() { ".".into() } else { common.join("/") }
}

fn relative(root: &Path, path: &Path) -> String {
    let relative = path.strip_prefix(root).unwrap_or(path).display().to_string().replace('\\', "/");
    if relative.is_empty() { ".".into() } else { relative }
}

fn pattern(regex: &str) -> Regex {
    Regex::new(regex).expect("a valid pattern")
}

static NODE_FILE: LazyLock<Regex> = LazyLock::new(|| pattern(r"(?i)\bnode\s+(?:--\S+\s+)*([\w./\\-]+\.[cm]?js)\b"));
static CODE_PORT: LazyLock<Regex> =
    LazyLock::new(|| pattern(r"(?i)\bport\s*[=:]\s*(\d{4,5})\b|\.listen\(\s*(\d{4,5})\b"));
static VITE_CONFIG_PORT: LazyLock<Regex> = LazyLock::new(|| pattern(r"(?i)\bport\s*:\s*(\d{4,5})\b"));
static ANGULAR_PORT: LazyLock<Regex> = LazyLock::new(|| pattern(r#"(?i)"port"\s*:\s*(\d{4,5})\b"#));
static ENV_PORT: LazyLock<Regex> = LazyLock::new(|| pattern(r"(?im)^\s*PORT\s*=\s*(\d{4,5})\b"));
static SCRIPT_PORT: LazyLock<Regex> =
    LazyLock::new(|| pattern(r"(?i)(?:--port|(?:^|\s)-p)[= ]?(\d{2,5})\b|\bPORT=(\d{2,5})\b"));
static LONE_PORT: LazyLock<Regex> = LazyLock::new(|| pattern(r"(?i)\s(\d{4,5})(\s|$)"));
static VITE: LazyLock<Regex> = LazyLock::new(|| pattern(r"(?i)\bvite\b"));
static NG_SERVE: LazyLock<Regex> = LazyLock::new(|| pattern(r"(?i)\bng\b.*\bserve\b"));
static WEBPACK: LazyLock<Regex> = LazyLock::new(|| pattern(r"(?i)\bwebpack\b"));
static SWAGGER_PACKAGE: LazyLock<Regex> = LazyLock::new(|| pattern(r"(?i)Swashbuckle|NSwag"));
static USE_SWAGGER: LazyLock<Regex> = LazyLock::new(|| pattern(r"(?i)UseSwagger"));
static WATCH_SCRIPT: LazyLock<Regex> = LazyLock::new(|| pattern(r"(?i)watch"));
static NOT_ID: LazyLock<Regex> = LazyLock::new(|| pattern(r"[^a-z0-9-]+"));

/// The number in the first (or second) group of a match.
fn port_of(regex: &Regex, text: &str) -> Option<u16> {
    let found = regex.captures(text)?;
    found.get(1).or_else(|| found.get(2)).and_then(|m| m.as_str().parse().ok())
}

/// The port an npm app's files name: the server file a `node <file>` script runs (a PORT constant, listen(4300)),
/// the Vite config (server.port), angular.json (port) or .env (PORT=).
pub fn config_port(dir: &Path, command: &str) -> Option<u16> {
    let mut candidates: Vec<(String, &Regex)> = Vec::new();
    if let Some(found) = NODE_FILE.captures(command) {
        candidates.push((crate::profile::native(&found[1]), &CODE_PORT));
    }
    for file in ["vite.config.ts", "vite.config.js", "vite.config.mjs"] {
        candidates.push((file.into(), &VITE_CONFIG_PORT));
    }
    candidates.push(("angular.json".into(), &ANGULAR_PORT));
    candidates.push((".env".into(), &ENV_PORT));
    candidates.into_iter().find_map(|(file, regex)| {
        let path = dir.join(file);
        if !path.is_file() {
            return None;
        }
        port_of(regex, &json::read_text(&path).ok()?)
    })
}

/// The apps init proposes, the notes for the person, and the .NET code folders (for watch).
pub struct Candidates {
    pub apps: Vec<Map<String, Value>>,
    pub notes: Vec<String>,
    pub code_folders: Vec<String>,
}

/// Each runnable .NET project (a launch profile with an applicationUrl, not a test project) and each package.json
/// with a dev, start or serve script.
pub fn find_candidates(root: &Path, files: &[PathBuf]) -> Candidates {
    let mut found = Candidates { apps: Vec::new(), notes: Vec::new(), code_folders: Vec::new() };
    let is_csproj = |path: &&PathBuf| file_name(path).to_lowercase().ends_with(".csproj");
    for file in files.iter().filter(is_csproj) {
        let project_text = json::read_text(file).unwrap_or_default();
        if is_test_project(&project_text) {
            continue;
        }
        let dir = file.parent().unwrap_or(root);
        found.code_folders.push(relative(root, dir));
        if let Some(app) = dotnet_candidate(root, file, &project_text, &mut found.notes) {
            found.apps.push(app);
        }
    }
    for file in files.iter().filter(|path| file_name(path).eq_ignore_ascii_case("package.json")) {
        if let Some(app) = npm_candidate(root, file, &mut found.notes) {
            found.apps.push(app);
        }
    }
    found
}

fn dotnet_candidate(
    root: &Path,
    file: &Path,
    project_text: &str,
    notes: &mut Vec<String>,
) -> Option<Map<String, Value>> {
    let dir = file.parent().unwrap_or(root);
    let settings = json::read(&dir.join("Properties").join("launchSettings.json")).unwrap_or(Value::Null);
    let profiles = field(&settings, "profiles");
    let mut launch: Vec<&str> = keys(profiles)
        .into_iter()
        .filter(|name| {
            let profile = field(profiles, name);
            truthy(field(profile, "applicationUrl"))
                && text(field(profile, "commandName")).eq_ignore_ascii_case("Project")
        })
        .collect();
    launch.sort_by_key(|name| name.to_lowercase());
    let path = relative(root, file);
    if launch.is_empty() {
        notes.push(format!(
            "skipped {path} - no launch profile with an applicationUrl, so lazyrunner can't tell where it listens"
        ));
        return None;
    }
    let name = file.file_stem().map_or(String::new(), |stem| stem.to_string_lossy().into_owned());
    let profile = launch.iter().find(|launch| launch.eq_ignore_ascii_case(&name)).unwrap_or(&launch[0]);
    let mut app = Map::new();
    for (key, value) in
        [("key", ""), ("name", &name), ("type", "dotnet"), ("project", &path), ("launchProfile", profile)]
    {
        app.insert(key.into(), json!(value));
    }
    let program = json::read_text(&dir.join("Program.cs")).unwrap_or_default();
    if SWAGGER_PACKAGE.is_match(project_text) || USE_SWAGGER.is_match(&program) {
        app.insert("swagger".into(), json!("/swagger"));
    }
    Some(app)
}

fn npm_candidate(root: &Path, file: &Path, notes: &mut Vec<String>) -> Option<Map<String, Value>> {
    let dir = file.parent().unwrap_or(root);
    let path = relative(root, dir);
    let data = json::read(file).unwrap_or(Value::Null);
    let scripts = field(&data, "scripts");
    let Some(script) = ["dev", "start", "serve"].into_iter().find(|script| field_ci(scripts, script).is_some()) else {
        let watch: Vec<&str> = keys(scripts).into_iter().filter(|name| WATCH_SCRIPT.is_match(name)).collect();
        if !watch.is_empty() {
            notes.push(format!(
                "{path}/package.json has no dev, start or serve script but has {}: if a .NET app serves that build, add it as a watch build (servedBy)",
                watch.join(", ")
            ));
        }
        return None;
    };
    let command = field_ci(scripts, script).map(text).unwrap_or_default();
    // The port: said in the script (--port 4300, -p 4300, PORT=4300, or a lone number), else in the files it
    // uses, else the tool's default.
    let port = port_of(&SCRIPT_PORT, &command)
        .or_else(|| port_of(&LONE_PORT, &command))
        .or_else(|| config_port(dir, &command))
        .unwrap_or_else(|| {
            if VITE.is_match(&command) {
                VITE_PORT
            } else if NG_SERVE.is_match(&command) {
                NG_PORT
            } else if WEBPACK.is_match(&command) {
                WEBPACK_PORT
            } else {
                notes.push(format!(
                    "{path}/package.json: '{script}' runs '{command}' - port {GUESSED_PORT} is a guess, check it"
                ));
                GUESSED_PORT
            }
        });
    let mut name = if path == "." { text(field(&data, "name")) } else { file_name(dir) };
    if name.is_empty() {
        name = file_name(root);
    }
    let mut chars = name.chars();
    let name: String = chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or_default();
    let mut app = Map::new();
    app.insert("key".into(), json!(""));
    app.insert("name".into(), json!(name));
    app.insert("type".into(), json!("npm"));
    app.insert("dir".into(), json!(path));
    app.insert("variants".into(), json!([{ "script": script, "port": port }]));
    Some(app)
}

/// A first profile for a checkout, and the notes about what init left out or guessed.
pub fn scaffold(root: &Path) -> (Map<String, Value>, Vec<String>) {
    let files = scaffold_files(root);
    let mut found = find_candidates(root, &files);
    let mut taken = HashSet::new();
    for app in &mut found.apps {
        let key = app_key(&text(&app["name"]), &mut taken);
        app.insert("key".into(), json!(key));
    }
    let folder = file_name(root);
    let mut id = NOT_ID.replace_all(&folder.to_lowercase(), "-").trim_matches('-').to_string();
    if !id.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit()) {
        id = "project".into();
    }
    let mut profile = Map::new();
    profile.insert("schemaVersion".into(), json!(1));
    profile.insert("id".into(), json!(id));
    profile.insert("name".into(), json!(folder));
    let solution =
        files.iter().find(|file| file_name(file).to_lowercase().ends_with(".sln") && file.parent() == Some(root));
    if let Some(solution) = solution {
        profile.insert("detect".into(), json!({ "anyFile": [file_name(solution)] }));
    }
    // watch: the folder all the .NET code shares (not the tests); its changes are what needs a rebuild.
    profile.insert("watch".into(), json!([common_folder(&found.code_folders)]));
    profile.insert("apps".into(), Value::Array(found.apps.into_iter().map(Value::Object).collect()));
    (profile, found.notes)
}

fn create_new(path: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(format!("{text}\n").as_bytes())
}

/// The proposal in a new temp file of its own, for checking it.
fn temp_profile(text: &str) -> std::io::Result<PathBuf> {
    let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    for attempt in 0..100u32 {
        let path = std::env::temp_dir().join(format!("lazyrunner-init-{}-{seed:x}-{attempt}.json", std::process::id()));
        match create_new(&path, text) {
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            other => return other.map(|()| path),
        }
    }
    Err(std::io::Error::other("no free temp file name"))
}

/// `lazyrunner profile init [folder] [--save]`: the profile goes to `data` (unless saved), the rest to `notes`.
pub fn init(dir: Option<&Path>, save: bool, here: &Path, dirs: &ProfileDirs) -> Outcome {
    let fail = |text: String, extra: Vec<Note>| {
        let mut notes = vec![note(text, Tone::Error)];
        notes.extend(extra);
        Outcome { code: 1, data: None, notes }
    };
    let root = match init_root(dir, here) {
        Ok(root) => root,
        Err(error) => return fail(error.to_string(), Vec::new()),
    };
    let target = root.join(REPO_PROFILE_FILE);
    if save && target.exists() {
        return fail(
            format!("lazyrunner: {} exists already - edit it, then: lazyrunner profile check", target.display()),
            Vec::new(),
        );
    }
    let (profile, scaffold_notes) = scaffold(&root);
    let dim = |text: &String| note(format!("  - {text}"), Tone::Dim);
    let apps: Vec<String> = json::list(&profile["apps"]).into_iter().map(|app| text(field(app, "name"))).collect();
    if apps.is_empty() {
        let message = format!(
            "lazyrunner: found nothing to run in {} - no .csproj with a launch profile that has an applicationUrl, no package.json with a dev, start or serve script",
            root.display()
        );
        return fail(message, scaffold_notes.iter().map(dim).collect());
    }
    let text_out = serde_json::to_string_pretty(&Value::Object(profile)).unwrap_or_default();
    // Written only as a new file: never through a file or a link that is already there.
    let written = if save { create_new(&target, &text_out).map(|()| target.clone()) } else { temp_profile(&text_out) };
    let file = match written {
        Ok(file) => file,
        Err(error) => return fail(format!("lazyrunner: can't write {}: {error}", target.display()), Vec::new()),
    };
    let (_, found_problems) =
        problems(&Found { path: file.clone(), source: Source::Repo, root: Some(root.clone()) }, dirs);
    if !save {
        let _ = std::fs::remove_file(&file);
    }
    let headline = if save {
        format!("Saved {}", target.display())
    } else {
        format!("A start for {} (--save writes it)", target.display())
    };
    let mut notes = vec![note(format!("{headline}: {} apps - {}", apps.len(), apps.join(", ")), Tone::Accent)];
    notes.extend(scaffold_notes.iter().map(dim));
    for problem in found_problems {
        let error = problem.level == Severity::Error;
        let at = if problem.at.is_empty() { String::new() } else { format!("{}: ", problem.at) };
        let text = format!("  {} {at}{}", if error { "x" } else { "-" }, problem.message);
        notes.push(note(text, if error { Tone::Error } else { Tone::Dim }));
    }
    notes.push(note(
        "Check the names, ports and variants (projects/README.md), then: lazyrunner profile check. lazyrunner asks you to trust it the first time it runs it.",
        Tone::Plain,
    ));
    Outcome { code: 0, data: (!save).then_some(text_out), notes }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_unique_lowercase_and_start_with_a_letter() {
        let mut taken = HashSet::new();
        assert_eq!(app_key("Orders", &mut taken), "orders");
        assert_eq!(app_key("orders", &mut taken), "orders2");
        assert_eq!(app_key("Admin-UI", &mut taken), "adminui");
        assert_eq!(app_key("3d", &mut taken), "app3d");
    }

    #[test]
    fn a_port_the_repos_files_name() {
        use crate::testing::{TempDir, write};
        let client = TempDir::new("config-port");
        write(
            client.join("server.mjs"),
            "import http from 'node:http';
const PORT = 58410;
http.createServer().listen(PORT);",
        );
        assert_eq!(config_port(client.path(), "node server.mjs"), Some(58410), "the server code");
        write(client.join("vite.config.ts"), "export default { server: { port: 5300 } }");
        assert_eq!(config_port(client.path(), "vite"), Some(5300), "vite.config");
        std::fs::remove_file(client.join("vite.config.ts")).unwrap();
        write(
            client.join(".env"),
            "API=x
PORT=4100",
        );
        assert_eq!(config_port(client.path(), "vite"), Some(4100), ".env");
        std::fs::remove_file(client.join(".env")).unwrap();
        assert_eq!(config_port(client.path(), "vite"), None);
    }

    #[test]
    fn check_warns_about_a_test_project_in_apps() {
        use crate::profile::{App, AppKind};
        use crate::testing::{TempDir, write};
        let repo = TempDir::new("test-project");
        write(
            repo.join("tests/Shop.Tests/Shop.Tests.csproj"),
            r#"<Project><PackageReference Include="Microsoft.NET.Test.Sdk" /></Project>"#,
        );
        write(
            repo.join("tests/Shop.Tests/Properties/launchSettings.json"),
            r#"{ "profiles": { "Shop.Tests": { "applicationUrl": "http://localhost:58499" } } }"#,
        );
        let app = App {
            key: "tests".into(),
            name: "Tests".into(),
            icon: None,
            nerd_icon: None,
            tests: Vec::new(),
            migrations: None,
            kind: AppKind::Dotnet {
                project: crate::profile::native("tests/Shop.Tests/Shop.Tests.csproj"),
                launch_profile: "Shop.Tests".into(),
                swagger: None,
                args: Vec::new(),
            },
        };
        let problems =
            crate::apptypes::setup_problems(repo.path(), &app, "apps[0] (tests)", std::slice::from_ref(&app));
        let warning = problems.iter().find(|problem| problem.level == Severity::Warning).expect("a warning");
        assert_eq!(warning.at, "apps[0] (tests).project");
        assert!(warning.message.contains("is a test project"), "{}", warning.message);
        assert_eq!(problems.len(), 1, "otherwise fine: {problems:?}");
    }

    #[test]
    fn the_common_folder_of_paths() {
        assert_eq!(common_folder(&["services/Orders".into(), "services/Billing".into()]), "services");
        assert_eq!(common_folder(&["a/b".into(), "c".into()]), ".");
        assert_eq!(common_folder(&[]), ".");
    }
}
