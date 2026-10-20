//! The spike end to end: the real binary, real emitter processes, driven over the control channel like a person at
//! the keyboard would, drawn headless so it runs anywhere (CI included).

use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

const BIN: &str = env!("CARGO_BIN_EXE_lazyrunner");

struct Spike {
    child: Child,
    name: String,
}

impl Spike {
    fn start(name: &str, extra: &[&str]) -> Self {
        let name = format!("{name}-{}", std::process::id());
        let mut args = vec!["spike", "--headless", "140x30", "--no-splash", "--apps", "3", "--rate", "300"];
        args.extend_from_slice(&["--ready-after-ms", "150", "--ipc", &name]);
        args.extend_from_slice(extra);
        let child = Command::new(BIN).args(&args).stdout(Stdio::null()).spawn().expect("lazyrunner starts");
        Self { child, name }
    }

    fn ask(&self, request: Value) -> Value {
        let output = Command::new(BIN).args(["ctl", &self.name, &request.to_string()]).output().unwrap();
        if !output.status.success() {
            return json!({ "ok": false, "error": String::from_utf8_lossy(&output.stderr) });
        }
        serde_json::from_slice(&output.stdout).unwrap()
    }

    fn state(&self) -> Value {
        self.ask(json!({ "state": true }))["state"].clone()
    }

    fn frame(&self) -> String {
        self.ask(json!({ "dump": true }))["frame"].as_str().unwrap_or_default().to_string()
    }

    fn press(&self, keys: &str) {
        assert_eq!(self.ask(json!({ "keys": keys }))["ok"], true, "keys '{keys}'");
    }

    fn wait_for(&self, what: &str, condition: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let state = self.state();
            if condition(&state) {
                return state;
            }
            assert!(Instant::now() < deadline, "timed out waiting for {what}; last state: {state}");
            thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for Spike {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

fn all_running(state: &Value) -> bool {
    state["columns"].as_array().is_some_and(|columns| columns.iter().all(|column| column["status"] == "running"))
}

#[test]
fn apps_start_stream_and_follow_the_keys() {
    let spike = Spike::start("keys", &[]);
    let state = spike.wait_for("the apps to be up and printing", |state| {
        all_running(state) && state["columns"][2]["lines"].as_u64().unwrap_or(0) > 100
    });
    assert_eq!(state["focus"], 0);

    let frame = spike.frame();
    assert!(frame.contains("▸ ◆ Orders"), "{frame}");
    assert!(frame.contains("● Billing") && frame.contains("▲ Users"), "{frame}");
    assert!(frame.contains("● running") && frame.contains(" INF "), "{frame}");

    spike.press("l");
    assert_eq!(spike.state()["focus"], 1);
    spike.press("z");
    let frame = spike.frame();
    assert!(frame.contains("▸ ● Billing") && !frame.contains("◆ Orders"), "zoomed to Billing:\n{frame}");
    spike.press("Esc 3");
    let state = spike.state();
    assert_eq!((state["focus"].as_u64(), state["zoom"].as_bool()), (Some(2), Some(false)));

    spike.press("k k k k k");
    let scroll = spike.state()["columns"][2]["scroll"].as_u64().unwrap();
    assert!(scroll >= 5, "scrolled back, and the view stays put as lines arrive ({scroll})");
    assert!(spike.frame().contains("G follows"));
    spike.press("G");
    assert_eq!(spike.state()["columns"][2]["scroll"], 0);

    assert_eq!(spike.ask(json!({ "keys": "Hyper" }))["ok"], false);
    spike.press("q");
    let mut child = spike;
    let status = wait_exit(&mut child.child);
    assert!(status.success(), "q quits cleanly: {status}");
}

#[test]
fn bench_mode_reports_its_numbers() {
    let output = Command::new(BIN)
        .args(["spike", "--headless", "120x30", "--no-splash", "--apps", "2", "--rate", "500"])
        .args(["--ready-after-ms", "100", "--bench", "2"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(report["frames"].as_u64().unwrap() > 20, "{report}");
    assert!(report["lines"].as_u64().unwrap() > 500, "{report}");
    assert!(report["first_frame_ms"].as_f64().unwrap() > 0.0, "{report}");
    assert!(report["frame_interval_ms"]["p50"].as_f64().is_some(), "{report}");
}

fn wait_exit(child: &mut Child) -> std::process::ExitStatus {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        assert!(Instant::now() < deadline, "lazyrunner didn't exit");
        thread::sleep(Duration::from_millis(50));
    }
}
