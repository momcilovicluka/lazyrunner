//! The event loop for a running project: keys, the controller's events and its tick, control requests (tests,
//! `lazyrunner stop`), and frames at most 60 times a second, only when something changed or is animating.

use std::io;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use futures_util::{Stream, StreamExt};
use lazyrunner_core::ipc;
use lazyrunner_core::output::Hue;
use lazyrunner_core::paths::Paths;
use lazyrunner_core::profile::Project;
use lazyrunner_core::run::Controller;
use lazyrunner_core::run::child::Guard;
use lazyrunner_core::run::controller::Setup;
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};
use tokio::time::sleep_until;

use crate::keys;
use crate::project::{self, ViewState};
use crate::run::{Surface, TimerResolution};
use crate::theme::{ColorMode, NIGHT, downsample};

const FRAME: Duration = Duration::from_micros(16_667);
const MIN_GAP: Duration = Duration::from_millis(2);
const TICK: Duration = Duration::from_millis(250);
const FRAME_DUMP_EVERY: Duration = Duration::from_millis(100);
const ANSWER_GRACE: Duration = Duration::from_millis(100);

pub struct ProjectOptions {
    /// Control channel names to listen on: the tests' own, and the one `lazyrunner stop` uses.
    pub ipc: Vec<String>,
    pub frame_dump: Option<PathBuf>,
    pub colors: ColorMode,
    pub headless: bool,
    /// Start everything right away (autoStartOnOpen, `lazyrunner start`).
    pub start_now: bool,
}

pub struct ProjectRun {
    pub project: Project,
    pub root: PathBuf,
    pub paths: Paths,
    pub guard: Option<Arc<Guard>>,
}

