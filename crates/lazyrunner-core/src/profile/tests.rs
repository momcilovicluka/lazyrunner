//! The PowerShell version's profile and projects suites (tests/profile.Tests.ps1, tests/projects.Tests.ps1),
//! check by check: a profile loads into the settings lazyrunner runs with, broken ones stop it with clear messages,
//! which profile a folder gets, extends, trust, settings per project, and a branch that changes the profile.

use std::path::Path;

use serde_json::{Value, json};

use super::change::{Tone, describe};
use super::*;
use crate::apptypes::{self, ModeKind};
use crate::config::ConfigFile;
use crate::profiletools::schema_errors;
use crate::testing::{TempDir, sample_profile, write};

fn sample() -> Value {
    serde_json::from_str(&std::fs::read_to_string(sample_profile()).unwrap()).unwrap()
}

fn dirs(temp: &TempDir) -> ProfileDirs {
    ProfileDirs { user: temp.join("profiles"), bundled: temp.join("app").join("projects"), only: None }
}

fn save(data: &Value, path: &Path) {
    write(path, &serde_json::to_string_pretty(data).unwrap());
}

fn keys(project: &Project) -> Vec<&str> {
    project.apps.iter().map(|app| app.key.as_str()).collect()
}

fn variants(project: &Project, key: &str) -> Vec<Variant> {
    match &project.app(key).unwrap().kind {
        AppKind::Npm { variants, .. } => variants.clone(),
        AppKind::Dotnet { .. } => panic!("{key} is an npm app"),
    }
}

fn message(result: Result<Project>) -> String {
    result.err().map(|error| error.to_string()).unwrap_or_else(|| "(loaded without an error)".into())
}

// ---------------------------------------------------------------- the settings lazyrunner runs with

#[test]
fn a_profile_loads_into_the_settings_lazyrunner_runs_with() {
    let temp = TempDir::new("settings");
    let project = import(&sample_profile(), &dirs(&temp)).unwrap();
    assert_eq!(keys(&project), ["orders", "billing", "shop", "docs", "console"], "the columns keep their order");
    assert_eq!((project.name.as_str(), project.id.as_str()), ("Sample", "sample"));
    assert_eq!((project.repo_marker.as_str(), project.source_dir.as_str()), ("Sample.sln", "services"));
    let expected = AppKind::Dotnet {
        project: native("services/Orders/Orders.csproj"),
        launch_profile: "Orders".into(),
        swagger: Some("/swagger".into()),
        args: Vec::new(),
    };
    assert_eq!(project.app("orders").unwrap().kind, expected, "this OS's separators");
    let icons: Vec<(&str, &str)> =
        project.apps.iter().filter_map(|app| Some((app.key.as_str(), app.icon.as_deref()?))).collect();
    assert_eq!(icons, [("orders", "◆")], "the rest get the default");
    let shop = &variants(&project, "shop")[0];
    assert_eq!((shop.port, shop.label.as_str()), (Some(47301), "dev server"));
    let docs = &variants(&project, "docs")[0];
    assert_eq!(docs.served_by.as_deref(), Some("orders"));
    assert_eq!(docs.ready_file, Some(native("dist/index.html")));
    assert_eq!((docs.label.as_str(), docs.port), ("watch build", None));
    let console = variants(&project, "console");
    assert_eq!(console.len(), 2);
    assert_eq!(console[0].when.as_deref(), Some("dev:mock"));
    assert_eq!((console[1].when.clone(), console[1].port), (None, Some(47303)));
}

#[test]
fn each_app_type_has_its_traits() {
    let temp = TempDir::new("traits");
    let project = import(&sample_profile(), &dirs(&temp)).unwrap();
    let with = |trait_of: fn(&apptypes::Traits) -> bool| -> Vec<&str> {
        project.apps.iter().filter(|app| trait_of(app.traits())).map(|app| app.key.as_str()).collect()
    };
    assert_eq!(with(|t| t.builds), ["orders", "billing"]);
    assert_eq!(with(|t| t.installs), ["shop", "docs", "console"]);
    assert_eq!(with(|t| t.uses_secrets), ["orders", "billing"]);
}

#[test]
fn the_sample_passes_the_schema() {
    assert_eq!(schema_errors(&sample()), Vec::<String>::new());
}

