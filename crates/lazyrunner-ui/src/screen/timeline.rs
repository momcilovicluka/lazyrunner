//! The timeline (`~`): every app's output in one list, in the order it came - each line tagged with its app, coloured
//! by its level as in the columns, and a quiet stretch of more than a few seconds shown as a dim `· 4.2 s ·` row, so
//! bursts stand apart. `j` `k` scroll (the bottom row is the one picked), `G` follows new output again, `Enter` jumps
//! to the picked line in its own column. `/` narrows it to lines holding some words, `app:orders` to an app's lines,
//! `level:warn` to warnings and up; `f` cycles the levels as in the columns.

use std::time::{Duration, Instant};

use crossterm::event::KeyEvent;
use lazyrunner_core::output::{Level, LevelFilter, OutputLine};
use lazyrunner_core::run::Controller;

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

/// A row: a line (its app, its place among the app's kept lines), or a quiet stretch.
enum Row {
    Line { app: usize, position: usize },
    Gap(Duration),
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
    let mut rows = Vec::with_capacity(lines.len());
    let mut before: Option<Instant> = None;
    for (born, app, position) in lines {
        if let Some(quiet) = before.map(|before| born.saturating_duration_since(before)).filter(|quiet| *quiet > GAP) {
            rows.push(Row::Gap(quiet));
        }
        before = Some(born);
        rows.push(Row::Line { app, position });
    }
    rows
}

/// `~`: the timeline, following new output.
pub fn open(ui: &mut Ui) {
    ui.overlay = Some(Overlay::Timeline(View::default()));
}

/// The picked line: the bottom row's, or the nearest line above a quiet stretch there.
fn picked(rows: &[Row], scroll: usize) -> Option<(usize, usize)> {
    rows.iter().rev().skip(scroll).find_map(|row| match row {
        Row::Line { app, position } => Some((*app, *position)),
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
        let bottom = start + at + 1 == end;
        out.push(match row {
            Row::Gap(quiet) => fit(vec![seg(format!("      · {:.1} s ·", quiet.as_secs_f64()), C::DarkGray)], cells),
            Row::Line { app, position } => {
                let state = &controller.states[*app];
                let Some(line) = state.output.newest_first(state.output.len() - 1 - position).next() else {
                    continue;
                };
                let name = controller.apps.get(*app).map_or("", |app| app.name.as_str());
                let icon = controller.apps.get(*app).map_or(String::new(), |found| {
                    icons::app(ui.nerd, found, controller.framework(&found.key)).to_string()
                });
                let tag = format!(" {icon} {} ", pad(name, NAME_CELLS));
                if bottom && scroll > 0 {
                    vec![on(pad(&format!("{tag}{}", line.text), cells), C::FocusText, C::FocusBack)]
                } else {
                    fit(vec![seg(tag, TAGS[app % TAGS.len()]), seg(line.text.clone(), line_color(line))], cells)
                }
            }
        });
    }
    while out.len() < height.saturating_sub(1) {
        out.push(Vec::new());
    }
    out.push(fit(vec![seg(hint(view), C::DarkGray)], cells));
    out.truncate(height);
    out
}
