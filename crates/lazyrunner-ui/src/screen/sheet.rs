//! The start sheet (`x` then `e`, or the palette): what the focused app is started with - the command line and its
//! folder, the launch profile (or the npm script), its environment and the user secrets - and what it gets on this
//! machine on top (#80). Each variable says where it comes from (`launch`, `profile`, `secrets`, `override`), the one
//! that wins with what it hides under it: an override wins over a launch profile's variable, which wins over the
//! profile's `env` (`dotnet run` puts its launch profile's over what it's handed), and a variable over the user secret
//! it names (`Section__Key` over `Section:Key`).
//!
//! `a` adds an override, `Enter` overrides the picked variable (a user secret through its variable), `d` drops one,
//! `b` keeps one to this branch or gives it back to every branch, `s` moves one that looks like a secret into the
//! user secrets; `v` shows only the overrides and `V` copies them as what they change. Overrides live in lazyrunner's
//! state for this machine, never in a shared file, and apply from the app's next start: the sheet says when what it
//! runs with changed since it started, and `r` restarts it. Secret values are masked (`Space` shows the picked one a
//! few seconds, wiping in and out); a connection string reads "database @ server", as the database picker shows it.
//! `y` copies the sheet, secrets masked. A running process's environment can't be read on Windows, so this is what
//! lazyrunner passes it, not what it holds.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use crossterm::event::KeyEvent;
use lazyrunner_core::apptypes;
use lazyrunner_core::json;
use lazyrunner_core::output::Hue;
use lazyrunner_core::profile::{App, AppKind, Ready};
use lazyrunner_core::run::app::AppState;
use lazyrunner_core::run::{Controller, secrets};
use serde_json::Value;
use unicode_width::UnicodeWidthStr;

use super::actions::Run;
use super::icons::{self, Icon};
use super::motion::FLASH;
use super::overlays::boxed;
use super::seg::{Seg, on, pad, seg};
use super::theme::C;
use super::{Overlay, Ui, keys};

const INNER: usize = 100;
const KEY_CELLS: usize = 28;
/// How long `Space` shows a secret's value, and how long the value takes to wipe in, and out again.
const SHOWN_FOR: Duration = Duration::from_secs(5);
const WIPE: Duration = Duration::from_millis(80);
const MASKED: &str = "••••";
const BULLET: char = '•';
/// Where a variable comes from, at the end of its line.
const PILL_CELLS: usize = 10;
/// The box's border, title and footer: the rows the sheet itself can't use.
const CHROME_ROWS: usize = 5;

/// A .NET app's launch profile: its file, its name and what it says.
type Launch = Option<(PathBuf, String, Value)>;

/// Where a variable comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Source {
    Launch,
    Profile,
    Secrets,
    Override,
}

impl Source {
    fn name(self) -> &'static str {
        match self {
            Self::Launch => "launch",
            Self::Profile => "profile",
            Self::Secrets => "secrets",
            Self::Override => "override",
        }
    }

    fn color(self) -> C {
        match self {
            Self::Launch => C::Cyan,
            Self::Profile => C::DarkGray,
            Self::Secrets => C::Yellow,
            Self::Override => C::Green,
        }
    }
}

/// A variable or a user secret: its value, where it comes from, and what it wins over.
#[derive(Clone, Debug)]
struct Var {
    name: String,
    value: String,
    source: Source,
    /// The same variable from elsewhere, which this one wins over: the nearest first.
    hides: Vec<(Source, String)>,
    /// Masked until `Space` shows it: a user secret, or a value that looks like one.
    secret: bool,
    /// A connection string as the database picker shows it.
    label: Option<String>,
    /// An override kept to this branch.
    branch: bool,
    /// A user secret a variable wins over.
    overridden: bool,
}

/// A line of the sheet.
enum Row {
    Heading {
        title: String,
        changed: bool,
    },
    Pair {
        key: String,
        value: String,
    },
    /// What's suggested, not set: dimmed until it is.
    Suggested {
        key: String,
        value: String,
    },
    Text(String),
    Var(Var),
}

/// What the sheet's keys type: a new override (`NAME=value`), or a new value for `name`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Typing {
    pub name: Option<String>,
    pub text: String,
    /// Enter found no `NAME=value` in it.
    pub wrong: bool,
}

