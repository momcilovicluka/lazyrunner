//! E: the Problems screen (#98) - every app's errors and warnings, grouped as an editor's Problems panel groups them.
//! An entry (a red or amber line and the indented lines under it) is one occurrence; entries alike but for what
//! changes every time - ids, times, ports, numbers - are one problem (the shape `Alt+d` compares runs by). Each row
//! says how often, in which apps, when last, and whether it's new since its app's last run or gone since its app
//! restarted; the picked one's details have its message, a chart of when it happened and an occurrence's lines, its
//! frames links. `Enter` jumps to the occurrence in its column, `n` `N` step through them, `y` copies it.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use lazyrunner_core::links;
use lazyrunner_core::output::Level;
use lazyrunner_core::run::{Controller, diff, format_duration};

use super::columns::{SPARKS, chunk, contains_ignore_case, fit, linked};
use super::mouse::Target;
use super::seg::{Seg, aim, on, pad, seg};
use super::theme::C;
use super::{Overlay, Ui};

/// What `s` orders the problems by, errors before warnings always.
const SORTS: [&str; 3] = ["count", "latest", "first seen"];
const KEYS: &str = " j k choose · Enter its line · n N its occurrences · s sort · / filter · v errors only · y copy · \
     Esc closes";
/// From this wide the details go beside the list, this wide; else under it.
const BESIDE_FROM: usize = 150;
const DETAIL_WIDTH: usize = 52;
const COUNT_CELLS: usize = 7;
const APPS_CELLS: usize = 24;
const WHEN_CELLS: usize = 11;
const FLAG_CELLS: usize = 8;
/// An occurrence's lines the details show, and the chart's bars.
const MOST_LINES: usize = 14;
const CHART_BARS: usize = 32;
/// A problem that happens again pulses this long; a new one slides in this long, from this far.
const PULSE: Duration = Duration::from_millis(400);
const SLIDE: Duration = Duration::from_millis(200);
const SLIDE_CELLS: f64 = 12.0;

/// One time a problem happened: the app, its first and last line, and when it came.
#[derive(Clone, Debug)]
pub struct Occurrence {
    pub app: usize,
    pub first: u64,
    pub last: u64,
    pub at: Instant,
}

/// Entries alike but for what changes every time, across the apps.
#[derive(Clone, Debug)]
pub struct Problem {
    pub shape: u64,
    pub error: bool,
    /// The newest occurrence's first line.
    pub text: String,
    /// Oldest first.
    pub occurrences: Vec<Occurrence>,
    /// It happened in an app's current run, and that app's last run didn't have it.
    pub new: bool,
    /// Every app it happened in has started again since, and it hasn't come back.
    pub fixed: bool,
}

impl Problem {
    fn newest(&self) -> Option<&Occurrence> {
        self.occurrences.last()
    }

    /// The apps it happened in, the most first: `Orders ×38 · Billing ×1`.
    fn apps(&self, controller: &Controller) -> String {
        let mut counts: Vec<(usize, usize)> = Vec::new();
        for occurrence in &self.occurrences {
            match counts.iter_mut().find(|(app, _)| *app == occurrence.app) {
                Some((_, count)) => *count += 1,
                None => counts.push((occurrence.app, 1)),
            }
        }
        counts.sort_by_key(|(app, count)| (std::cmp::Reverse(*count), *app));
        let name = |app: usize| controller.apps.get(app).map_or("?", |app| app.name.as_str());
        counts.iter().map(|(app, count)| format!("{} ×{count}", name(*app))).collect::<Vec<_>>().join(" · ")
    }
}

/// The problems as last worked out, and what from: each app's last line and run.
#[derive(Debug, Default)]
pub struct Cache {
    from: Vec<(u64, u64)>,
    pub list: Vec<Problem>,
}

