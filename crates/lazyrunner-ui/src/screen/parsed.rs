//! A column's parsed view (#27, `v` switches it with the raw one, per app): a row per log entry - its level as a
//! filled capsule, the time it came, its category trimmed to its last parts (a thin `│` under the same one), its
//! message - in its level's colour. An exception folds to its first lines, its first frame in the app's own code and
//! `+18 frames` (`Enter` unfolds it, a `╭ │ ╰` bracket beside it); an EF Core command is its time, a bar against the
//! slowest of its endpoint's, and its SQL on one row; a line repeated is one row `×12`, lit a moment as the count
//! rises. The entries come from `lazyrunner_core::run::entries`, built as the lines arrive; search, marks, links,
//! copy and the rail go by them.

use std::time::Duration;

use lazyrunner_core::links;
use lazyrunner_core::output::{ColumnView, Level, OutputLine};
use lazyrunner_core::run::Controller;
use lazyrunner_core::run::app::AppState;
use lazyrunner_core::run::browser;
use lazyrunner_core::run::entries::{self, Entry};
use lazyrunner_core::run::requests::Request;

use super::columns::{self, Shown};
use super::diff::View;
use super::icons::{self, Icon};
use super::motion::{self, Spot};
use super::mouse::Target;
use super::seg::{self, Seg, on, seg};
use super::theme::C;
use super::{Search, Ui};

/// From this wide a row says the time its entry came, and from this wide its category, in at most so many cells.
const TIME_FROM: usize = 44;
const CATEGORY_FROM: usize = 28;
const CATEGORY_CELLS: usize = 18;
/// The level's capsule and the space after it; the lines under an entry start after it.
const BADGE_CELLS: usize = 6;
/// An EF Core command's bar, and from how long it is slow.
const SQL_BAR: usize = 4;
const SLOW_SQL_MS: f64 = 100.0;
/// A folded exception's lines before its frame: its type and message.
const FOLDED_DETAILS: usize = 1;
/// What `v` did stays in the column's title this long.
const SAID: Duration = Duration::from_secs(2);
/// A repeat's count is bright this much of its flash, then fades.
const BUMP_BRIGHT: f64 = 0.5;

/// What a column's rows are drawn into: its text's cells, its rows, how far it's scrolled back, its search and its
/// diff mode.
pub struct Frame<'a> {
    pub cells: usize,
    pub rows: usize,
    pub scroll: usize,
    pub search: Option<&'a Search>,
    pub diff: Option<&'a View>,
}

/// The entries a parsed column shows, oldest first: those its level filter (f) lets through, and while its search
/// filters (Tab), only the matching ones, with an entry either side.
pub fn shown<'a>(state: &'a AppState, search: Option<&Search>) -> Vec<Shown<'a>> {
    let output = &state.output;
    let kept: Vec<Shown<'a>> = output
        .entries()
        .iter()
        .filter(|entry| state.filter.admits_level(entry.level))
        .filter_map(|entry| {
            let position = output.position_of(entry.first)?;
            Some(Shown { position, line: output.at(position)?, context: false, entry: Some(entry) })
        })
        .collect();
    let filtering = search.filter(|search| search.filter && !search.query.is_empty() && search.key == state.key);
    let Some(search) = filtering else { return kept };
    let hits: Vec<bool> = kept.iter().map(|item| item.matches(state, search)).collect();
    kept.into_iter()
        .enumerate()
        .filter_map(|(at, mut item)| {
            let near = (at > 0 && hits[at - 1]) || hits.get(at + 1).copied().unwrap_or(false);
            item.context = !hits[at];
            (hits[at] || near).then_some(item)
        })
        .collect()
}

/// An entry's lines still kept, oldest first.
pub fn lines<'a>(state: &'a AppState, entry: &Entry) -> impl Iterator<Item = &'a OutputLine> + 'a {
    let output = &state.output;
    (entry.first..=entry.last).filter_map(move |number| output.position_of(number).and_then(|at| output.at(at)))
}

/// The capsule's colour for a level: the theme's level colours.
fn level_color(level: Level) -> C {
    match level {
        Level::Error => C::Red,
        Level::Warn => C::Yellow,
        Level::Info => C::Cyan,
        Level::Debug => C::DarkGray,
    }
}

