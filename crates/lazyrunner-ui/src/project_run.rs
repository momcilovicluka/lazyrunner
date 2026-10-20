//! The event loop for a running project: keys, the controller's events and its tick, control requests (tests,
//! `lazyrunner stop`), and frames at most 60 times a second, only when something changed or is animating. Outside
//! the frame it keeps the terminal tab's title and progress, the theme's terminal colours and the desktop
//! notifications up to date.

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{Event, EventStream, KeyEventKind};
use futures_util::{Stream, StreamExt};
use lazyrunner_core::changelog;
use lazyrunner_core::ipc;
use lazyrunner_core::output::Hue;
use lazyrunner_core::paths::Paths;
use lazyrunner_core::profile::{self, ProfileDirs, Project};
use lazyrunner_core::run::Controller;
use lazyrunner_core::run::child::Guard;
use lazyrunner_core::run::controller::Setup;
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};
use tokio::time::sleep_until;

use crate::keys;
use crate::run::{Surface, TimerResolution};
use crate::screen::checklist::Checklist;
use crate::screen::{self, Exit, Overlay, Ui, status};
use crate::theme::{ColorMode, downsample};

/// While something animates: the animations step every 100 ms, so 30 frames a second is plenty.
const FRAME: Duration = Duration::from_micros(33_333);
const MIN_GAP: Duration = Duration::from_millis(2);
const TICK: Duration = Duration::from_millis(250);
const FRAME_DUMP_EVERY: Duration = Duration::from_millis(100);
const ANSWER_GRACE: Duration = Duration::from_millis(100);
/// What's new in the log (when the checklist is off): this many bullets.
const WHATS_NEW_LOG_LINES: usize = 5;

pub struct ProjectOptions {
    /// Control channel names to listen on: the tests' own, and the one `lazyrunner stop` uses.
    pub ipc: Vec<String>,
    pub frame_dump: Option<PathBuf>,
    pub colors: ColorMode,
    pub headless: bool,
    /// Start everything right away (autoStartOnOpen, `lazyrunner start`).
    pub start_now: bool,
    /// Said in the log at the start (why lazyrunner reopened on this project, say).
    pub notes: Vec<String>,
}

pub struct ProjectRun {
    pub project: Project,
    pub root: PathBuf,
    pub paths: Paths,
    pub guard: Option<Arc<Guard>>,
}