impl Typing {
    /// The override it says: a name without spaces, and its value.
    fn parsed(&self) -> Option<(String, String)> {
        let (name, value) = match &self.name {
            Some(name) => (name.clone(), self.text.clone()),
            None => {
                let (name, value) = self.text.split_once('=')?;
                (name.trim().to_string(), value.to_string())
            }
        };
        let usable = !name.is_empty() && !name.contains(char::is_whitespace) && !name.contains('=');
        usable.then_some((name, value))
    }
}

/// x then e: the focused app's sheet.
pub fn open(ui: &mut Ui, controller: &Controller) {
    if ui.focus < controller.apps.len() {
        let app = ui.focus;
        ui.overlay = Some(Overlay::Sheet { app, index: 0, shown: None, typing: None, only: false, saved: None });
    }
}

/// The sheet's keys (as the module says); while an override is typed, keys type it.
pub fn key(ui: &mut Ui, controller: &mut Controller, name: &str, key: &KeyEvent) {
    let now = ui.now;
    let Some(Overlay::Sheet { app, index, shown, typing, only, saved }) = &mut ui.overlay else { return };
    let app = *app;
    let Some(app_key) = controller.apps.get(app).map(|app| app.key.clone()) else { return };
    if let Some(input) = typing {
        match name {
            "Escape" => *typing = None,
            "Enter" => match input.parsed() {
                Some((name, value)) => {
                    save(controller, &app_key, &name, &value);
                    (*typing, *saved) = (None, Some((name, now)));
                }
                None => input.wrong = true,
            },
            "Backspace" => {
                input.text.pop();
                input.wrong = false;
            }
            _ => {
                if let Some(c) = keys::typed(key) {
                    input.text.push(c);
                    input.wrong = false;
                }
            }
        }
        return;
    }
    let vars = picks(controller, app, *only);
    let at = (*index).min(vars.len().saturating_sub(1));
    let picked = vars.get(at).cloned();
    let overriding = picked.clone().filter(|var| var.source == Source::Override);
    match name {
        "Escape" | "q" | "e" => ui.overlay = None,
        "j" | "DownArrow" => (*index, *shown) = (keys::wrapped(at, 1, vars.len().max(1)), None),
        "k" | "UpArrow" => (*index, *shown) = (keys::wrapped(at, -1, vars.len().max(1)), None),
        "Space" => *shown = Some(now),
        "a" => *typing = Some(Typing::default()),
        "Enter" => {
            if let Some(var) = picked {
                let name = if var.source == Source::Secrets { var.name.replace(':', "__") } else { var.name };
                let text = if var.secret { String::new() } else { var.value };
                *typing = Some(Typing { name: Some(name), text, wrong: false });
            }
        }
        "d" | "Delete" => match overriding {
            Some(var) => controller.remove_env_override(&app_key, &var.name),
            None => controller.add_log("Only an override can be dropped - Enter overrides the variable", Hue::DarkGray),
        },
        "b" => {
            if let Some(var) = overriding {
                controller.set_env_override(&app_key, &var.name, &var.value, !var.branch);
                *saved = Some((var.name, now));
            }
        }
        "s" => {
            if let Some(var) = overriding
                && let Err(error) = controller.override_to_secrets(&app_key, &var.name)
            {
                controller.add_log(format!("✗ Couldn't move {} into the user secrets: {error}", var.name), Hue::Red);
            }
        }
        "v" => (*only, *index, *shown) = (!*only, 0, None),
        "V" => {
            let text = changes(controller, app);
            let name = controller.apps.get(app).map_or(String::new(), |app| app.name.clone());
            super::copy::put(ui.clipboard, controller, &text, &format!("what {name} gets on this machine"));
        }
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

/// Keeps an override typed in the sheet - kept to this branch when the one it replaces was - and warns when its value
/// looks like a secret: overrides are kept in plain text.
fn save(controller: &mut Controller, key: &str, name: &str, value: &str) {
    let branch = controller.env_overrides(key).iter().any(|found| found.name == name && found.branch);
    controller.set_env_override(key, name, value, branch);
    if secrets::looks_secret(name, value) {
        let dotnet = matches!(controller.app(key).map(|app| &app.kind), Some(AppKind::Dotnet { .. }));
        let fix = if dotnet { " - s moves it into the app's user secrets" } else { "" };
        controller
            .add_log(format!("⚠ {name} looks like a secret, and overrides are kept in plain text{fix}"), Hue::Yellow);
    }
}

/// The variables `j` `k` move through, in the sheet's order.
fn picks(controller: &Controller, app: usize, only: bool) -> Vec<Var> {
    let rows = build(controller, app, only).into_iter();
    rows.filter_map(|row| match row {
        Row::Var(var) => Some(var),
        _ => None,
    })
    .collect()
}

/// What the app is started with, part by part; `only`: what this machine gives it on top.
fn build(controller: &Controller, index: usize, only: bool) -> Vec<Row> {
    let (Some(app), Some(state)) = (controller.apps.get(index), controller.states.get(index)) else {
        return Vec::new();
    };
    let started = state.started_wall.filter(|_| state.is_running());
    let changed = |path: &Path| started.is_some_and(|at| modified(path).is_some_and(|when| when > at));
    let launch = match &app.kind {
        AppKind::Dotnet { .. } => apptypes::launch_profile(&controller.root, app),
        AppKind::Npm { .. } | AppKind::Command(_) => None,
    };
    let launch_vars: Vec<(String, String)> = launch
        .iter()
        .flat_map(|(_, _, profile)| profile["environmentVariables"].as_object().into_iter().flatten())
        .map(|(name, value)| (name.clone(), plain(value)))
        .collect();
    let vars = environment(controller, app, &launch_vars);
    let now: Vec<(String, String)> =
        controller.env_overrides(&app.key).iter().map(|found| (found.name.clone(), found.value.clone())).collect();
    let env_changed = (started.is_some() && state.env_at_start != now)
        || changed(&controller.project.path)
        || launch.as_ref().is_some_and(|(file, ..)| changed(file));
    let mut rows = Vec::new();
    if only {
        rows.push(heading("On this machine", env_changed));
        let overrides: Vec<Var> = vars.into_iter().filter(|var| var.source == Source::Override).collect();
        if overrides.is_empty() {
            rows.push(Row::Text("nothing overridden - a adds an override, Enter on a variable overrides it".into()));
        }
        rows.extend(overrides.into_iter().map(Row::Var));
        return rows;
    }
    rows.extend(started_with(controller, app, state, &launch, &changed));
    rows.push(heading("Environment", env_changed));
    if vars.is_empty() {
        rows.push(Row::Text("none".into()));
    }
    let names: Vec<String> = vars.iter().map(|var| var.name.clone()).collect();
    rows.extend(vars.into_iter().map(Row::Var));
    rows.extend(user_secrets(controller, app, &names, &changed));
    rows
}

/// What it runs: the command line, the launch profile (or the npm script) and the profile's arguments.
fn started_with(
    controller: &Controller,
    app: &App,
    state: &AppState,
    launch: &Launch,
    changed: &dyn Fn(&Path) -> bool,
) -> Vec<Row> {
    let mut rows = Vec::new();
    match controller.server_command(&app.key) {
        Ok(command) => {
            let running = state.started_wall.is_some() && state.is_running();
            let differs = running && state.label.as_deref() != Some(command.label.as_str());
            rows.push(heading("Command line", differs));
            rows.push(Row::Text(command_line(&command.spec.program, &command.spec.args)));
            rows.push(pair("in", &command.spec.dir.display().to_string()));
        }
        Err(error) => {
            rows.push(heading("Command line", false));
            rows.push(Row::Text(format!("it can't start: {error}")));
        }
    }
    match &app.kind {
        AppKind::Dotnet { args, .. } => {
            match launch {
                Some((file, name, profile)) => {
                    rows.push(heading(&format!("Launch profile {name}"), changed(file)));
                    for field in ["applicationUrl", "commandLineArgs"] {
                        if let Some(text) = profile[field].as_str() {
                            rows.push(pair(field, text));
                        }
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
        }
        AppKind::Command(command) => {
            rows.push(heading("Profile", changed(&controller.project.path)));
            let none = |words: &[String]| if words.is_empty() { "none".to_string() } else { words.join(" ") };
            if let Some(preset) = &command.preset {
                rows.push(pair("preset", preset));
            } else if let Some(found) =
                lazyrunner_core::profile::preset::looks_like(&controller.root.join(&command.cwd))
                    .filter(|_| command.compose.is_none())
            {
                let id = lazyrunner_core::json::text(&found["id"]);
                let value = format!("{id} - suggested: \"preset\": \"{id}\" in the profile sets it");
                rows.push(Row::Suggested { key: "preset".into(), value });
            }
            rows.push(pair("run", &command.run.join(" ")));
            if let Some(compose) = &command.compose {
                let services =
                    if compose.services.is_empty() { "all".to_string() } else { compose.services.join(", ") };
                rows.push(pair("services", &services));
                rows.push(pair("ready", "once its services run, healthy when they have a healthcheck"));
                return rows;
            }
            rows.push(pair("build", &none(&command.build)));
            rows.push(pair("install", &none(&command.install)));
            rows.push(pair("port", &apptypes::command_port(command).map_or("none".into(), |port| port.to_string())));
            let ready = match &command.ready {
                Ready::Started => "once started".to_string(),
                Ready::Port(port) => format!("once :{port} listens"),
                Ready::Line(line) => format!("once a line matches {line}"),
                Ready::LineOrPort(line) => format!("once a line matches {line}, or its port listens"),
                Ready::Http(path) => format!("once {path} answers 2xx"),
                Ready::Services => "once its services are up".to_string(),
            };
            rows.push(pair("ready", &ready));
        }
    }
    rows
}

/// The app's environment as it gets it: an override over a launch profile's variable over the profile's (`env`) -
/// `dotnet run` puts its launch profile's over what it's handed - each with what it wins over.
fn environment(controller: &Controller, app: &App, launch: &[(String, String)]) -> Vec<Var> {
    let profile = app.env.iter().map(|(name, value)| (Source::Profile, name.clone(), value.clone(), false));
    let launch = launch.iter().map(|(name, value)| (Source::Launch, name.clone(), value.clone(), false));
    let overrides = controller
        .env_overrides(&app.key)
        .iter()
        .map(|found| (Source::Override, found.name.clone(), found.value.clone(), found.branch));
    let mut vars: Vec<Var> = Vec::new();
    for (source, name, value, branch) in profile.chain(launch).chain(overrides) {
        let secret = secrets::looks_secret(&name, &value);
        let var = Var { name, value, source, hides: Vec::new(), secret, label: None, branch, overridden: false };
        match vars.iter().position(|other| other.name.eq_ignore_ascii_case(&var.name)) {
            Some(at) => {
                let under = std::mem::replace(&mut vars[at], var);
                vars[at].secret |= under.secret;
                vars[at].hides = std::iter::once((under.source, under.value)).chain(under.hides).collect();
            }
            None => vars.push(var),
        }
    }
    vars
}

/// A .NET app's user secrets, masked; one a variable wins over says so.
fn user_secrets(controller: &Controller, app: &App, names: &[String], changed: &dyn Fn(&Path) -> bool) -> Vec<Row> {
    let mut rows = Vec::new();
    if !matches!(app.kind, AppKind::Dotnet { .. }) {
        return rows;
    }
    let Some(path) = secrets::app_secrets_path(&controller.root, app) else { return rows };
    rows.push(heading("User secrets", changed(&path)));
    rows.push(Row::Text(path.display().to_string()));
    let found = json::read_text(&path).map(|text| secrets::flatten(&text)).unwrap_or_default();
    if found.is_empty() {
        rows.push(Row::Text("none".into()));
    }
    rows.extend(found.into_iter().map(|(key, value)| {
        let overridden = names.iter().any(|name| name.replace("__", ":").eq_ignore_ascii_case(&key));
        Row::Var(Var {
            label: secrets::connection_label(&value),
            name: key,
            value,
            source: Source::Secrets,
            hides: Vec::new(),
            secret: true,
            branch: false,
            overridden,
        })
    }));
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

/// The command line as a shell shows it, an override's value left out: the environment lists it (masked when it's
/// a secret).
fn command_line(program: &Path, args: &[OsString]) -> String {
    let mut shown = vec![quoted(&program.to_string_lossy())];
    let mut after_flag = false;
    for arg in args {
        let arg = arg.to_string_lossy();
        shown.push(match arg.split_once('=') {
            Some((name, _)) if after_flag => format!("{name}=…"),
            _ => quoted(&arg),
        });
        after_flag = arg == "-e";
    }
    shown.join(" ")
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|meta| meta.modified()).ok()
}

/// A variable's value as text that can be copied or shared: a secret masked (a connection string as its label).
fn masked(var: &Var) -> String {
    match (&var.label, var.secret) {
        (_, false) => var.value.clone(),
        (Some(label), true) => label.clone(),
        (None, true) => MASKED.into(),
    }
}

/// y: the sheet as text, the secrets masked.
pub(crate) fn copied(controller: &Controller, app: usize) -> String {
    let lines: Vec<String> = build(controller, app, false)
        .into_iter()
        .flat_map(|row| match row {
            Row::Heading { title, changed } => {
                vec![format!("{title}{}", if changed { " (changed since it started)" } else { "" })]
            }
            Row::Pair { key, value } => vec![format!("  {key}: {value}")],
            Row::Suggested { key, value } => vec![format!("  {key}: {value}")],
            Row::Text(text) => vec![format!("  {text}")],
            Row::Var(var) => {
                let mut lines = vec![format!("  {}: {} [{}]", var.name, masked(&var), var.source.name())];
                let hidden = var.hides.iter().map(|(source, value)| {
                    format!("    hides {}: {}", source.name(), if var.secret { MASKED } else { value })
                });
                lines.extend(hidden);
                lines
            }
        })
        .collect();
    lines.join(super::copy::NEWLINE)
}

/// V: what the app gets on this machine against what it would get without, the secrets masked.
fn changes(controller: &Controller, app: usize) -> String {
    let name = controller.apps.get(app).map_or("", |app| app.name.as_str());
    let branch = if controller.branch.is_empty() { String::new() } else { format!(" ({})", controller.branch) };
    let mut lines = vec![format!("{name} on this machine{branch}:")];
    for var in picks(controller, app, true) {
        let shown = |value: &str| if var.secret { MASKED.to_string() } else { value.to_string() };
        let was = var
            .hides
            .first()
            .map_or("not set".into(), |(source, value)| format!("{} ({})", shown(value), source.name()));
        let scope = if var.branch { " (this branch)" } else { "" };
        lines.push(format!("  {}: {was} → {}{scope}", var.name, shown(&var.value)));
    }
    if lines.len() == 1 {
        lines.push("  nothing overridden".into());
    }
    lines.join(super::copy::NEWLINE)
}

/// Whether the sheet's motion runs: not with reduced motion, nor in the screen tests' frozen frames.
fn moving(ui: &Ui) -> bool {
    ui.motion.enabled && !ui.reduced_motion
}

/// How much of the picked secret's value `Space` shows: it wipes in, holds, and wipes out again; none once it's over.
fn revealed(ui: &Ui, shown: Option<Instant>) -> Option<f64> {
    let since = ui.now.saturating_duration_since(shown?);
    if since >= SHOWN_FOR {
        return None;
    }
    if !moving(ui) {
        return Some(1.0);
    }
    let edge = since.min(SHOWN_FOR - since);
    Some((edge.as_secs_f64() / WIPE.as_secs_f64()).min(1.0))
}

/// `value` with its first `done` (0 to 1) shown and the rest masked, a character a cell.
fn wiped(value: &str, done: f64) -> String {
    let count = value.chars().count();
    let shown = (done.clamp(0.0, 1.0) * count as f64).round() as usize;
    value.chars().take(shown).chain(std::iter::repeat_n(BULLET, count - shown)).collect()
}

/// What a variable's value shows, and in which colour: a secret masked (a connection string as "database @ server")
/// unless `Space` shows it; a user secret a variable wins over, and an override kept to this branch, say so.
fn shown_value(var: &Var, reveal: Option<f64>) -> (String, C) {
    let (mut text, mut color) = match (reveal, &var.label) {
        _ if !var.secret => (var.value.clone(), C::White),
        (Some(done), _) => (wiped(&var.value, done), C::White),
        (None, Some(label)) => (label.clone(), C::Green),
        (None, None) => (MASKED.to_string(), C::DarkGray),
    };
    if var.overridden {
        text.push_str("  · a variable overrides it");
        color = C::DarkGray;
    }
    if var.branch {
        text.push_str("  · this branch");
    }
    (text, color)
}

/// A variable's line - an override's amber edge, its icon, name, value and where it comes from - then what it wins
/// over, dimmed. The picked one is lit - its value scrolling sideways when it's longer than its room - and one just
/// saved flashes green.
fn var_lines(ui: &Ui, var: &Var, picked: bool, reveal: Option<f64>, flash: bool, inner: usize) -> Vec<Vec<Seg>> {
    let edge = if var.source == Source::Override { seg("▌", C::Yellow) } else { seg(" ", C::Gray) };
    let icon = match var.source {
        _ if var.secret => icons::icon(ui.nerd, Icon::Lock),
        Source::Override => icons::icon(ui.nerd, Icon::Pencil),
        _ => " ",
    };
    let (value, color) = shown_value(var, reveal);
    let room = inner.saturating_sub(3 + KEY_CELLS + 1 + PILL_CELLS);
    let mut line = vec![
        edge,
        seg(format!("{icon} "), C::DarkGray),
        seg(pad(&var.name, KEY_CELLS), C::DarkGray),
        seg(format!(" {}", if picked { ui.scrolling(&value, room) } else { pad(&value, room) }), color),
        seg(format!("{:>PILL_CELLS$}", var.source.name()), var.source.color()),
    ];
    if picked || flash {
        let back = if flash { C::Green } else { C::Cyan };
        for part in line.iter_mut().skip(1) {
            (part.fg, part.bg) = (C::Black, Some(back));
        }
    }
    let mut lines = vec![line];
    for (source, value) in &var.hides {
        let value = if var.secret { MASKED } else { value.as_str() };
        lines.push(vec![
            seg(format!("{}└ {value}", " ".repeat(4 + KEY_CELLS)), C::DarkGray),
            seg(format!("  {} · hidden", source.name()), C::DarkGray),
        ]);
    }
    lines
}

/// The line an override is typed on.
fn typing_line(ui: &Ui, input: &Typing, inner: usize) -> Vec<Seg> {
    let lead = match &input.name {
        Some(name) => format!(" {name} = "),
        None => " new override (NAME=value): ".into(),
    };
    let back = if input.wrong { C::Red } else { C::Yellow };
    vec![on(pad(&format!("{lead}{}{}", input.text, ui.cursor()), inner), C::Black, back)]
}

/// The keys, for what's picked: as many as fit, `Esc` last.
fn footer(input: Option<&Typing>, picked: Option<&Var>, only: bool, dotnet: bool, inner: usize) -> String {
    if let Some(input) = input {
        let text = match (input.wrong, &input.name) {
            (true, _) => "NAME=value: a name without spaces, =, then its value · Esc leaves it",
            (false, None) => "type NAME=value · Enter keeps it · Esc leaves it",
            (false, Some(_)) => "type its value · Enter overrides it from the next start · Esc leaves it",
        };
        return text.into();
    }
    let mut parts = vec!["j k pick"];
    match picked {
        Some(var) if var.source == Source::Override => {
            parts.extend(["Enter edits", "d drops", if var.branch { "b every branch" } else { "b this branch only" }]);
            if var.secret && dotnet {
                parts.push("s to user secrets");
            }
        }
        Some(_) => parts.push("Enter overrides"),
        None => {}
    }
    if picked.is_some_and(|var| var.secret) {
        parts.push("Space shows");
    }
    parts.extend(["a adds", if only { "v everything" } else { "v only overrides" }, "V copies them"]);
    parts.extend(["y copies", "r restarts", "Esc closes"]);
    let mut text = parts.join(" · ");
    while text.width() > inner && parts.len() > 2 {
        parts.remove(parts.len() - 2);
        text = parts.join(" · ");
    }
    text
}

pub fn rows(ui: &Ui, controller: &Controller, cells: usize, rows: usize) -> Vec<Vec<Seg>> {
    let Some(Overlay::Sheet { app, index, shown, typing, only, saved }) = &ui.overlay else { return Vec::new() };
    let inner = cells.saturating_sub(8).clamp(20, INNER);
    let built = build(controller, *app, *only);
    let count = built.iter().filter(|row| matches!(row, Row::Var(_))).count();
    let selected = (*index).min(count.saturating_sub(1));
    let reveal = revealed(ui, *shown);
    let lit = saved.as_ref().filter(|(_, at)| moving(ui) && ui.now.saturating_duration_since(*at) < FLASH);
    let (mut picked, mut var_at, mut at_selected) = (None, 0, 0);
    let mut lines: Vec<Vec<Seg>> = Vec::new();
    for row in built {
        match row {
            Row::Text(text) => {
                let pieces = super::columns::chunk(&text, inner.saturating_sub(4));
                lines.extend(pieces.into_iter().map(|piece| vec![seg(format!("   {piece}"), C::Gray)]));
            }
            Row::Heading { title, changed } => {
                let mut line = vec![seg(format!(" {title}"), C::Cyan)];
                if changed {
                    line.push(seg("  changed since it started · r restarts it", C::Yellow));
                }
                lines.push(line);
            }
            Row::Pair { key, value } => {
                lines.push(vec![seg(format!("   {}", pad(&key, KEY_CELLS)), C::DarkGray), seg(value, C::White)]);
            }
            Row::Suggested { key, value } => {
                let idea = icons::icon(ui.nerd, Icon::Idea);
                lines.push(vec![
                    seg(format!("   {}", pad(&key, KEY_CELLS)), C::DarkGray),
                    seg(format!("{idea} {value}"), C::DarkGray),
                ]);
            }
            Row::Var(var) => {
                let this = var_at == selected;
                var_at += 1;
                if this {
                    at_selected = lines.len();
                }
                let flash = lit.is_some_and(|(name, _)| *name == var.name);
                lines.extend(var_lines(ui, &var, this, reveal.filter(|_| this), flash, inner));
                if this {
                    picked = Some(var);
                }
            }
        }
    }
    if let Some(input) = typing {
        lines.extend([Vec::new(), typing_line(ui, input, inner)]);
        at_selected = lines.len() - 1;
    }
    let room = rows.saturating_sub(CHROME_ROWS).max(1);
    if lines.len() > room {
        let first = at_selected.saturating_sub(room / 2).min(lines.len() - room);
        lines = lines.into_iter().skip(first).take(room).collect();
    }
    let app = controller.apps.get(*app);
    let name = app.map_or("", |app| app.name.as_str());
    let dotnet = app.is_some_and(|app| matches!(app.kind, AppKind::Dotnet { .. }));
    let footer = footer(typing.as_ref(), picked.as_ref(), *only, dotnet, inner);
    let title =
        if *only { format!("Start sheet · {name} · on this machine") } else { format!("Start sheet · {name}") };
    boxed(&icons::titled(ui.nerd, Icon::Terminal, &title), lines, &footer, cells, rows, inner)
}

/// Whether the sheet moves: a secret's value on screen (the box redraws to wipe and mask it again), or a row just
/// saved, lit.
pub fn animating(ui: &Ui) -> bool {
    let Some(Overlay::Sheet { shown, saved, .. }) = &ui.overlay else { return false };
    let since = |at: Instant| ui.now.saturating_duration_since(at);
    shown.is_some_and(|at| since(at) < SHOWN_FOR)
        || saved.as_ref().is_some_and(|(_, at)| moving(ui) && since(*at) < FLASH)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_typed_override_needs_a_name_without_spaces_and_a_secret_wipes_in_from_the_left() {
        let typed = |text: &str| Typing { name: None, text: text.into(), wrong: false }.parsed();
        assert_eq!(typed("FEATURE_X=on"), Some(("FEATURE_X".into(), "on".into())));
        assert_eq!(typed(" URL =http://a=b"), Some(("URL".into(), "http://a=b".into())), "the first = splits");
        assert_eq!(typed("EMPTY="), Some(("EMPTY".into(), String::new())), "an empty value is a value");
        assert_eq!(typed("no equals"), None);
        assert_eq!(typed("TWO WORDS=x"), None);
        assert_eq!(typed("=x"), None);
        let named = Typing { name: Some("A".into()), text: "x=y".into(), wrong: false };
        assert_eq!(named.parsed(), Some(("A".into(), "x=y".into())), "a picked variable's value is all of it");
        assert_eq!(wiped("hunter2", 0.0), "•••••••");
        assert_eq!(wiped("hunter2", 0.5), "hunt•••");
        assert_eq!(wiped("hunter2", 1.0), "hunter2");
        let args = ["run".into(), "-e".into(), "KEY=secret".into(), "--".into()];
        assert_eq!(command_line(Path::new("dotnet"), &args), "dotnet run -e KEY=… --", "the value isn't shown");
    }
}
