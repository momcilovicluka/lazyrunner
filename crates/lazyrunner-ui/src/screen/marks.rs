//! Marks in an app's output: `m` marks the line at the bottom of the focused column (the search match, when it is on
//! one) or unmarks it, `[` / `]` jump to the older / newer mark, `'` lists every column's marks. A mark is a line's
//! number, so it stays on its line as new ones arrive; an app's marks go when it starts again (its output starts over).
//! A mark can have a note (`a` in the box), and `y` there copies them all as a timeline. `b` there keeps one as a
//! bookmark (#84, `bookmarks`): the box lists them above this run's marks.

use std::time::Instant;

use crossterm::event::KeyEvent;

use lazyrunner_core::output::Hue;
use lazyrunner_core::run::bookmarks::Bookmark;
use lazyrunner_core::run::{Controller, format_age};

use super::columns;
use super::icons::{self, Icon};
use super::motion::Spot;
use super::overlays::boxed;
use super::seg::{Seg, on, pad, seg};
use super::theme::C;
use super::{Overlay, Ui};

const INNER: usize = 84;
const NAME_CELLS: usize = 12;
/// How much of a marked line the log repeats.
const LOGGED_CELLS: usize = 60;

/// m: marks the line at the bottom of the focused column, or the search match it is on; again unmarks it.
pub fn toggle(ui: &mut Ui, controller: &mut Controller) {
    let focus = ui.focus;
    let Some(state) = controller.states.get(focus) else { return };
    let matched =
        ui.search.as_ref().filter(|search| search.key == state.key && !search.typing).and_then(|search| search.index);
    let shown = columns::shown_lines(state, ui.search.as_ref());
    let bottom = shown.len().checked_sub(1 + state.scroll).map(|at| shown[at].position);
    let Some(position) = matched.or(bottom) else {
        controller.add_log("Nothing to mark yet: the column has no output.", Hue::DarkGray);
        return;
    };
    let number = state.output.number_at(position);
    let text =
        state.output.numbered(number, number).next().map(|line| line.text.trim().to_string()).unwrap_or_default();
    let text = super::seg::ellipsis(&text, LOGGED_CELLS).trim_end().to_string();
    let name = controller.apps.get(focus).map_or(String::new(), |app| app.name.clone());
    if state.bookmarked.contains_key(&number) {
        controller.add_log("That line is a bookmark's: ' lists it, and d there deletes it.", Hue::DarkGray);
        return;
    }
    let Some(state) = controller.states.get_mut(focus) else { return };
    let message = match state.marks.binary_search(&number) {
        Ok(at) => {
            state.marks.remove(at);
            state.notes.remove(&number);
            format!("Unmarked {name}: {text}")
        }
        Err(at) => {
            state.marks.insert(at, number);
            format!("Marked {name}: {text}  ([ ] jump between marks, ' lists them)")
        }
    };
    controller.add_log(message, Hue::Magenta);
    ui.motion.flash(Spot::Match { app: focus, line: position }, ui.now);
}

