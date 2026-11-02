//! The timeline (`~`): every app's output in one list, in the order it came - each line tagged with its app, coloured
//! by its level as in the columns, and a quiet stretch of more than a few seconds shown as a dim `· 4.2 s ·` row, so
//! bursts stand apart. `j` `k` scroll (the bottom row is the one picked), `G` follows new output again, `Enter` jumps
//! to the picked line in its own column.

use std::time::{Duration, Instant};

use lazyrunner_core::run::Controller;

use super::columns::{fit, line_color};
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

/// A row: a line (its app, its place among the app's kept lines), or a quiet stretch.
enum Row {
    Line { app: usize, position: usize },
    Gap(Duration),
}

/// Every app's kept lines, the oldest first, with the quiet stretches between them.
fn rows_of(controller: &Controller) -> Vec<Row> {
    let mut lines: Vec<(Instant, usize, usize)> = Vec::new();
    for (app, state) in controller.states.iter().enumerate() {
        let len = state.output.len();
        lines.extend(state.output.newest_first(0).enumerate().map(|(back, line)| (line.born, app, len - 1 - back)));
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
    ui.overlay = Some(Overlay::Timeline { scroll: 0 });
}

/// The picked line: the bottom row's, or the nearest line above a quiet stretch there.
fn picked(rows: &[Row], scroll: usize) -> Option<(usize, usize)> {
    rows.iter().rev().skip(scroll).find_map(|row| match row {
        Row::Line { app, position } => Some((*app, *position)),
        Row::Gap(_) => None,
    })
}

pub fn key(ui: &mut Ui, controller: &mut Controller, name: &str) {
    let rows = rows_of(controller);
    let Some(Overlay::Timeline { scroll }) = &mut ui.overlay else { return };
    let last = rows.len().saturating_sub(1);
    match name {
        "Escape" | "q" | "~" => ui.overlay = None,
        "k" | "UpArrow" => *scroll = (*scroll + 1).min(last),
        "j" | "DownArrow" => *scroll = scroll.saturating_sub(1),
        "Ctrl+U" | "PageUp" => *scroll = (*scroll + PAGE).min(last),
        "Ctrl+D" | "PageDown" => *scroll = scroll.saturating_sub(PAGE),
        "g" | "Home" | "<" => *scroll = last,
        "G" | "End" | ">" => *scroll = 0,
        "Enter" => {
            let picked = picked(&rows, *scroll);
            ui.overlay = None;
            if let Some((app, position)) = picked {
                let number = controller.states[app].output.number_at(position);
                super::marks::go_to(ui, controller, app, number);
            }
        }
        _ => {}
    }
}

pub fn rows(ui: &Ui, controller: &Controller, cells: usize, height: usize) -> Vec<Vec<Seg>> {
    let Some(Overlay::Timeline { scroll }) = &ui.overlay else { return Vec::new() };
    let all = rows_of(controller);
    let lines = all.iter().filter(|row| matches!(row, Row::Line { .. })).count();
    let body = height.saturating_sub(2);
    let scroll = (*scroll).min(all.len().saturating_sub(1));
    let end = all.len().saturating_sub(scroll);
    let start = end.saturating_sub(body);
    let following = if scroll == 0 { "following new output" } else { "G follows new output again" };
    let mut out = vec![fit(
        vec![
            on(" ~ Timeline ", C::FocusText, C::FocusBack),
            seg(format!("  every app's output, in the order it came · {lines} lines · {following}"), C::DarkGray),
        ],
        cells,
    )];
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
    out.push(fit(
        vec![seg(
            " j k scroll · G follow · g the oldest · Enter jumps to the picked line in its column · Esc closes",
            C::DarkGray,
        )],
        cells,
    ));
    out.truncate(height);
    out
}