/// A row's text in its entry's level colour; a plain line keeps its own (lazyrunner's notes, a dev server's green
/// "compiled"), as an info entry keeps a green message.
fn text_color(entry: &Entry, message: Option<&OutputLine>) -> C {
    let own = message.map(columns::line_color);
    match entry.level {
        _ if entry.category.is_none() && entry.stamp.is_none() => own.unwrap_or(C::Gray),
        Level::Error => C::Red,
        Level::Warn => C::Yellow,
        Level::Debug => C::DarkGray,
        Level::Info if own == Some(C::Green) => C::Green,
        Level::Info => C::Gray,
    }
}

/// The line its message is on: the header's own, or the first indented line after one that had none.
fn message_number(state: &AppState, entry: &Entry) -> u64 {
    let head = lines(state, entry).next().map(|line| line.text.trim_end());
    let on_head = head.is_some_and(|head| head.ends_with(entry.message.as_str()));
    if on_head || entry.lines() == 1 { entry.first } else { entry.first + 1 }
}

/// The lines under an entry's first row: the rest of it but its message.
fn detail_lines<'a>(state: &'a AppState, entry: &Entry) -> Vec<&'a OutputLine> {
    let message = message_number(state, entry);
    let output = &state.output;
    (entry.first + 1..=entry.last)
        .filter(|number| *number != message)
        .filter_map(|number| output.position_of(number).and_then(|at| output.at(at)))
        .collect()
}

/// What draws a column's rows: the column, its app, and where the links on them go.
struct Drawing<'a, 'f> {
    ui: &'a Ui,
    controller: &'a Controller,
    state: &'a AppState,
    index: usize,
    frame: &'a Frame<'a>,
    found: &'f mut Vec<links::Target>,
    base: usize,
    /// The wall clock now, for the times entries came.
    wall: jiff::ZonedDateTime,
    /// The slowest EF Core command kept, for the bars of one outside any request.
    slowest: f64,
}

/// The rows of a parsed column, newest first (`frame.rows` of them at most), `frame.scroll` entries back.
pub fn rows(
    ui: &Ui,
    controller: &Controller,
    index: usize,
    list: &[Shown],
    frame: &Frame,
    found: &mut Vec<links::Target>,
    base: usize,
) -> Vec<Vec<Seg>> {
    let state = &controller.states[index];
    let slowest =
        state.output.entries().iter().filter_map(|entry| entry.sql.as_ref()).map(|sql| sql.ms).fold(0.0, f64::max);
    let mut drawing =
        Drawing { ui, controller, state, index, frame, found, base, wall: jiff::ZonedDateTime::now(), slowest };
    let mut result: Vec<Vec<Seg>> = Vec::with_capacity(frame.rows);
    for (at, item) in list.iter().enumerate().rev().skip(frame.scroll) {
        if result.len() >= frame.rows {
            break;
        }
        let Some(entry) = item.entry else { continue };
        let before = at.checked_sub(1).and_then(|before| list[before].entry);
        let rows = drawing.entry(item, entry, before);
        for row in rows.into_iter().rev() {
            if result.len() >= frame.rows {
                break;
            }
            result.push(columns::fit(row, frame.cells));
        }
    }
    result
}