/// Runs the project until it quits (or asks to be opened again, on another project or with its profile reloaded).
pub async fn run_project(surface: &mut dyn Surface, run: ProjectRun, options: ProjectOptions) -> Result<Exit> {
    let _timer = TimerResolution::high();
    let (events, mut event_rx) = mpsc::unbounded_channel();
    let ProjectRun { project, root, paths, guard } = run;
    let user_themes = paths.config_file.parent().map(|dir| dir.join("themes"));
    let state_dir = paths.state_dir.clone();
    let profiles = paths.profiles.clone();
    let mut controller = Controller::new(Setup { project, root, paths, events, guard })?;
    let (control_tx, mut control_rx) = mpsc::unbounded_channel::<ipc::Request>();
    for name in &options.ipc {
        match ipc::listen(name) {
            Ok(listener) => {
                tokio::spawn(ipc::serve(listener, control_tx.clone()));
            }
            Err(error) => controller.add_log(format!("Control channel '{name}' unavailable: {error}"), Hue::Yellow),
        }
    }
    drop(control_tx);
    let mut terminal_events: Pin<Box<dyn Stream<Item = io::Result<Event>> + Send>> =
        if options.headless { Box::pin(futures_util::stream::pending()) } else { Box::pin(EventStream::new()) };

    let started = Instant::now();
    let (mut ui, warnings) = Ui::new(&controller, user_themes, started);
    for warning in warnings {
        controller.add_log(warning, Hue::Yellow);
    }
    let mut outside = Outside::new(!options.headless);
    for note in &options.notes {
        controller.add_log(note.clone(), Hue::Yellow);
    }
    let whats_new = changelog::whats_new(&state_dir);
    if controller.setting("startupChecklist") {
        let checklist = Checklist::start(&controller, false, &whats_new, options.start_now);
        ui.overlay = Some(Overlay::Checklist(Box::new(checklist)));
    } else {
        log_whats_new(&mut controller, &whats_new);
        begin(&mut controller, options.start_now);
    }

    let mut dumps: Vec<oneshot::Sender<Value>> = Vec::new();
    let mut next_frame = Some(started);
    let mut last_dump: Option<Instant> = None;
    let mut tick = tokio::time::interval(TICK);
    let far = started + Duration::from_secs(365 * 24 * 3600);

    loop {
        let mut dirty = false;
        tokio::select! {
            biased;
            event = terminal_events.next() => match event {
                Some(Ok(Event::Key(key))) if key.kind != KeyEventKind::Release => {
                    ui.now = Instant::now();
                    screen::keys::press(&mut ui, &mut controller, key);
                    dirty = true;
                }
                Some(Ok(Event::Resize(..))) => dirty = true,
                Some(Ok(_)) => {}
                Some(Err(error)) => {
                    controller.shutdown();
                    return Err(error.into());
                }
                None => ui.exit = Some(Exit::Quit),
            },
            Some(request) = control_rx.recv() => {
                ui.now = Instant::now();
                if control(request, &mut ui, &mut controller, &profiles, &mut dumps) {
                    tokio::time::sleep(ANSWER_GRACE).await;
                }
                dirty = true;
            }
            _ = sleep_until(next_frame.unwrap_or(far).into()), if next_frame.is_some() => {
                let now = Instant::now();
                ui.now = now;
                update_checklist(&mut ui, &mut controller);
                let dump_due = options.frame_dump.is_some() && last_dump.is_none_or(|at| now - at >= FRAME_DUMP_EVERY);
                let text = surface.render(&mut |frame| {
                    screen::draw(frame, &mut ui, &controller);
                    if options.colors == ColorMode::Ansi256 {
                        downsample(frame.buffer_mut());
                    }
                }, dump_due || !dumps.is_empty());
                let text = match text {
                    Ok(text) => text,
                    Err(error) => {
                        controller.shutdown();
                        return Err(error);
                    }
                };
                if let Some(text) = text {
                    for reply in dumps.drain(..) {
                        let _ = reply.send(json!({ "ok": true, "frame": text }));
                    }
                    if dump_due && let Some(path) = &options.frame_dump {
                        let _ = std::fs::write(path, &text);
                        last_dump = Some(now);
                    }
                }
                outside.update(&ui, &controller);
                if ui.deferred.is_some() {
                    // The frame saying "Checking for source changes…" is out: now the action that waited for it.
                    screen::keys::run_deferred(&mut ui, &mut controller);
                    next_frame = Some(Instant::now() + MIN_GAP);
                    continue;
                }
                next_frame = if ui.is_animating(&controller) {
                    Some((next_frame.unwrap_or(now) + FRAME).max(Instant::now() + MIN_GAP))
                } else if options.frame_dump.is_some() && !dump_due {
                    Some(now + FRAME_DUMP_EVERY)
                } else {
                    None
                };
            }
            _ = tick.tick() => {
                ui.now = Instant::now();
                controller.tick();
                update_checklist(&mut ui, &mut controller);
                dirty = true;
            }
            Some(event) = event_rx.recv() => {
                controller.handle(event);
                for _ in 0..10_000 {
                    match event_rx.try_recv() {
                        Ok(event) => controller.handle(event),
                        Err(_) => break,
                    }
                }
                dirty = true;
            }
        }
        notify(&mut controller, options.headless);
        if let Some(exit) = ui.exit.clone() {
            if exit == Exit::Quit {
                controller.add_log("Quitting - stopping everything", Hue::DarkGray);
            }
            controller.shutdown();
            return Ok(exit);
        }
        if dirty && next_frame.is_none() {
            next_frame = Some(Instant::now() + MIN_GAP);
        }
    }
}

/// After the checklist (or without it): start everything, or say how.
fn begin(controller: &mut Controller, start_now: bool) {
    if start_now {
        controller.start(None, false);
    } else {
        controller.add_log(
            "Ready - press U to start everything (auto-start when lazyrunner opens is off: , then u turns it on).",
            Hue::Cyan,
        );
    }
}

fn update_checklist(ui: &mut Ui, controller: &mut Controller) {
    let Some(Overlay::Checklist(checklist)) = &mut ui.overlay else { return };
    if !checklist.update(ui.now, controller) {
        return;
    }
    let (manual, start_after) = (checklist.manual, checklist.start_after);
    ui.overlay = None;
    if !manual {
        begin(controller, start_after);
    }
}

/// What's new after an update, when the checklist (which shows it otherwise) is off.
fn log_whats_new(controller: &mut Controller, entries: &[changelog::Entry]) {
    if entries.is_empty() {
        return;
    }
    controller.add_log(
        format!("lazyrunner updated to v{} - what's new (, then c shows the changelog):", changelog::VERSION),
        Hue::Cyan,
    );
    let lines = changelog::format(entries);
    for line in lines.iter().filter(|line| line.text.trim_start().starts_with('•')).take(WHATS_NEW_LOG_LINES) {
        controller.add_log(line.text.clone(), Hue::Gray);
    }
}

/// Desktop notifications for what happened while you were elsewhere ("notifications": false turns them off).
fn notify(controller: &mut Controller, headless: bool) {
    if controller.notifications.is_empty() {
        return;
    }
    let notifications = std::mem::take(&mut controller.notifications);
    if headless || !controller.setting("notifications") {
        return;
    }
    for (title, message) in notifications {
        status::notify(&title, &message);
    }
}

/// The terminal outside the frame: the tab's title and progress, and the theme's terminal colours. Dropping it
/// puts both back.
struct Outside {
    enabled: bool,
    title: Option<status::Title>,
    tab: String,
    theme: Option<String>,
    colored: bool,
}

