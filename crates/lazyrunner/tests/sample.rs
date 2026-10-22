//! Many apps end to end (the PowerShell version's tests/sample.e2e.ps1): lazyrunner on the sample repo, two .NET
//! APIs and three npm apps (a dev server, a watch build an API serves, one with two variants), driven over the
//! control channel and drawn headless. U starts all five and every one must get ready; 5 focuses the fifth
//! column; S stops all but the app with includeInAll false; and nothing lazyrunner started survives it, whether
//! it quits or is killed. Needs dotnet, node, npm and git (skipped without them).

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use lazyrunner_core::run::net::Listening;
use lazyrunner_core::testing::{
    CONSOLE_PORT, ORDERS_PORT, SHOP_PORT, TempDir, has_tools, npm_app, sample_profile, sample_repo, write,
};
use serde_json::{Value, json};

const BIN: &str = env!("CARGO_BIN_EXE_lazyrunner");
const BILLING_PORT: u16 = lazyrunner_core::testing::BILLING_PORT;

struct Lazyrunner {
    child: Child,
    name: String,
    state: PathBuf,
}

impl Lazyrunner {
    fn start(temp: &TempDir, root: &Path, profile: &Path, config: Value, extra: &[&str]) -> Self {
        let state = temp.join("state");
        std::fs::create_dir_all(&state).unwrap();
        write(temp.join("config.json"), &config.to_string());
        let name = format!("e2e-{}-{}", std::process::id(), temp.path().file_name().unwrap().to_string_lossy());
        let child = Command::new(BIN)
            .args(["run", "--root"])
            .arg(root)
            .args(["--headless", "170x45", "--ipc", &name])
            .args(extra)
            .env("LAZYRUNNER_STATE_DIR", &state)
            .env("LAZYRUNNER_CONFIG_PATH", temp.join("config.json"))
            .env("LAZYRUNNER_PROFILES_DIR", temp.join("profiles"))
            .env("LAZYRUNNER_PROFILE", profile)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("lazyrunner starts");
        let lazyrunner = Self { child, name, state };
        lazyrunner.wait_for("the control channel", Duration::from_secs(30), || {
            lazyrunner.ask(json!({ "state": true }))["ok"] == true
        });
        lazyrunner
    }

    fn ask(&self, request: Value) -> Value {
        let output = Command::new(BIN).args(["ctl", &self.name, &request.to_string()]).output().unwrap();
        serde_json::from_slice(&output.stdout).unwrap_or(json!({ "ok": false }))
    }

    fn press(&self, keys: &str) {
        assert_eq!(self.ask(json!({ "keys": keys }))["ok"], true, "keys '{keys}'");
    }

    fn frame(&self) -> String {
        self.ask(json!({ "dump": true }))["frame"].as_str().unwrap_or_default().to_string()
    }

    fn log(&self) -> String {
        std::fs::read_to_string(self.state.join("lazyrunner.log")).unwrap_or_default()
    }

    /// The newest log line matching `part`, without its date and time.
    fn log_line(&self, part: &str) -> String {
        self.log()
            .lines()
            .rev()
            .find(|line| line.contains(part))
            .map(|line| line.splitn(3, ' ').nth(2).unwrap_or(line).to_string())
            .unwrap_or_default()
    }

    fn wait_for(&self, what: &str, timeout: Duration, mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + timeout;
        while !done() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}; log:\n{}", self.log());
            thread::sleep(Duration::from_millis(300));
        }
    }

    fn quit(mut self) {
        let _ = self.ask(json!({ "action": "quit" }));
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(status.success(), "lazyrunner quits cleanly: {status}");
                return;
            }
            assert!(Instant::now() < deadline, "lazyrunner didn't quit");
            thread::sleep(Duration::from_millis(100));
        }
    }
}

impl Drop for Lazyrunner {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

/// Waits until none of these ports is listening any more.
fn wait_free(ports: &[u16], why: &str) {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let listening = Listening::now();
        let held: Vec<u16> = ports.iter().copied().filter(|port| listening.contains(*port)).collect();
        if held.is_empty() {
            return;
        }
        assert!(Instant::now() < deadline, "{why}: still listening on {held:?}");
        thread::sleep(Duration::from_millis(300));
    }
}