impl Drawing<'_, '_> {
    /// An entry's rows, top to bottom.
    fn entry(&mut self, item: &Shown, entry: &Entry, before: Option<&Entry>) -> Vec<Vec<Seg>> {
        let open = entry.folds() && self.ui.unfolded.contains(&(self.state.key.clone(), entry.first));
        let color = level_color(entry.level);
        let details = if entry.folds() { detail_lines(self.state, entry) } else { Vec::new() };
        let mut rows = vec![self.head(item, entry, before, open)];
        // Just folded, its lines still wipe up into the fold.
        let (app, now) = (self.index, self.ui.now);
        let folding = (entry.folds() && !open)
            .then(|| self.ui.motion.lit(&Spot::Folded { app, line: entry.first }, now))
            .flatten();
        if open || folding.is_some() {
            let wiping = self.ui.motion.lit(&Spot::Unfolded { app, line: entry.first }, now);
            let revealed = match folding {
                Some(left) => (left * details.len() as f64).ceil() as usize,
                None => wiping.map_or(details.len(), |left| ((1.0 - left) * details.len() as f64).ceil() as usize),
            };
            let revealed = revealed.min(details.len());
            rows.extend(details.iter().take(revealed).map(|line| self.detail(line, entry, "│")));
            let left = details.len() - revealed;
            if left > 0 {
                rows.push(self.fold_row(entry, left, entry.frames > 0, "╰"));
            } else if let Some(last) = rows.last_mut() {
                last[0] = seg("╰", color);
            }
            if rows[0][0].text == " " {
                rows[0][0] = seg("╭", color);
            }
            return rows;
        }
        if entry.frames > 0 && entry.folds() {
            let (frames, others): (Vec<&OutputLine>, Vec<&OutputLine>) =
                details.iter().partition(|line| entries::is_frame(&line.text));
            let shown_details: Vec<&OutputLine> = others.into_iter().take(FOLDED_DETAILS).collect();
            let own = frames.iter().find(|line| self.own_frame(line)).copied();
            rows.extend(shown_details.iter().map(|line| self.detail(line, entry, " ")));
            rows.extend(own.map(|line| self.detail(line, entry, " ")));
            let hidden = details.len() - shown_details.len() - usize::from(own.is_some());
            if hidden > 0 {
                let only_frames = entry.details == shown_details.len();
                rows.push(self.fold_row(entry, hidden, only_frames, " "));
            }
        }
        rows
    }

    /// Whether a frame is the app's own: a link to its file, not a library's.
    fn own_frame(&self, line: &OutputLine) -> bool {
        let frames = self.state.frames();
        !links::library_frame(&line.text, frames) && !links::find_with(&line.text, frames).is_empty()
    }

    /// An entry's first row: its gutter (a mark, or the bracket's top), its level, time and category, its message.
    fn head(&mut self, item: &Shown, entry: &Entry, before: Option<&Entry>, open: bool) -> Vec<Seg> {
        let (state, cells) = (self.state, self.frame.cells);
        let color = level_color(entry.level);
        let gutter = match item.bookmarked(state) {
            Some(number) => seg("▌", super::bookmarks::edge(self.ui, self.index, number)),
            None if item.marked(state) => seg("▌", C::Magenta),
            None => seg(" ", color),
        };
        let label = entries::level_name(entry.level);
        let label_fg = if entry.level == Level::Debug { C::Gray } else { C::Black };
        let mut row = vec![gutter, seg("▐", color), on(label, label_fg, color), seg("▌ ", color)];
        if cells >= TIME_FROM {
            row.push(seg(format!("{} ", self.time(entry)), C::DarkGray));
        }
        let category_cells = if cells >= CATEGORY_FROM { (cells / 4).min(CATEGORY_CELLS) } else { 0 };
        if let Some(category) = entry.category.as_deref().filter(|_| category_cells > 0) {
            let same = before.is_some_and(|before| before.category.as_deref() == Some(category));
            let short = if category == browser::CATEGORY {
                format!("{} {category}", icons::icon(self.ui.nerd, Icon::Globe))
            } else {
                entries::short_category(category)
            };
            let text = if same { seg::pad("│", category_cells) } else { seg::ellipsis(&short, category_cells) };
            let target = Target::Entry { column: self.index, line: entry.first };
            row.push(seg(text, if same { C::DarkGray } else { C::DarkCyan }).at(target));
            row.push(seg(" ", C::Gray));
        }
        let suffix = self.suffix(entry, open);
        let room = cells.saturating_sub(seg::width(&row) + seg::width(&suffix));
        row.extend(self.message(item, entry, room));
        row.extend(suffix);
        row
    }

    /// The time an entry came: the one its line says (Serilog's), else when lazyrunner saw it.
    fn time(&self, entry: &Entry) -> String {
        if let Some(stamp) = &entry.stamp {
            return stamp.clone();
        }
        let born = lines(self.state, entry).next().map_or(self.ui.now, |line| line.born);
        let ago = self.ui.now.saturating_duration_since(born);
        (self.wall.clone() - ago).strftime(lazyrunner_core::time::CLOCK).to_string()
    }

