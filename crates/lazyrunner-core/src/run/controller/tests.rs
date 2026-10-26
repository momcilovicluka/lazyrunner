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
async fn request_lines_go_to_the_requests_view_and_the_probe_is_left_out() {
    let temp = TempDir::new("controller-requests");
    let (mut sample, _rx) = controller(&temp, json!({}));
    let generation = sample.states[0].generation;
    for line in [
        "info: Microsoft.AspNetCore.Hosting.Diagnostics[2]",
        "      Request finished HTTP/1.1 GET http://localhost:58201/swagger/index.html - 200 - text/html 3.1ms",
        "info: Microsoft.AspNetCore.Hosting.Diagnostics[1]",
        "      Request starting HTTP/1.1 GET http://localhost:58201/api/orders/7 - - -",
        "warn: Orders.Api[0]",
        "      Order 7 not found",
        "info: Microsoft.AspNetCore.Hosting.Diagnostics[2]",
        "      Request finished HTTP/1.1 GET http://localhost:58201/api/orders/7 - 404 0 - 12.5ms",
        "info: Orders.Api[0]",
    ] {
        let tag = crate::run::Tag::App { key: "orders".into(), generation };
        sample.handle(crate::run::Event::Output { tag, line: line.into() });
    }
    let paths: Vec<&str> = sample.requests.items.iter().map(|request| request.path.as_str()).collect();
    assert_eq!(paths, ["/api/orders/7"], "not the probe");
    assert_eq!(sample.requests.items[0].endpoint, "/api/orders/{id}");
    assert_eq!(sample.states[0].requests, 1, "the req/s count");
    assert_eq!(sample.states[0].output.len(), 3, "only the app's own lines show in its column");
    let (first, last) = sample.requests.items[0].logs.expect("its start line was seen");
    let logged: Vec<&str> = sample.states[0].output.numbered(first, last).map(|line| line.text.as_str()).collect();
    assert_eq!(logged, ["warn: Orders.Api[0]", "      Order 7 not found"], "what it logged while it ran");
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
async fn commands_must_name_an_app_this_project_has() {
    use crate::config::Toggle;
    use crate::run::{Command, Origin};
    let temp = TempDir::new("controller-commands");
    let (mut sample, _rx) = controller(&temp, json!({}));
    let refused = sample.execute(Command::Restart("nope".into()), Origin::Agent).unwrap_err();
    assert!(refused.contains("no app 'nope'") && refused.contains("orders"), "{refused}");
    sample.execute(Command::StopAll, Origin::Agent).unwrap();
    assert_eq!(last_log(&sample), "Nothing was running.");
    sample.execute(Command::SetToggle { toggle: Toggle::AutoUpdate, on: false }, Origin::Agent).unwrap();
    assert!(!sample.setting(Toggle::AutoUpdate), "saved");
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

/// The controller with quick scripts in place of dotnet and npm: builds, failures, restarts and waits, without
/// those tools.
mod scripted {
    use std::path::Path;
    use std::sync::Arc;
    use std::time::Duration;

    use serde_json::json;
    use tokio::sync::mpsc::UnboundedReceiver;

    use super::controller;
    use crate::profile::App;
    use crate::run::child::Spec;
    use crate::run::history::Kind;
    use crate::run::tools::{Commands, ServerCommand};
    use crate::run::{Controller, Event, Tag};
    use crate::testing::{CONSOLE_PORT, TempDir, clock};

    /// A build prints its project's line and exits 0 (1 for the projects named in `failing`); a server just waits.
    struct Scripts {
        failing: Vec<&'static str>,
    }

    fn shell(script: String, dir: &Path) -> Spec {
        let (program, flag) = if cfg!(windows) { ("cmd", "/C") } else { ("sh", "-c") };
        Spec {
            program: program.into(),
            args: vec![flag.into(), script.into()],
            dir: dir.to_path_buf(),
            env: Vec::new(),
        }
    }

    impl Commands for Scripts {
        fn server(&self, root: &Path, app: &App, _: &[App]) -> anyhow::Result<ServerCommand> {
            let wait = if cfg!(windows) { "ping -n 30 127.0.0.1 >nul" } else { "sleep 30" };
            Ok(ServerCommand {
                spec: shell(wait.into(), root),
                label: format!("{} (a script)", app.name),
                script: None,
            })
        }

        fn build(&self, root: &Path, project: &str) -> Spec {
            let name =
                Path::new(project).file_stem().map(|stem| stem.to_string_lossy().into_owned()).unwrap_or_default();
            let code = i32::from(self.failing.iter().any(|failing| project.contains(failing)));
            let script = if cfg!(windows) {
                format!("echo   {name} -^> bin\\{name}.dll& exit /b {code}")
            } else {
                format!("echo '  {name} -> bin/{name}.dll'; exit {code}")
            };
            shell(script, root)
        }

        fn npm_ci(&self, dir: &Path) -> anyhow::Result<Spec> {
            Ok(shell("echo added 1 package".into(), dir))
        }

        fn npm_install(&self, dir: &Path) -> anyhow::Result<Spec> {
            Ok(shell("echo changed 2 packages".into(), dir))
        }
    }

    fn scripted(label: &str, failing: Vec<&'static str>) -> (TempDir, Controller, UnboundedReceiver<Event>) {
        let temp = TempDir::new(label);
        let (mut controller, events) = controller(&temp, json!({ "skipUnchangedBuilds": false }));
        controller.commands = Arc::new(Scripts { failing });
        (temp, controller, events)
    }

    fn keys(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    fn log(controller: &Controller) -> String {
        controller.log.iter().map(|entry| entry.text.as_str()).collect::<Vec<_>>().join("\n")
    }

    /// Handles the controller's events until `done`.
    async fn run_until(
        controller: &mut Controller,
        events: &mut UnboundedReceiver<Event>,
        done: impl Fn(&Controller) -> bool,
    ) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        while !done(controller) {
            match tokio::time::timeout_at(deadline, events.recv()).await {
                Ok(Some(event)) => controller.handle(event),
                _ => panic!("timed out; the log:\n{}", log(controller)),
            }
        }
    }

    #[tokio::test]
    async fn npm_install_runs_without_a_lock_file_and_is_timed_on_its_own() {
        let (_temp, mut sample, mut events) = scripted("scripted-npm-install", Vec::new());
        sample.npm_ci(Some("shop"));
        assert!(log(&sample).ends_with("npm ci needs one (npm install makes it)."), "{}", log(&sample));
        sample.npm_install(Some("shop"));
        assert_eq!(sample.npm_ci.as_ref().map(|job| job.what), Some("npm install"));
        assert_eq!(sample.handover_blocked().as_deref(), Some("npm install is running"));
        run_until(&mut sample, &mut events, |c| c.npm_ci.is_none()).await;
        let log = log(&sample);
        assert!(log.contains("Running npm install for Shop") && log.contains("✓ npm install done in"), "{log}");
        assert_eq!(sample.durations.item(Kind::NpmInstall, "shop").map(|item| item.runs.len()), Some(1));
        assert!(sample.durations.item(Kind::NpmCi, "shop").is_none());
    }

    #[tokio::test]
    async fn a_start_builds_one_app_at_a_time_then_starts_them() {
        let (_temp, mut sample, mut events) = scripted("scripted-start", Vec::new());
        sample.start(Some(&keys(&["orders", "billing"])), false);
        assert_eq!(sample.building.as_ref().map(|job| job.key.as_str()), Some("orders"), "one at a time");
        assert_eq!(sample.build_queue, keys(&["orders", "billing"]));
        run_until(&mut sample, &mut events, |c| c.is_running("orders") && c.is_running("billing")).await;
        let log = log(&sample);
        assert!(log.contains("✓ Orders built in") && log.contains("✓ Billing built in"), "{log}");
        assert!(log.contains("Starting Orders + Billing"), "{log}");
        assert_eq!(sample.durations.item(Kind::Build, "billing").map(|item| item.runs.len()), Some(1));
    }

    #[tokio::test]
    async fn a_failed_build_starts_nothing_and_says_how_to_go_on() {
        let (_temp, mut sample, mut events) = scripted("scripted-failed", vec!["Orders"]);
        sample.start(Some(&keys(&["orders", "billing"])), false);
        run_until(&mut sample, &mut events, |c| c.building.is_none()).await;
        assert!(!sample.is_running("orders") && !sample.is_running("billing"));
        assert!(sample.build_queue.is_empty(), "billing isn't built either");
        let log = log(&sample);
        assert!(log.contains("✗ Orders build failed"), "{log}");
        assert!(log.contains("APIs not started - fix the build and press U or R."), "{log}");
        let runs = &sample.durations.item(Kind::Build, "orders").unwrap().runs;
        assert!(!runs[0].ok, "recorded as failed");
    }

    #[tokio::test]
    async fn b_builds_and_leaves_the_app_stopped() {
        let (_temp, mut sample, mut events) = scripted("scripted-b", Vec::new());
        sample.build_only(Some(&keys(&["orders"])));
        run_until(&mut sample, &mut events, |c| c.building.is_none()).await;
        assert!(!sample.is_running("orders"));
        assert!(log(&sample).contains("Build done - u starts it"), "{}", log(&sample));
    }

    #[tokio::test]
    async fn the_exit_of_a_run_that_was_restarted_is_ignored() {
        let (_temp, mut sample, _events) = scripted("scripted-generation", Vec::new());
        sample.start(Some(&keys(&["shop"])), false);
        let index = sample.index("shop").unwrap();
        let first = sample.states[index].generation;
        sample.restart("a test", Some(&keys(&["shop"])), false);
        assert!(sample.is_running("shop"));
        assert_ne!(sample.states[index].generation, first);
        sample.handle(Event::Exited { tag: Tag::App { key: "shop".into(), generation: first }, code: Some(1) });
        assert!(sample.is_running("shop"), "the old run's exit doesn't stop the new one");
    }

    #[tokio::test]
    async fn a_start_waits_for_a_port_only_as_long_as_a_stop_may_take() {
        let Ok(holder) = std::net::TcpListener::bind(("127.0.0.1", CONSOLE_PORT)) else {
            eprintln!("skipped: something else holds port {CONSOLE_PORT}");
            return;
        };
        let (_temp, mut sample, _events) = scripted("scripted-port-wait", Vec::new());
        sample.now = clock::now;
        sample.last_stop = Some(clock::now()); // something just stopped: its port may still be held
        sample.start(Some(&keys(&["console"])), false);
        assert!(sample.waiting_start.is_some(), "it waits for the port");
        assert!(!sample.is_running("console"));
        clock::advance(Duration::from_secs(4));
        sample.tick();
        assert!(sample.waiting_start.is_none(), "no longer than a stop may take");
        assert!(log(&sample).contains(&format!("Port {CONSOLE_PORT} is already used by")), "{}", log(&sample));
        drop(holder);
    }
}
