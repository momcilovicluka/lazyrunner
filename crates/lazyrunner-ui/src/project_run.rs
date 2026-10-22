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
use lazyrunner_core::config::Toggle;
use lazyrunner_core::ipc;
use lazyrunner_core::output::Hue;
use lazyrunner_core::paths::Paths;
use lazyrunner_core::profile::{self, ProfileDirs, Project};
use lazyrunner_core::run::child::Guard;
use lazyrunner_core::run::controller::{Passed, Setup, read_handover};
use lazyrunner_core::run::{Command, Controller, Origin};
use lazyrunner_core::update;
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};
use tokio::time::sleep_until;

use crate::git_tool::{self, GitTool};
use crate::keys;
use crate::screen::checklist::{Checklist, StillChecking};
use crate::screen::{self, Exit, Overlay, Ui, status};
use crate::terminal::{MouseCapture, Surface, TimerResolution};
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
    /// The handover file of the lazyrunner this one takes the running apps over from.
    pub handover: Option<PathBuf>,
    /// Every handle value it passed on (on the command line too): closed if the file can't be read, so the apps
    /// stop instead of running on with nobody reading their output.
    pub handover_handles: Vec<u64>,
}

/// How a run ended: how to go on, and the running apps handed over to the next lazyrunner, if they were.
pub struct Finished {
    pub exit: Exit,
    pub handover: Option<Passed>,
    /// What the next lazyrunner says first in its log (it may be another process: a new version).
    pub notes: Vec<String>,
}

pub struct ProjectRun {
    pub project: Project,
    pub root: PathBuf,
    pub paths: Paths,
    pub guard: Option<Arc<Guard>>,
}

