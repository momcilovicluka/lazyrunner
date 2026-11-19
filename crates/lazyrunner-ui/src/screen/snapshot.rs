//! `, A v` (#106): a snapshot of this run to hand a teammate or an agent - one self-contained HTML file in the project's
//! state folder, its path copied: each app's state, port, errors and last error, the last 5 minutes' requests and
//! slowest endpoints, the tests' failures and their likely causes, Health's alerts, the stack map, what's on screen and
//! the screen itself as a picture - masked as the diagnostics zip masks. Read-only: nothing in it changes anything and
//! it never updates itself; the JSON it holds is what `get_snapshot` answers. The last few are kept.
//!
//! It moves (not with reduced motion): the screen dims for a moment as it's taken, as a camera's flash; the panel says
//! so for a few seconds.

use std::path::{Path, PathBuf};
use std::time::Duration;

use lazyrunner_core::changelog::VERSION;
use lazyrunner_core::links;
use lazyrunner_core::output::Hue;
use lazyrunner_core::run::requests::endpoints;
use lazyrunner_core::run::secrets::mask_text;
use lazyrunner_core::run::suites::Outcome;
use lazyrunner_core::run::{Controller, tools};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use serde_json::{Value, json};

use super::Ui;
use super::copy::{Pick, gather_app};
use super::icons::{self, Icon};
use super::seg::{Seg, seg};
use super::theme::C;
use crate::svg;

/// Where the snapshots are kept, how they're named, and how many are kept.
const FOLDER: &str = "snapshots";
const PREFIX: &str = "snapshot-";
const EXTENSION: &str = ".html";
const KEPT: usize = 5;
/// The requests looked at, the slowest endpoints listed, the screen's size when the terminal's isn't known.
const WINDOW: Duration = Duration::from_secs(5 * 60);
const SLOWEST: usize = 5;
const SCREEN: (u16, u16) = (160, 45);
/// The shutter's dimming, and how long the panel says a snapshot was taken.
const SHUTTER: Duration = Duration::from_millis(150);
const SAID: Duration = Duration::from_secs(4);
const LEFT_OUT: &str = "Masked as the diagnostics zip masks: passwords in connection strings, a URL's user:password and \
                        bearer tokens; no environment values, no logs but each app's last error.";

/// This run as the snapshot holds it, masked.
pub fn data(ui: &Ui, controller: &Controller) -> Value {
    let since = jiff::ZonedDateTime::now() - WINDOW;
    let apps: Vec<Value> = controller
        .apps
        .iter()
        .enumerate()
        .map(|(at, app)| {
            let (error, _) = gather_app(controller, at, Pick::LastError);
            let mine: Vec<_> =
                controller.requests.items.iter().filter(|request| request.app == app.key && request.at >= since).collect();
            let failed = |from: u16, to: u16| mine.iter().filter(|request| (from..to).contains(&request.status)).count();
            let mut slow = endpoints(mine.iter().copied());
            slow.sort_by(|a, b| b.p95.total_cmp(&a.p95));
            let slowest: Vec<Value> = slow
                .iter()
                .take(SLOWEST)
                .map(|found| json!({ "method": found.method, "endpoint": found.endpoint, "count": found.count, "p95Ms": found.p95.round() }))
                .collect();
            let requests = json!({ "count": mine.len(), "4xx": failed(400, 500), "5xx": failed(500, 600), "slowest": slowest });
            json!({ "key": app.key, "name": app.name, "lastError": error, "requests": requests })
        })
        .collect();
    let commit = tools::git(&controller.root, &["rev-parse", "--short", "HEAD"]);
    let taken = json!({
        "takenAt": lazyrunner_core::time::now_rfc3339(),
        "version": VERSION,
        "project": controller.project.name,
        "branch": controller.branch,
        "commit": commit,
        "status": crate::agent::status(ui, controller),
        "apps": apps,
        "tests": tests(controller),
        "health": crate::agent::views::health(controller),
        "stack": crate::agent::stack(controller),
        "leftOut": LEFT_OUT,
    });
    let text = mask_text(&taken.to_string());
    serde_json::from_str(&text).unwrap_or(taken)
}

/// The last test runs' totals, and each failure with its likely cause.
fn tests(controller: &Controller) -> Value {
    let suites = controller.suites.as_deref().unwrap_or_default();
    let failures: Vec<Value> = suites
        .iter()
        .flat_map(|suite| {
            suite.tests.iter().filter(|test| test.outcome == Outcome::Failed).map(move |test| {
                let cause = super::suite_cause::text(controller, &suite.key, &test.name);
                let message = test.message.lines().next().unwrap_or_default();
                json!({ "suite": suite.name, "name": test.name, "message": message, "likelyCause": cause })
            })
        })
        .collect();
    let totals = controller.test_totals();
    json!({ "passed": totals.passed, "failed": totals.failed, "skipped": totals.skipped, "failures": failures })
}

