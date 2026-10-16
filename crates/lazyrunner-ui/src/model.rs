//! What the screen shows, kept apart from drawing it: the apps' columns, focus, zoom and scroll, and what keys do.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use lazyrunner_core::metrics::Sample;
use lazyrunner_core::output::{Output, RateMeter};
use lazyrunner_core::process::{AppEvent, Launch};
use ratatui::layout::Rect;
use serde_json::{Value, json};

/// Lines each column keeps; older ones scroll away.
const SCROLLBACK: usize = 10_000;
/// Samples (one a second) kept for the sparklines.
const HISTORY: usize = 60;
/// How long fresh output glows.
pub const FADE: Duration = Duration::from_millis(700);
const BAR_GLIDE: f32 = 9.0;

pub struct AppSpec {
    pub name: String,
    pub icon: String,
    pub launch: Launch,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Waiting,
    Starting { since: Instant, done: u32, total: u32 },
    Running { up_after: Duration },
    Exited(Option<i32>),
    Failed(String),
}

impl Status {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Waiting => "waiting",
            Self::Starting { .. } => "starting",
            Self::Running { .. } => "running",
            Self::Exited(_) => "exited",
            Self::Failed(_) => "failed",
        }
    }
}

pub struct Column {
    pub name: String,
    pub icon: String,
    pub status: Status,
    pub pid: Option<u32>,
    pub output: Output,
    pub rate: RateMeter,
    pub cpu: VecDeque<f32>,
    pub memory_mb: f32,
    /// Lines scrolled back from the newest; 0 follows the output.
    pub scroll: usize,
    /// The progress bar as drawn: it glides towards the real progress instead of jumping.
    pub bar: f32,
}

impl Column {
    fn new(spec: &AppSpec, now: Instant) -> Self {
        Self {
            name: spec.name.clone(),
            icon: spec.icon.clone(),
            status: Status::Waiting,
            pid: None,
            output: Output::new(SCROLLBACK),
            rate: RateMeter::new(now, HISTORY),
            cpu: VecDeque::with_capacity(HISTORY),
            memory_mb: 0.0,
            scroll: 0,
            bar: 0.0,
        }
    }

    fn line(&mut self, text: &str, now: Instant) {
        self.output.push(text, now);
        self.rate.add(now, 1);
        if self.scroll > 0 {
            self.scroll = (self.scroll + 1).min(self.output.len().saturating_sub(1)); // scrolled back: the view stays put
        }
        let text = text.trim();
        if let Status::Starting { since, .. } = self.status {
            if let Some((done, total)) = compile_step(text) {
                self.status = Status::Starting { since, done, total };
            } else if text.starts_with("ready in ") {
                self.status = Status::Running { up_after: now.saturating_duration_since(since) };
            }
        }
    }

    /// Fraction done while starting, for the bar.
    pub fn progress(&self) -> Option<f32> {
        match self.status {
            Status::Starting { done, total, .. } if total > 0 => Some(done as f32 / total as f32),
            Status::Starting { .. } => Some(0.0),
            _ => None,
        }
    }
}

/// "compiling 3/12 orders" -> (3, 12). The spike's apps say this; P2 brings real readiness detection.
fn compile_step(text: &str) -> Option<(u32, u32)> {
    let rest = text.strip_prefix("compiling ")?;
    let (done, rest) = rest.split_once('/')?;
    let total = rest.split_whitespace().next()?;
    Some((done.parse().ok()?, total.parse().ok()?))
}

#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    None,
    Redraw,
    FocusChanged,
    ZoomChanged,
    Quit,
}

pub struct Model {
    pub columns: Vec<Column>,
    pub focus: usize,
    pub zoom: bool,
    pub started: Instant,
    pub splash_until: Option<Instant>,
    /// lazyrunner's own CPU (% of the machine) and memory, and frames per second, for the header.
    pub own: Option<Sample>,
    pub own_cpu: VecDeque<f32>,
    pub fps: f32,
    /// Where the last frame put things, for effects and paging.
    pub layout: Layout,
}

#[derive(Clone, Debug, Default)]
pub struct Layout {
    pub screen: Rect,
    pub columns_area: Rect,
    /// Indexed like `columns`; empty for columns not on screen (zoomed out of view).
    pub columns: Vec<Rect>,
    pub body_rows: u16,
}

impl Model {
    pub fn new(specs: &[AppSpec], now: Instant, splash: Option<Duration>) -> Self {
        Self {
            columns: specs.iter().map(|spec| Column::new(spec, now)).collect(),
            focus: 0,
            zoom: false,
            started: now,
            splash_until: splash.map(|length| now + length),
            own: None,
            own_cpu: VecDeque::with_capacity(HISTORY),
            fps: 0.0,
            layout: Layout::default(),
        }
    }

