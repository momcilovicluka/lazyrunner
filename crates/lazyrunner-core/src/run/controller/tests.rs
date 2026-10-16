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
