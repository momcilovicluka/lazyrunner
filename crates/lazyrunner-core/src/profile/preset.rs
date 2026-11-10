//! Presets (#61): what a language needs to run as a command app, as data - its run, build and install, when it's up,
//! what counts as its code, what its errors and stack frames look like, the tools it needs and the folders it puts
//! first on PATH (a `.venv`). A command app names one (`"preset": "python"`) and gets each field it leaves out; a
//! preset's `options` fill in what depends on the app's folder (a `uv.lock`: `uv sync`), the first that holds for
//! each field. lazyrunner's own are in `projects/presets`; a `presets` folder beside your profiles and a profile's own
//! `"presets"` add to them or replace them.

use std::path::Path;

use serde_json::{Map, Value};

use super::{ProfileDirs, TYPE_COMMAND, native};
use crate::json::{field, list, text};

/// lazyrunner's own presets.
const BUNDLED: [&str; 3] = [
    include_str!("../../../../projects/presets/python.json"),
    include_str!("../../../../projects/presets/go.json"),
    include_str!("../../../../projects/presets/rust.json"),
];
/// The fields a preset, and each of its options, fills in.
pub const FILLED: [&str; 14] = [
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
    "path",
    "tools",
    "env",
    "nerdIcon",
];
/// What an option says besides those: when it holds - a file in the app's folder (any of them), a program on PATH.
pub const WHEN: [&str; 2] = ["anyFile", "program"];
/// The folder of your profiles that holds your presets.
const PRESETS_FOLDER: &str = "presets";

/// lazyrunner's own presets.
pub fn bundled() -> Vec<Value> {
    BUNDLED.iter().filter_map(|preset| serde_json::from_str(preset).ok()).collect()
}

fn own<'a>(profile: &'a Value, id: &str) -> Option<&'a Value> {
    field(profile, "presets")
        .as_object()?
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(id))
        .map(|(_, preset)| preset)
}

fn yours(dirs: &ProfileDirs, id: &str) -> Option<Value> {
    crate::json::read(&dirs.user.join(PRESETS_FOLDER).join(format!("{}.json", id.to_lowercase())))
}

/// A preset by its id: the profile's own (`"presets"`), then yours (a `presets` folder beside your profiles), then
/// lazyrunner's.
pub fn find(id: &str, profile: &Value, dirs: &ProfileDirs) -> Option<Value> {
    own(profile, id)
        .cloned()
        .or_else(|| yours(dirs, id))
        .or_else(|| bundled().into_iter().find(|preset| text(field(preset, "id")).eq_ignore_ascii_case(id)))
}

/// The presets a profile can name.
pub fn ids(profile: &Value, dirs: &ProfileDirs) -> Vec<String> {
    let mut ids: Vec<String> =
        field(profile, "presets").as_object().into_iter().flatten().map(|(id, _)| id.clone()).collect();
    if let Ok(entries) = std::fs::read_dir(dirs.user.join(PRESETS_FOLDER)) {
        let files = entries.filter_map(|entry| entry.ok().map(|entry| entry.path()));
        let json = |path: &std::path::PathBuf| path.extension().is_some_and(|extension| extension == "json");
        ids.extend(files.filter(json).filter_map(|path| Some(path.file_stem()?.to_string_lossy().into_owned())));
    }
    ids.extend(bundled().iter().map(|preset| text(field(preset, "id"))));
    ids.sort_by_key(|id| id.to_lowercase());
    ids.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    ids
}

/// Whether an option holds in `folder`: any of its files is there (none: any folder), its program is on PATH.
fn holds(when: &Value, folder: Option<&Path>) -> bool {
    let files = list(field(when, "anyFile"));
    let there =
        files.is_empty() || folder.is_some_and(|folder| files.iter().any(|file| folder.join(text(file)).exists()));
    let program = field(when, "program");
    there && (program.is_null() || crate::run::tools::find_on_path(&text(program)).is_some())
}

