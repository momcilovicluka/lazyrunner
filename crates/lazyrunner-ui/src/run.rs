//! The event loop. Everything that happens (keys, app output, samples, control requests) is a message; the screen is
//! redrawn at most 60 times a second, only when something changed or is animating, and not at all when idle.

use std::collections::VecDeque;
use std::io::{self, Stdout};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use crossterm::event::{Event, EventStream, KeyEvent, KeyEventKind};
use crossterm::terminal::{BeginSynchronizedUpdate, EndSynchronizedUpdate};
use crossterm::{execute, queue};
use futures_util::{Stream, StreamExt};
use lazyrunner_core::ipc;
use lazyrunner_core::metrics::{Sample, Sampler};
use lazyrunner_core::process::AppProcess;
use ratatui::backend::{CrosstermBackend, TestBackend};
use ratatui::layout::Rect;
use ratatui::{Frame, Terminal};
use serde_json::{Value, json};
use tachyonfx::{EffectManager, Interpolation, Motion, fx};
use tokio::sync::{mpsc, oneshot};
use tokio::time::sleep_until;

use crate::model::{Action, AppSpec, Model};
use crate::theme::{ColorMode, NIGHT, Palette, downsample};
use crate::{dump, keys, view};

const FRAME: Duration = Duration::from_micros(16_667);
const SPLASH: Duration = Duration::from_millis(1400);
const SAMPLE_EVERY: Duration = Duration::from_secs(1);
const FRAME_DUMP_EVERY: Duration = Duration::from_millis(100);
const QUIT_FADE: Duration = Duration::from_millis(320);
/// App output handled per wake-up before the loop looks at keys and the clock again.
const DRAIN: usize = 20_000;
/// App events buffered between the readers and the loop; beyond that the apps' pipes fill and they wait.
const EVENT_BUFFER: usize = 8192;
/// However late a frame is, the loop gets this long to look at keys and output before the next one.
const MIN_GAP: Duration = Duration::from_millis(2);
/// Time for a control client to get its answer before lazyrunner exits.
const ANSWER_GRACE: Duration = Duration::from_millis(100);

pub struct RunOptions {
    pub splash: bool,
    /// Quit after this long and report the numbers.
    pub bench: Option<Duration>,
    /// Name of the control channel to listen on (tests send keys and read frames through it).
    pub ipc: Option<String>,
    /// Write the screen as text here, at most every 100 ms (LAZYRUNNER_FRAME_DUMP).
    pub frame_dump: Option<PathBuf>,
    /// When the process started, for the first-frame time.
    pub process_started: Instant,
    pub colors: ColorMode,
    /// No terminal input (the headless test surface).
    pub headless: bool,
}

/// Something that shows frames: the real terminal, or an in-memory one for tests and headless runs.
pub trait Surface {
    /// Draws a frame; returns it as text when `want_text`.
    fn render(&mut self, draw: &mut dyn FnMut(&mut Frame), want_text: bool) -> Result<Option<String>>;
}

impl Surface for Terminal<CrosstermBackend<Stdout>> {
    fn render(&mut self, draw: &mut dyn FnMut(&mut Frame), want_text: bool) -> Result<Option<String>> {
        // Synchronized output: the terminal shows the whole frame at once, never half of it.
        queue!(self.backend_mut(), BeginSynchronizedUpdate)?;
        let text = {
            let frame = self.draw(|frame| draw(frame))?;
            want_text.then(|| dump::text(frame.buffer))
        };
        execute!(self.backend_mut(), EndSynchronizedUpdate)?;
        Ok(text)
    }
}

impl Surface for Terminal<TestBackend> {
    fn render(&mut self, draw: &mut dyn FnMut(&mut Frame), want_text: bool) -> Result<Option<String>> {
        let frame = self.draw(|frame| draw(frame)).map_err(|error| anyhow!("{error}"))?;
        Ok(want_text.then(|| dump::text(frame.buffer)))
    }
}

type Samples = (Vec<u32>, Vec<Option<Sample>>);