/// Runs the project until it quits (or asks to be opened again, on another project or with its profile reloaded).
pub async fn run_project(surface: &mut dyn Surface, run: ProjectRun, options: ProjectOptions) -> Result<Finished> {
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
    let mut terminal_events = terminal_input(options.headless);

    let started = Instant::now();
    let (mut ui, warnings) = Ui::new(&controller, user_themes, started);
    for warning in warnings {
        controller.add_log(warning, Hue::Yellow);
    }
    let mut outside = Outside::new(!options.headless);
    let mut mouse = MouseCapture::new(!options.headless);
    mouse.set(controller.setting(Toggle::Mouse));
    for note in &options.notes {
        controller.add_log(note.clone(), Hue::Yellow);
    }
    let whats_new = changelog::whats_new(&state_dir);
    let mut quiet: Option<std::sync::mpsc::Receiver<update::Outcome>>;
    let mut still_checking: Option<StillChecking> = None;
    if let Some(file) = &options.handover {
        // Taking over from the previous lazyrunner: its apps keep running; no checklist, no auto-start.
        log_whats_new(&mut controller, &whats_new);
        take_over(&mut controller, file, &options.handover_handles);
        quiet = None;
    } else if controller.setting(Toggle::StartupChecklist) {
        let checklist = Checklist::start(&controller, false, &whats_new, options.start_now);
        ui.overlay = Some(Overlay::Checklist(Box::new(checklist)));
        quiet = None;
    } else {
        log_whats_new(&mut controller, &whats_new);
        quiet = quiet_update(&controller);
        begin(&mut controller, options.start_now);
    }

    let mut dumps: Vec<oneshot::Sender<Value>> = Vec::new();
    let mut next_frame = Some(started);
    // When the last frame was drawn: output, ticks and the like redraw at most once a FRAME.
    let mut last_frame = started;
    let mut last_dump: Option<Instant> = None;
    let mut tick = tokio::time::interval(TICK);
    let far = started + Duration::from_secs(365 * 24 * 3600);
    // The git tool running in this terminal (L): lazyrunner neither draws nor reads keys until it quits.
    let mut git_tool: Option<(String, tokio::task::JoinHandle<io::Result<std::process::ExitStatus>>)> = None;

    loop {
        let mut dirty = false;
        // A key or a resize is drawn right away; everything else waits for the next frame slot.
        let mut urgent = false;
        tokio::select! {
            biased;
            event = terminal_events.next() => match event {
                Some(Ok(Event::Key(key))) if key.kind != KeyEventKind::Release => {
                    ui.now = Instant::now();
                    screen::keys::press(&mut ui, &mut controller, key);
                    dirty = true;
                    urgent = true;
                }
                Some(Ok(Event::Resize(..))) => {
                    dirty = true;
                    urgent = true;
                }
                // A moving pointer, drags and releases are dropped here, before anything redraws.
                Some(Ok(Event::Mouse(event))) if screen::mouse::wanted(&event) => {
                    ui.now = Instant::now();
                    if screen::mouse::handle(&mut ui, &mut controller, event) {
                        dirty = true;
                        // A click shows at once; the wheel's notches wait for the next frame slot.
                        urgent = matches!(event.kind, crossterm::event::MouseEventKind::Down(_));
                    }
                }
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
            done = async {
                match git_tool.as_mut() {
                    Some((_, handle)) => handle.await,
                    None => std::future::pending().await,
                }
            }, if git_tool.is_some() => {
                let name = git_tool.take().map(|(name, _)| name).unwrap_or_default();
                ui.away = None;
                if let Err(error) = surface.take_back() {
                    controller.shutdown();
                    return Err(error);
                }
                terminal_events = terminal_input(options.headless);
                outside = Outside::new(!options.headless); // its title and progress are written again
                match done {
                    Ok(Ok(status)) if status.success() => controller.add_log(format!("Back from {name}"), Hue::DarkGray),
                    Ok(Ok(status)) => controller.add_log(format!("{name} ended ({status})"), Hue::Yellow),
                    Ok(Err(error)) => controller.add_log(format!("✗ {name} didn't start - {error}"), Hue::Red),
                    Err(error) => controller.add_log(format!("✗ {name}: {error}"), Hue::Red),
                }
                dirty = true;
                urgent = true;
            }
            _ = sleep_until(next_frame.unwrap_or(far).into()), if next_frame.is_some() && git_tool.is_none() => {
                let now = Instant::now();
                ui.now = now;
                last_frame = now;
                update_checklist(&mut ui, &mut controller, &mut still_checking);
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
                update_checklist(&mut ui, &mut controller, &mut still_checking);
                poll_quiet_update(&mut quiet, &mut ui, &mut controller);
                poll_still_checking(&mut still_checking, &mut ui, &mut controller);
                for panic in crate::panics::take() {
                    controller.add_log(format!("✗ Internal error in {panic} - lazyrunner carries on"), Hue::Red);
                }
                switch_when_idle(&mut ui, &mut controller);
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
        if std::mem::take(&mut ui.git_tool_wanted) && git_tool.is_none() {
            match open_git_tool(&mut controller) {
                Some(tool) => {
                    terminal_events = Box::pin(futures_util::stream::pending()); // its keys are its own
                    mouse.set(false);
                    if let Err(error) = surface.hand_over() {
                        controller.shutdown();
                        return Err(error);
                    }
                    let root = controller.root.clone();
                    ui.away = Some(tool.name.clone());
                    git_tool = Some((tool.name.clone(), tokio::task::spawn_blocking(move || tool.run(&root))));
                }
                None => dirty = true,
            }
        }
        mouse.set(controller.setting(Toggle::Mouse) && git_tool.is_none());
        if let Some(Exit::Reopen { root, .. }) = &ui.exit
            && hands_over_to(&controller, &profiles, root)
            && let Some(why) = controller.handover_blocked()
        {
            controller.add_log(format!("{why} - , then p again once it's done (the apps keep running)"), Hue::Yellow);
            ui.exit = None;
        }
        if let Some(mut exit) = ui.exit.clone().filter(|_| git_tool.is_none()) {
            let mut handover = None;
            let mut notes = Vec::new();
            if let Exit::Reopen { root, .. } = &exit
                && hands_over_to(&controller, &profiles, root)
            {
                // What already happened first (an exit on its way), then let go, then the output read until then.
                drain(&mut event_rx, &mut controller);
                let passed = controller.release_running().and_then(|releasing| {
                    drain(&mut event_rx, &mut controller);
                    controller.write_handover(releasing)
                });
                match passed {
                    Ok(passed) => handover = Some(passed),
                    Err(why) => {
                        // The apps stop with this lazyrunner: the next one starts them again, and says why.
                        let note = format!("✗ Couldn't hand the running apps over ({why}) - starting them again");
                        controller.add_log(note.clone(), Hue::Yellow);
                        notes.push(note);
                        if let Exit::Reopen { start, .. } = &mut exit {
                            *start = true;
                        }
                    }
                }
            }
            if exit == Exit::Quit {
                controller.add_log("Quitting - stopping everything", Hue::DarkGray);
            }
            controller.shutdown();
            return Ok(Finished { exit, handover, notes });
        }
        if dirty {
            let soonest = Instant::now() + MIN_GAP;
            let slot = if urgent { soonest } else { (last_frame + FRAME).max(soonest) };
            next_frame = Some(next_frame.map_or(slot, |planned| planned.min(slot)));
        }
    }
}

/// The terminal's keys, mouse and resizes (none headless). A new one after the git tool: dropping the old one stops
/// its reading, so the tool gets every key.
fn terminal_input(headless: bool) -> Pin<Box<dyn Stream<Item = io::Result<Event>> + Send>> {
    if headless { Box::pin(futures_util::stream::pending()) } else { Box::pin(EventStream::new()) }
}

/// L: the git tool found, or why not (in the log). Beside lazyrunner (Windows Terminal, "gitBeside") it opens there
/// and `None` comes back too; here, the caller hands it the terminal.
fn open_git_tool(controller: &mut Controller) -> Option<GitTool> {
    let tool = match GitTool::from_config(&controller.config) {
        Ok(tool) => tool,
        Err(why) => {
            controller.add_log(format!("✗ {why}"), Hue::Red);
            return None;
        }
    };
    let beside = git_tool::place(&controller.config, git_tool::in_windows_terminal()) == git_tool::Place::Beside;
    if !beside {
        if controller.setting(Toggle::GitBeside) {
            controller.add_log(format!("{} opens here: beside needs Windows Terminal", tool.name), Hue::DarkGray);
        }
        controller.add_log(format!("{} - q comes back to lazyrunner; the apps keep running", tool.name), Hue::Cyan);
        return Some(tool);
    }
    match tool.open_beside(&controller.root) {
        Ok(()) => controller.add_log(format!("Opened {} beside lazyrunner", tool.name), Hue::Cyan),
        Err(why) => controller.add_log(format!("✗ {why}"), Hue::Red),
    }
    None
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

fn update_checklist(ui: &mut Ui, controller: &mut Controller, still_checking: &mut Option<StillChecking>) {
    let Some(Overlay::Checklist(checklist)) = &mut ui.overlay else { return };
    if !checklist.update(ui.now, controller) {
        return;
    }
    let Some(Overlay::Checklist(checklist)) = ui.overlay.take() else { return };
    let (manual, start_after, updated_to) = (checklist.manual, checklist.start_after, checklist.updated_to.clone());
    if let Some(still) = checklist.still_checking() {
        *still_checking = Some(still); // a checklist opened by hand (, r) never has one: keeps the startup's
    }
    if let Some(version) = updated_to {
        after_update(ui, controller, &version, manual, start_after);
    } else if !manual {
        begin(controller, start_after);
    }
}

/// After an update lazyrunner reopens on the new binary. At startup nothing runs yet, so it reopens right away (and
/// starts the apps if it was going to). Later (, then r) running apps would be stopped, so that waits for , then p.
fn after_update(ui: &mut Ui, controller: &mut Controller, version: &str, manual: bool, start: bool) {
    let busy = controller.building.is_some()
        || controller.npm_ci.is_some()
        || controller.states.iter().any(|state| state.is_running());
    if busy && cfg!(windows) {
        // The new version takes the running apps over: no restart, so it switches as soon as nothing is in the way.
        ui.switch_to = Some(version.to_string());
        return;
    }
    if manual && busy {
        controller.add_log(
            format!(
                "lazyrunner v{version} is installed - , then p reopens lazyrunner on it (the running apps restart)"
            ),
            Hue::Cyan,
        );
        return;
    }
    let root = controller.root.clone();
    screen::keys::reopen(ui, controller, root, start && !manual, &format!("lazyrunner v{version} installed"));
}

/// An installed update waiting for a quiet moment (no build, npm ci, overlay, picker or search being typed): then
/// lazyrunner switches to it, and the new version takes the running apps over.
fn switch_when_idle(ui: &mut Ui, controller: &mut Controller) {
    let Some(version) = ui.switch_to.clone() else { return };
    let typing = ui.search.as_ref().is_some_and(|search| search.typing);
    let waiting = ui.busy.is_some() || ui.deferred.is_some() || ui.pending_g.is_some();
    let away = ui.away.is_some();
    if controller.handover_blocked().is_some()
        || ui.overlay.is_some()
        || ui.picker.is_some()
        || typing
        || waiting
        || away
    {
        return;
    }
    ui.switch_to = None;
    controller.add_log(format!("Switching to lazyrunner v{version} - the apps keep running"), Hue::Cyan);
    let root = controller.root.clone();
    screen::keys::reopen(ui, controller, root, false, &format!("lazyrunner v{version} installed"));
}

/// Whether reopening on `root` hands the running apps over instead of stopping them: on Windows, for this same
/// project and checkout (a reloaded profile, a new version), when its profile there is one this lazyrunner may run without
/// asking (trusted) and something is running.
fn hands_over_to(controller: &Controller, profiles: &ProfileDirs, root: &Path) -> bool {
    if !cfg!(windows) || !controller.states.iter().any(|state| state.is_running()) {
        return false;
    }
    let Ok(Some(found)) = profile::find_project_profile(root, profiles) else { return false };
    let Ok(next) = profile::resolve(&found, profiles) else { return false };
    next.id == controller.project.id
        && profile::trust::is_trusted(&next, &controller.paths().trust_store())
        && same_path(root, &controller.root)
}

/// Handles the events waiting (without waiting for more).
fn drain(event_rx: &mut mpsc::UnboundedReceiver<lazyrunner_core::run::Event>, controller: &mut Controller) {
    while let Ok(event) = event_rx.try_recv() {
        controller.handle(event);
    }
}

/// Takes over the apps the previous lazyrunner handed over (and starts again the ones whose command changed). If
/// its file can't be read, the handles it passed on are closed: those apps stop, and U starts them again.
fn take_over(controller: &mut Controller, file: &Path, handles: &[u64]) {
    match read_handover(file) {
        Ok(handover) => {
            // SAFETY: this process was started for this handover and inherited exactly these handles; nothing else
            // in it owns them.
            let restart = unsafe { controller.take_over(handover) };
            if !restart.is_empty() {
                controller.start(Some(&restart), false);
            }
        }
        Err(why) => {
            // SAFETY: the handle values the previous lazyrunner passed on to this one, owned by nothing else here.
            unsafe { lazyrunner_core::run::child::close_inherited(handles) };
            controller.add_log(
                format!("✗ Couldn't take over the running apps ({why}) - they stopped; U starts them again"),
                Hue::Red,
            );
        }
    }
}

/// The checklist is off: the same update check, in the background, while the apps start.
fn quiet_update(controller: &Controller) -> Option<std::sync::mpsc::Receiver<update::Outcome>> {
    if !controller.setting(Toggle::AutoUpdate) {
        return None;
    }
    let (config, state_dir) = (controller.config.clone(), controller.paths().state_dir.clone());
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut quiet = |_: &str| {};
        let _ = sender.send(update::update(&config, &state_dir, false, &mut update::Progress::new(&mut quiet)));
    });
    Some(receiver)
}

/// The update check the startup didn't wait for: its outcome in the log; a version it installed is switched to
/// once nothing is in the way (the new version takes the running apps over).
fn poll_still_checking(still_checking: &mut Option<StillChecking>, ui: &mut Ui, controller: &mut Controller) {
    let Some(checking) = still_checking else { return };
    let Some(updated_to) = checking.poll(controller) else { return };
    *still_checking = None;
    match updated_to {
        // Not mid-typing or in a menu: it switches once nothing is in the way.
        Some(version) if cfg!(windows) => ui.switch_to = Some(version),
        Some(version) => after_update(ui, controller, &version, true, false),
        None => {}
    }
}

/// Its outcome in the log; what it installed reopens lazyrunner, or waits for , then p while apps run.
fn poll_quiet_update(
    quiet: &mut Option<std::sync::mpsc::Receiver<update::Outcome>>,
    ui: &mut Ui,
    controller: &mut Controller,
) {
    let Some(receiver) = quiet else { return };
    let outcome = match receiver.try_recv() {
        Ok(outcome) => Some(outcome),
        Err(std::sync::mpsc::TryRecvError::Empty) => return,
        Err(std::sync::mpsc::TryRecvError::Disconnected) => None,
    };
    *quiet = None;
    if let Some(outcome) = &outcome
        && outcome.status == update::Status::Warn
    {
        controller.add_log(format!("! lazyrunner: {}", outcome.detail), Hue::Yellow);
    }
    if let Some(version) = outcome.and_then(|outcome| outcome.updated_to) {
        after_update(ui, controller, &version, true, false);
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
    if headless || !controller.setting(Toggle::Notifications) {
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

/// `{"keys": "U"}`, `{"click": [x, y]}`, `{"wheel": [x, y, 1 | -1]}`, `{"dump": true}` (answered after the next
/// frame), `{"state": true}`, `{"command": <Command>}`
/// (lazyrunner_core::run::Command, e.g. `{"command": {"restart": "orders"}}`), `{"action": "start" | "stop"
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
    if let Some(tool) = &ui.away
        && ["keys", "click", "wheel", "dump"].iter().any(|key| body.get(*key).is_some())
    {
        let error = format!("{tool} has the terminal - try again once it quits");
        let _ = request.reply.send(json!({ "ok": false, "error": error }));
        return false;
    }
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
    if let Some(at) = body.get("click").or_else(|| body.get("wheel")) {
        let cell = |index: usize| at.get(index).and_then(Value::as_u64).and_then(|value| u16::try_from(value).ok());
        let answer = match (cell(0), cell(1)) {
            (Some(x), Some(y)) if body.get("click").is_some() => {
                screen::mouse::click_at(ui, controller, x, y, Instant::now());
                json!({ "ok": true, "target": format!("{:?}", screen::mouse::target_at(ui, x, y)) })
            }
            (Some(x), Some(y)) => {
                let back = at.get(2).and_then(Value::as_i64).unwrap_or(1) > 0;
                screen::mouse::wheel_at(ui, controller, x, y, back);
                json!({ "ok": true })
            }
            _ => json!({ "ok": false, "error": "wants [x, y] (a wheel also 1 back or -1)" }),
        };
        let _ = request.reply.send(answer);
        return false;
    }
    if let Some(command) = body.get("command") {
        let answer = match serde_json::from_value::<Command>(command.clone()) {
            Ok(command) => match controller.execute(command, Origin::CommandLine) {
                Ok(()) => json!({ "ok": true }),
                Err(why) => json!({ "ok": false, "error": why }),
            },
            Err(error) => json!({ "ok": false, "error": format!("not a command lazyrunner knows - {error}") }),
        };
        let _ = request.reply.send(answer);
        return false;
    }
    if body.get("state").is_some() {
        let mut state = controller.state_json();
        state["focus"] = json!(ui.focus);
        state["zoom"] = json!(ui.zoom);
        state["overlay"] = json!(ui.overlay.as_ref().map(Overlay::kind));
        state["picker"] = json!(ui.picker.is_some());
        state["away"] = json!(ui.away);
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
    if action == Some("quit") {
        let _ = request.reply.send(json!({ "ok": true }));
        ui.exit = Some(Exit::Quit);
        return true;
    }
    let command = match action {
        Some("start") => Some(Command::StartAll),
        Some("stop") => Some(Command::StopAll),
        Some("restart") => Some(Command::RestartAll),
        _ => None,
    };
    let answer = match command.map(|command| controller.execute(command, Origin::CommandLine)) {
        Some(Ok(())) => json!({ "ok": true }),
        Some(Err(why)) => json!({ "ok": false, "error": why }),
        None => json!({ "ok": false, "error": "unknown request" }),
    };
    let _ = request.reply.send(answer);
    false
}
