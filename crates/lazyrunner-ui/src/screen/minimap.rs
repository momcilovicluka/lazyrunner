//! A small picture of a graph too big for the room it has (#89): its boxes, each a cell or a few, in their colors,
//! the part in view on a dotted ground, framed.

use super::graph::Rect;
use super::seg::{Seg, seg};
use super::theme::C;

/// The minimap, `size` cells (its frame included), of a map `whole` cells: its boxes, each in its color, `view` the
/// part of it in view.
pub fn rows(boxes: &[(Rect, C)], whole: (usize, usize), view: Rect, (width, height): (usize, usize)) -> Vec<Vec<Seg>> {
    let (inner_w, inner_h) = (width.saturating_sub(2).max(1), height.saturating_sub(2).max(1));
    let (map_w, map_h) = (whole.0.max(1), whole.1.max(1));
    let span = |cell: usize, cells: usize, whole: usize| (cell * whole / cells, ((cell + 1) * whole).div_ceil(cells));
    let meets = |rect: &Rect, (x0, x1): (usize, usize), (y0, y1): (usize, usize)| {
        rect.x < x1 && x0 < rect.x + rect.w && rect.y < y1 && y0 < rect.y + rect.h
    };
    let mut lines = vec![frame("╭", "╮", width)];
    for row in 0..inner_h {
        let ys = span(row, inner_h, map_h);
        let mut line = vec![seg("│", C::DarkGray)];
        for column in 0..inner_w {
            let xs = span(column, inner_w, map_w);
            let seen = meets(&view, xs, ys);
            let found = boxes.iter().filter(|(rect, _)| meets(rect, xs, ys)).map(|&(_, color)| color);
            let color = found.reduce(|kept, color| if kept == C::Gray || kept == C::DarkGray { color } else { kept });
            line.push(match color {
                Some(color) if seen => seg("■", color),
                Some(_) => seg("▪", C::DarkGray),
                None if seen => seg("·", C::DarkGray),
                None => seg(" ", C::Gray),
            });
        }
        line.push(seg("│", C::DarkGray));
        lines.push(line);
    }
    lines.push(frame("╰", "╯", width));
    lines
}

fn frame(left: &str, right: &str, width: usize) -> Vec<Seg> {
    vec![seg(format!("{left}{}{right}", "─".repeat(width.saturating_sub(2))), C::DarkGray)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_minimap_shows_the_boxes_and_dots_the_part_in_view() {
        let rect = |x, y, w, h| Rect { x, y, w, h };
        let boxes = [(rect(0, 0, 10, 3), C::Cyan), (rect(90, 27, 10, 3), C::Gray)];
        let text = |line: &Vec<Seg>| line.iter().map(|part| part.text.as_str()).collect::<String>();
        let drawn: Vec<String> = rows(&boxes, (100, 30), rect(0, 0, 50, 15), (12, 5)).iter().map(text).collect();
        assert_eq!(drawn.len(), 5, "{drawn:#?}");
        assert!(drawn[1].starts_with("│■····"), "the box in view, then the view's ground: {drawn:#?}");
        assert!(drawn[3].ends_with("▪│"), "the box out of view: {drawn:#?}");
    }
}