pub async fn run(surface: &mut dyn Surface, specs: Vec<AppSpec>, options: RunOptions) -> Result<Value> {
    let _timer = TimerResolution::high();
    let palette = NIGHT;
    let started = Instant::now();
    let mut model = Model::new(&specs, started, options.splash.then_some(SPLASH));

    let (app_tx, mut app_rx) = mpsc::channel(EVENT_BUFFER);
    let processes: Vec<AppProcess> =
        specs.iter().enumerate().map(|(index, spec)| AppProcess::spawn(index, &spec.launch, app_tx.clone())).collect();
    drop(app_tx);
    let pids = Arc::new(Mutex::new(model.pids()));
    let (sample_tx, mut sample_rx) = mpsc::unbounded_channel::<Samples>();
    spawn_sampler(Arc::clone(&pids), sample_tx);
    let (control_tx, mut control_rx) = mpsc::unbounded_channel::<ipc::Request>();
    if let Some(name) = &options.ipc {
        let listener = ipc::listen(name).with_context(|| format!("can't listen on control channel '{name}'"))?;
        tokio::spawn(ipc::serve(listener, control_tx));
    }
    let mut terminal_events: Pin<Box<dyn Stream<Item = io::Result<Event>> + Send>> =
        if options.headless { Box::pin(futures_util::stream::pending()) } else { Box::pin(EventStream::new()) };

    let mut effects: EffectManager<&'static str> = EffectManager::default();
    let mut stats = FrameStats::new(options.process_started, options.bench.is_some());
    let mut dumps: Vec<oneshot::Sender<Value>> = Vec::new();
    let mut next_frame = Some(started);
    let mut last_frame = started;
    let mut last_dump: Option<Instant> = None;
    let mut splash_on = model.in_splash(started);
    let mut quit_at: Option<Instant> = None;
    let instant_quit = options.headless || options.bench.is_some();
    let far = started + Duration::from_secs(365 * 24 * 3600);
    let bench_end = options.bench.map(|length| started + length);

    if splash_on {
        // Only the logo's cells have anything to assemble, so the whole screen is its area.
        effects.add_unique_effect("logo", fx::coalesce((700, Interpolation::QuadOut)));
    }

    loop {
        let mut dirty = false;
        // Keys and control requests first, then the frame, then app output: a flood of output can't hold up a frame,
        // and a slow frame can't hold up keys (MIN_GAP).
        tokio::select! {
            biased;
            event = terminal_events.next() => match event {
                Some(Ok(Event::Key(key))) if key.kind != KeyEventKind::Release => {
                    if press(key, &mut model, &mut effects, &palette, &mut quit_at, instant_quit) {
                        break;
                    }
                    dirty = true;
                }
                Some(Ok(Event::Resize(..))) => dirty = true,
                Some(Ok(_)) => {}
                Some(Err(error)) => return Err(error.into()),
                None => break,
            },
            Some(request) = control_rx.recv() => {
                match control(request, &mut model, &mut effects, &palette, &mut quit_at, &mut dumps) {
                    Control::Continue => dirty = true,
                    Control::Quit => {
                        tokio::time::sleep(ANSWER_GRACE).await;
                        break;
                    }
                }
            }
            _ = sleep_until(next_frame.unwrap_or(far).into()), if next_frame.is_some() => {
                let now = Instant::now();
                if splash_on && !model.in_splash(now) {
                    splash_on = false;
                    effects.add_unique_effect("enter", fx::sweep_in(Motion::LeftToRight, 30, 6, palette.bg, (800, Interpolation::QuadOut)));
                }
                model.glide(now - last_frame);
                let dump_due = options.frame_dump.is_some() && last_dump.is_none_or(|at| now - at >= FRAME_DUMP_EVERY);
                let want_text = dump_due || !dumps.is_empty();
                let fx_elapsed = now - last_frame;
                let draw_started = Instant::now();
                let text = surface.render(&mut |frame| {
                    view::draw(frame, &mut model, &palette, now);
                    let area = frame.area();
                    effects.process_effects(fx_elapsed, frame.buffer_mut(), area);
                    if options.colors == ColorMode::Ansi256 {
                        downsample(frame.buffer_mut());
                    }
                }, want_text)?;
                stats.frame(now, draw_started.elapsed());
                model.fps = stats.fps(now);
                last_frame = now;
                if let Some(text) = text {
                    for reply in dumps.drain(..) {
                        let _ = reply.send(json!({ "ok": true, "frame": text }));
                    }
                    if dump_due && let Some(path) = &options.frame_dump {
                        let _ = std::fs::write(path, &text);
                        last_dump = Some(now);
                    }
                }
                if quit_at.is_some_and(|at| now >= at) {
                    break;
                }
                let animating = model.is_animating(now) || effects.is_running() || quit_at.is_some();
                next_frame = if animating {
                    // Against the planned time, not the wake-up time, so timer overshoot doesn't add up to fewer fps.
                    let planned = next_frame.unwrap_or(now) + FRAME;
                    Some(planned.max(Instant::now() + MIN_GAP))
                } else if options.frame_dump.is_some() && !dump_due {
                    Some(now + FRAME_DUMP_EVERY) // the last change still has to reach the dump file
                } else {
                    None
                };
            }
            _ = sleep_until(bench_end.unwrap_or(far).into()), if bench_end.is_some() => break,
            Some(event) = app_rx.recv() => {
                let now = Instant::now();
                model.apply(event, now);
                for _ in 1..DRAIN {
                    match app_rx.try_recv() {
                        Ok(event) => model.apply(event, now),
                        Err(_) => break,
                    }
                }
                if let Ok(mut shared) = pids.lock() {
                    *shared = model.pids();
                }
                dirty = true;
            }
            Some((sampled, samples)) = sample_rx.recv() => {
                model.apply_samples(&sampled, &samples);
                stats.sample(model.own);
                dirty = true;
            }
        }
        if dirty && next_frame.is_none() {
            next_frame = Some((last_frame + FRAME).max(Instant::now()));
        }
    }
    drop(processes);
    Ok(stats.report(&model))
}

