//! The rail beside a column's output, in place of a scrollbar: a map of every line the column shows - errors red,
//! warnings yellow, search matches cyan, marks magenta, the rest dotted by how fast they came (`rail_density`) - with
//! a bracket on the lines on screen.
//! Each row stands for an equal share of the lines and takes the colour of the most serious one in it; a click on a
//! row brings that line to the middle of the column, a drag along it scrolls, and the pointer on a row previews that
//! line over the column, in its own colours. A narrow column gets the map alone, the lines on screen lit in it. What
//! moves on it is in `rail_motion`.

use std::cell::RefCell;
use std::collections::HashMap;

use lazyrunner_core::output::{Level, LevelFilter};
use lazyrunner_core::run::app::AppState;

use super::columns::{self, Shown};
use super::mouse::Target;
use super::rail_density::{self, Density};
use super::rail_motion::{self, Now};
use super::seg::{self, Seg, on, seg};
use super::theme::C;
use super::{Search, Ui};

/// From this wide a column's rail has its bracket beside the map.
const WIDE_FROM: usize = 30;

/// What a row of the rail shows: the most serious line in its share.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Mark {
    Empty,
    Normal,
    Marked,
    Match,
    Warning,
    Error,
}

/// A row of the rail: what it shows, and the line (in the column's list) a click brings into view.
type Row = (Mark, usize);

/// What a map was drawn from: it's drawn again only when one of these changes.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Drawn {
    total: u64,
    shown: usize,
    rows: usize,
    filter: LevelFilter,
    search: Option<(String, bool, bool)>,
    marks: usize,
}

/// A column's map: its rows, and how fast each one's lines came.
#[derive(Clone)]
struct Map {
    rows: Vec<Row>,
    density: Vec<Density>,
}

/// Each column's map, by its app's key.
#[derive(Default)]
pub struct Maps(RefCell<HashMap<String, (Drawn, Map)>>);

/// The cells a `column` cells wide column's rail takes.
pub fn width(column: usize) -> usize {
    if column >= WIDE_FROM { 2 } else { 1 }
}

/// Adds the rail, `cells` wide, to the rows of app `index`'s column (oldest at the top): `scroll` lines back from the
/// newest of `list`.
#[allow(clippy::too_many_arguments)]
pub fn add(
    ui: &Ui,
    state: &AppState,
    index: usize,
    list: &[Shown],
    search: Option<&Search>,
    rows: &mut [Vec<Seg>],
    scroll: usize,
    cells: usize,
) {
    let (height, total) = (rows.len(), list.len());
    if height == 0 || total == 0 {
        return;
    }
    let map = map(ui, state, list, search, height);
    let marks = &map.rows;
    let thumb = (height * height / total).clamp(1, height);
    let newest_shown = total.saturating_sub(scroll).max(1); // the lines up to this one end at the bottom row
    let bottom = (newest_shown * height).div_ceil(total).clamp(thumb, height); // at the top it keeps its size
    let top = bottom.saturating_sub(thumb);
    let bracket = if scroll > 0 { C::Cyan } else { C::DarkGray };
    let matched = search.and_then(|search| search.index);
    let match_at = matched.and_then(|position| list.iter().position(|item| item.position == position));
    let now = Now {
        live_top: height.saturating_sub(thumb),
        narrow: cells == 1,
        errors: state.errors,
        dropped: state.output.total().saturating_sub(state.output.len() as u64),
        scroll,
        target: state.scroll,
        matched,
        error_row: marks.iter().rposition(|(mark, _)| *mark == Mark::Error),
        match_row: match_at.map(|at| at * height / total),
        top,
        bottom,
        height,
    };
    let moving = rail_motion::moving(ui, &state.key, now);
    for (at, row) in rows.iter_mut().enumerate() {
        let (mark, line) = marks[at];
        let position = list.get(line).map_or(0, |item| item.position);
        let target = Target::Rail { column: index, row: at, scroll: jump(line, height, total), line: position };
        let seen = (top..bottom).contains(&at);
        if cells > 1 {
            let edge = match at {
                _ if !seen => " ",
                _ if scroll == 0 && at + 1 == bottom => "▼",
                _ if thumb == 1 => "▸",
                _ if at == top => "╭",
                _ if at + 1 == bottom => "╰",
                _ => "│",
            };
            row.push(moving.paint_edge(at, seg(edge, bracket)).at(target));
        }
        let cell = match mark {
            Mark::Error => seg("█", C::Red),
            Mark::Warning => seg("█", C::Yellow),
            Mark::Match => seg("█", C::Cyan),
            Mark::Marked => seg("▌", C::Magenta),
            _ if seen && cells == 1 => seg("▐", bracket),
            Mark::Normal => match map.density.get(at) {
                Some(density) => seg(density.glyph.to_string(), if density.burst { C::Gray } else { C::DarkGray }),
                None => seg("╎", C::DarkGray),
            },
            Mark::Empty => seg(" ", C::Gray),
        };
        row.push(moving.paint(at, cell).at(target));
    }
}