    /// What ends a first row: the fold of an entry with more lines than its rows show (an exception's has a row of
    /// its own), and how many times it came.
    fn suffix(&self, entry: &Entry, open: bool) -> Vec<Seg> {
        let mut suffix = Vec::new();
        let nerd = self.ui.nerd;
        if open {
            suffix.push(seg(format!(" {}", icons::icon(nerd, Icon::Fold { open: true })), C::DarkGray));
        } else if entry.folds() && entry.frames == 0 {
            let fold = icons::icon(nerd, Icon::Fold { open: false });
            let target = Target::Entry { column: self.index, line: entry.first };
            suffix.push(seg(format!(" {fold} +{}", entry.details), C::DarkGray).at(target));
        }
        if entry.repeats > 1 {
            let text = format!(" ×{}", entry.repeats);
            suffix.push(match self.bump(entry) {
                Some(lit) if lit >= BUMP_BRIGHT => on(text, C::Black, C::White),
                Some(_) => seg(text, C::White),
                None => seg(text, C::Gray),
            });
        }
        suffix
    }

    /// How lit a repeat's count is as it rises: the flash from its newest line's coming (none with reduced motion).
    fn bump(&self, entry: &Entry) -> Option<f64> {
        let (ui, output) = (self.ui, &self.state.output);
        if !ui.motion.enabled || ui.reduced_motion {
            return None;
        }
        let newest = output.position_of(entry.last).and_then(|at| output.at(at))?;
        let done = ui.now.saturating_duration_since(newest.born).as_secs_f64() / motion::FLASH.as_secs_f64();
        (done < 1.0).then_some(1.0 - done)
    }

    /// An entry's message, `room` cells at most: its links clickable, or - an EF Core command - its time, its bar and
    /// its SQL. A search's match, copied or read lines, the line that made the app up stand out as in the raw view.
    fn message(&mut self, item: &Shown, entry: &Entry, room: usize) -> Vec<Seg> {
        let (ui, state, index) = (self.ui, self.state, self.index);
        let message_line = state.output.position_of(message_number(state, entry)).and_then(|at| state.output.at(at));
        let mut color = text_color(entry, message_line);
        let newest = state.output.position_of(entry.last).and_then(|at| state.output.at(at));
        let age = newest.map_or(Duration::MAX, |line| ui.now.saturating_duration_since(line.born));
        if !ui.reduced_motion && age < columns::FRESH {
            color = C::White;
        } else if !ui.reduced_motion && age < columns::SETTLING {
            color = columns::bright(color);
        }
        if let Some((diff, kind)) = self.frame.diff.and_then(|diff| Some((diff, diff.kind(entry.first)?))) {
            color = diff.shade(kind, color);
        }
        if let Some((fg, bg)) = self.highlight(item, entry) {
            return vec![on(seg::pad(&entry.message, room), fg, bg)];
        }
        if let Some(sql) = &entry.sql {
            return seg::limit(&self.sql(entry, sql.ms, &sql.command), room);
        }
        let text = entry.message.as_str();
        let line_links = links::find_with(text, state.frames());
        let ids: Vec<usize> = line_links
            .iter()
            .map(|link| {
                self.found.push(link.target.clone());
                self.base + self.found.len() - 1
            })
            .collect();
        seg::limit(&columns::linked(text, 0, &line_links, &ids, color, index), room)
    }

    /// The colours a first row stands out in: a search's match (the current one in the focus colours), copied lines,
    /// lines an agent read, the line that made the app up.
    fn highlight(&self, item: &Shown, entry: &Entry) -> Option<(C, C)> {
        let (ui, state, index) = (self.ui, self.state, self.index);
        let has = |set: Option<&std::collections::HashSet<String>>| {
            set.is_some_and(|set| item.any_line(state, |line| set.contains(&line.text)))
        };
        if has(ui.motion.copied(index, ui.now)) {
            return Some((C::Black, C::Cyan));
        }
        if has(ui.motion.looked(index, ui.now)) {
            return Some((C::Black, C::Magenta));
        }
        let ready = state.ready_at.filter(|(ready, since)| {
            entry.holds(*ready) && ui.now.saturating_duration_since(*since) < columns::READY_OUTLINE
        });
        if ready.is_some() {
            return Some((C::Black, C::Green));
        }
        let search = self.frame.search.filter(|search| item.matches(state, search))?;
        let current = search.index.is_some_and(|at| item.holds(state, at));
        let jumped = ui.motion.lit(&Spot::Match { app: index, line: item.position }, ui.now).is_some();
        Some(match (current, jumped) {
            (true, true) => (C::Black, C::White),
            (true, false) => (C::FocusText, C::FocusBack),
            _ => (C::Black, C::Yellow),
        })
    }