/// Handles a key; true when lazyrunner should stop right away.
fn press(
    key: KeyEvent,
    model: &mut Model,
    effects: &mut EffectManager<&'static str>,
    palette: &Palette,
    quit_at: &mut Option<Instant>,
    instant_quit: bool,
) -> bool {
    match model.key(key) {
        Action::Quit if instant_quit => return true,
        Action::Quit => {
            if quit_at.is_none() {
                effects.add_unique_effect("quit", fx::dissolve((QUIT_FADE.as_millis() as u32, Interpolation::QuadIn)));
                *quit_at = Some(Instant::now() + QUIT_FADE);
            }
        }
        Action::FocusChanged => {
            let column = model.layout.columns.get(model.focus).copied().unwrap_or_default();
            if column.area() > 0 {
                let title = Rect { height: 1, ..column };
                let flash = fx::fade_from_fg(palette.glint, (350, Interpolation::QuadOut)).with_area(title);
                effects.add_unique_effect("focus", flash);
            }
        }
        Action::ZoomChanged => {
            let area = model.layout.columns_area;
            effects.add_unique_effect(
                "zoom",
                fx::fade_from(palette.bg, palette.bg, (220, Interpolation::QuadOut)).with_area(area),
            );
        }
        Action::Redraw | Action::None => {}
    }
    false
}

enum Control {
    Continue,
    Quit,
}

