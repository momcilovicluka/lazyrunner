//! Marks in an app's output: `m` marks the line at the bottom of the focused column (the search match, when it is on
//! one) or unmarks it, `[` / `]` jump to the older / newer mark, `'` lists every column's marks. A mark is a line's
//! number, so it stays on its line as new ones arrive; an app's marks go when it starts again (its output starts over).
//! A mark can have a note (`a` in the box), and `y` there copies them all as a timeline.

use std::time::Instant;

use crossterm::event::KeyEvent;

use lazyrunner_core::output::Hue;
use lazyrunner_core::run::{Controller, format_duration};

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
    let marked: Vec<usize> = shown
        .iter()
        .enumerate()
        .filter(|(_, item)| state.marks.binary_search(&state.output.number_at(item.position)).is_ok())
        .map(|(at, _)| at)
        .collect();
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
        .and_then(|(_, position)| shown.iter().position(|item| item.position == position))
        .map_or(bottom, |at| at as isize);
    let next = if older {
        marked.iter().rev().find(|&&at| (at as isize) < from)
    } else {
        marked.iter().find(|&&at| at as isize > from)
    };
    let found = next.copied().unwrap_or(if older { last } else { first });
    let note = state.notes.get(&state.output.number_at(shown[found].position)).cloned();
    let (scroll, position) = (columns::middle(&shown, found, ui.focus_cells, ui.body_rows), shown[found].position);
    controller.states[focus].scroll = scroll;
    ui.mark_at = Some((focus, position));
    ui.motion.flash(Spot::Match { app: focus, line: position }, ui.now);
    if let Some(note) = note {
        controller.add_log(format!("Mark: {note}"), Hue::Magenta);
    }
}