    pub fn apply(&mut self, event: AppEvent, now: Instant) {
        match event {
            AppEvent::Started { app, pid } => {
                if let Some(column) = self.columns.get_mut(app) {
                    column.pid = pid;
                    column.status = Status::Starting { since: now, done: 0, total: 0 };
                }
            }
            AppEvent::Line { app, text, .. } => {
                if let Some(column) = self.columns.get_mut(app) {
                    column.line(&text, now);
                }
            }
            AppEvent::Exited { app, code } => {
                if let Some(column) = self.columns.get_mut(app) {
                    column.status = Status::Exited(code);
                    column.pid = None;
                }
            }
            AppEvent::Failed { app, error } => {
                if let Some(column) = self.columns.get_mut(app) {
                    column.status = Status::Failed(error);
                    column.pid = None;
                }
            }
        }
    }

    /// The pids to sample: lazyrunner itself first, then each app's (0 when it has none).
    pub fn pids(&self) -> Vec<u32> {
        std::iter::once(std::process::id()).chain(self.columns.iter().map(|column| column.pid.unwrap_or(0))).collect()
    }

    pub fn apply_samples(&mut self, pids: &[u32], samples: &[Option<Sample>]) {
        let mut samples = pids.iter().zip(samples);
        if let Some((_, own)) = samples.next() {
            self.own = *own;
            push_bounded(&mut self.own_cpu, own.map_or(0.0, |sample| sample.cpu));
        }
        for (column, (&pid, sample)) in self.columns.iter_mut().zip(samples) {
            if column.pid != Some(pid) {
                continue; // restarted since the sample was asked for
            }
            if let Some(sample) = sample {
                push_bounded(&mut column.cpu, sample.cpu);
                column.memory_mb = sample.memory_bytes as f32 / 1_048_576.0;
            }
        }
    }

    pub fn key(&mut self, key: KeyEvent) -> Action {
        let count = self.columns.len();
        if count == 0 {
            return if matches!(key.code, KeyCode::Char('q')) { Action::Quit } else { Action::None };
        }
        let page = usize::from(self.layout.body_rows.max(2) - 1);
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('c') if ctrl => Action::Quit,
            KeyCode::Char('q') => Action::Quit,
            KeyCode::Esc if self.zoom => self.set_zoom(false),
            KeyCode::Char('h') | KeyCode::Left | KeyCode::BackTab => self.set_focus((self.focus + count - 1) % count),
            KeyCode::Char('l') | KeyCode::Right | KeyCode::Tab => self.set_focus((self.focus + 1) % count),
            KeyCode::Char(digit @ '1'..='9') => {
                let index = digit as usize - '1' as usize;
                if index < count { self.set_focus(index) } else { Action::None }
            }
            KeyCode::Char('z') | KeyCode::Enter => self.set_zoom(!self.zoom),
            KeyCode::Char('k') | KeyCode::Up => self.scroll_by(1),
            KeyCode::Char('j') | KeyCode::Down => self.scroll_by(-1),
            KeyCode::PageUp => self.scroll_by(page as isize),
            KeyCode::PageDown => self.scroll_by(-(page as isize)),
            KeyCode::Char('g') | KeyCode::Home => self.scroll_by(isize::MAX / 2),
            KeyCode::Char('G') | KeyCode::End => self.scroll_by(isize::MIN / 2),
            _ => Action::None,
        }
    }

    fn set_focus(&mut self, index: usize) -> Action {
        if index == self.focus {
            return Action::None;
        }
        self.focus = index;
        Action::FocusChanged
    }

    fn set_zoom(&mut self, zoom: bool) -> Action {
        self.zoom = zoom;
        Action::ZoomChanged
    }

    fn scroll_by(&mut self, delta: isize) -> Action {
        let column = &mut self.columns[self.focus];
        let max = column.output.len().saturating_sub(1);
        column.scroll = column.scroll.saturating_add_signed(delta).min(max);
        Action::Redraw
    }

    pub fn in_splash(&self, now: Instant) -> bool {
        self.splash_until.is_some_and(|until| now < until)
    }

    /// Whether the next frame differs from this one even if nothing happens: the splash, spinners and bars, fresh
    /// output still glowing, a bar still gliding.
    pub fn is_animating(&self, now: Instant) -> bool {
        self.in_splash(now)
            || self.columns.iter().any(|column| {
                let glowing = column.output.last_line_at().is_some_and(|at| now.saturating_duration_since(at) < FADE);
                matches!(column.status, Status::Starting { .. }) || (glowing && column.scroll == 0)
            })
    }

    /// Moves the bars towards their real progress; `dt` is the time since the last frame.
    pub fn glide(&mut self, dt: Duration) {
        let step = 1.0 - (-dt.as_secs_f32() * BAR_GLIDE).exp();
        for column in &mut self.columns {
            match column.progress() {
                Some(target) if target >= column.bar => column.bar += (target - column.bar) * step,
                Some(target) => column.bar = target, // restarted: no gliding backwards
                None => column.bar = 0.0,
            }
        }
    }

    pub fn state(&self) -> Value {
        json!({
            "focus": self.focus,
            "zoom": self.zoom,
            "columns": self.columns.iter().map(|column| json!({
                "name": column.name,
                "status": column.status.name(),
                "lines": column.output.total(),
                "scroll": column.scroll,
            })).collect::<Vec<_>>(),
        })
    }
}