// ---------------------------------------------------------------- broken profiles

/// The loader's message for a changed copy of sample.json, and whether the schema rejects it too.
fn broken(change: impl FnOnce(&mut Value)) -> (String, bool) {
    let temp = TempDir::new("broken");
    let mut data = sample();
    change(&mut data);
    let file = temp.join("broken.json");
    save(&data, &file);
    (message(import(&file, &dirs(&temp))), !schema_errors(&data).is_empty())
}

fn remove(value: &mut Value, key: &str) {
    value.as_object_mut().unwrap().remove(key);
}

#[test]
fn broken_profiles_get_one_clear_message_naming_the_field() {
    type Change = Box<dyn FnOnce(&mut Value)>;
    let cases: Vec<(&str, Change, &str, bool)> = vec![
        (
            "a newer format",
            Box::new(|d| d["schemaVersion"] = json!(2)),
            "schemaVersion 2 needs a newer lazyrunner",
            false,
        ),
        ("no apps", Box::new(|d| remove(d, "apps")), "apps is required", false),
        (
            "no launch profile",
            Box::new(|d| remove(&mut d["apps"][0], "launchProfile")),
            "apps[0] (orders).launchProfile is required",
            false,
        ),
        (
            "unknown type",
            Box::new(|d| d["apps"][1]["type"] = json!("python")),
            "apps[1] (billing).type must be 'dotnet' or 'npm'",
            false,
        ),
        (
            "one key twice",
            Box::new(|d| d["apps"][1]["key"] = json!("orders")),
            "apps[1].key 'orders' is used twice",
            true,
        ),
        (
            "an absolute path",
            Box::new(|d| d["apps"][0]["project"] = json!(r"C:\src\Orders\Orders.csproj")),
            "must be relative to the repo root",
            false,
        ),
        (
            "not a .csproj",
            Box::new(|d| d["apps"][0]["project"] = json!("services/Orders/Orders.fsproj")),
            "must be a .csproj file",
            false,
        ),
        (
            "a two-letter icon",
            Box::new(|d| d["apps"][0]["icon"] = json!("ab")),
            "apps[0] (orders).icon must be one character",
            false,
        ),
        (
            "args that aren't a list of text",
            Box::new(|d| d["apps"][0]["args"] = json!("--Seq:ApiKey=")),
            "apps[0] (orders).args must be a list of arguments",
            false,
        ),
        (
            "a bad port",
            Box::new(|d| d["apps"][2]["variants"][0]["port"] = json!(70000)),
            "port must be a port number",
            false,
        ),
        (
            "no variants",
            Box::new(|d| d["apps"][2]["variants"] = json!([])),
            "apps[2] (shop).variants is required",
            false,
        ),
        (
            "no when before the last",
            Box::new(|d| remove(&mut d["apps"][4]["variants"][0], "when")),
            "apps[4] (console).variants[0] has no when, so it always applies",
            true,
        ),
        (
            "port and servedBy",
            Box::new(|d| d["apps"][3]["variants"][0]["port"] = json!(4300)),
            "needs either port (a dev server) or servedBy (a watch build), not both",
            false,
        ),
        (
            "two marker files",
            Box::new(|d| d["detect"]["anyFile"] = json!(["Sample.sln", "Other.sln"])),
            "detect.anyFile: one file for now",
            false,
        ),
        (
            "an unknown server",
            Box::new(|d| d["apps"][3]["variants"][0]["servedBy"] = json!("nope")),
            "servedBy 'nope' must be the key of a dotnet app",
            true,
        ),
    ];
    for (label, change, expect, loader_only) in cases {
        let (message, schema_rejects) = broken(change);
        assert!(message.lines().any(|line| line.contains(expect)), "{label}: expected '{expect}' in:\n{message}");
        assert!(loader_only || schema_rejects, "{label}: the schema rejects it too");
    }
}

