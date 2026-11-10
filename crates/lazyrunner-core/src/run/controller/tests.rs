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
async fn a_hook_runs_when_its_event_happens_and_says_when_it_fails() {
    let temp = TempDir::new("hooks");
    let hooks = json!([{ "on": "tests.red", "run": "exit 4" }, { "on": "tests.blue", "run": "x" }]);
    let (mut controller, mut rx) = controller(&temp, json!({ "hooks": hooks }));
    controller.happened("tests.red", None, "1 failed".into());
    assert!(last_log(&controller).starts_with("▸ hook 2: \"on\""), "{}", last_log(&controller));
    let logged = controller.log.len();
    controller.happened("tests.green", None, "all passed".into());
    assert_eq!(controller.log.len(), logged, "a problem is said once");
    let failed = async {
        loop {
            if let Some(event @ crate::run::Event::HookFailed { .. }) = rx.recv().await {
                return event;
            }
        }
    };
    controller.handle(tokio::time::timeout(std::time::Duration::from_secs(30), failed).await.unwrap());
    assert_eq!(last_log(&controller), "▸ hook tests.red (exit 4): exit 4");
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
async fn an_apps_calls_to_another_are_the_stack_maps_edges() {
    let temp = TempDir::new("controller-calls");
    let (mut sample, _rx) = controller(&temp, json!({}));
    let generation = sample.states[0].generation;
    let lines = [
        "info: System.Net.Http.HttpClient.billing.LogicalHandler[100]",
        "      => SpanId:a1 => HTTP GET http://localhost:47202/api/invoices",
        "      Start processing HTTP request GET http://localhost:47202/api/invoices",
        "info: System.Net.Http.HttpClient.billing.LogicalHandler[101]",
        "      => SpanId:a1 => HTTP GET http://localhost:47202/api/invoices",
        "      End processing HTTP request after 31.2ms - 200",
    ];
    for line in lines {
        let tag = crate::run::Tag::App { key: "orders".into(), generation };
        sample.handle(crate::run::Event::Output { tag, line: line.into() });
    }
    assert_eq!(sample.calls.items.len(), 1);
    let edges = sample.stack_edges();
    let billing =
        edges.iter().find(|edge| edge.from == "orders" && edge.to == "billing").expect("orders calls billing");
    assert_eq!((billing.calls, billing.p95_ms, billing.failed), (1, Some(31.2), 0));
    let shown: Vec<String> = sample.states[0].output.newest_first(0).map(|line| line.text.clone()).collect();
    assert!(shown.iter().any(|text| text.contains("Start processing")), "the call stays in the column: {shown:?}");
}

#[tokio::test]
async fn an_app_whose_project_references_ef_core_uses_the_database_before_its_first_command() {
    let temp = TempDir::new("controller-database-apps");
    let (mut sample, _rx) = controller(&temp, json!({}));
    assert!(sample.database_users().is_empty(), "nothing says any app uses it yet");
    let project = sample.root.join("services/Orders/Orders.csproj");
    let text = std::fs::read_to_string(&project).expect("the sample's project");
    let with_ef = text.replace(
        "</Project>",
        r#"<ItemGroup><PackageReference Include="Microsoft.EntityFrameworkCore.SqlServer" Version="8.0.0" /></ItemGroup></Project>"#,
    );
    std::fs::write(&project, with_ef).expect("written");
    sample.find_configured_calls();
    assert_eq!(sample.database_users(), ["orders"]);
    assert!(!sample.database_seen("orders"), "declared by its packages, no command seen yet");
}

#[tokio::test]
async fn an_npm_app_starts_with_the_node_hook_on_top_of_its_own_node_options() {
    let temp = TempDir::new("controller-node-hook");
    let (sample, _rx) = controller(&temp, json!({}));
    let shop = sample.server_command("shop").expect("the shop starts");
    let options: Vec<String> = shop
        .spec
        .env
        .iter()
        .filter(|(name, _)| name == "NODE_OPTIONS")
        .map(|(_, value)| value.to_string_lossy().into_owned())
        .collect();
    assert_eq!(options.len(), 1, "{:?}", shop.spec.env);
    assert!(options[0].ends_with("call-hook.cjs\""), "{}", options[0]);
    assert!(!options[0].contains('\\'), "forward slashes: {}", options[0]);
    let hook = sample.state_dir.join(crate::run::calls::NODE_HOOK_FILE);
    assert_eq!(std::fs::read_to_string(hook).ok().as_deref(), Some(crate::run::calls::NODE_HOOK));
    let orders = sample.server_command("orders").expect("orders starts");
    assert!(orders.spec.env.iter().all(|(name, _)| name != "NODE_OPTIONS"), "a .NET app logs its calls itself");
}

#[tokio::test]
async fn a_browser_apps_calls_are_in_its_config_and_their_figures_what_the_api_served() {
    let temp = TempDir::new("controller-configured");
    let (mut sample, _rx) = controller(&temp, json!({}));
    crate::testing::write(
        sample.root.join("web/shop/proxy.conf.json"),
        r#"{ "/api": { "target": "http://localhost:47201" } }"#,
    );
    sample.find_configured_calls();
    let edges = sample.stack_edges();
    let shop =
        edges.iter().find(|edge| edge.from == "shop" && edge.to == "orders").expect("its proxy names orders' port");
    assert_eq!((shop.declared_in.as_deref(), shop.seen, shop.served), (Some("proxy.conf.json"), 0, false));

    let finished = |status: u16, ms: f64| crate::run::requests::Finished {
        method: "GET".into(),
        path: "/api/orders".into(),
        status,
        ms,
        ..Default::default()
    };
    sample.requests.add("orders", finished(200, 20.0), chrono::Local::now());
    sample.requests.add("orders", finished(500, 80.0), chrono::Local::now());
    let edges = sample.stack_edges();
    let shop = edges.iter().find(|edge| edge.from == "shop" && edge.to == "orders").expect("still there");
    assert!(shop.served, "a browser app can't log its calls: what orders served stands for them");
    assert_eq!((shop.calls, shop.failed, shop.p95_ms), (2, 1, Some(80.0)));
}

#[tokio::test]
async fn an_app_that_logs_scopes_has_each_requests_own_lines_even_when_they_overlap() {
    let temp = TempDir::new("controller-scopes");
    let (mut sample, _rx) = controller(&temp, json!({}));
    let generation = sample.states[0].generation;
    let scope = |connection: &str, path: &str| {
        format!(
            "      => SpanId:a1, TraceId:b2 => ConnectionId:{connection} => RequestPath:{path} RequestId:{connection}:00000001"
        )
    };
    let lines = [
        "info: Microsoft.AspNetCore.Hosting.Diagnostics[1]".to_string(),
        scope("c1", "/api/orders/7"),
        "      Request starting HTTP/1.1 GET http://localhost:47201/api/orders/7 - - -".into(),
        "info: Microsoft.AspNetCore.Hosting.Diagnostics[1]".into(),
        scope("c2", "/api/orders/8"),
        "      Request starting HTTP/1.1 GET http://localhost:47201/api/orders/8 - - -".into(),
        "warn: Orders.Api[0]".into(),
        scope("c1", "/api/orders/7"),
        "      Order 7 not found".into(),
        "info: Orders.Api[0]".into(),
        scope("c2", "/api/orders/8"),
        "      Order 8 found".into(),
        "info: Microsoft.AspNetCore.Hosting.Diagnostics[2]".into(),
        scope("c1", "/api/orders/7"),
        "      Request finished HTTP/1.1 GET http://localhost:47201/api/orders/7 - 404 0 - 12.5ms".into(),
        "info: Microsoft.AspNetCore.Hosting.Diagnostics[2]".into(),
        scope("c2", "/api/orders/8"),
        "      Request finished HTTP/1.1 GET http://localhost:47201/api/orders/8 - 200 0 - 15.5ms".into(),
    ];
    for line in lines {
        let tag = crate::run::Tag::App { key: "orders".into(), generation };
        sample.handle(crate::run::Event::Output { tag, line });
    }
    let output = &sample.states[0].output;
    assert_eq!(output.len(), 4, "the scope lines stay out of the column");
    let own = |at: usize| -> Vec<String> {
        let request = &sample.requests.items[at];
        assert!(!request.mixed, "{} is matched by its RequestId", request.path);
        let numbers = request.exact.clone().expect("matched by its RequestId");
        numbers
            .iter()
            .filter_map(|number| output.numbered(*number, *number).next())
            .map(|line| line.text.clone())
            .collect()
    };
    assert_eq!(own(0), ["warn: Orders.Api[0]", "      Order 7 not found"]);
    assert_eq!(own(1), ["info: Orders.Api[0]", "      Order 8 found"]);
}

#[tokio::test]
async fn request_lines_go_to_the_requests_view_and_the_probe_is_left_out() {
    let temp = TempDir::new("controller-requests");
    let (mut sample, _rx) = controller(&temp, json!({}));
    let generation = sample.states[0].generation;
    for line in [
        "info: Microsoft.AspNetCore.Hosting.Diagnostics[2]",
        "      Request finished HTTP/1.1 GET http://localhost:47201/swagger/index.html - 200 - text/html 3.1ms",
        "info: Microsoft.AspNetCore.Hosting.Diagnostics[1]",
        "      Request starting HTTP/1.1 GET http://localhost:47201/api/orders/7 - - -",
        "warn: Orders.Api[0]",
        "      Order 7 not found",
        "info: Microsoft.EntityFrameworkCore.Database.Command[20101]",
        "      Executed DbCommand (3ms) [Parameters=[@p0='7'], CommandType='Text']",
        "      SELECT [o].[Id] FROM [Orders] AS [o] WHERE [o].[Id] = @p0",
        "info: Microsoft.AspNetCore.Hosting.Diagnostics[2]",
        "      Request finished HTTP/1.1 GET http://localhost:47201/api/orders/7 - 404 0 - 12.5ms",
        "info: Orders.Api[0]",
    ] {
        let tag = crate::run::Tag::App { key: "orders".into(), generation };
        sample.handle(crate::run::Event::Output { tag, line: line.into() });
    }
    let paths: Vec<&str> = sample.requests.items.iter().map(|request| request.path.as_str()).collect();
    assert_eq!(paths, ["/api/orders/7"], "not the probe");
    assert_eq!(sample.requests.items[0].endpoint, "/api/orders/{id}");
    assert_eq!(sample.states[0].requests, 1, "the req/s count");
    assert_eq!(sample.states[0].output.len(), 6, "only the app's own lines show in its column");
    let (first, last) = sample.requests.items[0].logs.expect("its start line was seen");
    let logged: Vec<&str> = sample.states[0].output.numbered(first, last).map(|line| line.text.as_str()).collect();
    assert_eq!(logged[..2], ["warn: Orders.Api[0]", "      Order 7 not found"], "what it logged while it ran");
    let db = sample.requests.items[0].db.clone().expect("its query, summed up as it finished");
    assert_eq!((db.commands, db.total_ms, db.repeated), (1, 3.0, None));
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
    assert_eq!(controller.port("orders"), Some(47201), "ports come from the launch settings");
}

#[tokio::test]
async fn variants_decide_ports_and_a_missing_script_skips_the_app() {
    let temp = TempDir::new("controller-variants");
    let (mut controller, _rx) = controller(&temp, json!({}));
    assert_eq!(controller.port("console"), Some(47303), "the live variant: package.json has no dev:mock");
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
    async fn an_app_whose_port_windows_reserved_doesnt_start_and_the_log_says_why() {
        let (_temp, mut sample, _events) = scripted("scripted-reserved-port", Vec::new());
        sample.reserved = || vec![(CONSOLE_PORT - 5, CONSOLE_PORT + 5)];
        sample.start(Some(&keys(&["console"])), false);
        assert!(!sample.is_running("console"));
        let text = log(&sample);
        assert!(text.contains(&format!("✗ Console can't start: port {CONSOLE_PORT} is reserved by Windows")), "{text}");
        assert!(text.contains("net stop winnat"), "{text}");
        assert!(sample.conflicts.is_empty(), "! can't free a reserved port");
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

/// A command app run for real (by cmd or sh): in its folder, its port in its command, up once its ready line shows,
/// its error line counted.
#[tokio::test]
async fn a_command_app_runs_in_its_folder_with_its_port_and_is_up_once_its_line_shows() {
    use std::time::Duration;
    let temp = TempDir::new("command-run");
    let repo = temp.join("repo");
    std::fs::create_dir_all(repo.join("svc")).unwrap();
    let (shell, flag, script) = if cfg!(windows) {
        ("cmd", "/C", "echo port {port} in %CD%& echo BOOM bad& echo Worker ready& ping -n 30 127.0.0.1 >nul")
    } else {
        ("sh", "-c", "echo port {port} in $PWD; echo BOOM bad; echo Worker ready; sleep 30")
    };
    let profile = json!({
        "schemaVersion": 1, "id": "cmd", "name": "Commands", "detect": { "anyFile": ["marker"] }, "watch": ["svc"],
        "apps": [{ "key": "worker", "name": "Worker", "type": "command", "run": [shell, flag, script], "cwd": "svc",
                   "port": 47998, "ready": { "line": "Worker ready" }, "errors": ["^BOOM"] }]
    });
    let file = temp.join("profile.json");
    crate::config::write(&file, &profile).unwrap();
    let paths = Paths {
        config_file: temp.join("config.json"),
        state_dir: temp.join("state"),
        profiles: ProfileDirs { user: temp.join("profiles"), bundled: temp.join("bundled"), only: Some(file.clone()) },
    };
    crate::config::write(&paths.config_file, &json!({})).unwrap();
    let project = import(&file, &paths.profiles).unwrap();
    let (events, mut rx) = mpsc::unbounded_channel();
    let mut controller = Controller::new(Setup { project, root: repo, paths, events, guard: None }).unwrap();
    controller.start(None, false);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while !controller.state("worker").is_some_and(|state| state.is_up()) {
        let log: Vec<String> = controller.log.iter().map(|entry| entry.text.clone()).collect();
        match tokio::time::timeout_at(deadline, rx.recv()).await {
            Ok(Some(event)) => controller.handle(event),
            _ => panic!("not up; the log:\n{}", log.join("\n")),
        }
    }
    let state = controller.state("worker").unwrap();
    let said: Vec<String> = state.output.newest_first(0).map(|line| line.text.clone()).collect();
    let started = said.iter().find(|line| line.starts_with("port ")).cloned().unwrap_or_default();
    assert!(started.starts_with("port 47998 in") && started.trim_end().ends_with("svc"), "{said:?}");
    assert_eq!(state.errors, 1, "{said:?}");
    controller.stop(None, true);
}

/// Tasks run for real (by cmd or sh): their output under a rule in the panel's log, a good one's time, a failed one's
/// first error line, both kept in the durations and as their last result.
#[tokio::test]
async fn a_task_runs_in_the_background_its_output_in_the_log_and_its_result_said() {
    use std::time::Duration;
    let temp = TempDir::new("task-run");
    let repo = temp.join("repo");
    std::fs::create_dir_all(repo.join("tools")).unwrap();
    let (shell, flag) = if cfg!(windows) { ("cmd", "/C") } else { ("sh", "-c") };
    let (good, bad) = if cfg!(windows) {
        ("echo seeding& echo 3 rows in %CD%", "echo starting& echo error: no database& exit /b 3")
    } else {
        ("echo seeding; echo 3 rows in $PWD", "echo starting; echo error: no database; exit 3")
    };
    let profile = json!({
        "schemaVersion": 1, "id": "tasks", "name": "Tasks", "detect": { "anyFile": ["marker"] }, "watch": ["tools"],
        "apps": [{ "key": "worker", "name": "Worker", "type": "command", "run": [shell, flag, "echo up"] }],
        "tasks": [
            { "key": "seed", "name": "Seed the database", "run": [shell, flag, good], "cwd": "tools" },
            { "key": "broken", "name": "Broken", "run": [shell, flag, bad] }
        ]
    });
    let file = temp.join("profile.json");
    crate::config::write(&file, &profile).unwrap();
    let paths = Paths {
        config_file: temp.join("config.json"),
        state_dir: temp.join("state"),
        profiles: ProfileDirs { user: temp.join("profiles"), bundled: temp.join("bundled"), only: Some(file.clone()) },
    };
    crate::config::write(&paths.config_file, &json!({})).unwrap();
    let project = import(&file, &paths.profiles).unwrap();
    let (events, mut rx) = mpsc::unbounded_channel();
    let mut controller = Controller::new(Setup { project, root: repo, paths, events, guard: None }).unwrap();
    controller.run_task("seed");
    controller.run_task("broken");
    assert_eq!(controller.tasks_running().len(), 2);
    controller.run_task("seed");
    assert!(last_log(&controller).contains("already running"), "{}", last_log(&controller));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while !controller.tasks_running().is_empty() {
        let log: Vec<String> = controller.log.iter().map(|entry| entry.text.clone()).collect();
        match tokio::time::timeout_at(deadline, rx.recv()).await {
            Ok(Some(event)) => controller.handle(event),
            _ => panic!("still running; the log:\n{}", log.join("\n")),
        }
    }
    let log: Vec<String> = controller.log.iter().map(|entry| entry.text.clone()).collect();
    let said = log.join("\n");
    assert!(said.contains("╭─ ▸ Seed the database ·") && !said.contains("│ seeding"), "it passed: folded {said}");
    let passed = log.iter().find(|line| line.starts_with("╰─ ✓ Seed the database in")).cloned().unwrap_or_default();
    assert!(passed.ends_with("2 lines folded (the log file has them)"), "{said}");
    let file = std::fs::read_to_string(&controller.log_path).unwrap();
    assert!(file.lines().any(|line| line.contains("│ 3 rows in") && line.trim_end().ends_with("tools")), "{file}");
    assert!(
        said.contains("│ starting") && said.contains("╰─ ✗ Broken failed after"),
        "a failure keeps its lines: {said}"
    );
    assert!(said.contains(": error: no database"), "{said}");
    let failed: Vec<&str> = controller.task_failures().iter().map(|(task, _)| task.key.as_str()).collect();
    assert_eq!(failed, ["broken"]);
    controller.dismiss_task_result("broken");
    assert!(controller.task_failures().is_empty(), "put away");
    let seed = controller.task_result("seed").unwrap();
    let broken = controller.task_result("broken").unwrap();
    assert!(seed.ok && !broken.ok && broken.error.as_deref() == Some("error: no database"));
    assert!(controller.durations.item(crate::run::history::Kind::Task, "seed").is_some());
    controller.run_task("nothing");
    assert!(last_log(&controller).contains("There is no task 'nothing'"));
}

/// A command app's install for real (by cmd or sh): its setup first while its .venv isn't there, then its install;
/// in sync with what it installs from until that changes, which the log says.
#[tokio::test]
async fn a_command_apps_setup_makes_its_folder_then_its_install_runs_and_a_change_makes_it_due() {
    use std::time::Duration;
    let temp = TempDir::new("command-install");
    let repo = temp.join("repo");
    crate::testing::write(repo.join("svc/requirements.txt"), "flask\n");
    let (shell, flag, make, install) = if cfg!(windows) {
        ("cmd", "/C", "mkdir .venv\\Scripts", "echo installed")
    } else {
        ("sh", "-c", "mkdir -p .venv/bin", "echo installed")
    };
    let profile = json!({
        "schemaVersion": 1, "id": "cmd", "name": "Commands", "detect": { "anyFile": ["marker"] }, "watch": ["svc"],
        "apps": [{ "key": "svc", "name": "Svc", "type": "command", "run": "x", "cwd": "svc",
                   "path": [".venv/Scripts", ".venv/bin"], "setup": [shell, flag, make], "install": [shell, flag, install],
                   "installFrom": ["requirements.txt"] }]
    });
    let file = temp.join("profile.json");
    crate::config::write(&file, &profile).unwrap();
    let paths = Paths {
        config_file: temp.join("config.json"),
        state_dir: temp.join("state"),
        profiles: ProfileDirs { user: temp.join("profiles"), bundled: temp.join("bundled"), only: Some(file.clone()) },
    };
    crate::config::write(&paths.config_file, &json!({})).unwrap();
    let project = import(&file, &paths.profiles).unwrap();
    let (events, mut rx) = mpsc::unbounded_channel();
    let mut controller = Controller::new(Setup { project, root: repo.clone(), paths, events, guard: None }).unwrap();
    controller.npm_ci(Some("svc"));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while !controller.log.iter().any(|entry| entry.text.starts_with("✓ install done")) {
        let log: Vec<String> = controller.log.iter().map(|entry| entry.text.clone()).collect();
        match tokio::time::timeout_at(deadline, rx.recv()).await {
            Ok(Some(event)) => controller.handle(event),
            _ => panic!("not installed; the log:\n{}", log.join("\n")),
        }
    }
    let said: Vec<String> = controller.log.iter().map(|entry| entry.text.clone()).collect();
    assert!(said.iter().any(|line| line.starts_with("Running setup for Svc")), "{said:?}");
    assert!(repo.join("svc/.venv").is_dir() && controller.lock_in_sync("svc") == Some(true), "{said:?}");
    crate::testing::write(repo.join("svc/requirements.txt"), "flask\nrequests\n");
    assert_eq!(controller.lock_in_sync("svc"), Some(false));
    controller.test_lock_changed();
    assert!(
        last_log(&controller).contains("requirements.txt changed since its last install - press i"),
        "{}",
        last_log(&controller)
    );
}