/// Every column's marks: (app, line number), oldest first (one whose line is gone from the buffer first).
fn all(controller: &Controller) -> Vec<(usize, u64)> {
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

/// ': the marks box (none yet: says how to make one).
pub fn open(ui: &mut Ui) {
    ui.overlay = Some(Overlay::Marks { index: 0, note: None });
}

/// The marks box: j k choose, Enter jumps to the mark, a writes its note, y copies them all, d unmarks it, Esc / q /
/// ' close. While a note is written, keys type it: Enter keeps it (an empty one removes it), Esc leaves it as it was.
pub fn key(ui: &mut Ui, controller: &mut Controller, name: &str, key: &KeyEvent) {
    let marks = all(controller);
    let writing = matches!(ui.overlay, Some(Overlay::Marks { note: Some(_), .. }));
    if name == "y" && !writing {
        copy_timeline(ui, controller, &marks);
        return;
    }
    let Some(Overlay::Marks { index, note }) = &mut ui.overlay else { return };
    let at = (*index).min(marks.len().saturating_sub(1));
    if let Some(typing) = note {
        match name {
            "Escape" => *note = None,
            "Enter" => {
                if let Some(&(app, number)) = marks.get(at) {
                    let text = typing.trim().to_string();
                    let notes = &mut controller.states[app].notes;
                    if text.is_empty() {
                        notes.remove(&number);
                    } else {
                        notes.insert(number, text);
                    }
                }
                *note = None;
            }
            "Backspace" => {
                typing.pop();
            }
            _ => {
                if let Some(c) = super::keys::typed(key) {
                    typing.push(c);
                }
            }
        }
        return;
    }
    match name {
        "a" => {
            if let Some(&(app, number)) = marks.get(at) {
                *note = Some(controller.states[app].notes.get(&number).cloned().unwrap_or_default());
            }
        }
        "Escape" | "q" | "'" => ui.overlay = None,
        "j" | "DownArrow" => *index = super::keys::wrapped(at, 1, marks.len()),
        "k" | "UpArrow" => *index = super::keys::wrapped(at, -1, marks.len()),
        "d" | "Delete" => {
            if let Some(&(app, number)) = marks.get(at) {
                controller.states[app].marks.retain(|&mark| mark != number);
                controller.states[app].notes.remove(&number);
                if all(controller).is_empty() {
                    ui.overlay = None;
                }
            }
        }
        "Enter" | "Space" => {
            if let Some(&(app, number)) = marks.get(at) {
                ui.overlay = None;
                go_to(ui, controller, app, number);
            }
        }
        _ => {}
    }
}

/// y: every mark, oldest first, as a timeline - its time, app, note and line - for a ticket or a chat.
fn copy_timeline(ui: &Ui, controller: &mut Controller, marks: &[(usize, u64)]) {
    if marks.is_empty() {
        controller.add_log("No marks to copy - m marks the line at the bottom of the focused column.", Hue::DarkGray);
        return;
    }
    let lines: Vec<String> = marks
        .iter()
        .map(|&(app, number)| {
            let state = &controller.states[app];
            let name = controller.apps.get(app).map_or("", |app| app.name.as_str());
            let line = state.output.numbered(number, number).next();
            let time = line.map_or("--:--:--".to_string(), |line| clock(ui, line.born));
            let text = line.map_or("(gone from the buffer)".to_string(), |line| line.text.trim().to_string());
            let note = state.notes.get(&number).map_or(String::new(), |note| format!("{note} - "));
            format!("{time}  {name}  {note}{text}")
        })
        .collect();
    let what = if marks.len() == 1 { "1 mark".to_string() } else { format!("{} marks", marks.len()) };
    super::copy::put(ui.clipboard, controller, &lines.join(super::copy::NEWLINE), &format!("{what} as a timeline"));
}

/// The time of day a line came.
fn clock(ui: &Ui, born: Instant) -> String {
    let ago = chrono::TimeDelta::from_std(ui.now.saturating_duration_since(born)).unwrap_or_default();
    (chrono::Local::now() - ago).format("%H:%M:%S").to_string()
}

/// Focuses `app` and scrolls its marked line `number` into the middle, when the column still shows it.
fn go_to(ui: &mut Ui, controller: &mut Controller, app: usize, number: u64) {
    ui.focus = app;
    let Some(state) = controller.states.get(app) else { return };
    let shown = columns::shown_lines(state, ui.search.as_ref());
    let kept = state.output.position_of(number);
    let Some(found) = kept.and_then(|position| shown.iter().position(|item| item.position == position)) else {
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

/// The marks box: each mark's app, how long ago its line came, and the line.
pub fn rows(ui: &Ui, controller: &Controller, cells: usize, rows: usize) -> Vec<Vec<Seg>> {
    let marks = all(controller);
    let (index, writing) = match &ui.overlay {
        Some(Overlay::Marks { index, note }) => ((*index).min(marks.len().saturating_sub(1)), note.as_deref()),
        _ => (0, None),
    };
    let inner = cells.saturating_sub(8).clamp(20, INNER);
    let mut lines: Vec<Vec<Seg>> = marks
        .iter()
        .enumerate()
        .map(|(at, &(app, number))| {
            let name = pad(controller.apps.get(app).map_or("", |app| app.name.as_str()), NAME_CELLS);
            let line = controller.states[app].output.numbered(number, number).next();
            let (age, text) = match line {
                Some(line) => {
                    let age = format_duration(ui.now.saturating_duration_since(line.born).as_secs_f64());
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
            if at == index {
                vec![on(ui.scrolling_parts(&[&lead, &noted, &text], inner), C::Black, C::Cyan)]
            } else {
                vec![
                    seg(format!(" {name}"), C::Magenta),
                    seg(format!(" {age:>9}  "), C::DarkGray),
                    seg(noted, C::Yellow),
                    seg(text, if line.is_some() { C::White } else { C::DarkGray }),
                ]
            }
        })
        .collect();
    if marks.is_empty() {
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
    boxed(
        &icons::titled(ui.nerd, Icon::Bookmark, "Marks"),
        lines,
        match (marks.is_empty(), writing.is_some()) {
            (true, _) => "Esc closes",
            (false, true) => "type the note · Enter keeps it (empty: none) · Esc leaves it",
            (false, false) => "j k + Enter jumps · a note · y copies them all · d unmarks · Esc closes",
        },
        cells,
        rows,
        inner,
    )
}