/// `, A v`: the snapshot written, its path copied; the oldest beyond the last few go.
pub fn take(ui: &mut Ui, controller: &mut Controller) {
    let taken = data(ui, controller);
    let size = crossterm::terminal::size().unwrap_or(SCREEN);
    let picture = mask_text(&super::bundle::screen(ui, controller, size));
    let html = page(ui, controller, &taken, &picture);
    let folder = controller.state_folder().join(FOLDER);
    let name = format!("{PREFIX}{}{EXTENSION}", jiff::ZonedDateTime::now().strftime(lazyrunner_core::time::STAMP));
    let file = folder.join(name);
    match std::fs::create_dir_all(&folder).and_then(|()| std::fs::write(&file, html)) {
        Ok(()) => {
            prune(&folder);
            let shown = file.display().to_string();
            controller.add_log(format!("◉ A snapshot of this run, read-only, to hand on: {shown}"), Hue::Cyan);
            super::copy::put(ui.clipboard, controller, &shown, "the snapshot's path");
            ui.snapshot = Some(ui.now);
        }
        Err(error) => controller.add_log(format!("✗ The snapshot wasn't written: {error}"), Hue::Red),
    }
}

/// Keeps the last [`KEPT`] snapshots in `folder`.
fn prune(folder: &Path) {
    let Ok(entries) = std::fs::read_dir(folder) else { return };
    let mut kept: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.file_name().is_some_and(|name| name.to_string_lossy().starts_with(PREFIX)))
        .collect();
    kept.sort();
    for old in kept.iter().rev().skip(KEPT) {
        let _ = std::fs::remove_file(old);
    }
}

/// The panel's chip while a snapshot was just taken.
pub fn chip(ui: &Ui) -> Option<Seg> {
    let at = ui.snapshot?;
    (ui.now.saturating_duration_since(at) < SAID)
        .then(|| seg(format!("   {} snapshot · path copied", icons::icon(ui.nerd, Icon::Camera)), C::Cyan))
}

/// Whether the shutter is dimming the screen.
pub fn animating(ui: &Ui) -> bool {
    !ui.reduced_motion && ui.snapshot.is_some_and(|at| ui.now.saturating_duration_since(at) < SHUTTER)
}

/// The shutter: the screen dimmed for a moment as the snapshot is taken.
pub fn paint(buf: &mut Buffer, area: Rect, ui: &Ui) {
    if !animating(ui) {
        return;
    }
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.modifier.insert(Modifier::DIM);
            }
        }
    }
}

/// Text as HTML holds it.
fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// A value of the snapshot as a line of text (`-` when it has none).
fn said(value: &Value) -> String {
    match value {
        Value::Null => "-".into(),
        Value::String(text) => escape(text),
        other => escape(&other.to_string()),
    }
}