    /// An EF Core command's row: `SQL 12 ms`, its bar against the slowest of its endpoint's, its SQL.
    fn sql(&self, entry: &Entry, ms: f64, command: &str) -> Vec<Seg> {
        let label = if self.ui.nerd { icons::icon(true, Icon::Database) } else { "SQL" };
        let slowest = self.endpoint_slowest(entry).unwrap_or(self.slowest).max(ms);
        let mut segs = vec![
            seg(format!("{label} "), C::Magenta),
            seg(format!("{ms:.0} ms "), if ms >= SLOW_SQL_MS { C::Yellow } else { C::Gray }),
        ];
        segs.extend(bar(ms, slowest));
        segs.push(seg(format!(" {command}"), C::Gray));
        segs
    }

    /// The slowest EF Core command of the endpoint whose request ran this one, over the requests kept.
    fn endpoint_slowest(&self, entry: &Entry) -> Option<f64> {
        let items = &self.controller.requests.items;
        let key = &self.state.key;
        let ran: &Request =
            items.iter().rev().filter(|request| request.app == *key).find(|request| ran_in(request, entry.first))?;
        items
            .iter()
            .filter(|other| other.app == *key && other.method == ran.method && other.endpoint == ran.endpoint)
            .filter_map(|other| other.db.as_ref().map(|db| db.slowest_ms))
            .reduce(f64::max)
    }

    /// A line under an entry: in its level's colour - a library's frame dimmed - its links clickable, after the
    /// bracket's `edge`.
    fn detail(&mut self, line: &OutputLine, entry: &Entry, edge: &str) -> Vec<Seg> {
        let color = if links::library_frame(&line.text, self.state.frames())
            || (entries::is_frame(&line.text) && !self.own_frame(line))
        {
            C::DarkGray
        } else {
            match entry.level {
                Level::Error => C::Red,
                Level::Warn => C::DarkYellow,
                Level::Debug => C::DarkGray,
                Level::Info => C::Gray,
            }
        };
        let text = line.text.trim();
        let line_links = links::find_with(text, self.state.frames());
        let ids: Vec<usize> = line_links
            .iter()
            .map(|link| {
                self.found.push(link.target.clone());
                self.base + self.found.len() - 1
            })
            .collect();
        let mut row = vec![seg(edge, level_color(entry.level)), seg(" ".repeat(BADGE_CELLS), C::Gray)];
        row.extend(columns::linked(text, 0, &line_links, &ids, color, self.index));
        row
    }

    /// The row an exception folds the rest of its lines into - `▸ +18 frames` - and, as it unfolds, counts them down.
    fn fold_row(&self, entry: &Entry, hidden: usize, frames: bool, edge: &str) -> Vec<Seg> {
        let fold = icons::icon(self.ui.nerd, Icon::Fold { open: false });
        let noun = match (frames, hidden) {
            (true, 1) => "frame",
            (true, _) => "frames",
            (false, 1) => "line",
            (false, _) => "lines",
        };
        let target = Target::Entry { column: self.index, line: entry.first };
        vec![
            seg(edge, level_color(entry.level)),
            seg(" ".repeat(BADGE_CELLS), C::Gray),
            seg(format!("{fold} +{hidden} {noun}"), C::DarkGray).at(target),
        ]
    }
}

/// Whether line `number` was logged while `request` ran: in its scope, else between its start and end.
fn ran_in(request: &Request, number: u64) -> bool {
    match (&request.exact, request.logs) {
        (Some(exact), _) => exact.contains(&number),
        (None, Some((first, last))) => (first..=last).contains(&number),
        (None, None) => false,
    }
}

/// A command's time as a bar of eighths against the slowest, over a dotted track.
fn bar(ms: f64, slowest: f64) -> Vec<Seg> {
    let eighths = ((ms / slowest.max(f64::MIN_POSITIVE) * (SQL_BAR * 8) as f64).round() as usize).clamp(1, SQL_BAR * 8);
    (0..SQL_BAR)
        .map(|cell| match eighths.saturating_sub(cell * 8).min(8) {
            0 => seg("·", C::DarkGray),
            filled => seg(seg::eighth(filled), C::DarkCyan),
        })
        .collect()
}