/// The screen: the picked problem and occurrence (back from the newest), the order, the filter and whether it's
/// being typed, errors only; each problem's count as last drawn and when it rose, and when it first showed.
#[derive(Debug)]
pub struct View {
    pub index: usize,
    pub occurrence: usize,
    pub sort: usize,
    pub filter: String,
    pub typing: bool,
    pub errors_only: bool,
    counts: HashMap<u64, (usize, Option<Instant>)>,
    appeared: HashMap<u64, Instant>,
    opened: Instant,
}

/// Works the problems out again when an app's output or run changed since.
pub fn refresh(ui: &mut Ui, controller: &Controller) {
    let from: Vec<(u64, u64)> = controller.states.iter().map(|state| (state.output.total(), state.run_from)).collect();
    if from != ui.problems.from {
        ui.problems = Cache { list: gather(controller), from };
    }
}

fn gather(controller: &Controller) -> Vec<Problem> {
    let mut found: HashMap<u64, Problem> = HashMap::new();
    for (app, state) in controller.states.iter().enumerate() {
        let lines = diff::numbered(&state.output, 0);
        let Some(&(oldest, _)) = lines.first() else { continue };
        let known: HashSet<u64> =
            state.previous.iter().flat_map(|run| run.problems.iter().map(|problem| problem.shape)).collect();
        for block in diff::blocks(&lines) {
            let Some(&(_, head)) = lines.get((block.first - oldest) as usize) else { continue };
            let current = block.first >= state.run_from;
            let problem = found.entry(block.shape).or_insert_with(|| Problem {
                shape: block.shape,
                error: block.level == Level::Error,
                text: String::new(),
                occurrences: Vec::new(),
                new: false,
                fixed: true,
            });
            problem.occurrences.push(Occurrence { app, first: block.first, last: block.last, at: head.born });
            problem.text = head.text.trim().to_string();
            problem.new |= current && state.previous.is_some() && !known.contains(&block.shape);
            problem.fixed &= state.run_from > 0 && !current;
        }
    }
    let mut list: Vec<Problem> = found.into_values().collect();
    for problem in &mut list {
        problem.occurrences.sort_by_key(|occurrence| occurrence.at);
    }
    list
}

/// The shapes of the problems there are now (the panel's chip counts the new ones since the screen was open).
pub fn shapes(ui: &Ui) -> HashSet<u64> {
    ui.problems.list.iter().map(|problem| problem.shape).collect()
}

/// The problems there are that `seen` didn't have: how many, and whether one is an error.
pub fn news(ui: &Ui, seen: &HashSet<u64>) -> (usize, bool) {
    let fresh = ui.problems.list.iter().filter(|problem| !seen.contains(&problem.shape));
    fresh.fold((0, false), |(count, error), problem| (count + 1, error || problem.error))
}

pub fn open(ui: &mut Ui, controller: &Controller) {
    refresh(ui, controller);
    let view = View {
        index: 0,
        occurrence: 0,
        sort: 0,
        filter: String::new(),
        typing: false,
        errors_only: false,
        counts: HashMap::new(),
        appeared: HashMap::new(),
        opened: ui.now,
    };
    ui.overlay = Some(Overlay::Problems(Box::new(view)));
}

/// Opens it on the first problem `seen` didn't have (the chip's).
pub fn open_on_news(ui: &mut Ui, controller: &Controller, seen: &HashSet<u64>) {
    open(ui, controller);
    let Some(Overlay::Problems(view)) = &ui.overlay else { return };
    let at = shown(&ui.problems.list, view).iter().position(|problem| !seen.contains(&problem.shape));
    if let (Some(at), Some(Overlay::Problems(view))) = (at, &mut ui.overlay) {
        view.index = at;
    }
}