/// [ / ]: the older / newer mark in the focused column - from the one jumped to last, else from its bottom line -
/// wrapping around. Only marks on lines the column shows count.
pub fn jump(ui: &mut Ui, controller: &mut Controller, older: bool) {
    let focus = ui.focus;
    let Some(state) = controller.states.get(focus) else { return };
    let shown = columns::shown_lines(state, ui.search.as_ref());
    let marked: Vec<usize> =
        shown.iter().enumerate().filter(|(_, item)| item.marked(state)).map(|(at, _)| at).collect();
    let (Some(&first), Some(&last)) = (marked.first(), marked.last()) else {
        let why = if state.marks.is_empty() {
            "No marks in this column - m marks the line at its bottom."
        } else {
            "This column's marks are on lines it hides (f, or the search)."
        };
        controller.add_log(why, Hue::DarkGray);
        return;
    };
    let bottom = shown.len() as isize - 1 - state.scroll as isize;
    let from = ui
        .mark_at
        .filter(|(app, _)| *app == focus)
        .and_then(|(_, position)| shown.iter().position(|item| item.holds(state, position)))
        .map_or(bottom, |at| at as isize);
    let next = if older {
        marked.iter().rev().find(|&&at| (at as isize) < from)
    } else {
        marked.iter().find(|&&at| at as isize > from)
    };
    let found = next.copied().unwrap_or(if older { last } else { first });
    let number = state.output.number_at(shown[found].position);
    let bookmark = state.bookmarked.get(&number).and_then(|id| controller.bookmark(id));
    let said = match bookmark {
        Some(bookmark) if bookmark.note.is_empty() => Some((format!("Bookmark: {}", bookmark.name), Hue::Cyan)),
        Some(bookmark) => Some((format!("Bookmark: {} - {}", bookmark.name, bookmark.note), Hue::Cyan)),
        None => state.notes.get(&number).map(|note| (format!("Mark: {note}"), Hue::Magenta)),
    };
    let (scroll, position) = (columns::middle(&shown, found, ui.focus_cells, ui.body_rows), shown[found].position);
    controller.states[focus].scroll = scroll;
    ui.mark_at = Some((focus, position));
    ui.motion.flash(Spot::Match { app: focus, line: position }, ui.now);
    if let Some((said, hue)) = said {
        controller.add_log(said, hue);
    }
}

/// Every column's marks: (app, line number), oldest first (one whose line is gone from the buffer first).
pub(crate) fn all(controller: &Controller) -> Vec<(usize, u64)> {
    let mut marks: Vec<(usize, u64)> = controller
        .states
        .iter()
        .enumerate()
        .flat_map(|(app, state)| state.marks.iter().map(move |&number| (app, number)))
        .collect();
    marks.sort_by_key(|&(app, number)| {
        controller.states[app].output.numbered(number, number).next().map(|line| line.born)
    });
    marks
}

/// A row of the marks box: a bookmark (by its id), or a mark of this run (its app and line number).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Item {
    Bookmark(String),
    Mark(usize, u64),
}

/// The marks box's rows: the bookmarks - those not found for 30 days last - then this run's marks that aren't one.
pub(crate) fn items(controller: &Controller) -> Vec<Item> {
    let now = jiff::Timestamp::now();
    let mut bookmarks: Vec<&Bookmark> = controller.bookmarks().iter().collect();
    bookmarks
        .sort_by_key(|bookmark| controller.bookmark_line(&bookmark.id).is_none() && bookmark.stale_days(now).is_some());
    let marks = all(controller)
        .into_iter()
        .filter(|&(app, number)| !controller.states[app].bookmarked.contains_key(&number))
        .map(|(app, number)| Item::Mark(app, number));
    bookmarks.into_iter().map(|bookmark| Item::Bookmark(bookmark.id.clone())).chain(marks).collect()
}

/// ': the marks box (none yet: says how to make one).
pub fn open(ui: &mut Ui) {
    ui.overlay = Some(Overlay::Marks { index: 0, note: None, naming: None, kept: false });
}

/// The marks box: j k choose, Enter jumps to the mark or bookmark (one from an earlier run shows its kept lines), a
/// writes its note, b keeps a mark as a bookmark, e exports the bookmarks, y copies them all, d unmarks a mark or
/// deletes a bookmark, Esc / q / ' close. While a note or a name is typed, keys type it: Enter keeps it, Esc leaves it.
pub fn key(ui: &mut Ui, controller: &mut Controller, name: &str, key: &KeyEvent) {
    let items = items(controller);
    let Some(Overlay::Marks { index, note, naming, kept }) = &ui.overlay else { return };
    let picked = items.get((*index).min(items.len().saturating_sub(1))).cloned();
    match (*kept, naming.is_some(), note.is_some()) {
        (true, ..) => kept_key(ui, controller, name, picked),
        (_, true, _) => naming_key(ui, controller, name, key, picked),
        (.., true) => note_key(ui, controller, name, key, picked),
        _ => list_key(ui, controller, name, &items, picked),
    }
}

