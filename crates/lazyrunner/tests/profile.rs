//! The PowerShell version's profiletools suite (tests/profiletools.Tests.ps1), check by check: `lazyrunner profile
//! check` (fields, schema, the checkout) and `lazyrunner profile init` (a first profile from a checkout), run as the
//! real binary on the sample repo.

use std::path::{MAIN_SEPARATOR_STR, Path};
use std::process::Command;

use lazyrunner_core::testing::{TempDir, sample_profile, sample_repo, write};
use serde_json::{Value, json};

const BIN: &str = env!("CARGO_BIN_EXE_lazyrunner");

struct Output {
    code: i32,
    out: String,
    err: String,
}

/// `lazyrunner profile ...` in `dir`, with its state, settings and profiles in `temp` (a real session's stay apart).
fn lazyrunner(temp: &TempDir, dir: &Path, args: &[&str]) -> Output {
    let output = Command::new(BIN)
        .arg("profile")
        .args(args)
        .current_dir(dir)
        .env("LAZYRUNNER_STATE_DIR", temp.join("state"))
        .env("LAZYRUNNER_CONFIG_PATH", temp.join("config.json"))
        .env("LAZYRUNNER_PROFILES_DIR", temp.join("profiles"))
        .env_remove("LAZYRUNNER_PROFILE")
        .env("NO_COLOR", "1")
        .output()
        .expect("lazyrunner runs");
    Output {
        code: output.status.code().unwrap_or(-1),
        out: String::from_utf8_lossy(&output.stdout).into_owned(),
        err: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

fn check(temp: &TempDir, dir: &Path, file: Option<&Path>) -> (Value, i32) {
    let file = file.map(|file| file.to_string_lossy().into_owned());
    let mut args = vec!["check"];
    args.extend(file.as_deref());
    args.push("--json");
    let output = lazyrunner(temp, dir, &args);
    let report =
        serde_json::from_str(&output.out).unwrap_or_else(|_| panic!("JSON from check: {}{}", output.out, output.err));
    (report, output.code)
}

fn sample() -> Value {
    serde_json::from_str(&std::fs::read_to_string(sample_profile()).unwrap()).unwrap()
}

fn same_folder(a: &Value, b: &Path) -> bool {
    a.as_str().is_some_and(|a| std::fs::canonicalize(a).ok() == std::fs::canonicalize(b).ok())
}

fn native(path: &str) -> String {
    path.replace('/', MAIN_SEPARATOR_STR)
}

/// The problem whose "level at: message" matches all the parts.
fn find<'a>(report: &'a Value, level: &str, at: &str, parts: &[&str]) -> Option<&'a Value> {
    report["problems"].as_array()?.iter().find(|problem| {
        problem["level"] == level
            && problem["at"] == at
            && parts.iter().all(|part| problem["message"].as_str().unwrap_or("").contains(part))
    })
}

#[test]
fn check_passes_the_sample_repo_and_names_each_problem_of_a_broken_profile() {
    let temp = TempDir::new("check");
    let repo = sample_repo(&temp.join("repo"));
    std::fs::copy(sample_profile(), repo.join(".lazyrunner.json")).unwrap();
    let (report, code) = check(&temp, &repo, None);
    assert!(report["ok"] == true && code == 0, "{report}");
    assert_eq!(report["source"], "repo");
    assert!(same_folder(&report["root"], &repo), "{report}");
    let not_info: Vec<&Value> =
        report["problems"].as_array().unwrap().iter().filter(|p| p["level"] != "info").collect();
    assert!(not_info.is_empty(), "only information (its mock variant is for other branches): {report}");

    let text = lazyrunner(&temp, &repo, &["check"]);
    assert_eq!(text.code, 0);
    assert_eq!(text.out.lines().last(), Some("OK - 5 apps: Orders, Billing, Shop, Docs, Console"), "{}", text.out);

    let mut data = sample();
    data["apps"][0]["launchProfile"] = json!("Ordres");
    data["apps"][1]["project"] = json!("services/Billing/Billing2.csproj");
    data["apps"][3]["variants"][0]["script"] = json!("serve");
    data["apps"][4]["variants"][1]["port"] = json!(47301); // Shop's port
    data["apps"][4]["variants"][0]["when"]["packageScript"] = json!("dev:mock");
    let broken = repo.join("broken.lazyrunner.json");
    write(&broken, &serde_json::to_string_pretty(&data).unwrap());
    let (report, code) = check(&temp, &repo, Some(&broken));
    assert!(report["ok"] == false && code == 1, "{report}");
    assert!(
        find(
            &report,
            "error",
            "apps[0] (orders).launchProfile",
            &["'Ordres' is not in ", "launchSettings.json (it has: Orders)"]
        )
        .is_some(),
        "{report}"
    );
    let billing = format!("{} doesn't exist", native("services/Billing/Billing2.csproj"));
    assert!(find(&report, "error", "apps[1] (billing).project", &[&billing]).is_some(), "{report}");
    assert!(
        find(&report, "error", "apps[3] (docs).variants[0].script", &["package.json has no script 'serve'"]).is_some(),
        "{report}"
    );
    assert!(
        find(&report, "info", "apps[4] (console).variants[0]", &["has no dev:mock on this branch"]).is_some(),
        "{report}"
    );
    assert!(find(&report, "error", "apps", &["Shop and Console both use port 47301"]).is_some(), "{report}");
    let text = lazyrunner(&temp, &repo, &["check", "broken.lazyrunner.json"]);
    assert!(text.out.contains("  x apps[0] (orders).launchProfile: 'Ordres' is not in "), "{}", text.out);
    assert!(text.out.lines().last().unwrap_or_default().ends_with("- fix and check again"), "{}", text.out);
}

