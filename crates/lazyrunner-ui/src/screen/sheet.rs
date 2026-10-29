//! The start sheet (`x` then `e`, or the palette): what the focused app is started with - the command line and its
//! folder, the launch profile (or the npm script), the profile's `args` and `env`, and the user secrets with their
//! values masked (`Space` shows the selected one a few seconds; a connection string reads "database @ server", as
//! the database picker shows it). A part that changed since the app started says so, and `r` restarts it; `y` copies
//! the sheet, the secrets masked. A running process's environment can't be read on Windows, so this is what lazyrunner
//! passes it, not what it holds.

use std::path::Path;
use std::time::{Duration, SystemTime};

use lazyrunner_core::apptypes;
use lazyrunner_core::json;
use lazyrunner_core::profile::AppKind;
use lazyrunner_core::run::{Controller, secrets};
use serde_json::Value;

use super::actions::Run;
use super::icons::{self, Icon};
use super::overlays::boxed;
use super::seg::{Seg, on, pad, seg};
use super::theme::C;
use super::{Overlay, Ui, keys};

const INNER: usize = 100;
const KEY_CELLS: usize = 28;
/// How long `Space` shows a secret's value.
const SHOWN_FOR: Duration = Duration::from_secs(5);
const MASKED: &str = "••••";
/// The box's border, title and footer: the rows the sheet itself can't use.
const CHROME_ROWS: usize = 5;

/// A line of the sheet.
enum Row {
    Heading { title: String, changed: bool },
    Pair { key: String, value: String },
    Text(String),
    Secret { key: String, value: String, label: Option<String> },
}

/// x then e: the focused app's sheet.
pub fn open(ui: &mut Ui, controller: &Controller) {
    if ui.focus < controller.apps.len() {
        ui.overlay = Some(Overlay::Sheet { app: ui.focus, index: 0, shown: None });
    }
}

pub fn key(ui: &mut Ui, controller: &mut Controller, name: &str) {
    let now = ui.now;
    let Some(Overlay::Sheet { app, index, shown }) = &mut ui.overlay else { return };
    let app = *app;
    let secrets = build(controller, app).iter().filter(|row| matches!(row, Row::Secret { .. })).count().max(1);
    match name {
        "Escape" | "q" | "e" => ui.overlay = None,
        "j" | "DownArrow" => (*index, *shown) = (keys::wrapped(*index, 1, secrets), None),
        "k" | "UpArrow" => (*index, *shown) = (keys::wrapped(*index, -1, secrets), None),
        "Space" => *shown = Some(now),
        "y" => {
            let text = copied(controller, app);
            let name = controller.apps.get(app).map_or(String::new(), |app| app.name.clone());
            super::copy::put(ui.clipboard, controller, &text, &format!("{name}'s start sheet (secrets masked)"));
        }
        "r" => {
            ui.overlay = None;
            ui.focus = app;
            keys::run(ui, controller, &Run::RestartFocused);
        }
        _ => {}
    }
}

/// What the app is started with, part by part.
fn build(controller: &Controller, index: usize) -> Vec<Row> {
    let (Some(app), Some(state)) = (controller.apps.get(index), controller.states.get(index)) else {
        return Vec::new();
    };
    let started = state.started_wall.filter(|_| state.is_running());
    let changed = |path: &Path| started.is_some_and(|at| modified(path).is_some_and(|when| when > at));
    let mut rows = Vec::new();
    match controller.server_command(&app.key) {
        Ok(command) => {
            let differs = started.is_some() && state.label.as_deref() != Some(command.label.as_str());
            rows.push(heading("Command line", differs));
            let args: Vec<String> = command.spec.args.iter().map(|arg| quoted(&arg.to_string_lossy())).collect();
            rows.push(Row::Text(format!("{} {}", quoted(&command.spec.program.to_string_lossy()), args.join(" "))));
            rows.push(pair("in", &command.spec.dir.display().to_string()));
        }
        Err(error) => {
            rows.push(heading("Command line", false));
            rows.push(Row::Text(format!("it can't start: {error}")));
        }
    }
    match &app.kind {
        AppKind::Dotnet { args, .. } => {
            match apptypes::launch_profile(&controller.root, app) {
                Some((file, name, profile)) => {
                    rows.push(heading(&format!("Launch profile {name}"), changed(&file)));
                    for field in ["applicationUrl", "commandLineArgs"] {
                        if let Some(text) = profile[field].as_str() {
                            rows.push(pair(field, text));
                        }
                    }
                    for (name, value) in profile["environmentVariables"].as_object().into_iter().flatten() {
                        rows.push(pair(name, &plain(value)));
                    }
                }
                None => {
                    rows.push(heading("Launch profile", false));
                    rows.push(Row::Text("not in its Properties/launchSettings.json".into()));
                }
            }
            rows.push(heading("Profile", changed(&controller.project.path)));
            rows.push(pair("args", &if args.is_empty() { "none".to_string() } else { args.join(" ") }));
        }
        AppKind::Npm { .. } => {
            if let Some(mode) = apptypes::mode(&controller.root, app, &controller.apps) {
                rows.push(heading("npm script", false));
                rows.push(pair("variant", &mode.label));
                rows.push(pair("script", &mode.script));
                rows.push(pair("port", &mode.port.map_or("none (a watch build)".into(), |port| port.to_string())));
            }
            rows.push(heading("Profile", changed(&controller.project.path)));
        }
    }
    if app.env.is_empty() {
        rows.push(pair("env", "none"));
    }
    for (name, value) in &app.env {
        rows.push(pair(name, value));
    }
    if matches!(app.kind, AppKind::Dotnet { .. })
        && let Some(path) = secrets::app_secrets_path(&controller.root, app)
    {
        rows.push(heading("User secrets", changed(&path)));
        rows.push(Row::Text(path.display().to_string()));
        let found = json::read_text(&path).map(|text| secrets::flatten(&text)).unwrap_or_default();
        if found.is_empty() {
            rows.push(Row::Text("none".into()));
        }
        rows.extend(found.into_iter().map(|(key, value)| Row::Secret {
            label: secrets::connection_label(&value),
            key,
            value,
        }));
    }
    rows
}