/// The problems the screen lists: filtered, errors first, then by the order picked.
fn shown<'a>(list: &'a [Problem], view: &View) -> Vec<&'a Problem> {
    let mut shown: Vec<&Problem> = list
        .iter()
        .filter(|problem| !view.errors_only || problem.error)
        .filter(|problem| view.filter.is_empty() || contains_ignore_case(&problem.text, &view.filter))
        .collect();
    let last = |problem: &Problem| problem.newest().map(|occurrence| occurrence.at);
    let first = |problem: &Problem| problem.occurrences.first().map(|occurrence| occurrence.at);
    shown.sort_by(|a, b| {
        let order = match view.sort {
            1 => last(b).cmp(&last(a)),
            2 => first(a).cmp(&first(b)),
            _ => b.occurrences.len().cmp(&a.occurrences.len()),
        };
        b.error.cmp(&a.error).then(order).then_with(|| a.text.cmp(&b.text))
    });
    shown
}

/// The picked problem, and the occurrence picked in it.
fn picked(ui: &Ui) -> Option<(Problem, Occurrence)> {
    let Some(Overlay::Problems(view)) = &ui.overlay else { return None };
    let problem = shown(&ui.problems.list, view).get(view.index).copied()?.clone();
    let back = view.occurrence.min(problem.occurrences.len().saturating_sub(1));
    let occurrence = problem.occurrences.iter().rev().nth(back)?.clone();
    Some((problem, occurrence))
}

pub fn typing(ui: &Ui) -> bool {
    matches!(&ui.overlay, Some(Overlay::Problems(view)) if view.typing)
}

pub fn key(ui: &mut Ui, controller: &mut Controller, name: &str) {
    refresh(ui, controller);
    if typing(ui) {
        return type_filter(ui, name);
    }
    match name {
        "Escape" | "q" | "E" => ui.overlay = None,
        "Enter" | "Space" => jump(ui, controller),
        "y" => copy(ui, controller),
        _ => pick(ui, name),
    }
}

fn type_filter(ui: &mut Ui, name: &str) {
    let Some(Overlay::Problems(view)) = &mut ui.overlay else { return };
    match name {
        "Escape" => (view.filter, view.typing) = (String::new(), false),
        "Enter" => view.typing = false,
        "Backspace" => {
            view.filter.pop();
        }
        "Space" => view.filter.push(' '),
        _ if name.chars().count() == 1 => view.filter.push_str(name),
        _ => return,
    }
    (view.index, view.occurrence) = (0, 0);
}

/// j k g G move, n N step through the picked problem's occurrences, s sorts, v errors only, / filters.
fn pick(ui: &mut Ui, name: &str) {
    let count = match &ui.overlay {
        Some(Overlay::Problems(view)) => shown(&ui.problems.list, view).len(),
        _ => return,
    };
    let occurrences = picked(ui).map_or(0, |(problem, _)| problem.occurrences.len());
    let Some(Overlay::Problems(view)) = &mut ui.overlay else { return };
    let last = count.saturating_sub(1);
    let index = match name {
        "j" | "DownArrow" => super::keys::wrapped(view.index, 1, count.max(1)),
        "k" | "UpArrow" => super::keys::wrapped(view.index, -1, count.max(1)),
        "Ctrl+D" | "PageDown" => (view.index + 10).min(last),
        "Ctrl+U" | "PageUp" => view.index.saturating_sub(10),
        "g" | "Home" => 0,
        "G" | "End" => last,
        "n" | "N" => {
            let by = if name == "n" { 1 } else { -1 };
            view.occurrence = super::keys::wrapped(view.occurrence, by, occurrences.max(1));
            return;
        }
        "s" => {
            view.sort = (view.sort + 1) % SORTS.len();
            0
        }
        "v" => {
            view.errors_only = !view.errors_only;
            0
        }
        "/" => {
            view.typing = true;
            0
        }
        _ => return,
    };
    if index != view.index {
        view.occurrence = 0;
    }
    view.index = index;
}

/// Enter: the picked occurrence's first line in its column - a jump, `Ctrl+O` comes back.
fn jump(ui: &mut Ui, controller: &mut Controller) {
    let Some((_, occurrence)) = picked(ui) else { return };
    ui.overlay = None;
    super::marks::go_to(ui, controller, occurrence.app, occurrence.first);
}

