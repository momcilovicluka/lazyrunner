//! The timeline (`~`): every app's output in one list, in the order it came - each line tagged with its app, coloured
//! by its level as in the columns, and a quiet stretch of more than a few seconds shown as a dim `· 4.2 s ·` row, so
//! bursts stand apart. `j` `k` scroll (the bottom row is the one picked), `G` follows new output again, `Enter` jumps
//! to the picked line in its own column. `/` narrows it to lines holding some words, `app:orders` to an app's lines,
//! `level:warn` to warnings and up; `f` cycles the levels as in the columns. A request's lines are bracketed by a
//! faint rule with its method, path and status - its own lines by RequestId, or the lines it logged while it ran
//! when no other request ran with it.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crossterm::event::KeyEvent;
use lazyrunner_core::output::{Level, LevelFilter, OutputLine};
use lazyrunner_core::run::Controller;
use lazyrunner_core::run::requests::Request;

use super::columns::{contains_ignore_case, fit, line_color};
use super::icons;
use super::seg::{Seg, on, pad, seg};
use super::theme::C;
use super::{Overlay, Ui};

/// A quiet stretch longer than this gets a row of its own.
const GAP: Duration = Duration::from_secs(3);
const NAME_CELLS: usize = 10;
const PAGE: usize = 20;
/// Each app's tag colour, by its column's place.
const TAGS: [C; 6] = [C::Cyan, C::Magenta, C::Yellow, C::Blue, C::Green, C::DarkCyan];
/// What `level:` takes, any start of it: `level:w`, `level:warn`, `level:warnings`.
const LEVELS: [(&str, Level); 4] =
    [("error", Level::Error), ("warning", Level::Warn), ("info", Level::Info), ("debug", Level::Debug)];

/// What the timeline shows: how many rows back from the newest, and what it's narrowed to.
#[derive(Default)]
pub struct View {
    pub scroll: usize,
    /// `/`: words every shown line holds, `app:` and `level:` among them.
    pub query: String,
    pub typing: bool,
    /// `f`: which levels show, as in the columns.
    pub level: LevelFilter,
}

impl View {
    fn narrowed(&self) -> bool {
        !self.query.trim().is_empty() || self.level != LevelFilter::All
    }
}

/// The query taken apart: the apps (any of them), the least level, the words (all of them).
struct Query {
    apps: Vec<String>,
    least: Option<Level>,
    words: Vec<String>,
}

impl Query {
    fn of(text: &str) -> Self {
        let mut query = Self { apps: Vec::new(), least: None, words: Vec::new() };
        for word in text.split_whitespace() {
            if let Some(app) = word.strip_prefix("app:") {
                query.apps.push(app.to_string());
            } else if let Some(level) = word.strip_prefix("level:") {
                let level = level.to_ascii_lowercase();
                let found = LEVELS.iter().find(|(name, _)| name.starts_with(level.trim_end_matches('s')));
                query.least = found.map(|(_, least)| *least).or(query.least);
            } else {
                query.words.push(word.to_string());
            }
        }
        query
    }

    fn admits(&self, view: &View, app: &str, line: &OutputLine) -> bool {
        view.level.admits(line)
            && self.least.is_none_or(|least| line.severity() >= least)
            && (self.apps.is_empty() || self.apps.iter().any(|wanted| contains_ignore_case(app, wanted)))
            && self.words.iter().all(|word| contains_ignore_case(&line.text, word))
    }
}

