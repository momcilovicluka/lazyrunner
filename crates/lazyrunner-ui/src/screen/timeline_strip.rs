//! The timeline's activity strip, down its right edge: one lane per app, the whole stretch of time kept from the oldest
//! line at the top to the newest at the bottom - each cell shaded by how many lines the app logged then (against the
//! busiest cell), red where one was an error - and an edge beside the time the rows on screen cover.

use std::time::Instant;

use super::seg::{Seg, seg};
use super::theme::C;

/// From this wide the timeline has a strip.
pub const FROM_CELLS: usize = 60;
const SHADES: [char; 4] = ['░', '▒', '▓', '█'];

/// One cell of a lane: how many lines, and whether an error was among them.
#[derive(Clone, Copy, Default)]
struct Cell {
    lines: usize,
    error: bool,
}

/// How wide the strip is with `lanes` apps: a space, the edge, a cell a lane.
pub fn width(lanes: usize) -> usize {
    lanes + 2
}

/// The strip's `rows` rows: `lines` (when each came, its app, whether it's an error) the oldest first, a lane each
/// coloured as `colors`; the time the rows on screen cover, and whether the view follows new output.
pub fn strip(
    lines: &[(Instant, usize, bool)],
    colors: &[C],
    rows: usize,
    on_screen: Option<(Instant, Instant)>,
    live: bool,
) -> Vec<Vec<Seg>> {
    let (Some(oldest), Some(newest)) = (lines.first().map(|line| line.0), lines.last().map(|line| line.0)) else {
        return vec![vec![seg(" ".repeat(width(colors.len())), C::DarkGray)]; rows];
    };
    let span = newest.saturating_duration_since(oldest).as_secs_f64();
    let row_of = |born: Instant| {
        let share = if span > 0.0 { born.saturating_duration_since(oldest).as_secs_f64() / span } else { 1.0 };
        ((share * rows as f64) as usize).min(rows.saturating_sub(1))
    };
    let mut cells = vec![vec![Cell::default(); colors.len()]; rows];
    for (born, app, error) in lines {
        if let Some(cell) = cells[row_of(*born)].get_mut(*app) {
            cell.lines += 1;
            cell.error |= *error;
        }
    }
    let busiest = cells.iter().flatten().map(|cell| cell.lines).max().unwrap_or(0).max(1);
    let shown = on_screen.map(|(first, last)| row_of(first)..=row_of(last));
    let edge = if live { C::DarkGray } else { C::Cyan };
    cells
        .iter()
        .enumerate()
        .map(|(row, lanes)| {
            let on = shown.as_ref().is_some_and(|shown| shown.contains(&row));
            let mut segs = vec![seg(" ", C::DarkGray), seg(if on { "│" } else { " " }, edge)];
            segs.extend(lanes.iter().zip(colors).map(|(cell, color)| match cell.lines {
                0 => seg(" ", C::DarkGray),
                lines => {
                    let shade = SHADES[(lines * SHADES.len()).div_ceil(busiest) - 1];
                    seg(shade.to_string(), if cell.error { C::Red } else { *color })
                }
            }));
            segs
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn text(row: &[Seg]) -> String {
        row.iter().map(|seg| seg.text.as_str()).collect()
    }

    #[test]
    fn a_lane_is_shaded_where_its_app_logged_and_the_edge_marks_the_time_on_screen() {
        let at = Instant::now();
        let s = |secs| at + Duration::from_secs(secs);
        let lines = [(s(0), 0, false), (s(0), 0, false), (s(5), 1, true), (s(9), 0, false)];
        let rows = strip(&lines, &[C::Cyan, C::Magenta], 10, Some((s(5), s(9))), false);
        assert_eq!(rows.len(), 10);
        assert_eq!(text(&rows[0]), "  █ ", "the busiest cell, off screen");
        assert_eq!(text(&rows[3]), "    ", "a quiet stretch");
        assert_eq!(text(&rows[5]), " │ ▒", "half as busy, on screen");
        assert_eq!(text(&rows[9]), " │▒ ", "the newest line in the last row");
        assert_eq!((rows[0][2].fg, rows[5][3].fg, rows[5][1].fg), (C::Cyan, C::Red, C::Cyan), "lane, error, edge");
        let live = strip(&lines, &[C::Cyan, C::Magenta], 10, Some((s(5), s(9))), true);
        assert_eq!(live[5][1].fg, C::DarkGray, "the edge is dim while following");
    }

    #[test]
    fn with_no_lines_it_is_blank_and_as_wide() {
        let rows = strip(&[], &[C::Cyan, C::Magenta, C::Yellow], 4, None, true);
        assert_eq!(rows.iter().map(|row| text(row)).collect::<Vec<_>>(), vec!["     "; 4]);
    }
}