/// Answers a control request: `{"keys": "l z"}`, `{"dump": true}` (answered after the next frame), `{"state": true}`,
/// `{"quit": true}`.
fn control(
    request: ipc::Request,
    model: &mut Model,
    effects: &mut EffectManager<&'static str>,
    palette: &Palette,
    quit_at: &mut Option<Instant>,
    dumps: &mut Vec<oneshot::Sender<Value>>,
) -> Control {
    let body = &request.body;
    if let Some(spec) = body.get("keys").and_then(Value::as_str) {
        let answer = match keys::parse(spec) {
            Ok(keys) => {
                for key in keys {
                    if press(key, model, effects, palette, quit_at, true) {
                        let _ = request.reply.send(json!({ "ok": true, "quit": true }));
                        return Control::Quit;
                    }
                }
                json!({ "ok": true })
            }
            Err(error) => json!({ "ok": false, "error": error }),
        };
        let _ = request.reply.send(answer);
    } else if body.get("dump").is_some() {
        dumps.push(request.reply);
    } else if body.get("state").is_some() {
        let _ = request.reply.send(json!({ "ok": true, "state": model.state() }));
    } else if body.get("quit").is_some() {
        let _ = request.reply.send(json!({ "ok": true }));
        return Control::Quit;
    } else {
        let _ = request.reply.send(json!({ "ok": false, "error": "unknown request" }));
    }
    Control::Continue
}

fn spawn_sampler(pids: Arc<Mutex<Vec<u32>>>, samples: mpsc::UnboundedSender<Samples>) {
    std::thread::spawn(move || {
        let mut sampler = Sampler::new();
        loop {
            let wanted: Vec<u32> = pids.lock().map(|pids| pids.clone()).unwrap_or_default();
            let taken = sampler.sample(&wanted);
            if samples.send((wanted, taken)).is_err() {
                return;
            }
            std::thread::sleep(SAMPLE_EVERY);
        }
    });
}

/// Frame timing and lazyrunner's own CPU and memory, for the header and the benchmark report.
struct FrameStats {
    process_started: Instant,
    /// Keep every frame's timings (benchmarks); otherwise only what the header needs.
    detailed: bool,
    first_frame: Option<Duration>,
    frames: u64,
    started: Instant,
    last: Option<Instant>,
    /// Time between frames drawn back to back (animating), ms.
    intervals: Vec<f32>,
    /// Time to build and write a frame, ms.
    draws: Vec<f32>,
    recent: VecDeque<Instant>,
    /// When each frame was drawn, for the steady-state numbers.
    frame_times: Vec<Instant>,
    cpu: Vec<(Instant, f32)>,
    peak_memory: u64,
}

impl FrameStats {
    fn new(process_started: Instant, detailed: bool) -> Self {
        Self {
            process_started,
            detailed,
            first_frame: None,
            frames: 0,
            started: Instant::now(),
            last: None,
            intervals: Vec::new(),
            draws: Vec::new(),
            recent: VecDeque::new(),
            frame_times: Vec::new(),
            cpu: Vec::new(),
            peak_memory: 0,
        }
    }

    fn frame(&mut self, now: Instant, draw: Duration) {
        let done = Instant::now();
        self.first_frame.get_or_insert(done - self.process_started);
        self.frames += 1;
        self.recent.push_back(now);
        while self.recent.front().is_some_and(|&at| now - at > Duration::from_secs(1)) {
            self.recent.pop_front();
        }
        if !self.detailed {
            return;
        }
        if let Some(last) = self.last {
            let gap = now - last;
            if gap < FRAME * 3 {
                self.intervals.push(gap.as_secs_f32() * 1000.0);
            }
        }
        self.last = Some(now);
        self.draws.push(draw.as_secs_f32() * 1000.0);
        self.frame_times.push(now);
    }

    fn fps(&self, now: Instant) -> f32 {
        self.recent.iter().filter(|&&at| now - at <= Duration::from_secs(1)).count() as f32
    }

    fn sample(&mut self, own: Option<Sample>) {
        if let Some(own) = own
            && self.detailed
        {
            self.cpu.push((Instant::now(), own.cpu));
            self.peak_memory = self.peak_memory.max(own.memory_bytes);
        }
    }