/// While the pointer is on a row of app `index`'s rail: the line that row brings into view, over the column's row
/// beside it (`cells` wide, before the rail's `rail` cells), in the line's own colour.
pub fn preview(ui: &Ui, state: &AppState, index: usize, rows: &mut [Vec<Seg>], cells: usize, rail: usize) {
    let Some(Target::Rail { column, row, line, .. }) = ui.hover else { return };
    let Some(found) = rows.get_mut(row).filter(|_| column == index) else { return };
    let back = state.output.len().saturating_sub(line + 1);
    let Some(shown) = state.output.newest_first(back).next() else { return };
    let edge = found.split_off(found.len().saturating_sub(rail));
    let text = seg::ellipsis(&format!(" {} ", shown.text.trim()), cells);
    *found = vec![on(text, C::Black, columns::line_color(shown))];
    found.extend(edge);
}

/// Where a click scrolls to: line `line` of `total` (0 = the oldest) in the middle of the column.
fn jump(line: usize, height: usize, total: usize) -> usize {
    (total - 1 - line.min(total - 1)).saturating_sub(height / 2).min(total.saturating_sub(height))
}

/// The rail's rows: the most serious line in each one's share of `list`, and where it is in it (the first of them);
/// and how fast each one's lines came.
fn map(ui: &Ui, state: &AppState, list: &[Shown], search: Option<&Search>, rows: usize) -> Map {
    let drawn = Drawn {
        total: state.output.total(),
        shown: list.len(),
        rows,
        filter: state.filter,
        search: search.map(|search| (search.query.clone(), search.regex, search.case)),
        marks: state.marks.len(),
    };
    let mut maps = ui.rails.0.borrow_mut();
    if let Some((was, map)) = maps.get(&state.key)
        && *was == drawn
    {
        return map.clone();
    }
    let mut marks = vec![(Mark::Empty, 0); rows];
    for (at, item) in list.iter().enumerate() {
        let row = at * rows / list.len();
        let mark = mark(state, item, search);
        if mark > marks[row].0 {
            marks[row] = (mark, at);
        }
    }
    let map = Map { rows: marks, density: rail_density::strip(list, rows) };
    maps.insert(state.key.clone(), (drawn, map.clone()));
    map
}

fn mark(state: &AppState, item: &Shown, search: Option<&Search>) -> Mark {
    match item.line.severity() {
        Level::Error => Mark::Error,
        Level::Warn => Mark::Warning,
        _ if search.is_some_and(|search| search.matches(&item.line.text)) => Mark::Match,
        _ if state.marks.binary_search(&state.output.number_at(item.position)).is_ok() => Mark::Marked,
        _ => Mark::Normal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_is_brought_to_the_middle_as_far_as_there_is_room() {
        assert_eq!(jump(500, 10, 1000), 1000 - 1 - 500 - 5);
        assert_eq!(jump(0, 10, 1000), 1000 - 10, "the oldest: as far back as there is");
        assert_eq!(jump(998, 10, 1000), 0, "near the newest: following again");
    }
}