#[test]
fn check_finds_the_checkout_a_profile_is_for() {
    let temp = TempDir::new("check-where");
    let repo = sample_repo(&temp.join("repo"));
    let elsewhere = temp.join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let (report, _) = check(&temp, &elsewhere, Some(&sample_profile()));
    assert_eq!(report["ok"], true);
    assert_eq!(report["problems"][0]["level"], "info");
    assert!(
        report["problems"][0]["message"].as_str().unwrap().contains("no folder from here up has Sample.sln"),
        "{report}"
    );

    let (report, _) = check(&temp, &repo.join("services"), Some(&sample_profile()));
    assert!(report["ok"] == true && report["source"] == "given", "{report}");
    assert!(
        same_folder(&report["root"], &repo),
        "a file from elsewhere finds the checkout by its detect file: {report}"
    );

    let nowhere = lazyrunner(&temp, &elsewhere, &["check"]);
    assert_eq!(nowhere.code, 1);
    assert!(nowhere.out.contains("no profile applies here"), "{}", nowhere.out);
    assert!(
        lazyrunner(&temp, &elsewhere, &["check", "-Json"]).out.contains("no profile applies here"),
        "-Json still works"
    );
}

#[test]
fn init_proposes_a_profile_from_the_checkout() {
    let temp = TempDir::new("init");
    let repo = sample_repo(&temp.join("repo"));
    let init = lazyrunner(&temp, &repo, &["init"]);
    assert_eq!(init.code, 0, "{}", init.err);
    let proposal: Value =
        serde_json::from_str(&init.out).unwrap_or_else(|_| panic!("only the profile on stdout: {}", init.out));
    let apps = proposal["apps"].as_array().unwrap();
    let keys: Vec<&str> = apps.iter().map(|app| app["key"].as_str().unwrap()).collect();
    assert_eq!(keys, ["billing", "orders", "console", "shop"], "the runnable apps");
    assert_eq!(apps[0]["launchProfile"], "Billing");
    assert_eq!(
        (apps[2]["variants"][0]["port"].clone(), apps[3]["variants"][0]["port"].clone()),
        (json!(47303), json!(47301)),
        "the ports their scripts name"
    );
    assert_eq!(
        (proposal["watch"][0].clone(), proposal["detect"]["anyFile"][0].clone()),
        (json!("services"), json!("Sample.sln"))
    );
    assert!(
        init.err.contains("web/docs/package.json has no dev, start or serve script but has watch"),
        "notes go to stderr: {}",
        init.err
    );

    let saved = lazyrunner(&temp, &repo, &["init", "-Save"]);
    assert_eq!(saved.code, 0, "{}", saved.err);
    assert!(repo.join(".lazyrunner.json").is_file());
    assert_eq!(check(&temp, &repo, None).0["ok"], true, "what init writes passes check");
    let again = lazyrunner(&temp, &repo, &["init", "--save"]);
    assert!(again.code == 1 && again.err.contains("exists already"), "it never overwrites one: {}", again.err);

    let empty = temp.join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    let nothing = lazyrunner(&temp, &empty, &["init", empty.to_str().unwrap()]);
    assert!(nothing.code == 1 && nothing.err.contains("found nothing to run"), "{}", nothing.err);
}

#[test]
fn init_skips_workers_and_test_projects() {
    let temp = TempDir::new("init-skips");
    let repo = sample_repo(&temp.join("repo"));
    write(repo.join("services/Worker/Worker.csproj"), r#"<Project Sdk="Microsoft.NET.Sdk.Worker"></Project>"#);
    write(
        repo.join("tests/Shop.Tests/Shop.Tests.csproj"),
        r#"<Project Sdk="Microsoft.NET.Sdk"><ItemGroup><PackageReference Include="Microsoft.NET.Test.Sdk" Version="17.11.1" /></ItemGroup></Project>"#,
    );
    write(
        repo.join("tests/Shop.Tests/Properties/launchSettings.json"),
        r#"{ "profiles": { "Shop.Tests": { "commandName": "Project", "applicationUrl": "http://localhost:58499" } } }"#,
    );
    let init = lazyrunner(&temp, &repo, &["init"]);
    assert_eq!(init.code, 0, "{}", init.err);
    let proposal: Value = serde_json::from_str(&init.out).unwrap();
    let keys: Vec<&str> = proposal["apps"].as_array().unwrap().iter().map(|app| app["key"].as_str().unwrap()).collect();
    assert_eq!(keys, ["billing", "orders", "console", "shop"], "test projects left out");
    assert!(
        init.err.contains("skipped services/Worker/Worker.csproj"),
        "a worker without launch settings: {}",
        init.err
    );
    assert_eq!(proposal["watch"][0], "services", "watch is the .NET code without the tests");
}