fn skip_without(tools: &[&str]) -> bool {
    if has_tools(tools) {
        return false;
    }
    eprintln!("skipped: this test needs {}", tools.join(", "));
    true
}

#[test]
fn the_sample_repo_starts_gets_ready_and_stops() {
    if skip_without(&["dotnet", "node", "npm", "git"]) {
        return;
    }
    let temp = TempDir::new("sample");
    let repo = sample_repo(&temp.join("repo"));
    let config = json!({
        "autoStartOnOpen": false, "startupChecklist": false, "notifications": false,
        "projects": { "sample": { "apps": { "docs": { "includeInAll": false } } } }
    });
    let lazyrunner = Lazyrunner::start(&temp, &repo, &sample_profile(), config, &[]);

    // Five columns.
    let frame = lazyrunner.frame();
    let titles = frame
        .lines()
        .find(|line| ["Orders", "Billing", "Shop", "Docs", "Console"].iter().all(|name| line.contains(name)));
    assert!(titles.is_some(), "one title row with Orders, Billing, Shop, Docs, Console:\n{frame}");

    // U starts all five.
    lazyrunner.press("U");
    lazyrunner.wait_for("Ready in", Duration::from_secs(300), || lazyrunner.log().contains("Ready in"));
    let log = lazyrunner.log();
    for expected in [
        format!("● Orders is listening on :{ORDERS_PORT}"),
        format!("● Billing is listening on :{BILLING_PORT}"),
        format!("● Shop is listening on :{SHOP_PORT}"),
        format!("● Docs is built (served by Orders at http://localhost:{ORDERS_PORT})"),
        format!("● Console is listening on :{CONSOLE_PORT}"),
    ] {
        assert!(log.contains(&expected), "'{expected}' in the log:\n{log}");
    }
    let console = std::fs::read_to_string(lazyrunner.state.join("sample").join("logs").join("console.log")).unwrap();
    assert!(console.contains("npm run dev --verbose (live)"), "the variant's label in its column:\n{console}");

    // b on the running Orders: stopped first, a full build, and it stays stopped; u then starts it without one.
    lazyrunner.press("1 b");
    lazyrunner.wait_for("the build", Duration::from_secs(300), || lazyrunner.log().contains("Build done"));
    assert_eq!(lazyrunner.log_line("Stopping Orders"), "Stopping Orders first - a running app locks its build output");
    assert!(
        lazyrunner.log_line("✓ Orders built in").contains("projects"),
        "a real build:
{}",
        lazyrunner.log()
    );
    assert_eq!(lazyrunner.log_line("Build done"), "Build done - u starts it without building again");
    let status = || lazyrunner.ask(json!({ "state": true }))["state"]["apps"][0]["status"].clone();
    assert_eq!(status(), "stopped", "b leaves it stopped");
    wait_free(&[ORDERS_PORT], "b stops the running Orders");
    lazyrunner.press("u");
    lazyrunner.wait_for("Orders again", Duration::from_secs(120), || {
        lazyrunner.log().matches(&format!("● Orders is listening on :{ORDERS_PORT}")).count() == 2
    });
    assert!(lazyrunner.log_line("Orders unchanged").ends_with("skipped (b / B force a full build)"));

    // 5 focuses the fifth column.
    lazyrunner.press("5");
    assert!(lazyrunner.frame().contains("▸ ■ Console"), "the Console column has the focus marker");

    // S stops all but the app with includeInAll false.
    lazyrunner.press("S");
    lazyrunner.wait_for("kept running", Duration::from_secs(30), || lazyrunner.log().contains("kept running"));
    assert_eq!(lazyrunner.log_line("Stopped "), "Stopped Orders + Billing + Shop + Console.");
    assert!(lazyrunner.log_line("kept running").starts_with("Docs kept running"));
    wait_free(&[ORDERS_PORT, BILLING_PORT, SHOP_PORT, CONSOLE_PORT], "S stops the whole trees");

    lazyrunner.quit();
}