fn list_key(ui: &mut Ui, controller: &mut Controller, name: &str, listed: &[Item], picked: Option<Item>) {
    match name {
        "y" => copy_timeline(ui, controller, listed),
        "e" => super::bookmarks::export(ui, controller),
        "b" => match picked {
            Some(Item::Mark(..)) => super::bookmarks::name(ui),
            Some(Item::Bookmark(_)) => controller.add_log("That one is a bookmark already.", Hue::DarkGray),
            None => controller.add_log("No mark to keep - m marks the line at the bottom of a column.", Hue::DarkGray),
        },
        "a" => {
            let written = match &picked {
                Some(Item::Mark(app, number)) => controller.states[*app].notes.get(number).cloned(),
                Some(Item::Bookmark(id)) => controller.bookmark(id).map(|bookmark| bookmark.note.clone()),
                None => return,
            };
            if let Some(Overlay::Marks { note, .. }) = &mut ui.overlay {
                *note = Some(written.unwrap_or_default());
            }
        }
        "Escape" | "q" | "'" => ui.overlay = None,
        "j" | "DownArrow" | "k" | "UpArrow" => {
            if let Some(Overlay::Marks { index, .. }) = &mut ui.overlay {
                let step = if matches!(name, "j" | "DownArrow") { 1 } else { -1 };
                *index = super::keys::wrapped((*index).min(listed.len().saturating_sub(1)), step, listed.len());
            }
        }
        "d" | "Delete" => {
            match picked {
                Some(Item::Mark(app, number)) => {
                    controller.states[app].marks.retain(|&mark| mark != number);
                    controller.states[app].notes.remove(&number);
                }
                Some(Item::Bookmark(id)) => delete(controller, &id),
                None => return,
            }
            if items(controller).is_empty() {
                ui.overlay = None;
            }
        }
        "Enter" | "Space" => match picked {
            Some(Item::Mark(app, number)) => {
                ui.overlay = None;
                go_to(ui, controller, app, number);
            }
            Some(Item::Bookmark(id)) => match controller.bookmark_line(&id) {
                Some((app, number)) => {
                    ui.overlay = None;
                    go_to(ui, controller, app, number);
                }
                None => {
                    if let Some(Overlay::Marks { kept, .. }) = &mut ui.overlay {
                        *kept = true;
                    }
                }
            },
            None => {}
        },
        _ => {}
    }
}

fn delete(controller: &mut Controller, id: &str) {
    if let Some(deleted) = controller.delete_bookmark(id) {
        controller.add_log(format!("Deleted the bookmark {}", deleted.name), Hue::DarkGray);
    }
}

/// While a note is written: Enter keeps it (an empty one removes it), Esc leaves it as it was.
fn note_key(ui: &mut Ui, controller: &mut Controller, name: &str, key: &KeyEvent, picked: Option<Item>) {
    let Some(Overlay::Marks { note: Some(typing), .. }) = &mut ui.overlay else { return };
    match name {
        "Escape" => {}
        "Enter" => {
            let text = typing.trim().to_string();
            match picked {
                Some(Item::Mark(app, number)) if text.is_empty() => {
                    controller.states[app].notes.remove(&number);
                }
                Some(Item::Mark(app, number)) => {
                    controller.states[app].notes.insert(number, text);
                }
                Some(Item::Bookmark(id)) => controller.set_bookmark_note(&id, &text),
                None => {}
            }
        }
        "Backspace" => {
            typing.pop();
            return;
        }
        _ => {
            if let Some(c) = super::keys::typed(key) {
                typing.push(c);
            }
            return;
        }
    }
    if let Some(Overlay::Marks { note, .. }) = &mut ui.overlay {
        *note = None;
    }
}