/// The page: the snapshot drawn in the theme's colours, its JSON in it, the screen's picture at the bottom.
fn page(ui: &Ui, controller: &Controller, taken: &Value, picture: &str) -> String {
    let (fg, bg) = ui.theme.terminal.clone().unwrap_or((None, None));
    let (fg, bg) = (fg.unwrap_or_else(|| svg::DEFAULT_FG.into()), bg.unwrap_or_else(|| svg::DEFAULT_BG.into()));
    let mut body = String::new();
    body.push_str(&format!(
        "<h1>◉ {} · {} @ {}</h1><p class=dim>Taken {} · lazyrunner {} · a picture of the run, not a remote</p>",
        said(&taken["project"]),
        said(&taken["branch"]),
        said(&taken["commit"]),
        said(&taken["takenAt"]),
        said(&taken["version"]),
    ));
    body.push_str(&apps(controller, taken));
    body.push_str(&tests_section(&taken["tests"]));
    body.push_str(&alerts(&taken["status"]["alerts"]));
    body.push_str(&stack(&taken["stack"]["edges"]));
    let on_screen = &taken["status"]["ui"];
    body.push_str(&format!(
        "<h2>On screen</h2><p>{} · focused on {}</p><div class=screen>{picture}</div>",
        said(&on_screen["view"]),
        said(&on_screen["focus"]),
    ));
    body.push_str(&format!("<p class=dim>{}</p>", escape(LEFT_OUT)));
    let json = taken.to_string().replace("</", "<\\/");
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>lazyrunner snapshot - {}</title><style>\
         body{{background:{bg};color:{fg};font:14px/1.45 ui-monospace,Consolas,monospace;margin:24px;max-width:1400px}}\
         h1{{font-size:18px}}h2{{font-size:15px;margin-top:24px}}.dim{{opacity:.6}}.bad{{color:#f7768e}}.ok{{color:#9ece6a}}\
         .card{{border:1px solid #8884;border-left:4px solid var(--edge);border-radius:6px;padding:8px 14px;margin:8px 0}}\
         pre{{white-space:pre-wrap;margin:6px 0}}table{{border-collapse:collapse}}td,th{{padding:1px 14px 1px 0;text-align:left}}\
         .screen svg{{max-width:100%;height:auto}}</style></head><body>{body}\
         <script type=\"application/json\" id=\"snapshot\">{json}</script></body></html>",
        said(&taken["project"]),
    )
}

/// An app's card: its state's colour on its edge, its port, errors, requests and last error (its own frames bright).
fn apps(controller: &Controller, taken: &Value) -> String {
    let states = taken["status"]["apps"].as_array().cloned().unwrap_or_default();
    let details = taken["apps"].as_array().cloned().unwrap_or_default();
    let mut out = String::from("<h2>Apps</h2>");
    for (at, app) in states.iter().enumerate() {
        let detail = details.iter().find(|detail| detail["key"] == app["key"]).cloned().unwrap_or_default();
        let edge = match app["status"].as_str() {
            Some("running") => "#9ece6a",
            Some("building" | "starting" | "restarting") => "#e0af68",
            Some("crashed" | "failed") => "#f7768e",
            _ => "#8888",
        };
        let requests = &detail["requests"];
        out.push_str(&format!(
            "<div class=card style=\"--edge:{edge}\"><b>{}</b> · {} · {} · <span class=bad>{} errors</span> · {} warnings\
             <br>requests, 5 min: {} · 4xx {} · 5xx {}",
            said(&app["name"]),
            said(&app["status"]),
            said(&app["url"]),
            said(&app["errors"]),
            said(&app["warnings"]),
            said(&requests["count"]),
            said(&requests["4xx"]),
            said(&requests["5xx"]),
        ));
        let slowest = requests["slowest"].as_array().cloned().unwrap_or_default();
        if !slowest.is_empty() {
            out.push_str("<table><tr><th>slowest</th><th>p95 ms</th><th>calls</th></tr>");
            for found in &slowest {
                out.push_str(&format!(
                    "<tr><td>{} {}</td><td>{}</td><td>{}</td></tr>",
                    said(&found["method"]),
                    said(&found["endpoint"]),
                    said(&found["p95Ms"]),
                    said(&found["count"]),
                ));
            }
            out.push_str("</table>");
        }
        let lines: Vec<String> = detail["lastError"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect();
        if !lines.is_empty() {
            let frames = controller.states.get(at).and_then(|state| state.frames());
            out.push_str("<pre>");
            for line in &lines {
                let class = if links::library_frame(line, frames) { " class=dim" } else { "" };
                out.push_str(&format!("<span{class}>{}</span>\n", escape(line)));
            }
            out.push_str("</pre>");
        }
        out.push_str("</div>");
    }
    out
}

fn tests_section(tests: &Value) -> String {
    let mut out = format!(
        "<h2>Tests</h2><p><span class=ok>{} passed</span> · <span class=bad>{} failed</span> · {} skipped</p>",
        said(&tests["passed"]),
        said(&tests["failed"]),
        said(&tests["skipped"]),
    );
    for failure in tests["failures"].as_array().into_iter().flatten() {
        out.push_str(&format!(
            "<div class=card style=\"--edge:#f7768e\"><b>{}</b> <span class=dim>{}</span><br>{}<br><span class=dim>{}</span></div>",
            said(&failure["name"]),
            said(&failure["suite"]),
            said(&failure["message"]),
            said(&failure["likelyCause"]),
        ));
    }
    out
}

fn alerts(alerts: &Value) -> String {
    let list = alerts.as_array().cloned().unwrap_or_default();
    if list.is_empty() {
        return "<h2>Health</h2><p class=ok>no alerts</p>".into();
    }
    let mut out =
        String::from("<h2>Health</h2><table><tr><th>app</th><th>what</th><th>now</th><th>target</th><th>for</th></tr>");
    for alert in &list {
        out.push_str(&format!(
            "<tr><td>{}</td><td>{}</td><td class=bad>{}</td><td>{}</td><td>{} s</td></tr>",
            said(&alert["app"]),
            said(&alert["metric"]),
            said(&alert["value"]),
            said(&alert["target"]),
            said(&alert["seconds"]),
        ));
    }
    out.push_str("</table>");
    out
}

fn stack(edges: &Value) -> String {
    let list = edges.as_array().cloned().unwrap_or_default();
    if list.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        "<h2>The stack</h2><table><tr><th>calls</th><th>last minute</th><th>p95 ms</th><th>failed</th></tr>",
    );
    for edge in &list {
        out.push_str(&format!(
            "<tr><td>{} → {}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
            said(&edge["from"]),
            said(&edge["to"]),
            said(&edge["callsLastMinute"]),
            said(&edge["p95Ms"]),
            said(&edge["failedLastMinute"]),
        ));
    }
    out.push_str("</table>");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_last_few_snapshots_are_kept() {
        let temp = lazyrunner_core::testing::TempDir::new("snapshot-prune");
        for at in 0..8 {
            lazyrunner_core::testing::write(temp.join(format!("{PREFIX}2026111{at}-120000{EXTENSION}")), "");
        }
        lazyrunner_core::testing::write(temp.join("notes.txt"), "");
        prune(temp.path());
        let mut left: Vec<String> = std::fs::read_dir(temp.path())
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left.len(), KEPT + 1, "{left:?}");
        assert_eq!(left.first().map(String::as_str), Some("notes.txt"), "what isn't a snapshot stays");
        assert!(left.iter().any(|name| name.contains("20261117")), "the newest stay: {left:?}");
        assert_eq!(escape("<a href=\"x\">&</a>"), "&lt;a href=&quot;x&quot;&gt;&amp;&lt;/a&gt;");
    }
}