    fn report(&self, model: &Model) -> Value {
        let seconds = self.started.elapsed().as_secs_f32();
        let lines: u64 = model.columns.iter().map(|column| column.output.total()).sum();
        // The first sample has no previous one to measure CPU against.
        let cpu = &self.cpu[self.cpu.len().min(1)..];
        let average = |samples: &[(Instant, f32)]| {
            if samples.is_empty() {
                0.0
            } else {
                samples.iter().map(|(_, cpu)| cpu).sum::<f32>() / samples.len() as f32
            }
        };
        let average_cpu = average(cpu);
        let cores = std::thread::available_parallelism().map_or(1, |n| n.get()) as f32;
        // The second half of the run: startup (splash, compiling, bars) is over by then.
        let half = self.started + self.started.elapsed() / 2;
        let steady_cpu: Vec<(Instant, f32)> = cpu.iter().copied().filter(|(at, _)| *at >= half).collect();
        let steady_frames = self.frame_times.iter().filter(|&&at| at >= half).count();
        json!({
            "seconds": round(seconds),
            "first_frame_ms": round(self.first_frame.unwrap_or_default().as_secs_f32() * 1000.0),
            "frames": self.frames,
            "fps": round(self.frames as f32 / seconds.max(0.001)),
            "frame_interval_ms": percentiles(&self.intervals),
            "draw_ms": percentiles(&self.draws),
            "cpu_percent_of_machine": round(average_cpu),
            "cpu_percent_of_one_core": round(average_cpu * cores),
            "steady": {
                "fps": round(steady_frames as f32 / (seconds / 2.0).max(0.001)),
                "cpu_percent_of_one_core": round(average(&steady_cpu) * cores),
            },
            "memory_mb_peak": round(self.peak_memory as f32 / 1_048_576.0),
            "lines": lines,
            "lines_per_second": round(lines as f32 / seconds.max(0.001)),
        })
    }
}

fn percentiles(values: &[f32]) -> Value {
    if values.is_empty() {
        return json!(null);
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f32::total_cmp);
    let at = |q: f32| round(sorted[((sorted.len() - 1) as f32 * q).round() as usize]);
    json!({ "p50": at(0.5), "p95": at(0.95), "p99": at(0.99), "max": at(1.0) })
}

fn round(value: f32) -> f64 {
    (f64::from(value) * 100.0).round() / 100.0
}

/// Windows wakes sleeping threads every 15.6 ms unless a process asks for better; 60 fps needs 1 ms. Windows 11 also
/// ignores that request while the process's window is hidden or covered (power throttling) unless the process opts out,
/// which would halve the frame rate whenever another window is on top.
pub(crate) struct TimerResolution;

impl TimerResolution {
    pub(crate) fn high() -> Self {
        #[cfg(windows)]
        // SAFETY: both calls only change this process's own timer and throttling settings; the state struct lives on
        // the stack for the call and its size is passed with it; timeBeginPeriod is undone in drop.
        unsafe {
            use windows_sys::Win32::System::Threading::{
                GetCurrentProcess, PROCESS_POWER_THROTTLING_CURRENT_VERSION,
                PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION, PROCESS_POWER_THROTTLING_STATE,
                ProcessPowerThrottling, SetProcessInformation,
            };
            let state = PROCESS_POWER_THROTTLING_STATE {
                Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
                ControlMask: PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION,
                StateMask: 0, // controlled and off: always honour the timer resolution
            };
            SetProcessInformation(
                GetCurrentProcess(),
                ProcessPowerThrottling,
                (&raw const state).cast(),
                size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
            );
            windows_sys::Win32::Media::timeBeginPeriod(1);
        }
        Self
    }
}

impl Drop for TimerResolution {
    fn drop(&mut self) {
        #[cfg(windows)]
        // SAFETY: pairs the timeBeginPeriod call above.
        unsafe {
            windows_sys::Win32::Media::timeEndPeriod(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentiles_pick_from_the_sorted_values() {
        let values: Vec<f32> = (1..=100).map(|n| n as f32).collect();
        assert_eq!(percentiles(&values), json!({ "p50": 51.0, "p95": 95.0, "p99": 99.0, "max": 100.0 }));
        assert_eq!(percentiles(&[]), json!(null));
    }
}