/// While a bookmark's name is typed (`b`): Enter keeps the mark as one, Esc leaves it a mark.
fn naming_key(ui: &mut Ui, controller: &mut Controller, name: &str, key: &KeyEvent, picked: Option<Item>) {
    let Some(Overlay::Marks { naming: Some((typed, _)), .. }) = &mut ui.overlay else { return };
    match name {
        "Escape" => {}
        "Enter" => {
            let typed = std::mem::take(typed);
            if let Some(Overlay::Marks { naming, .. }) = &mut ui.overlay {
                *naming = None;
            }
            let Some(Item::Mark(app, number)) = picked else { return };
            let Some(id) = super::bookmarks::keep(ui, controller, (app, number), &typed) else { return };
            let at = items(controller).iter().position(|item| *item == Item::Bookmark(id.clone()));
            if let (Some(at), Some(Overlay::Marks { index, .. })) = (at, &mut ui.overlay) {
                *index = at;
            }
            return;
        }
        "Backspace" => {
            typed.pop();
            return;
        }
        _ => {
            if let Some(c) = super::keys::typed(key) {
                typed.push(c);
            }
            return;
        }
    }
    if let Some(Overlay::Marks { naming, .. }) = &mut ui.overlay {
        *naming = None;
    }
}

/// The kept lines of a bookmark from an earlier run: Esc (or Enter) goes back to the list, d deletes it, e exports.
fn kept_key(ui: &mut Ui, controller: &mut Controller, name: &str, picked: Option<Item>) {
    match name {
        "d" | "Delete" => {
            if let Some(Item::Bookmark(id)) = picked {
                delete(controller, &id);
            }
            if items(controller).is_empty() {
                ui.overlay = None;
                return;
            }
        }
        "e" => {
            super::bookmarks::export(ui, controller);
            return;
        }
        "Escape" | "Enter" | "Space" | "Backspace" | "q" => {}
        _ => return,
    }
    if let Some(Overlay::Marks { kept, .. }) = &mut ui.overlay {
        *kept = false;
    }
}

/// y: the bookmarks, then every mark, oldest first, as a timeline - its time, app, name, note and line - for a ticket
/// or a chat.
fn copy_timeline(ui: &Ui, controller: &mut Controller, items: &[Item]) {
    if items.is_empty() {
        controller.add_log("No marks to copy - m marks the line at the bottom of the focused column.", Hue::DarkGray);
        return;
    }
    let lines: Vec<String> = items
        .iter()
        .filter_map(|item| match item {
            Item::Bookmark(id) => {
                controller.bookmark(id).map(|bookmark| super::bookmarks::timeline_line(ui, controller, bookmark))
            }
            &Item::Mark(app, number) => Some(mark_line(ui, controller, app, number)),
        })
        .collect();
    let count = |what: &str, n: usize| if n == 1 { format!("1 {what}") } else { format!("{n} {what}s") };
    let kept = items.iter().filter(|item| matches!(item, Item::Bookmark(_))).count();
    let what = match (kept, items.len() - kept) {
        (0, marks) => count("mark", marks),
        (kept, 0) => count("bookmark", kept),
        (kept, marks) => format!("{} and {}", count("bookmark", kept), count("mark", marks)),
    };
    super::copy::put(ui.clipboard, controller, &lines.join(super::copy::NEWLINE), &format!("{what} as a timeline"));
}

/// A mark in the `y` timeline: when its line came, its app, its note and its line.
fn mark_line(ui: &Ui, controller: &Controller, app: usize, number: u64) -> String {
    let state = &controller.states[app];
    let name = controller.apps.get(app).map_or("", |app| app.name.as_str());
    let line = state.output.numbered(number, number).next();
    let time = line.map_or("--:--:--".to_string(), |line| clock(ui, line.born));
    let text = line.map_or("(gone from the buffer)".to_string(), |line| line.text.trim().to_string());
    let note = state.notes.get(&number).map_or(String::new(), |note| format!("{note} - "));
    format!("{time}  {name}  {note}{text}")
}