/// A row: a request's rule above its first line (the request, that line's place), a line (its app, its place among
/// the app's kept lines, its bracket mark), or a quiet stretch.
enum Row {
    Request { request: usize, app: usize, position: usize },
    Line { app: usize, position: usize, mark: &'static str },
    Gap(Duration),
}

/// Which request each line belongs to, by app and line number: its own lines by RequestId, else the lines it logged
/// while it ran when no other request ran with it.
fn owners(controller: &Controller) -> HashMap<(usize, u64), usize> {
    let mut owners = HashMap::new();
    for (index, request) in controller.requests.items.iter().enumerate() {
        let Some(app) = controller.apps.iter().position(|app| app.key == request.app) else { continue };
        let output = &controller.states[app].output;
        if output.is_empty() {
            continue;
        }
        let numbers: Vec<u64> = match (&request.exact, request.logs) {
            (Some(numbers), _) => numbers.clone(),
            (None, Some((first, last))) if !request.mixed => (first.max(output.number_at(0))..=last).collect(),
            _ => continue,
        };
        owners.extend(numbers.into_iter().map(|number| ((app, number), index)));
    }
    owners
}

/// For each line (in the timeline's order): the request whose rule goes above it, and its bracket mark - `│` a
/// request's line, `╰` its last, `┆` another line while one is open.
fn brackets(controller: &Controller, lines: &[(Instant, usize, usize)]) -> Vec<(Option<usize>, &'static str)> {
    let owners = owners(controller);
    let owner: Vec<Option<usize>> = lines
        .iter()
        .map(|(_, app, position)| owners.get(&(*app, controller.states[*app].output.number_at(*position))).copied())
        .collect();
    let mut spans: HashMap<usize, (usize, usize)> = HashMap::new();
    for (at, request) in owner.iter().enumerate() {
        if let Some(request) = request {
            spans.entry(*request).and_modify(|span| span.1 = at).or_insert((at, at));
        }
    }
    let mut opened = vec![0i64; lines.len() + 1];
    for (first, last) in spans.values().filter(|(first, last)| first < last) {
        opened[first + 1] += 1;
        opened[*last] -= 1;
    }
    let mut open = 0;
    let mut marks = Vec::with_capacity(lines.len());
    for (at, owner) in owner.into_iter().enumerate() {
        open += opened[at];
        marks.push(match owner.map(|request| (request, spans[&request])) {
            Some((request, (first, last))) => ((at == first).then_some(request), if at == last { "╰" } else { "│" }),
            None if open > 0 => (None, "┆"),
            None => (None, " "),
        });
    }
    marks
}

/// The lines the view shows, the oldest first, with the quiet stretches between them.
fn rows_of(controller: &Controller, view: &View) -> Vec<Row> {
    let query = Query::of(&view.query);
    let mut lines: Vec<(Instant, usize, usize)> = Vec::new();
    for (app, state) in controller.states.iter().enumerate() {
        let name = controller.apps.get(app).map_or("", |app| app.name.as_str());
        let len = state.output.len();
        let shown = state.output.newest_first(0).enumerate().filter(|(_, line)| query.admits(view, name, line));
        lines.extend(shown.map(|(back, line)| (line.born, app, len - 1 - back)));
    }
    lines.sort_by_key(|(born, app, position)| (*born, *app, *position));
    let brackets = brackets(controller, &lines);
    let mut rows = Vec::with_capacity(lines.len());
    let mut before: Option<Instant> = None;
    for ((born, app, position), (rule, mark)) in lines.into_iter().zip(brackets) {
        if let Some(quiet) = before.map(|before| born.saturating_duration_since(before)).filter(|quiet| *quiet > GAP) {
            rows.push(Row::Gap(quiet));
        }
        before = Some(born);
        if let Some(request) = rule {
            rows.push(Row::Request { request, app, position });
        }
        rows.push(Row::Line { app, position, mark });
    }
    rows
}

/// `~`: the timeline, following new output.
pub fn open(ui: &mut Ui) {
    ui.overlay = Some(Overlay::Timeline(View::default()));
}

/// The picked line: the bottom row's (a rule's: its request's first line), or the nearest line above a quiet stretch
/// there.
fn picked(rows: &[Row], scroll: usize) -> Option<(usize, usize)> {
    rows.iter().rev().skip(scroll).find_map(|row| match row {
        Row::Line { app, position, .. } | Row::Request { app, position, .. } => Some((*app, *position)),
        Row::Gap(_) => None,
    })
}

pub fn key(ui: &mut Ui, controller: &mut Controller, name: &str, key: &KeyEvent) {
    let Some(Overlay::Timeline(view)) = &mut ui.overlay else { return };
    if view.typing {
        match name {
            "Escape" => {
                view.query.clear();
                view.typing = false;
            }
            "Enter" | "Tab" => view.typing = false,
            "Backspace" => {
                view.query.pop();
            }
            _ => view.query.extend(super::keys::typed(key)),
        }
        view.scroll = 0;
        return;
    }
    let rows = rows_of(controller, view);
    let last = rows.len().saturating_sub(1);
    match name {
        "Escape" if view.narrowed() => *view = View::default(),
        "Escape" | "q" | "~" => ui.overlay = None,
        "/" => view.typing = true,
        "f" => (view.level, view.scroll) = (view.level.next(), 0),
        "k" | "UpArrow" => view.scroll = (view.scroll + 1).min(last),
        "j" | "DownArrow" => view.scroll = view.scroll.saturating_sub(1),
        "Ctrl+U" | "PageUp" => view.scroll = (view.scroll + PAGE).min(last),
        "Ctrl+D" | "PageDown" => view.scroll = view.scroll.saturating_sub(PAGE),
        "g" | "Home" | "<" => view.scroll = last,
        "G" | "End" | ">" => view.scroll = 0,
        "Enter" => {
            let picked = picked(&rows, view.scroll);
            ui.overlay = None;
            if let Some((app, position)) = picked {
                let number = controller.states[app].output.number_at(position);
                super::marks::go_to(ui, controller, app, number);
            }
        }
        _ => {}
    }
}

fn header(ui: &Ui, controller: &Controller, view: &View, shown: usize, cells: usize) -> Vec<Seg> {
    let mut segs = vec![on(" ~ Timeline ", C::FocusText, C::FocusBack)];
    if view.narrowed() || view.typing {
        let total: usize = controller.states.iter().map(|state| state.output.len()).sum();
        segs.push(seg(format!("  {shown} of {total} lines"), C::DarkGray));
        if view.level != LevelFilter::All {
            segs.push(seg(format!(" · {}", view.level.label()), C::Yellow));
        }
        if !view.query.is_empty() || view.typing {
            let cursor = if view.typing { ui.cursor() } else { "" };
            segs.push(seg(format!(" · /{}{cursor}", view.query), C::Yellow));
        }
    } else {
        segs.push(seg(format!("  every app's output, in the order it came · {shown} lines"), C::DarkGray));
    }
    let following = if view.scroll == 0 { "following new output" } else { "G follows new output again" };
    segs.push(seg(format!(" · {following}"), C::DarkGray));
    fit(segs, cells)
}

/// A request's rule, above its first line: `╭─ POST /api/invoices · 201 · 34 ms ───`.
fn rule(request: &Request, cells: usize) -> Vec<Seg> {
    let segs = vec![
        seg(format!("╭─ {} {} · ", request.method, request.path), C::DarkGray),
        seg(request.status.to_string(), super::requests::status_color(request.status)),
        seg(format!(" · {:.0} ms ", request.ms), C::DarkGray),
        seg("─".repeat(cells), C::DarkGray),
    ];
    fit(segs, cells)
}

fn hint(view: &View) -> &'static str {
    if view.typing {
        " words, app:name or level:warn · Enter keeps the filter · Esc clears it"
    } else if view.narrowed() {
        " j k scroll · / search · f level · Enter jumps to the picked line in its column · Esc shows every line"
    } else {
        " j k scroll · G follow · / search · f level · Enter jumps to the picked line in its column · Esc closes"
    }
}