impl Outside {
    fn new(enabled: bool) -> Self {
        let title = enabled.then(status::Title::save);
        Self { enabled, title, tab: String::new(), theme: None, colored: false }
    }

    fn write(sequence: &str) {
        if !sequence.is_empty() {
            let mut stdout = io::stdout();
            let _ = stdout.write_all(sequence.as_bytes()).and_then(|()| stdout.flush());
        }
    }

    fn update(&mut self, ui: &Ui, controller: &Controller) {
        if !self.enabled {
            return;
        }
        let tab = status::tab_sequence(controller, &status::summary(controller));
        if tab != self.tab {
            Self::write(&tab);
            self.tab = tab;
        }
        if self.theme.as_deref() != Some(ui.theme.id.as_str()) {
            let colors = ui.theme.terminal.as_ref();
            Self::write(&status::terminal_colors(colors, self.colored));
            self.colored = colors.is_some();
            self.theme = Some(ui.theme.id.clone());
        }
    }
}

impl Drop for Outside {
    fn drop(&mut self) {
        if self.enabled {
            let colors = if self.colored { status::terminal_colors(None, true) } else { String::new() };
            Self::write(&format!("{}{colors}", status::TAB_RESET));
        }
        if let Some(title) = &self.title {
            title.restore();
        }
    }
}

/// Paths as Windows compares them (case-insensitively, either slash, no trailing one); exactly elsewhere.
fn same_path(a: &Path, b: &Path) -> bool {
    let normal = |path: &Path| {
        let text = path.display().to_string().replace('\\', "/");
        let text = text.trim_end_matches('/').to_string();
        if cfg!(windows) { text.to_lowercase() } else { text }
    };
    normal(a) == normal(b)
}

/// Whether `root` is this project with the same profile (another worktree of it), so it can switch in place.
fn same_project(controller: &Controller, dirs: &ProfileDirs, root: &Path) -> bool {
    let Ok(Some(found)) = profile::find_project_profile(root, dirs) else { return false };
    let Ok(other) = profile::resolve(&found, dirs) else { return false };
    let own = &controller.project;
    other.id == own.id && other.hash == own.hash && other.source == own.source
}

/// `{"keys": "U"}`, `{"dump": true}` (answered after the next frame), `{"state": true}`, `{"action": "start" | "stop"
/// | "restart" | "quit", "root"?: "<dir>"}`, `{"quit": true}`. True when the answer needs a moment to get out before
/// lazyrunner exits.
fn control(
    request: ipc::Request,
    ui: &mut Ui,
    controller: &mut Controller,
    profiles: &ProfileDirs,
    dumps: &mut Vec<oneshot::Sender<Value>>,
) -> bool {
    let body = &request.body;
    if let Some(spec) = body.get("keys").and_then(Value::as_str) {
        let answer = match keys::parse(spec) {
            Ok(keys) => {
                for key in keys {
                    screen::keys::press(ui, controller, key);
                    if ui.exit.is_some() {
                        break;
                    }
                }
                json!({ "ok": true, "quit": ui.exit.is_some() })
            }
            Err(error) => json!({ "ok": false, "error": error }),
        };
        let _ = request.reply.send(answer);
        return ui.exit.is_some();
    }
    if body.get("dump").is_some() {
        dumps.push(request.reply);
        return false;
    }
    if body.get("state").is_some() {
        let mut state = controller.state_json();
        state["focus"] = json!(ui.focus);
        state["zoom"] = json!(ui.zoom);
        state["overlay"] = json!(ui.overlay.as_ref().map(Overlay::kind));
        state["picker"] = json!(ui.picker.is_some());
        state["theme"] = json!(ui.theme.id);
        let _ = request.reply.send(json!({ "ok": true, "state": state }));
        return false;
    }
    let action = body.get("action").and_then(Value::as_str).or_else(|| body.get("quit").map(|_| "quit"));
    let root = body.get("root").and_then(Value::as_str).map(PathBuf::from);
    let elsewhere =
        root.filter(|root| matches!(action, Some("start" | "restart")) && !same_path(root, &controller.root));
    if let Some(root) = elsewhere {
        let _ = request.reply.send(json!({ "ok": true }));
        if !same_project(controller, profiles, &root) {
            screen::keys::reopen(ui, controller, root, true, "another project");
            return true;
        }
        controller.add_log(format!("Switching to {}", root.display()), Hue::Cyan);
        controller.shutdown();
        controller.set_root(&root);
        controller.start(None, false);
        return false;
    }
    let known = match action {
        Some("start") => {
            controller.start(None, false);
            true
        }
        Some("stop") => {
            controller.stop_all();
            true
        }
        Some("restart") => {
            controller.restart("requested from the command line", None, false);
            true
        }
        Some("quit") => {
            let _ = request.reply.send(json!({ "ok": true }));
            ui.exit = Some(Exit::Quit);
            return true;
        }
        _ => false,
    };
    let answer = if known { json!({ "ok": true }) } else { json!({ "ok": false, "error": "unknown request" }) };
    let _ = request.reply.send(answer);
    false
}