#[test]
fn a_file_that_isnt_json_says_so() {
    let temp = TempDir::new("not-json");
    let file = temp.join("broken.json");
    write(&file, r#"{ "apps": [ "#);
    assert!(message(import(&file, &dirs(&temp))).starts_with("lazyrunner: broken.json is not valid JSON - "));
}

#[test]
fn several_problems_are_listed_together() {
    let (message, _) = broken(|d| {
        d["name"] = json!("");
        remove(&mut d["apps"][0], "project");
        remove(&mut d["apps"][2], "dir");
    });
    assert!(message.starts_with("lazyrunner: broken.json has 3 problems:\n  - "), "{message}");
    for expect in ["name is required", "apps[0] (orders).project is required", "apps[2] (shop).dir is required"] {
        assert!(message.contains(expect), "{expect} in {message}");
    }
}

#[test]
fn a_typo_gets_a_suggestion() {
    let (message, _) = broken(|d| {
        d["apps"][0]["swager"] = json!("/swagger");
        d["apps"][2]["variants"][0]["prot"] = json!(1);
        d["colour"] = json!("blue");
    });
    assert!(message.contains("apps[0] (orders).swager is not a profile field - did you mean swagger?"), "{message}");
    assert!(message.contains("variants[0].prot is not a profile field - did you mean port?"), "{message}");
    assert!(message.contains("colour is not a profile field - it can have: $schema, schemaVersion"), "{message}");
}

// ---------------------------------------------------------------- on one machine

#[test]
fn an_app_is_on_unless_the_settings_turn_it_off() {
    let temp = TempDir::new("enabled");
    let project = import(&sample_profile(), &dirs(&temp)).unwrap();
    let on = |config: Value, key: &str| apptypes::is_enabled(&config, project.app(key).unwrap());
    assert!(on(json!({ "frontend": true }), "shop"));
    assert!(!on(json!({ "frontend": false }), "shop"), "frontend: false turns off the npm apps");
    assert!(on(json!({ "frontend": false }), "orders"));
    assert!(!on(json!({ "frontend": true, "apps": { "orders": { "enabled": false } } }), "orders"));
    assert!(on(json!({ "frontend": false, "apps": { "shop": { "enabled": true } } }), "shop"));
    let config = json!({ "stopFrontendOnStop": false, "apps": { "billing": { "includeInAll": false } } });
    let left_out: Vec<&str> =
        project.apps.iter().filter(|app| !apptypes::in_stop_all(&config, app)).map(|app| app.key.as_str()).collect();
    assert_eq!(left_out, ["billing", "shop", "docs", "console"]);
}

// ---------------------------------------------------------------- variants

#[test]
fn the_first_variant_that_applies_runs() {
    let temp = TempDir::new("variants");
    let mut data = sample();
    data["apps"] = json!([
        data["apps"][0].clone(),
        { "key": "shop", "name": "Shop", "type": "npm", "dir": "shop", "variants": [
            { "when": { "packageScript": "serve" }, "script": "serve", "port": 4300 },
            { "when": { "packageScript": "watch" }, "script": "watch", "servedBy": "orders", "ready": { "file": "dist/index.html" } } ] },
        { "key": "docs", "name": "Docs", "type": "npm", "dir": "docs", "variants": [{ "script": "dev", "port": 4400 }] },
        { "key": "adminui", "name": "Admin UI", "type": "npm", "dir": "admin-ui", "variants": [
            { "label": "mock", "when": { "packageScript": "dev:mock" }, "script": "dev:mock", "port": 4500 },
            { "label": "real", "when": { "packageScript": "dev" }, "script": "dev", "port": 4501 } ] }
    ]);
    assert_eq!(schema_errors(&data), Vec::<String>::new(), "the schema takes three npm apps with one or two variants");
    let file = temp.join("multi.json");
    save(&data, &file);
    let project = import(&file, &dirs(&temp)).unwrap();
    assert_eq!(keys(&project), ["orders", "shop", "docs", "adminui"]);
    let repo = temp.join("repo");
    let scripts = |dir: &str, scripts: Value| {
        save(&json!({ "name": dir, "scripts": scripts }), &repo.join(dir).join("package.json"))
    };
    let all = project.apps.clone();
    let mode = |key: &str, active: &[App]| apptypes::mode(&repo, project.app(key).unwrap(), active);
    let reason = |key: &str, active: &[App]| apptypes::no_mode_reason(&repo, project.app(key).unwrap(), active);

    scripts("shop", json!({ "watch": "node w.js" }));
    scripts("docs", json!({ "dev": "vite --port 4410" }));
    scripts("admin-ui", json!({ "dev": "vite", "dev:mock": "vite --mode mock" }));
    let shop = mode("shop", &all).unwrap();
    assert_eq!((shop.kind, shop.served_by.as_deref(), shop.port), (ModeKind::Build, Some("orders"), None));
    assert_eq!(shop.ready_file, Some(native("dist/index.html")));
    let docs = mode("docs", &all).unwrap();
    assert_eq!(
        (docs.kind, docs.port, docs.label.as_str()),
        (ModeKind::Serve, Some(4410), "dev server"),
        "--port in the script wins"
    );
    let admin = mode("adminui", &all).unwrap();
    assert_eq!((admin.label.as_str(), admin.port), ("mock", Some(4500)), "the first variant wins");

    scripts("shop", json!({ "build": "x" }));
    assert_eq!(mode("shop", &all), None);
    assert_eq!(reason("shop", &all), format!("{} has neither serve nor watch", join("shop", "package.json")));
    scripts("docs", json!({ "build": "vite build" }));
    assert_eq!(mode("docs", &all), None, "a variant without when still needs its script");
    assert_eq!(reason("docs", &all), format!("{} has no dev", join("docs", "package.json")));
    std::fs::remove_file(repo.join("docs").join("package.json")).unwrap();
    assert_eq!(reason("docs", &all), format!("{} is missing", join("docs", "package.json")));
    scripts("shop", json!({ "watch": "node w.js" }));
    let without_orders: Vec<App> = all.iter().filter(|app| app.key != "orders").cloned().collect();
    assert_eq!(mode("shop", &without_orders), None, "a watch build whose server is turned off doesn't run");
    assert!(reason("shop", &without_orders).contains("or orders, which would serve watch, is turned off"));
}

// ---------------------------------------------------------------- which profile a folder gets

/// A lazyrunner install folder with sample.json bundled, and the sample repo with a deep folder.
fn setup_projects(temp: &TempDir) -> (ProfileDirs, std::path::PathBuf) {
    let dirs = dirs(temp);
    std::fs::create_dir_all(&dirs.bundled).unwrap();
    std::fs::copy(sample_profile(), dirs.bundled.join("sample.json")).unwrap();
    let repo = temp.join("sample");
    write(repo.join("Sample.sln"), "");
    write(repo.join("web").join("shop").join("src").join("main.ts"), "");
    (dirs, repo)
}

fn project_in(dir: &Path, dirs: &ProfileDirs) -> Result<Project> {
    resolve(&find_project_profile(dir, dirs)?.expect("a profile applies"), dirs)
}

#[test]
fn a_folder_gets_its_repos_profile_then_yours_then_the_bundled_one() {
    let temp = TempDir::new("which");
    let (mut dirs, repo) = setup_projects(&temp);
    let found = find_project_profile(&repo.join("web").join("shop").join("src"), &dirs).unwrap().unwrap();
    assert_eq!((found.source, found.root.as_deref()), (Source::Bundled, Some(repo.as_path())), "found by walking up");
    assert_eq!(resolve(&found, &dirs).unwrap().id, "sample");
    let nothing = temp.join("nothing");
    write(nothing.join("readme.md"), "");
    assert!(find_project_profile(&nothing, &dirs).unwrap().is_none(), "a folder no profile knows");

    let mut mine = sample();
    mine["name"] = json!("Sample (mine)");
    save(&mine, &dirs.user.join("sample.json"));
    let found = find_project_profile(&repo, &dirs).unwrap().unwrap();
    assert_eq!(
        (found.source, resolve(&found, &dirs).unwrap().name.as_str()),
        (Source::User, "Sample (mine)"),
        "yours replace a bundled one"
    );
    std::fs::copy(sample_profile(), dirs.user.join("sample.json")).unwrap();
    dirs.bundled = temp.join("nowhere");
    assert_eq!(find_project_profile(&repo, &dirs).unwrap().unwrap().source, Source::User, "yours count on their own");

    let mut own = sample();
    own["id"] = json!("shop");
    own["name"] = json!("Shop repo");
    remove(&mut own, "detect");
    save(&own, &repo.join(REPO_PROFILE_FILE));
    let project = project_in(&repo, &dirs).unwrap();
    assert_eq!(
        (project.source, project.id.as_str(), project.root.as_deref()),
        (Source::Repo, "shop", Some(repo.as_path()))
    );
    assert_eq!(project.repo_marker, ".lazyrunner.json", "it needs no detect: the file itself marks the repo");
    assert_eq!(schema_errors(&own), Vec::<String>::new(), "the schema takes it without detect");
}

#[test]
fn a_repo_profile_can_extend_a_known_one() {
    let temp = TempDir::new("extends");
    let (dirs, repo) = setup_projects(&temp);
    let team = json!({
        "schemaVersion": 1, "extends": "sample", "name": "Sample (team)",
        "apps": [
            { "key": "shop", "name": "Storefront" },
            { "key": "blog", "name": "Blog", "type": "npm", "dir": "web/blog", "variants": [{ "script": "dev", "port": 58310 }] }
        ]
    });
    assert_eq!(schema_errors(&team), Vec::<String>::new(), "the schema takes just the changes");
    save(&team, &repo.join(REPO_PROFILE_FILE));
    let project = project_in(&repo, &dirs).unwrap();
    assert_eq!(
        (project.name.as_str(), project.id.as_str()),
        ("Sample (team)", "sample"),
        "its fields replace the base's"
    );
    assert_eq!(keys(&project), ["orders", "billing", "shop", "docs", "console", "blog"], "apps merge by key");
    let shop = project.app("shop").unwrap();
    assert_eq!(shop.name, "Storefront");
    assert!(matches!(&shop.kind, AppKind::Npm { dir, .. } if *dir == native("web/shop")), "and keep the rest");

    let mut unknown = team.clone();
    unknown["extends"] = json!("nope");
    save(&unknown, &repo.join(REPO_PROFILE_FILE));
    assert!(message(project_in(&repo, &dirs)).contains("extends 'nope', which is not a known profile"));
    let mut elsewhere = sample();
    elsewhere["extends"] = json!("other");
    let file = temp.join("extends.json");
    save(&elsewhere, &file);
    assert!(message(import(&file, &dirs)).contains("extends is only for a repo's .lazyrunner.json"));
}

// ---------------------------------------------------------------- trust

#[test]
fn a_repo_profile_runs_only_once_you_trust_it() {
    let temp = TempDir::new("trust");
    let (dirs, repo) = setup_projects(&temp);
    let store = temp.join("state").join("trusted-profiles.json");
    let mut team = json!({ "schemaVersion": 1, "extends": "sample", "name": "Sample (team)", "apps": [{ "key": "shop", "name": "Storefront" }] });
    save(&team, &repo.join(REPO_PROFILE_FILE));
    let project = project_in(&repo, &dirs).unwrap();
    assert!(!trust::is_trusted(&project, &store));

    let mut asked = Vec::new();
    let mut out = Vec::new();
    let answer = trust::confirm(&project, &store, &mut out, &mut |prompt| {
        asked.push(prompt.to_string());
        "n".into()
    });
    assert!(!answer.unwrap());
    assert_eq!(asked, ["Trust it and run these? [y/N]"]);
    assert!(String::from_utf8(out).unwrap().ends_with("Not trusted - lazyrunner did not start.\n"));
    assert!(!trust::is_trusted(&project, &store), "no: still not trusted");

    let summary = trust::summary(&project);
    assert_eq!(summary.len(), 5);
    assert!(
        summary[0].contains(&format!("dotnet build + run {}", native("services/Orders/Orders.csproj"))),
        "{}",
        summary[0]
    );
    assert!(summary[2].contains(&format!("npm run dev in {}", native("web/shop"))), "{}", summary[2]);

    assert!(trust::confirm(&project, &store, &mut Vec::new(), &mut |_| "y".into()).unwrap());
    assert!(trust::is_trusted(&project, &store));
    assert!(trust::confirm(&project, &store, &mut Vec::new(), &mut |_| panic!("asked again")).unwrap());

    team["name"] = json!("Sample (changed)");
    save(&team, &repo.join(REPO_PROFILE_FILE));
    let changed = project_in(&repo, &dirs).unwrap();
    assert!(!trust::is_trusted(&changed, &store), "once the file changes, lazyrunner asks again");
    let mut out = Vec::new();
    trust::confirm(&changed, &store, &mut out, &mut |_| "n".into()).unwrap();
    assert!(String::from_utf8(out).unwrap().contains("changed since you trusted it"));

    let bundled = Found { path: dirs.bundled.join("sample.json"), source: Source::Bundled, root: Some(repo.clone()) };
    assert!(trust::is_trusted(&resolve(&bundled, &dirs).unwrap(), &store), "bundled profiles need no trust");
}

// ---------------------------------------------------------------- settings and state per project

#[test]
fn another_project_has_its_own_settings() {
    let temp = TempDir::new("settings-per-project");
    let mut other = sample();
    other["id"] = json!("other");
    let file = temp.join("other.json");
    save(&other, &file);
    let config = ConfigFile::new(temp.join("config.json"));
    crate::config::write(&config.path, &json!({ "theme": "dark", "projects": { "other": { "frontend": false } } }))
        .unwrap();
    let project = import(&file, &dirs(&temp)).unwrap();
    let active = project.active_apps(&config.load(Some("other")), &config.path).unwrap();
    let active: Vec<&str> = active.iter().map(|app| app.key.as_str()).collect();
    assert_eq!(active, ["orders", "billing"], "frontend off in its section");
    let all_off =
        json!({ "frontend": false, "apps": { "orders": { "enabled": false }, "billing": { "enabled": false } } });
    let error = project.active_apps(&all_off, &config.path).unwrap_err().to_string();
    assert!(error.starts_with("lazyrunner: every app of Sample is turned off in "), "{error}");
}

// ---------------------------------------------------------------- a branch that changes the profile

#[test]
fn a_branch_that_changes_the_profile_is_noticed() {
    let temp = TempDir::new("change");
    let (mut dirs, repo) = setup_projects(&temp);
    std::fs::copy(sample_profile(), {
        std::fs::create_dir_all(&dirs.user).unwrap();
        dirs.user.join("sample.json")
    })
    .unwrap();
    dirs.bundled = temp.join("nowhere");
    let store = temp.join("state").join("trusted-profiles.json");
    let mut team = json!({
        "schemaVersion": 1, "extends": "sample", "name": "Sample (team)",
        "apps": [{ "key": "shop", "name": "Storefront" }, { "key": "blog", "name": "Blog", "type": "npm", "dir": "web/blog", "variants": [{ "script": "dev", "port": 58310 }] }]
    });
    save(&team, &repo.join(REPO_PROFILE_FILE));
    let running = project_in(&repo, &dirs).unwrap();
    trust::approve(&running, &store).unwrap();
    let active: Vec<&str> = running.apps.iter().map(|app| app.key.as_str()).collect();
    let same = project_in(&repo, &dirs).unwrap();
    assert_eq!(describe(&running, &active, Ok(Some(&same)), true), None, "the same profile: nothing to say");

    team["apps"].as_array_mut().unwrap().push(json!({ "key": "wiki", "name": "Wiki", "type": "npm", "dir": "web/wiki", "variants": [{ "script": "dev", "port": 58320 }] }));
    team["apps"][0]["name"] = json!("Store");
    save(&team, &repo.join(REPO_PROFILE_FILE));
    let now = project_in(&repo, &dirs).unwrap();
    let change = describe(&running, &active, Ok(Some(&now)), trust::is_trusted(&now, &store)).unwrap();
    assert_eq!(change.tone, Tone::Warning);
    for expect in ["+ Wiki", "~ Store", ", then p reopens lazyrunner with it", "needs your trust first"] {
        assert!(change.text.contains(expect), "'{expect}' in '{}'", change.text);
    }

    std::fs::remove_file(repo.join(REPO_PROFILE_FILE)).unwrap();
    let now = project_in(&repo, &dirs).unwrap();
    let change = describe(&running, &active, Ok(Some(&now)), trust::is_trusted(&now, &store)).unwrap();
    assert!(change.text.contains("sample.json") && change.text.contains("- blog"), "{}", change.text);
    assert!(!change.text.contains("trust"), "your own profile needs no trust: {}", change.text);

    let broken =
        describe(&running, &active, Err("lazyrunner: x.json has a problem:\n  - name is required".into()), true)
            .unwrap();
    assert_eq!(
        (broken.text.as_str(), broken.tone),
        ("✗ lazyrunner: x.json has a problem: - name is required", Tone::Error)
    );
    let gone = describe(&running, &active, Ok(None), true).unwrap();
    assert!(gone.text.starts_with("No profile matches this checkout any more"));
}
