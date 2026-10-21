//! The controller without real servers: what it says and decides. The sample repo end to end (real dotnet and
//! npm) is in the lazyrunner crate's tests/sample.rs.

use std::path::Path;

use serde_json::json;
use tokio::sync::mpsc;

use super::{Controller, Setup};
use crate::paths::Paths;
use crate::profile::{ProfileDirs, import};
use crate::testing::{TempDir, sample_profile, sample_repo};

fn controller(temp: &TempDir, config: serde_json::Value) -> (Controller, mpsc::UnboundedReceiver<crate::run::Event>) {
    let repo = sample_repo(&temp.join("repo"));
    let paths = Paths {
        config_file: temp.join("config.json"),
        state_dir: temp.join("state"),
        profiles: ProfileDirs {
            user: temp.join("profiles"),
            bundled: temp.join("bundled"),
            only: Some(sample_profile()),
        },
    };
    crate::config::write(&paths.config_file, &config).unwrap();
    let project = import(&sample_profile(), &paths.profiles).unwrap();
    let (events, rx) = mpsc::unbounded_channel();
    let controller = Controller::new(Setup { project, root: repo, paths, events, guard: None }).unwrap();
    (controller, rx)
}

fn last_log(controller: &Controller) -> String {
    controller.log.back().map(|entry| entry.text.clone()).unwrap_or_default()
}

#[tokio::test]
async fn stopping_nothing_says_so_and_stop_all_names_what_it_keeps() {
    let temp = TempDir::new("controller-stop");
    let (mut controller, _rx) = controller(&temp, json!({}));
    controller.stop(None, false);
    assert_eq!(last_log(&controller), "Nothing was running.");
    controller.stop_all();
    assert_eq!(last_log(&controller), "Nothing was running.");
    assert!(temp.join("state").join("lazyrunner.log").is_file(), "the log is written to lazyrunner.log");
    let log = std::fs::read_to_string(temp.join("state").join("lazyrunner.log")).unwrap();
    assert!(log.lines().all(|line| line.len() > 20 && line.as_bytes()[4] == b'-'), "dated lines: {log}");
}

#[tokio::test]
async fn b_on_an_app_without_a_separate_build_says_so() {
    let temp = TempDir::new("controller-build-only");
    let (mut sample, _rx) = controller(&temp, json!({}));
    sample.build_only(Some(&["shop".to_string()]));
    assert_eq!(last_log(&sample), "Shop has no separate build - it compiles while it runs (r restarts it).");
    assert!(sample.building.is_none() && sample.build_queue.is_empty());
    let npm_temp = TempDir::new("controller-build-only-npm");
    let (mut npm_only, _npm_rx) = controller(
        &npm_temp,
        json!({ "projects": { "sample": { "apps": {
        "orders": { "enabled": false }, "billing": { "enabled": false } } } } }),
    );
    npm_only.build_only(None);
    assert_eq!(last_log(&npm_only), "Nothing to build: no app here has a separate build step.");

    // B leaves what S leaves: here both .NET apps are out of "all".
    let kept_temp = TempDir::new("controller-build-only-kept");
    let (mut kept, _kept_rx) = controller(
        &kept_temp,
        json!({ "projects": { "sample": { "apps": {
        "orders": { "includeInAll": false }, "billing": { "includeInAll": false } } } } }),
    );
    kept.build_only(None);
    assert_eq!(last_log(&kept), "Nothing to build: no app here has a separate build step.");
    assert!(kept.building.is_none());
}

#[tokio::test]
async fn parked_starts_join_and_a_stop_only_drops_its_own_apps() {
    let temp = TempDir::new("controller-parked");
    let (mut sample, _rx) = controller(&temp, json!({}));
    let keys = |names: &[&str]| names.iter().map(|name| name.to_string()).collect::<Vec<_>>();
    sample.park_start(Some(&keys(&["orders"])), false);
    sample.park_start(Some(&keys(&["shop", "orders"])), true);
    assert_eq!(sample.waiting_start, Some((Some(keys(&["orders", "shop"])), true)), "both wait, forced");
    sample.stop(Some(&keys(&["shop"])), true);
    assert_eq!(sample.waiting_start, Some((Some(keys(&["orders"])), true)), "shop was stopped: orders still waits");
    sample.park_start(None, false);
    assert_eq!(sample.waiting_start, Some((None, true)), "everything");
    sample.stop(None, true);
    assert_eq!(sample.waiting_start, None);
}

#[tokio::test]
async fn apps_turned_off_on_this_machine_get_no_column() {
    let temp = TempDir::new("controller-off");
    let (controller, _rx) = controller(&temp, json!({ "projects": { "sample": { "frontend": false } } }));
    assert_eq!(controller.keys(), ["orders", "billing"]);
    assert_eq!(controller.port("orders"), Some(58201), "ports come from the launch settings");
}

#[tokio::test]
async fn variants_decide_ports_and_a_missing_script_skips_the_app() {
    let temp = TempDir::new("controller-variants");
    let (mut controller, _rx) = controller(&temp, json!({}));
    assert_eq!(controller.port("console"), Some(58303), "the live variant: package.json has no dev:mock");
    assert_eq!(controller.port("docs"), None, "a watch build has no port");
    crate::testing::write(controller.root.join("web/shop/package.json"), r#"{ "scripts": { "build": "x" } }"#);
    controller.start(Some(&["shop".to_string()]), false);
    let expected = format!("Shop skipped: {} has no dev", Path::new("web").join("shop").join("package.json").display());
    let texts: Vec<&str> = controller.log.iter().map(|entry| entry.text.as_str()).collect();
    assert_eq!(
        texts[texts.len() - 2..],
        [expected.as_str(), "Already running."],
        "as PowerShell: nothing else to start"
    );
}