fn push_bounded(values: &mut VecDeque<f32>, value: f32) {
    if values.len() == HISTORY {
        values.pop_front();
    }
    values.push_back(value);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(apps: usize) -> Model {
        let specs: Vec<AppSpec> = (0..apps)
            .map(|i| AppSpec {
                name: format!("App{i}"),
                icon: "◆".into(),
                launch: Launch { program: "x".into(), args: vec![], dir: None },
            })
            .collect();
        Model::new(&specs, Instant::now(), None)
    }

    fn press(model: &mut Model, spec: &str) -> Vec<Action> {
        crate::keys::parse(spec).unwrap().into_iter().map(|key| model.key(key)).collect()
    }

    fn line(app: usize, text: &str) -> AppEvent {
        AppEvent::Line { app, text: text.into(), stderr: false }
    }

    #[test]
    fn focus_moves_wraps_and_jumps() {
        let mut model = model(3);
        assert_eq!(press(&mut model, "l"), [Action::FocusChanged]);
        assert_eq!(model.focus, 1);
        press(&mut model, "l l");
        assert_eq!(model.focus, 0, "wraps to the first");
        press(&mut model, "h");
        assert_eq!(model.focus, 2, "and back to the last");
        assert_eq!(press(&mut model, "1 9"), [Action::FocusChanged, Action::None]);
        assert_eq!(model.focus, 0);
    }

    #[test]
    fn zoom_toggles_and_esc_leaves_it() {
        let mut model = model(2);
        press(&mut model, "z");
        assert!(model.zoom);
        press(&mut model, "Esc");
        assert!(!model.zoom);
        assert_eq!(press(&mut model, "q"), [Action::Quit]);
        assert_eq!(press(&mut model, "Ctrl+c"), [Action::Quit]);
    }

    #[test]
    fn a_column_goes_from_starting_through_progress_to_running() {
        let mut model = model(1);
        let now = Instant::now();
        model.apply(AppEvent::Started { app: 0, pid: Some(42) }, now);
        model.apply(line(0, "  compiling 3/12 orders\n"), now);
        assert_eq!(model.columns[0].status, Status::Starting { since: now, done: 3, total: 12 });
        assert_eq!(model.columns[0].progress(), Some(0.25));
        model.apply(line(0, "ready in 1500 ms"), now + Duration::from_millis(1500));
        assert_eq!(model.columns[0].status, Status::Running { up_after: Duration::from_millis(1500) });
        model.apply(AppEvent::Exited { app: 0, code: Some(1) }, now);
        assert_eq!(model.columns[0].status, Status::Exited(Some(1)));
    }

    #[test]
    fn scrolled_back_the_view_stays_on_the_same_lines() {
        let mut model = model(1);
        let now = Instant::now();
        for i in 0..20 {
            model.apply(line(0, &format!("line {i}")), now);
        }
        press(&mut model, "k k k");
        assert_eq!(model.columns[0].scroll, 3);
        model.apply(line(0, "new"), now);
        assert_eq!(model.columns[0].scroll, 4, "the new line pushes the view back by one");
        press(&mut model, "G");
        assert_eq!(model.columns[0].scroll, 0);
        press(&mut model, "g");
        assert_eq!(model.columns[0].scroll, 20, "the oldest line");
    }

    #[test]
    fn bars_glide_forward_but_never_back() {
        let mut model = model(1);
        let now = Instant::now();
        model.apply(AppEvent::Started { app: 0, pid: Some(1) }, now);
        model.apply(line(0, "compiling 6/12 x"), now);
        model.glide(Duration::from_millis(16));
        let first = model.columns[0].bar;
        assert!(first > 0.0 && first < 0.5);
        for _ in 0..200 {
            model.glide(Duration::from_millis(16));
        }
        assert!((model.columns[0].bar - 0.5).abs() < 0.01);
        model.apply(line(0, "compiling 1/12 x"), now);
        model.glide(Duration::from_millis(16));
        assert!((model.columns[0].bar - 1.0 / 12.0).abs() < 0.001, "a restart resets the bar");
    }

    #[test]
    fn idle_columns_stop_animating() {
        let mut model = model(1);
        let now = Instant::now();
        model.apply(AppEvent::Started { app: 0, pid: Some(1) }, now);
        assert!(model.is_animating(now), "starting: spinner and bar");
        model.apply(line(0, "ready in 5 ms"), now);
        assert!(model.is_animating(now), "the ready line still glows");
        assert!(!model.is_animating(now + FADE), "then nothing moves");
    }

    #[test]
    fn samples_land_on_the_right_columns() {
        let mut model = model(2);
        let now = Instant::now();
        model.apply(AppEvent::Started { app: 1, pid: Some(77) }, now);
        let pids = model.pids();
        assert_eq!(pids[1..], [0, 77]);
        let sample = |cpu| Some(Sample { cpu, memory_bytes: 2 * 1_048_576 });
        model.apply_samples(&pids, &[sample(1.5), None, sample(12.0)]);
        assert_eq!(model.own.unwrap().cpu, 1.5);
        assert!(model.columns[0].cpu.is_empty());
        assert_eq!(model.columns[1].cpu.back(), Some(&12.0));
        assert_eq!(model.columns[1].memory_mb, 2.0);
    }
}