/// As `v` switches a column, it opens from its middle (no motion: at once).
pub fn reveal(ui: &Ui, index: usize, rows: &mut [Vec<Seg>]) {
    let Some(left) = ui.motion.lit(&Spot::Switched(index), ui.now) else { return };
    let height = rows.len();
    let band = ((height as f64 * motion::ease(1.0 - left)).ceil() as usize).min(height);
    let top = (height - band) / 2;
    for (at, row) in rows.iter_mut().enumerate() {
        if at < top || at >= top + band {
            *row = vec![seg(" ".repeat(seg::width(row)), C::Gray)];
        }
    }
}

/// What a parsed column adds to its title: the eye (with Nerd Font icons), and a moment after `v` what it did.
pub fn title(ui: &Ui, state: &AppState) -> Vec<Seg> {
    let mut segs = Vec::new();
    if state.view == ColumnView::Parsed && ui.nerd {
        segs.push(seg(format!(" {}", icons::icon(true, Icon::View)), C::Cyan));
    }
    if let Some((said, _)) =
        ui.views_said.get(&state.key).filter(|(_, at)| ui.now.saturating_duration_since(*at) < SAID)
    {
        segs.push(seg(format!(" {said}"), C::Cyan));
    }
    segs
}

/// Whether a title still says what `v` did.
pub fn animating(ui: &Ui) -> bool {
    ui.views_said.values().any(|(_, at)| ui.now.saturating_duration_since(*at) < SAID)
}

/// The entry a rail's row previews, by its line's place (0 = the oldest kept): its level and message.
pub fn preview(state: &AppState, position: usize) -> Option<String> {
    if state.view != ColumnView::Parsed {
        return None;
    }
    let entry = state.output.entries().of(state.output.number_at(position))?;
    let message =
        entry.sql.as_ref().map_or(entry.message.clone(), |sql| format!("SQL {:.0} ms · {}", sql.ms, sql.command));
    Some(format!("{} {message}", entries::level_name(entry.level)))
}

/// `1,204`.
fn grouped(count: usize) -> String {
    let digits = count.to_string();
    let mut out = String::new();
    for (at, digit) in digits.chars().enumerate() {
        if at > 0 && (digits.len() - at).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// `v`: the focused column switches between raw and parsed - saved for its app - keeping what's at its bottom there:
/// the line's entry, or the entry's last line. Its title says what it did a moment.
pub fn switch(ui: &mut Ui, controller: &mut Controller) {
    let index = ui.focus;
    let Some(state) = controller.states.get(index) else { return };
    let shown = columns::shown_lines(state, ui.search.as_ref());
    let bottom = shown.len().checked_sub(1 + state.scroll).map(|at| match shown[at].entry {
        Some(entry) => state.output.position_of(entry.last).unwrap_or(shown[at].position),
        None => shown[at].position,
    });
    let (lines, count) = (grouped(state.output.len()), grouped(state.output.entries().len()));
    let (key, view) = (state.key.clone(), state.view.other());
    controller.set_column_view(&key, view);
    let state = &controller.states[index];
    let shown = columns::shown_lines(state, ui.search.as_ref());
    let at = bottom.and_then(|position| shown.iter().rposition(|item| item.holds(state, position)));
    let scroll = if state.scroll == 0 { 0 } else { at.map_or(0, |at| shown.len() - 1 - at) };
    controller.states[index].scroll = scroll;
    let said = match view {
        ColumnView::Parsed => format!("parsed · {lines} lines → {count} entries"),
        ColumnView::Raw => format!("raw · {count} entries → {lines} lines"),
    };
    ui.views_said.insert(key, (said, ui.now));
    ui.motion.jump_scroll(index);
    ui.motion.flash(Spot::Switched(index), ui.now);
}

/// Whether the focused column is parsed (`Enter` folds its entries).
pub fn focused(ui: &Ui, controller: &Controller) -> bool {
    controller.states.get(ui.focus).is_some_and(|state| state.view == ColumnView::Parsed)
}

/// The entry a parsed column's keys act on: the search's match, else the newest one on screen that folds, else the one
/// at its bottom.
fn selected<'a>(ui: &Ui, state: &'a AppState) -> Option<&'a Entry> {
    let output = &state.output;
    let matched =
        ui.search.as_ref().filter(|search| search.key == state.key && !search.typing).and_then(|search| search.index);
    if let Some(entry) = matched.and_then(|at| output.entries().of(output.number_at(at))) {
        return Some(entry);
    }
    let shown = shown(state, ui.search.as_ref());
    let on_screen = shown.iter().rev().skip(state.scroll).take(ui.body_rows.max(1));
    let entries: Vec<&Entry> = on_screen.filter_map(|item| item.entry).collect();
    entries.iter().find(|entry| entry.folds()).or(entries.first()).copied()
}