fn heading(title: &str, changed: bool) -> Row {
    Row::Heading { title: title.into(), changed }
}

fn pair(key: &str, value: &str) -> Row {
    Row::Pair { key: key.into(), value: value.into() }
}

/// A JSON value as text: a string as it is.
fn plain(value: &Value) -> String {
    value.as_str().map_or_else(|| value.to_string(), str::to_string)
}

/// An argument as a shell shows it: in quotes when it has a space.
fn quoted(arg: &str) -> String {
    if arg.contains(' ') { format!("\"{arg}\"") } else { arg.to_string() }
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|meta| meta.modified()).ok()
}

/// y: the sheet as text, the secrets masked.
fn copied(controller: &Controller, app: usize) -> String {
    let lines: Vec<String> = build(controller, app)
        .into_iter()
        .map(|row| match row {
            Row::Heading { title, changed } => {
                format!("{title}{}", if changed { " (changed since it started)" } else { "" })
            }
            Row::Pair { key, value } => format!("  {key}: {value}"),
            Row::Text(text) => format!("  {text}"),
            Row::Secret { key, label, .. } => format!("  {key}: {}", label.unwrap_or_else(|| MASKED.into())),
        })
        .collect();
    lines.join(super::copy::NEWLINE)
}

pub fn rows(ui: &Ui, controller: &Controller, cells: usize, rows: usize) -> Vec<Vec<Seg>> {
    let Some(Overlay::Sheet { app, index, shown }) = &ui.overlay else { return Vec::new() };
    let showing = shown.is_some_and(|at| ui.now.saturating_duration_since(at) < SHOWN_FOR);
    let inner = cells.saturating_sub(8).clamp(20, INNER);
    let built = build(controller, *app);
    let secrets = built.iter().filter(|row| matches!(row, Row::Secret { .. })).count();
    let selected = (*index).min(secrets.saturating_sub(1));
    let mut secret = 0;
    let mut at_selected = 0;
    let mut lines: Vec<Vec<Seg>> = Vec::new();
    for row in built {
        if let Row::Text(text) = &row {
            let pieces = super::columns::chunk(text, inner.saturating_sub(4));
            lines.extend(pieces.into_iter().map(|piece| vec![seg(format!("   {piece}"), C::Gray)]));
            continue;
        }
        lines.push(match row {
            Row::Heading { title, changed } => {
                let mut line = vec![seg(format!(" {title}"), C::Cyan)];
                if changed {
                    line.push(seg("  changed since it started · r restarts it", C::Yellow));
                }
                line
            }
            Row::Pair { key, value } => {
                vec![seg(format!("   {}", pad(&key, KEY_CELLS)), C::DarkGray), seg(value, C::White)]
            }
            Row::Text(_) => Vec::new(),
            Row::Secret { key, value, label } => {
                let this = secret == selected;
                secret += 1;
                if this {
                    at_selected = lines.len();
                }
                let (shown_value, color) = match (&label, this && showing) {
                    (_, true) => (value, C::White),
                    (Some(label), false) => (format!("{label}  (d switches the database)"), C::Green),
                    (None, false) => (MASKED.to_string(), C::DarkGray),
                };
                if this {
                    vec![on(pad(&format!("   {} {shown_value}", pad(&key, KEY_CELLS)), inner), C::Black, C::Cyan)]
                } else {
                    vec![
                        seg(format!("   {}", pad(&key, KEY_CELLS)), C::DarkGray),
                        seg(format!(" {shown_value}"), color),
                    ]
                }
            }
        });
    }
    let room = rows.saturating_sub(CHROME_ROWS).max(1);
    if lines.len() > room {
        let first = at_selected.saturating_sub(room / 2).min(lines.len() - room);
        lines = lines.into_iter().skip(first).take(room).collect();
    }
    let name = controller.apps.get(*app).map_or("", |app| app.name.as_str());
    let footer = if secrets > 0 {
        "j k a secret · Space shows it · y copies (masked) · r restarts · Esc closes"
    } else {
        "y copies · r restarts · Esc closes"
    };
    boxed(&icons::titled(ui.nerd, Icon::Terminal, &format!("Start sheet · {name}")), lines, footer, cells, rows, inner)
}

/// Whether a secret's value is on screen: the box redraws to mask it again.
pub fn showing(ui: &Ui) -> bool {
    matches!(&ui.overlay, Some(Overlay::Sheet { shown: Some(at), .. }) if ui.now.saturating_duration_since(*at) < SHOWN_FOR)
}