/// A small project of two dev servers, on ports of their own (below the ephemeral ranges) so it runs beside the
/// sample test.
fn two_servers(temp: &TempDir, ports: [u16; 2]) -> (PathBuf, PathBuf) {
    let repo = temp.join("repo");
    write(repo.join("Two.sln"), "");
    std::fs::create_dir_all(repo.join("services")).unwrap();
    for (dir, port) in ["one", "two"].iter().zip(ports) {
        npm_app(&repo.join(dir), &format!(r#"{{ "dev": "node serve.js {port}" }}"#));
    }
    let apps: Vec<Value> = ["one", "two"]
        .iter()
        .zip(ports)
        .map(|(dir, port)| json!({ "key": dir, "name": dir, "type": "npm", "dir": dir, "variants": [{ "script": "dev", "port": port }] }))
        .collect();
    let profile = temp.join("two.json");
    let data = json!({ "schemaVersion": 1, "id": "two", "name": "Two", "detect": { "anyFile": ["Two.sln"] }, "watch": ["services"], "apps": apps });
    write(&profile, &data.to_string());
    (repo, profile)
}

#[test]
fn a_click_on_a_column_title_focuses_it() {
    let temp = TempDir::new("click");
    let (repo, profile) = two_servers(&temp, [31395, 31396]);
    let lazyrunner = Lazyrunner::start(&temp, &repo, &profile, json!({ "startupChecklist": false }), &[]);
    let focus = || lazyrunner.ask(json!({ "state": true }))["state"]["focus"].as_u64();
    lazyrunner.wait_for("the first frame", Duration::from_secs(30), || lazyrunner.frame().contains("two"));
    assert_eq!(focus(), Some(0));
    // The second column starts after the first column's border on the title row.
    let title = lazyrunner.frame().lines().next().unwrap_or_default().to_string();
    let border = title.chars().position(|c| matches!(c, '┳' | '┬')).expect("two columns");
    let answer = lazyrunner.ask(json!({ "click": [border + 3, 0] }));
    assert_eq!(answer["target"], "Some(Title(1))", "{answer}");
    assert_eq!(focus(), Some(1));
    lazyrunner.quit();
}

#[test]
fn l_hands_the_terminal_to_the_git_tool_and_comes_back() {
    if !cfg!(windows) {
        return;
    }
    let temp = TempDir::new("git-tool");
    let (repo, profile) = two_servers(&temp, [31387, 31388]);
    // A stand-in for lazygit that takes a couple of seconds.
    let config = json!({ "startupChecklist": false, "gitTool": "cmd /c ping -n 3 127.0.0.1" });
    let lazyrunner = Lazyrunner::start(&temp, &repo, &profile, config, &[]);
    let away = || lazyrunner.ask(json!({ "state": true }))["state"]["away"].clone();
    lazyrunner.wait_for("the first frame", Duration::from_secs(30), || lazyrunner.frame().contains("two"));
    lazyrunner.press("L");
    assert_eq!(away(), "cmd", "{}", lazyrunner.log());
    assert_eq!(lazyrunner.ask(json!({ "action": "quit" }))["ok"], true);
    thread::sleep(Duration::from_millis(500));
    assert_eq!(away(), "cmd", "a quit waits until the git tool is done");
    lazyrunner.wait_for("the git tool to quit", Duration::from_secs(30), || lazyrunner.log().contains("Back from cmd"));
    lazyrunner.quit();
}

#[test]
fn nothing_survives_when_lazyrunner_is_killed() {
    if skip_without(&["node", "npm"]) {
        return;
    }
    let temp = TempDir::new("killed");
    let ports = [31391, 31392];
    let (repo, profile) = two_servers(&temp, ports);
    let mut lazyrunner = Lazyrunner::start(&temp, &repo, &profile, json!({}), &["--start"]);
    lazyrunner.wait_for("both servers", Duration::from_secs(60), || {
        let listening = Listening::now();
        ports.iter().all(|port| listening.contains(*port))
    });
    lazyrunner.child.kill().unwrap(); // no goodbye: TerminateProcess / SIGKILL
    let _ = lazyrunner.child.wait();
    wait_free(&ports, "the servers died with lazyrunner");
}

#[test]
fn quitting_stops_everything() {
    if skip_without(&["node", "npm"]) {
        return;
    }
    let temp = TempDir::new("quit");
    let ports = [31393, 31394];
    let (repo, profile) = two_servers(&temp, ports);
    let lazyrunner = Lazyrunner::start(&temp, &repo, &profile, json!({}), &["--start"]);
    lazyrunner.wait_for("Ready in", Duration::from_secs(60), || lazyrunner.log().contains("Ready in"));
    lazyrunner.quit();
    wait_free(&ports, "q stops the whole trees");
}

#[test]
fn restarting_reuses_the_ports_without_a_false_conflict() {
    if skip_without(&["node", "npm"]) {
        return;
    }
    let temp = TempDir::new("restart");
    let ports = [31395, 31396];
    let (repo, profile) = two_servers(&temp, ports);
    let lazyrunner = Lazyrunner::start(&temp, &repo, &profile, json!({}), &["--start"]);
    lazyrunner.wait_for("Ready in", Duration::from_secs(60), || lazyrunner.log().contains("Ready in"));
    lazyrunner.press("R");
    lazyrunner.wait_for("ready again", Duration::from_secs(60), || lazyrunner.log().matches("Ready in").count() >= 2);
    let log = lazyrunner.log();
    assert!(!log.contains("already used by"), "the old servers' ports are not a conflict:\n{log}");
    assert!(log.contains("Restarting (requested from the panel)"), "as PowerShell says it when nothing is kept: {log}");
    lazyrunner.quit();
    wait_free(&ports, "q stops the whole trees");
}

/// The handover (Windows): reloading the profile (, then p) hands the running apps over to the next lazyrunner
/// instead of stopping them - the same processes keep serving on the same ports - and quitting that one stops them.
#[test]
fn reloading_hands_the_running_apps_over_without_restarting_them() {
    if !cfg!(windows) || skip_without(&["node", "npm"]) {
        return;
    }
    let temp = TempDir::new("handover");
    let ports = [31397, 31398];
    let (repo, profile) = two_servers(&temp, ports);
    let lazyrunner = Lazyrunner::start(&temp, &repo, &profile, json!({ "startupChecklist": false }), &["--start"]);
    lazyrunner.wait_for("Ready in", Duration::from_secs(60), || lazyrunner.log().contains("Ready in"));
    let pids = || -> Vec<Option<u64>> {
        let state = lazyrunner.ask(json!({ "state": true }))["state"].clone();
        state["apps"].as_array().map_or(Vec::new(), |apps| apps.iter().map(|app| app["pid"].as_u64()).collect())
    };
    let before = pids();
    assert!(before.len() == 2 && before.iter().all(Option::is_some), "both servers have a process: {before:?}");
    // Both starts are in the durations history (D), as the log said them.
    let history = std::fs::read_to_string(lazyrunner.state.join("two").join("history.json")).unwrap_or_default();
    let history: Value = serde_json::from_str(&history).unwrap_or_default();
    let starts: Vec<&str> = history
        .as_array()
        .into_iter()
        .flatten()
        .filter(|item| item["kind"] == "start")
        .filter_map(|item| item["key"].as_str())
        .collect();
    assert_eq!(starts.len(), 2, "both starts recorded: {history}");
    assert!(lazyrunner.log_line("is listening on").contains(" - up in "), "{}", lazyrunner.log());

    let handovers = || lazyrunner.log().lines().filter(|line| line.contains("Took over")).count();
    // Reload the project profile twice: lazyrunner reopens and hands both servers over each time (the second
    // handover passes on apps that were themselves taken over).
    for handover in 1..=2 {
        lazyrunner.press(", p");
        lazyrunner.wait_for("the next lazyrunner to take over", Duration::from_secs(60), || handovers() == handover);
        assert_eq!(pids(), before, "the same processes, not restarted (handover {handover})");
        let listening = Listening::now();
        assert!(ports.iter().all(|port| listening.contains(*port)), "both still serving (handover {handover})");
        let took_over = lazyrunner.log_line("Took over");
        assert!(took_over.ends_with("from the previous lazyrunner - they kept running"), "{took_over}");
    }

    lazyrunner.quit();
    wait_free(&ports, "quitting the next lazyrunner stops the apps it took over");
}