/// The time of day a line came.
pub(super) fn clock(ui: &Ui, born: Instant) -> String {
    let ago = ui.now.saturating_duration_since(born);
    (jiff::ZonedDateTime::now() - ago).strftime(lazyrunner_core::time::CLOCK).to_string()
}

/// Focuses `app` and scrolls its marked line `number` into the middle, when the column still shows it.
pub(super) fn go_to(ui: &mut Ui, controller: &mut Controller, app: usize, number: u64) {
    ui.focus = app;
    let Some(state) = controller.states.get(app) else { return };
    let shown = columns::shown_lines(state, ui.search.as_ref());
    let kept = state.output.position_of(number);
    let Some(found) = kept.and_then(|position| shown.iter().position(|item| item.holds(state, position))) else {
        let why = if kept.is_none() {
            "That line has left the column's buffer."
        } else {
            "That line is hidden by the column's filter (f) or search."
        };
        controller.add_log(why, Hue::DarkGray);
        return;
    };
    let (scroll, position) = (columns::middle(&shown, found, ui.focus_cells, ui.body_rows), shown[found].position);
    controller.states[app].scroll = scroll;
    ui.mark_at = Some((app, position));
    ui.motion.flash(Spot::Match { app, line: position }, ui.now);
}

/// A mark's row: its app, how long ago, its note and its line - the one under the selection bar (`bar`) lit, the
/// picked one's note being typed yellow.
fn mark_row(
    ui: &Ui,
    controller: &Controller,
    (at, index, bar): (usize, usize, usize),
    (app, number): (usize, u64),
    (inner, writing): (usize, Option<&str>),
) -> Vec<Seg> {
    let name = pad(controller.apps.get(app).map_or("", |app| app.name.as_str()), NAME_CELLS);
    let line = controller.states[app].output.numbered(number, number).next();
    let (age, text) = match line {
        Some(line) => {
            let age = format_age(ui.now.saturating_duration_since(line.born).as_secs_f64());
            (format!("{age} ago"), line.text.trim().to_string())
        }
        None => (String::new(), "(gone from the buffer)".to_string()),
    };
    let lead = format!(" {name} {age:>9}  ");
    let noted = controller.states[app].notes.get(&number).map_or(String::new(), |note| format!("{note} · "));
    if at == index
        && let Some(typing) = writing
    {
        return vec![on(pad(&format!("{lead}note: {typing}{}", ui.cursor()), inner), C::Black, C::Yellow)];
    }
    let row = vec![
        seg(format!(" {name}"), C::Magenta),
        seg(format!(" {age:>9}  "), C::DarkGray),
        seg(noted, C::Yellow),
        seg(text, if line.is_some() { C::White } else { C::DarkGray }),
    ];
    if at == bar { super::overlays::on_bar(ui, row, (2, 2), inner) } else { row }
}

/// The lines around the picked mark, dim, so it reads in its place without jumping there.
fn around(controller: &Controller, (app, number): (usize, u64), inner: usize) -> Vec<Vec<Seg>> {
    let output = &controller.states[app].output;
    let line = |number: u64| output.numbered(number, number).next().map(|line| line.text.trim().to_string());
    let mut rows = Vec::new();
    for (near, mark) in [(number.saturating_sub(1), "  ┆ before "), (number + 1, "  ┆ after  ")] {
        if near != number
            && let Some(text) = line(near)
        {
            rows.push(super::columns::fit(vec![seg(mark, C::DarkGray), seg(text, C::DarkGray)], inner));
        }
    }
    rows
}