/// A command app with its preset filled in: each field it leaves out comes from the first of the preset's options
/// that holds in its folder and has it, else from the preset; its env is the preset's with its own on top.
pub fn apply(app: &Value, preset: &Value, folder: Option<&Path>) -> Value {
    let mut filled: Map<String, Value> = app.as_object().cloned().unwrap_or_default();
    let options: Vec<&Value> =
        list(field(preset, "options")).into_iter().filter(|option| holds(field(option, "when"), folder)).collect();
    for name in FILLED {
        let given = filled.get(name).filter(|value| !value.is_null()).cloned();
        let from = options
            .iter()
            .map(|option| field(option, name))
            .chain([field(preset, name)])
            .find(|value| !value.is_null());
        match (given, from.cloned()) {
            (Some(Value::Object(mine)), Some(Value::Object(mut env))) if name == "env" => {
                env.extend(mine);
                filled.insert(name.into(), Value::Object(env));
            }
            (None, Some(value)) => {
                filled.insert(name.into(), value);
            }
            _ => {}
        }
    }
    Value::Object(filled)
}

/// The profile with its command apps' presets filled in - `root` the repo's, so an option can look in an app's folder
/// - and a problem for each preset that isn't known.
pub fn apply_all(data: &Value, root: Option<&Path>, dirs: &ProfileDirs) -> (Value, Vec<String>) {
    let profile = data.clone();
    let mut data = data.clone();
    let mut problems = Vec::new();
    let Some(apps) = data.get_mut("apps").and_then(Value::as_array_mut) else { return (data, problems) };
    for (index, app) in apps.iter_mut().enumerate() {
        let id = text(field(app, "preset"));
        if id.is_empty() || !text(field(app, "type")).eq_ignore_ascii_case(TYPE_COMMAND) {
            continue;
        }
        match find(&id, &profile, dirs) {
            Some(preset) => {
                let folder = root.map(|root| root.join(native(&text(field(app, "cwd")))));
                *app = apply(app, &preset, folder.as_deref());
            }
            None => {
                let key = text(field(app, "key"));
                let known = ids(&profile, dirs).join(", ");
                problems
                    .push(format!("apps[{index}] ({key}).preset '{id}' is not a known preset (there are: {known})"));
            }
        }
    }
    (data, problems)
}