pub fn rows(ui: &Ui, controller: &Controller, cells: usize, height: usize) -> Vec<Vec<Seg>> {
    let Some(Overlay::Timeline(view)) = &ui.overlay else { return Vec::new() };
    let all = rows_of(controller, view);
    let shown = all.iter().filter(|row| matches!(row, Row::Line { .. })).count();
    let body = height.saturating_sub(2);
    let scroll = view.scroll.min(all.len().saturating_sub(1));
    let end = all.len().saturating_sub(scroll);
    let start = end.saturating_sub(body);
    let mut out = vec![header(ui, controller, view, shown, cells)];
    if all.is_empty() && view.narrowed() {
        out.push(fit(vec![seg("      no line matches - Esc shows every line", C::DarkGray)], cells));
    }
    for (at, row) in all[start..end].iter().enumerate() {
        let segs = match row {
            Row::Gap(quiet) => {
                out.push(fit(vec![seg(format!("      · {:.1} s ·", quiet.as_secs_f64()), C::DarkGray)], cells));
                continue;
            }
            Row::Request { request, .. } => {
                let Some(request) = controller.requests.items.get(*request) else { continue };
                rule(request, cells)
            }
            Row::Line { app, position, mark } => {
                let state = &controller.states[*app];
                let Some(line) = state.output.newest_first(state.output.len() - 1 - position).next() else {
                    continue;
                };
                let name = controller.apps.get(*app).map_or("", |app| app.name.as_str());
                let icon = controller.apps.get(*app).map_or(String::new(), |found| {
                    icons::app(ui.nerd, found, controller.framework(&found.key)).to_string()
                });
                let tag = format!("{icon} {} ", pad(name, NAME_CELLS));
                let segs = vec![
                    seg(*mark, C::DarkGray),
                    seg(tag, TAGS[app % TAGS.len()]),
                    seg(line.text.clone(), line_color(line)),
                ];
                fit(segs, cells)
            }
        };
        out.push(if start + at + 1 == end && scroll > 0 {
            let text: String = segs.iter().map(|seg| seg.text.as_str()).collect();
            vec![on(text, C::FocusText, C::FocusBack)]
        } else {
            segs
        });
    }
    while out.len() < height.saturating_sub(1) {
        out.push(Vec::new());
    }
    out.push(fit(vec![seg(hint(view), C::DarkGray)], cells));
    out.truncate(height);
    out
}