/// The marks box: the bookmarks (#84) under their heading, then this run's marks; the picked one's lines around it;
/// while a bookmark is named, its input under them; `Enter` on a bookmark from an earlier run shows its kept lines.
pub fn rows(ui: &mut Ui, controller: &Controller, cells: usize, rows: usize) -> Vec<Vec<Seg>> {
    let items = items(controller);
    let (index, writing, naming, kept) = match &ui.overlay {
        Some(Overlay::Marks { index, note, naming, kept }) => {
            ((*index).min(items.len().saturating_sub(1)), note.clone(), naming.clone(), *kept)
        }
        _ => (0, None, None, false),
    };
    if kept
        && let Some(Item::Bookmark(id)) = items.get(index)
        && let Some(bookmark) = controller.bookmark(id)
    {
        return super::bookmarks::kept_rows(ui, controller, bookmark, cells, rows);
    }
    let bar = ui.motion.bar(index, ui.now);
    let inner = cells.saturating_sub(8).clamp(20, INNER);
    let sections = items.iter().any(|item| matches!(item, Item::Bookmark(_)));
    let mut lines: Vec<Vec<Seg>> = Vec::new();
    for (at, item) in items.iter().enumerate() {
        if sections && at == 0 {
            lines.push(vec![seg(" Bookmarks", super::bookmarks::ACCENT)]);
        }
        let first_mark = matches!(item, Item::Mark(..)) && at > 0 && matches!(items[at - 1], Item::Bookmark(_));
        if first_mark {
            lines.extend([Vec::new(), vec![seg(" This run's marks", C::Magenta)]]);
        }
        let typing = writing.as_deref().filter(|_| at == index);
        match item {
            Item::Bookmark(id) => {
                let Some(bookmark) = controller.bookmark(id) else { continue };
                let row = super::bookmarks::row(ui, controller, bookmark, (at == index, at == bar), (inner, typing));
                lines.push(row);
                if let Some(line) = controller.bookmark_line(id).filter(|_| at == index) {
                    lines.extend(around(controller, line, inner));
                }
            }
            &Item::Mark(app, number) => {
                lines.push(mark_row(ui, controller, (at, index, bar), (app, number), (inner, typing)));
                if at == index {
                    lines.extend(around(controller, (app, number), inner));
                }
            }
        }
    }
    if sections && !items.iter().any(|item| matches!(item, Item::Mark(..))) {
        let none = " This run's marks: none yet - m marks a line, and b here keeps one as a bookmark";
        lines.extend([Vec::new(), vec![seg(none, C::DarkGray)]]);
    }
    if items.is_empty() {
        lines = [
            "",
            "No marks yet.",
            "m marks the line at the bottom of the focused column; ] and [ jump between",
            "the marks, and they're listed here.",
            "",
        ]
        .iter()
        .map(|text| vec![seg(format!(" {text}"), C::DarkGray)])
        .collect();
    }
    if let Some((typed, opened)) = &naming {
        let note = match items.get(index) {
            Some(&Item::Mark(app, number)) => controller.states[app].notes.get(&number).map(String::as_str),
            _ => None,
        };
        lines.extend(super::bookmarks::naming_rows(ui, (typed, *opened), note, inner));
    }
    let picked = items.get(index);
    let footer = match (picked, writing.is_some(), naming.is_some()) {
        (None, ..) => "Esc closes",
        (_, true, _) => "type the note · Enter keeps it (empty: none) · Esc leaves it",
        (_, _, true) => "type its name · Enter keeps it (empty: after its line) · Esc leaves it a mark",
        (Some(Item::Mark(..)), ..) => "Enter jumps · a note · b bookmarks it · y copies all · d unmarks · Esc closes",
        (Some(Item::Bookmark(id)), ..) if controller.bookmark_line(id).is_none() => {
            "Enter shows its lines · a note · e exports · y copies all · d deletes · Esc closes"
        }
        (Some(Item::Bookmark(_)), ..) => "Enter jumps · a note · e exports · y copies all · d deletes · Esc closes",
    };
    boxed(&icons::titled(ui.nerd, Icon::Bookmark, "Marks"), lines, footer, cells, rows, inner)
}