pub async fn run_project(surface: &mut dyn Surface, run: ProjectRun, options: ProjectOptions) -> Result<()> {
    let _timer = TimerResolution::high();
    let palette = NIGHT;
    let (events, mut event_rx) = mpsc::unbounded_channel();
    let ProjectRun { project, root, paths, guard } = run;
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

    if options.start_now {
        controller.start(None, false);
    } else {
        controller.add_log("Ready - press U to start everything (auto-start when lazyrunner opens is off).", Hue::Cyan);
    }
    let mut view = ViewState::default();
    let mut dumps: Vec<oneshot::Sender<Value>> = Vec::new();
    let started = Instant::now();
    let mut next_frame = Some(started);
    let mut last_dump: Option<Instant> = None;
    let mut tick = tokio::time::interval(TICK);
    let far = started + Duration::from_secs(365 * 24 * 3600);

    loop {
        let mut dirty = false;
        let mut quit = false;
        tokio::select! {
            biased;
            event = terminal_events.next() => match event {
                Some(Ok(Event::Key(key))) if key.kind != KeyEventKind::Release => {
                    quit = press(key, &mut controller, &mut view);
                    dirty = true;
                }
                Some(Ok(Event::Resize(..))) => dirty = true,
                Some(Ok(_)) => {}
                Some(Err(error)) => return Err(error.into()),
                None => quit = true,
            },
            Some(request) = control_rx.recv() => {
                match control(request, &mut controller, &mut view, &mut dumps) {
                    Control::Continue => dirty = true,
                    Control::Quit => {
                        tokio::time::sleep(ANSWER_GRACE).await;
                        quit = true;
                    }
                }
            }
            _ = sleep_until(next_frame.unwrap_or(far).into()), if next_frame.is_some() => {
                let now = Instant::now();
                let dump_due = options.frame_dump.is_some() && last_dump.is_none_or(|at| now - at >= FRAME_DUMP_EVERY);
                let text = surface.render(&mut |frame| {
                    project::draw(frame, &controller, &mut view, &palette, now);
                    if options.colors == ColorMode::Ansi256 {
                        downsample(frame.buffer_mut());
                    }
                }, dump_due || !dumps.is_empty())?;
                if let Some(text) = text {
                    for reply in dumps.drain(..) {
                        let _ = reply.send(json!({ "ok": true, "frame": text }));
                    }
                    if dump_due && let Some(path) = &options.frame_dump {
                        let _ = std::fs::write(path, &text);
                        last_dump = Some(now);
                    }
                }
                next_frame = if project::is_animating(&controller, now) {
                    Some((next_frame.unwrap_or(now) + FRAME).max(Instant::now() + MIN_GAP))
                } else if options.frame_dump.is_some() && !dump_due {
                    Some(now + FRAME_DUMP_EVERY)
                } else {
                    None
                };
            }
            _ = tick.tick() => {
                controller.tick();
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
        if quit {
            controller.add_log("Quitting - stopping everything", Hue::DarkGray);
            controller.shutdown();
            return Ok(());
        }
        if dirty && next_frame.is_none() {
            next_frame = Some(Instant::now() + MIN_GAP);
        }
    }
}

/// Handles a key; true when lazyrunner should quit.
fn press(key: KeyEvent, controller: &mut Controller, view: &mut ViewState) -> bool {
    let count = controller.apps.len();
    if count == 0 {
        return matches!(key.code, KeyCode::Char('q'));
    }
    let focused = controller.apps[view.focus.min(count - 1)].key.clone();
    let only = [focused.clone()];
    let page = usize::from(view.body_rows.max(2) - 1);
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Char('c') if ctrl => return true,
        KeyCode::Char('l') if ctrl => {
            for state in &mut controller.states {
                state.output.clear();
                state.scroll = 0;
            }
        }
        KeyCode::Char('q' | 'Q') => return true,
        KeyCode::Esc => view.zoom = false,
        KeyCode::Char('h') | KeyCode::Left | KeyCode::BackTab => view.focus = (view.focus + count - 1) % count,
        KeyCode::Char('l') | KeyCode::Right | KeyCode::Tab => view.focus = (view.focus + 1) % count,
        KeyCode::Char(digit @ '1'..='9') if (digit as usize - '1' as usize) < count => {
            view.focus = digit as usize - '1' as usize
        }
        KeyCode::Char('z') | KeyCode::Enter => view.zoom = !view.zoom,
        KeyCode::Char('k') | KeyCode::Up => scroll(controller, view.focus, 1),
        KeyCode::Char('j') | KeyCode::Down => scroll(controller, view.focus, -1),
        KeyCode::PageUp => scroll(controller, view.focus, page as isize),
        KeyCode::PageDown => scroll(controller, view.focus, -(page as isize)),
        KeyCode::Char('g') | KeyCode::Home => scroll(controller, view.focus, isize::MAX / 2),
        KeyCode::Char('G') | KeyCode::End => scroll(controller, view.focus, isize::MIN / 2),
        KeyCode::Char(' ') => controller.toggle(&focused),
        KeyCode::Char('u') => controller.start(Some(&only), false),
        KeyCode::Char('s') => controller.stop_focused(&focused),
        KeyCode::Char('r') => controller.restart("requested from the panel", Some(&only), false),
        KeyCode::Char('b') => controller.restart("full rebuild", Some(&only), true),
        KeyCode::Char('U') => controller.start(None, false),
        KeyCode::Char('S') => controller.stop_all(),
        KeyCode::Char('R') => controller.restart("requested from the panel", None, false),
        KeyCode::Char('B') => controller.restart("full rebuild", None, true),
        KeyCode::Char('!') => controller.resolve_port_conflicts(),
        KeyCode::Char('i') => controller.npm_ci(Some(&focused)),
        _ => {}
    }
    false
}

fn scroll(controller: &mut Controller, index: usize, delta: isize) {
    if let Some(state) = controller.states.get_mut(index) {
        let max = state.output.len().saturating_sub(1);
        state.scroll = state.scroll.saturating_add_signed(delta).min(max);
    }
}

enum Control {
    Continue,
    Quit,
}

/// `{"keys": "U"}`, `{"dump": true}` (answered after the next frame), `{"state": true}`, `{"action": "start" | "stop"
/// | "restart" | "quit"}`, `{"quit": true}`.
fn control(
    request: ipc::Request,
    controller: &mut Controller,
    view: &mut ViewState,
    dumps: &mut Vec<oneshot::Sender<Value>>,
) -> Control {
    let body = &request.body;
    if let Some(spec) = body.get("keys").and_then(Value::as_str) {
        match keys::parse(spec) {
            Ok(keys) => {
                for key in keys {
                    if press(key, controller, view) {
                        let _ = request.reply.send(json!({ "ok": true, "quit": true }));
                        return Control::Quit;
                    }
                }
                let _ = request.reply.send(json!({ "ok": true }));
            }
            Err(error) => {
                let _ = request.reply.send(json!({ "ok": false, "error": error }));
            }
        }
        return Control::Continue;
    }
    if body.get("dump").is_some() {
        dumps.push(request.reply);
        return Control::Continue;
    }
    if body.get("state").is_some() {
        let mut state = controller.state_json();
        state["focus"] = json!(view.focus);
        state["zoom"] = json!(view.zoom);
        let _ = request.reply.send(json!({ "ok": true, "state": state }));
        return Control::Continue;
    }
    let action = body.get("action").and_then(Value::as_str).or_else(|| body.get("quit").map(|_| "quit"));
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
            return Control::Quit;
        }
        _ => false,
    };
    let answer = if known { json!({ "ok": true }) } else { json!({ "ok": false, "error": "unknown request" }) };
    let _ = request.reply.send(answer);
    Control::Continue
}