/// y: the picked problem as Markdown - its message, how often and where, and an occurrence's lines.
fn copy(ui: &mut Ui, controller: &mut Controller) {
    let Some((problem, occurrence)) = picked(ui) else { return };
    let mut text = vec![
        format!("**{}**", problem.text),
        String::new(),
        format!("{} times · {}", problem.occurrences.len(), problem.apps(controller)),
        String::new(),
        "```".to_string(),
    ];
    text.extend(lines_of(controller, &occurrence).into_iter().map(|line| line.trim_end().to_string()));
    text.push("```".to_string());
    let what = format!("the problem ({} times)", problem.occurrences.len());
    super::copy::put(ui.clipboard, controller, &text.join(super::copy::NEWLINE), &what);
}

/// An occurrence's lines still kept, at most `MOST_LINES`.
fn lines_of(controller: &Controller, occurrence: &Occurrence) -> Vec<String> {
    let Some(state) = controller.states.get(occurrence.app) else { return Vec::new() };
    let last = occurrence.last.min(occurrence.first + MOST_LINES as u64 - 1);
    state.output.numbered(occurrence.first, last).map(|line| line.text.clone()).collect()
}

/// A click picks a problem.
pub fn click(ui: &mut Ui, target: Option<Target>) -> bool {
    let Some(Overlay::Problems(view)) = &mut ui.overlay else { return false };
    let Some(Target::Item(at)) = target else { return false };
    if view.index != at {
        (view.index, view.occurrence) = (at, 0);
    }
    true
}

/// Whether a row pulses or slides in.
pub fn animating(ui: &Ui) -> bool {
    let Some(Overlay::Problems(view)) = &ui.overlay else { return false };
    let moving = |at: &Instant, length: Duration| ui.now.saturating_duration_since(*at) < length;
    super::stack::moving(ui)
        && (view.counts.values().any(|(_, rose)| rose.as_ref().is_some_and(|at| moving(at, PULSE)))
            || view.appeared.values().any(|at| moving(at, SLIDE)))
}

/// Notes each problem's count: one that rose pulses, one not seen before slides in (not as the screen opens).
fn note_counts(ui: &mut Ui) {
    let now = ui.now;
    let counts: Vec<(u64, usize)> =
        ui.problems.list.iter().map(|problem| (problem.shape, problem.occurrences.len())).collect();
    let Some(Overlay::Problems(view)) = &mut ui.overlay else { return };
    let opening = now.saturating_duration_since(view.opened) < SLIDE;
    for (shape, count) in counts {
        match view.counts.get_mut(&shape) {
            Some((was, rose)) if count > *was => (*was, *rose) = (count, Some(now)),
            Some(_) => {}
            None => {
                view.counts.insert(shape, (count, None));
                if !opening {
                    view.appeared.insert(shape, now);
                }
            }
        }
    }
}

fn ago(ui: &Ui, at: Instant) -> String {
    format!("{} ago", format_duration(ui.now.saturating_duration_since(at).as_secs_f64()))
}

/// A problem's row: its mark, count (rolling up, lit as it rises), apps, when last, new / fixed?, and message; a new
/// one slides in from the right.
fn row(ui: &mut Ui, controller: &Controller, problem: &Problem, width: usize) -> Vec<Seg> {
    let (mark, color) = if problem.error { (" ✗ ", C::Red) } else { (" ! ", C::Yellow) };
    let (rolled, _) = ui.roll(&format!("problem:{}", problem.shape), problem.occurrences.len() as f64, false, true);
    let last = problem.newest().map_or(String::new(), |occurrence| ago(ui, occurrence.at));
    let (flag, flag_color) = match (problem.new, problem.fixed) {
        (_, true) => ("fixed?", C::Green),
        (true, _) => ("new", C::Cyan),
        _ => ("", C::Gray),
    };
    let mut segs = vec![
        seg(mark, color),
        seg(pad(&format!("{:>5}", rolled.round() as usize), COUNT_CELLS), C::White),
        seg(pad(&problem.apps(controller), APPS_CELLS), C::Gray),
        seg(pad(&last, WHEN_CELLS), C::DarkGray),
        seg(pad(flag, FLAG_CELLS), flag_color),
        seg(problem.text.clone(), if problem.fixed { C::DarkGray } else { color }),
    ];
    let moving = super::stack::moving(ui);
    let since = |at: Instant| ui.now.saturating_duration_since(at);
    let Some(Overlay::Problems(view)) = &ui.overlay else { return fit(segs, width) };
    let rose = view.counts.get(&problem.shape).and_then(|(_, rose)| *rose);
    if rose.is_some_and(|at| moving && since(at) < PULSE) {
        segs[1] = on(segs[1].text.clone(), C::Black, color);
    }
    if let Some(at) = view.appeared.get(&problem.shape).copied().filter(|at| moving && since(*at) < SLIDE) {
        let left = 1.0 - since(at).as_secs_f64() / SLIDE.as_secs_f64();
        segs.insert(0, seg(" ".repeat((left * left * SLIDE_CELLS).round() as usize), C::Gray));
    }
    fit(segs, width)
}