/// The preset of lazyrunner's whose files a folder has (a `pyproject.toml`: Python).
pub fn looks_like(folder: &Path) -> Option<Value> {
    bundled().into_iter().find(|preset| {
        list(field(field(preset, "detect"), "anyFile")).iter().any(|file| folder.join(text(file)).is_file())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{TempDir, write};
    use serde_json::json;

    fn dirs(temp: &TempDir) -> ProfileDirs {
        ProfileDirs { user: temp.join("profiles"), bundled: temp.join("bundled"), only: None }
    }

    #[test]
    fn lazyrunners_own_presets_are_valid_and_python_is_one() {
        let presets = bundled();
        assert_eq!(presets.len(), BUNDLED.len(), "each parses");
        for preset in &presets {
            let mut problems = Vec::new();
            super::super::check::check_preset(preset, &text(field(preset, "id")), &mut problems);
            assert!(problems.is_empty(), "{problems:?}");
        }
        assert!(presets.iter().any(|preset| text(field(preset, "id")) == "python"));
    }

    #[test]
    fn pythons_patterns_know_a_traceback_its_error_and_its_frames_and_leave_uvicorns_info_alone() {
        let python = bundled().into_iter().find(|preset| text(field(preset, "id")) == "python").unwrap();
        let patterns = |name: &str| -> Vec<regex::Regex> {
            list(field(&python, name)).into_iter().map(|pattern| regex::Regex::new(&text(pattern)).unwrap()).collect()
        };
        let (errors, warnings) = (patterns("errors"), patterns("warnings"));
        let error = |line: &str| errors.iter().any(|pattern| pattern.is_match(line));
        assert!(!error("Traceback (most recent call last):") && error("ValueError: bad input"), "the exception line");
        assert!(error("ERROR:    Exception in ASGI application") && error("pydantic.ValidationError: 1 error"));
        assert!(!error("INFO:     Uvicorn running on http://127.0.0.1:8000") && !error("Errors are logged to app.log"));
        assert!(warnings.iter().any(|pattern| pattern.is_match("DeprecationWarning: old api")));
        let frames = regex::Regex::new(&text(field(&python, "frames"))).unwrap();
        let frame = frames.captures(r#"  File "app/api.py", line 12, in handler"#).unwrap();
        assert_eq!((&frame["file"], &frame["line"]), ("app/api.py", "12"));
    }

    /// A bundled preset's errors and warnings (each: does a line match), and its frame pattern.
    type Says = (Box<dyn Fn(&str) -> bool>, Box<dyn Fn(&str) -> bool>, regex::Regex);

    fn says(id: &str) -> Says {
        let preset = bundled().into_iter().find(|preset| text(field(preset, "id")) == id).unwrap();
        let patterns = |name: &str| -> Vec<regex::Regex> {
            list(field(&preset, name)).into_iter().map(|pattern| regex::Regex::new(&text(pattern)).unwrap()).collect()
        };
        let (errors, warnings) = (patterns("errors"), patterns("warnings"));
        let frames = regex::Regex::new(&text(field(&preset, "frames"))).unwrap();
        (
            Box::new(move |line| errors.iter().any(|pattern| pattern.is_match(line))),
            Box::new(move |line| warnings.iter().any(|pattern| pattern.is_match(line))),
            frames,
        )
    }

    #[test]
    fn gos_patterns_know_a_panic_an_error_level_and_its_frames() {
        let (error, warning, frames) = says("go");
        assert!(error("panic: runtime error: index out of range") && error("fatal error: all goroutines are asleep"));
        assert!(
            error(r#"time=2026-11-10T10:00:00 level=ERROR msg="db down""#) && error(r#"{"level":"error","msg":"x"}"#)
        );
        assert!(!error("level=INFO msg=\"listening on :8080\"") && warning("level=WARN msg=slow"));
        let frame = frames.captures("\tC:/src/api/handler.go:42 +0x1d").unwrap();
        assert_eq!((&frame["file"], &frame["line"]), ("C:/src/api/handler.go", "42"));
        let built = frames.captures("./main.go:12:5: undefined: x").unwrap();
        assert_eq!((&built["file"], &built["col"]), ("./main.go", "5"));
    }

    #[test]
    fn rusts_patterns_know_a_compile_error_a_panic_and_its_frames() {
        let (error, warning, frames) = says("rust");
        assert!(error("error[E0425]: cannot find value `x` in this scope") && error("error: could not compile `api`"));
        assert!(
            error("thread 'main' panicked at src/main.rs:10:5:") && error("2026-11-10T10:00:00Z ERROR api: db down")
        );
        assert!(!error("    Finished `dev` profile in 0.5s") && warning("warning: unused variable: `x`"));
        let frame = frames.captures("  --> src/main.rs:42:5").unwrap();
        assert_eq!((&frame["file"], &frame["line"], &frame["col"]), ("src/main.rs", "42", "5"));
    }

    #[test]
    fn go_and_rust_run_where_they_have_a_main_and_build_first() {
        let temp = TempDir::new("preset-go-rust");
        write(temp.join("api/go.mod"), "");
        write(temp.join("api/main.go"), "");
        write(temp.join("svc/Cargo.toml"), "");
        write(temp.join("svc/src/main.rs"), "");
        let app = json!({ "key": "a", "type": "command" });
        let preset = |id: &str| bundled().into_iter().find(|preset| text(field(preset, "id")) == id).unwrap();
        let go = apply(&app, &preset("go"), Some(&temp.join("api")));
        assert_eq!((go["run"].as_str(), go["build"].as_str()), (Some("go run ."), Some("go build ./...")));
        let rust = apply(&app, &preset("rust"), Some(&temp.join("svc")));
        assert_eq!((rust["run"].as_str(), rust["env"]["RUST_BACKTRACE"].as_str()), (Some("cargo run"), Some("1")));
        let library = apply(&app, &preset("rust"), Some(temp.path()));
        assert!(library["run"].is_null(), "no src/main.rs: nothing to run");
        let ids: Vec<String> = [temp.join("api"), temp.join("svc")]
            .iter()
            .filter_map(|folder| looks_like(folder).map(|preset| text(field(&preset, "id"))))
            .collect();
        assert_eq!(ids, ["go", "rust"]);
    }

    #[test]
    fn an_app_gets_what_it_leaves_out_from_the_options_that_hold_in_its_folder_then_the_preset() {
        let temp = TempDir::new("preset-apply");
        let folder = temp.join("pricing");
        write(folder.join("uv.lock"), "");
        write(folder.join("main.py"), "");
        let python = bundled().into_iter().find(|preset| text(field(preset, "id")) == "python").unwrap();
        let app = json!({ "key": "pricing", "type": "command", "preset": "python", "watch": ["app/**/*.py"],
                          "env": { "APP_ENV": "dev", "PYTHONUNBUFFERED": "0" } });
        let filled = apply(&app, &python, Some(&folder));
        assert_eq!(filled["install"], "uv sync", "uv.lock: the first option that holds");
        assert_eq!(filled["run"], "python main.py");
        assert_eq!(filled["watch"], json!(["app/**/*.py"]), "its own wins");
        assert_eq!(filled["env"]["PYTHONUNBUFFERED"], "0", "its own env on top");
        assert_eq!(filled["env"]["VIRTUAL_ENV"], "{cwd}/.venv", "the preset's under it");
        assert_eq!(filled["path"], json!([".venv/Scripts", ".venv/bin"]));
        assert_eq!((&filled["setup"], &filled["installFrom"]), (&json!([]), &json!(["uv.lock", "pyproject.toml"])));
        let elsewhere = apply(&app, &python, Some(&temp.join("empty")));
        assert!(elsewhere["run"].is_null() && elsewhere["install"].is_null(), "no option holds in an empty folder");
        let pip = temp.join("pip");
        write(pip.join("requirements.txt"), "");
        let pip = apply(&app, &python, Some(&pip));
        assert_eq!(pip["setup"], "python -m venv .venv", "pip installs into a .venv it makes first");
        assert_eq!(pip["installFrom"], json!(["requirements.txt"]));
    }

    #[test]
    fn python_is_up_once_its_server_says_so_or_its_port_listens() {
        let python = bundled().into_iter().find(|preset| text(field(preset, "id")) == "python").unwrap();
        let ready = field(&python, "ready");
        assert_eq!(ready["orPort"], true);
        let line = regex::Regex::new(&text(&ready["line"])).unwrap();
        for said in [
            "INFO:     Uvicorn running on http://127.0.0.1:8000 (Press CTRL+C to quit)",
            "INFO:     Application startup complete.",
            " * Running on http://127.0.0.1:5000",
            "Starting development server at http://127.0.0.1:8000/",
        ] {
            assert!(line.is_match(said), "{said}");
        }
        assert!(!line.is_match("INFO:     Started server process [1234]"));
    }

    #[test]
    fn a_profiles_own_preset_and_yours_come_before_lazyrunners_and_an_unknown_one_is_named() {
        let temp = TempDir::new("preset-find");
        let dirs = dirs(&temp);
        write(dirs.user.join("presets").join("go.json"), r#"{ "id": "go", "build": "go build -tags dev ./..." }"#);
        let profile = json!({ "presets": { "python": { "run": "custom" } },
                              "apps": [{ "key": "a", "type": "command", "preset": "python" },
                                       { "key": "b", "type": "command", "preset": "go", "run": "./b" },
                                       { "key": "c", "type": "command", "preset": "ruby", "run": "x" }] });
        let (data, problems) = apply_all(&profile, None, &dirs);
        assert_eq!(data["apps"][0]["run"], "custom");
        assert_eq!(data["apps"][1]["build"], "go build -tags dev ./...", "yours, not lazyrunner's");
        assert_eq!(problems, ["apps[2] (c).preset 'ruby' is not a known preset (there are: go, python, rust)"]);
        assert!(looks_like(temp.path()).is_none());
        write(temp.join("requirements.txt"), "");
        assert_eq!(looks_like(temp.path()).map(|preset| text(field(&preset, "id"))), Some("python".into()));
    }
}