/// `Enter` in a parsed column: folds or unfolds its selected entry.
pub fn fold_selected(ui: &mut Ui, controller: &mut Controller) {
    let index = ui.focus;
    let Some(first) = controller.states.get(index).and_then(|state| selected(ui, state)).map(|entry| entry.first)
    else {
        return;
    };
    fold(ui, controller, index, first);
}

/// Folds or unfolds the entry of column `index` whose first line is `line` (a click on its fold); unfolding wipes
/// its lines in downwards, its fold counting them down.
pub fn fold(ui: &mut Ui, controller: &mut Controller, index: usize, line: u64) {
    let Some(state) = controller.states.get(index) else { return };
    if !state.output.entries().of(line).is_some_and(Entry::folds) {
        return;
    }
    let spot = (state.key.clone(), line);
    if ui.unfolded.remove(&spot) {
        ui.motion.flash(Spot::Folded { app: index, line }, ui.now);
    } else {
        ui.unfolded.insert(spot);
        ui.motion.flash(Spot::Unfolded { app: index, line }, ui.now);
    }
}

/// The panel's notice for a parsed column: the category of the entry under the pointer in full, else the selected
/// one's, and what `Enter` does to it.
pub fn notice(ui: &Ui, controller: &Controller) -> Option<Vec<Seg>> {
    let (state, entry) = match ui.hover {
        Some(Target::Entry { column, line }) => {
            let state = controller.states.get(column)?;
            (state, state.output.entries().of(line)?)
        }
        _ => {
            let state = controller.states.get(ui.focus).filter(|state| state.view == ColumnView::Parsed)?;
            (state, selected(ui, state)?)
        }
    };
    let open = ui.unfolded.contains(&(state.key.clone(), entry.first));
    let mut segs = vec![seg(format!(" {} ", entries::level_name(entry.level)), level_color(entry.level))];
    if let Some(category) = &entry.category {
        segs.push(seg(category.clone(), C::Cyan));
    }
    let lines = entry.lines();
    segs.push(seg(format!(" · {lines} line{}", if lines == 1 { "" } else { "s" }), C::DarkGray));
    if entry.folds() {
        segs.push(seg(if open { " · Enter folds it" } else { " · Enter unfolds it" }, C::DarkGray));
    }
    Some(segs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_are_grouped_by_thousands() {
        assert_eq!([grouped(7), grouped(312), grouped(1204), grouped(1_234_567)], ["7", "312", "1,204", "1,234,567"]);
    }

    #[test]
    fn a_bar_is_eighths_against_the_slowest() {
        let cells = |ms: f64| bar(ms, 80.0).into_iter().map(|seg| seg.text).collect::<String>();
        assert_eq!(cells(80.0), "████");
        assert_eq!(cells(20.0), "█···");
        assert_eq!(cells(10.0), "▌···");
        assert_eq!(cells(0.1), "▏···", "at least an eighth");
    }

    #[test]
    fn a_line_logged_in_a_requests_scope_or_while_it_ran_is_its() {
        let mut request = Request {
            seq: 1,
            at: jiff::ZonedDateTime::now(),
            app: "orders".into(),
            method: "GET".into(),
            path: "/orders".into(),
            endpoint: "/orders".into(),
            status: 200,
            ms: 20.0,
            length: String::new(),
            content: String::new(),
            proto: String::new(),
            sent_content: String::new(),
            sent_length: String::new(),
            logs: Some((10, 20)),
            mixed: false,
            exact: None,
            db: None,
            ended: None,
            outside: false,
            served: None,
        };
        assert!(ran_in(&request, 15) && !ran_in(&request, 21));
        request.exact = Some(vec![30]);
        assert!(ran_in(&request, 30) && !ran_in(&request, 15), "its scope says exactly");
    }
}