/// The picked problem's details: its message, how often and where, a chart of when, and the picked occurrence's
/// lines - its frames links.
fn details(ui: &mut Ui, controller: &Controller, width: usize) -> Vec<Vec<Seg>> {
    let Some((problem, occurrence)) = picked(ui) else { return Vec::new() };
    let color = if problem.error { C::Red } else { C::Yellow };
    let room = width.saturating_sub(1).max(10);
    let mut out: Vec<Vec<Seg>> = chunk(&problem.text, room).into_iter().map(|piece| vec![seg(piece, color)]).collect();
    let count = problem.occurrences.len();
    let first = problem.occurrences.first().map_or(String::new(), |occurrence| ago(ui, occurrence.at));
    out.push(vec![seg(format!("{count} time{} · first {first}", if count == 1 { "" } else { "s" }), C::Gray)]);
    out.push(vec![seg(problem.apps(controller), C::Gray)]);
    if problem.fixed {
        out.push(vec![seg("fixed? - not since its app started again", C::Green)]);
    } else if problem.new {
        out.push(vec![seg("new since its app's last run", C::Cyan)]);
    }
    out.extend([Vec::new(), vec![seg("WHEN, THIS SESSION", C::DarkCyan)], vec![seg(chart(ui, &problem), color)]]);
    let same = |each: &&Occurrence| each.first == occurrence.first && each.app == occurrence.app;
    let back = problem.occurrences.iter().rev().position(|each| same(&each)).unwrap_or(0);
    let app = controller.apps.get(occurrence.app).map_or("?", |app| app.name.as_str());
    let title = format!("OCCURRENCE {} OF {count} · {app} · {}", count - back, ago(ui, occurrence.at));
    out.extend([Vec::new(), vec![seg(title, C::DarkCyan)]]);
    for text in lines_of(controller, &occurrence) {
        let found = links::find(&text);
        let ids: Vec<usize> = found
            .iter()
            .map(|link| {
                ui.links.push(link.target.clone());
                ui.links.len() - 1
            })
            .collect();
        let mut start = 0;
        for piece in chunk(&text, room) {
            out.push(linked(&piece, start, &found, &ids, C::Gray, occurrence.app));
            start += piece.len();
        }
    }
    out
}

/// When it happened this session, as bars from lazyrunner's start to now - the newest on the right.
fn chart(ui: &Ui, problem: &Problem) -> String {
    let span = ui.now.saturating_duration_since(ui.started).as_secs_f64().max(1.0);
    let mut bars = [0.0_f32; CHART_BARS];
    for occurrence in &problem.occurrences {
        let since = occurrence.at.saturating_duration_since(ui.started).as_secs_f64();
        let at = ((since / span) * CHART_BARS as f64) as usize;
        bars[at.min(CHART_BARS - 1)] += 1.0;
    }
    let most = bars.iter().copied().fold(1.0, f32::max);
    bars.iter().map(|count| SPARKS[((count / most * 7.999).floor() as usize).min(SPARKS.len() - 1)]).collect()
}

fn counted(count: usize, what: &str) -> String {
    format!("{count} {what}{}", if count == 1 { "" } else { "s" })
}

/// The header: how many problems, errors, warnings and new ones, the order, errors only.
fn header(ui: &Ui, view: &View) -> Vec<Seg> {
    let list = &ui.problems.list;
    let errors = list.iter().filter(|problem| problem.error).count();
    let new = list.iter().filter(|problem| problem.new).count();
    let mut segs = vec![
        seg(format!("  {}", counted(list.len(), "problem")), C::White),
        seg(format!(" · {}", counted(errors, "error")), if errors > 0 { C::Red } else { C::DarkGray }),
        seg(format!(" · {}", counted(list.len() - errors, "warning")), C::Yellow),
    ];
    if new > 0 {
        segs.push(seg(format!(" · {new} new"), C::Cyan));
    }
    segs.push(seg(format!(" · by {}", SORTS[view.sort]), C::DarkGray));
    if view.errors_only {
        segs.push(seg(" · errors only", C::DarkGray));
    }
    segs
}

pub fn rows(ui: &mut Ui, controller: &Controller, cells: usize, rows: usize) -> Vec<Vec<Seg>> {
    refresh(ui, controller);
    note_counts(ui);
    let cursor = ui.cursor().to_string();
    let Some(Overlay::Problems(view)) = &ui.overlay else { return Vec::new() };
    let (index, typing, filter) = (view.index, view.typing, view.filter.clone());
    let mut out = vec![header(ui, view)];
    let problems: Vec<Problem> = shown(&ui.problems.list, view).into_iter().cloned().collect();
    out.extend(super::viewbar::keys(KEYS, cells));
    if typing || !filter.is_empty() {
        out.push(vec![seg(format!(" / {filter}{}", if typing { cursor.as_str() } else { "" }), C::Cyan)]);
    }
    out.push(Vec::new());
    let body = rows.saturating_sub(out.len());
    if problems.is_empty() {
        let why =
            if ui.problems.list.is_empty() { "no errors or warnings in the apps' output" } else { "nothing matches" };
        out.push(vec![seg(format!("   {why}"), C::DarkGray)]);
        out.truncate(rows);
        return out.into_iter().map(|line| aim(fit(line, cells), Target::Box)).collect();
    }
    let beside = cells >= BESIDE_FROM;
    let list_width = if beside { cells.saturating_sub(DETAIL_WIDTH + 3) } else { cells };
    let index = index.min(problems.len() - 1);
    let mut table: Vec<Vec<Seg>> = Vec::new();
    for (at, problem) in problems.iter().enumerate() {
        let mut line = row(ui, controller, problem, list_width);
        if at == index {
            for part in line.iter_mut().filter(|part| part.bg.is_none()) {
                (part.fg, part.bg) = (C::FocusText, Some(C::FocusBack));
            }
        }
        table.push(aim(line, Target::Item(at)));
    }
    let details = details(ui, controller, if beside { DETAIL_WIDTH } else { cells.saturating_sub(1) });
    let room = if beside { body } else { table.len().min(body / 2).max(3) };
    let first = index.saturating_sub(room / 2).min(table.len().saturating_sub(room));
    let table: Vec<Vec<Seg>> = table.into_iter().skip(first).take(room).collect();
    if beside {
        for at in 0..body {
            let mut line = fit(table.get(at).cloned().unwrap_or_default(), list_width);
            line.push(seg(" │ ", C::DarkGray));
            line.extend(fit(details.get(at).cloned().unwrap_or_default(), DETAIL_WIDTH));
            out.push(line);
        }
    } else {
        out.extend(table);
        out.push(vec![seg("─".repeat(cells), C::DarkGray)]);
        out.extend(details.into_iter().map(|line| [vec![seg(" ", C::Gray)], line].concat()));
    }
    out.truncate(rows);
    out.into_iter().map(|line| aim(fit(line, cells), Target::Box)).collect()
}
