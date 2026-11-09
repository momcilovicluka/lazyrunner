//! A layered graph drawn with box-drawing lines (the stack map, K; the build map can share it, #82): the nodes in
//! columns - a node's column comes after every node with an edge into it, cycles broken - their boxes stacked in each
//! column, level with what points into them. Every edge leaves its source's right side and enters its target's left
//! side, turning only in a lane of its own in a gap between columns: straight along its source's row (or its
//! target's) when no box is in the way, else down beside its source, along a row under the boxes and up beside its
//! target. No line runs behind a box.

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use super::mouse::Target;
use super::seg::{Seg, seg};
use super::theme::C;

const N: u8 = 1;
const E: u8 = 2;
const S: u8 = 4;
const W: u8 = 8;
/// Rows between the boxes of a column, and a gap's cells besides its lanes.
const ROW_GAP: usize = 1;
const GAP: usize = 4;

/// A box: its lines, the cells between them and its border, its border's colour, dashed or not, and what a click on
/// it means.
pub struct Node {
    pub lines: Vec<Vec<Seg>>,
    pub pad: usize,
    pub border: C,
    pub dashed: bool,
    pub target: Option<Target>,
}

/// A line from one box to another, with a label on its last stretch when there's room.
pub struct Edge {
    pub from: usize,
    pub to: usize,
    pub color: C,
    pub dashed: bool,
    pub label: String,
}

/// How an edge is drawn this frame: how much of it is in (0 to 1), a dot of a colour that has moved this many cells
/// along it (around and around), and a colour over its own.
#[derive(Clone, Copy, Debug)]
pub struct Motion {
    pub shown: f64,
    pub dot: Option<(f64, C)>,
    pub color: Option<C>,
}

impl Default for Motion {
    fn default() -> Self {
        Self { shown: 1.0, dot: None, color: None }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
}

#[derive(Clone, Debug)]
struct Cell {
    text: Option<String>,
    mask: u8,
    dashed: bool,
    color: C,
    bg: Option<C>,
    target: Option<Target>,
}

impl Default for Cell {
    fn default() -> Self {
        Self { text: None, mask: 0, dashed: false, color: C::Gray, bg: None, target: None }
    }
}

/// How an edge gets from its source to its target: a lane (a gap, its place in it) beside its source, one beside its
/// target, and a row under the boxes. Along its source's row it needs only the last; along its target's, only the
/// first; around the boxes, all three.
#[derive(Clone, Copy, Debug, Default)]
struct Way {
    first: Option<(usize, usize)>,
    last: Option<(usize, usize)>,
    bus: Option<usize>,
}

/// Where everything goes: each node's column and box.
pub struct Layout {
    pub columns: Vec<usize>,
    pub boxes: Vec<Rect>,
    /// Where each gap starts: the gap before column `g`, and one after the last.
    starts: Vec<usize>,
    /// Each edge's way, and its rows on its source's and its target's box, from their tops: the edges of a box spread
    /// over its rows, so lines into one box don't run together.
    ways: Vec<Way>,
    ends: Vec<(usize, usize)>,
    /// The first row under the boxes.
    below: usize,
    pub width: usize,
    pub height: usize,
}

/// The graph drawn.
pub struct Map {
    pub layout: Layout,
    cells: Vec<Vec<Cell>>,
}

/// The edges that close a cycle (to a node still being followed), found from the nodes in their order.
fn back_edges(count: usize, edges: &[(usize, usize)]) -> Vec<bool> {
    fn follow(node: usize, edges: &[(usize, usize)], state: &mut [u8], back: &mut [bool]) {
        state[node] = 1;
        for (at, &(from, to)) in edges.iter().enumerate() {
            if from != node {
                continue;
            }
            match state[to] {
                0 => follow(to, edges, state, back),
                1 => back[at] = true,
                _ => {}
            }
        }
        state[node] = 2;
    }
    let mut state = vec![0; count];
    let mut back = vec![false; edges.len()];
    for node in 0..count {
        if state[node] == 0 {
            follow(node, edges, &mut state, &mut back);
        }
    }
    back
}

/// Each node's column: after every node with an edge into it (cycles broken), the first column otherwise.
pub fn columns(count: usize, edges: &[(usize, usize)]) -> Vec<usize> {
    let back = back_edges(count, edges);
    let forward: Vec<(usize, usize)> =
        edges.iter().zip(&back).filter(|(edge, back)| !**back && edge.0 != edge.1).map(|(edge, _)| *edge).collect();
    let mut column = vec![0; count];
    for _ in 0..count {
        let mut moved = false;
        for &(from, to) in &forward {
            if column[to] < column[from] + 1 {
                column[to] = column[from] + 1;
                moved = true;
            }
        }
        if !moved {
            break;
        }
    }
    column
}

fn line_width(line: &[Seg]) -> usize {
    line.iter().map(|part| part.text.width()).sum()
}

/// Where the boxes and the edges go.
pub fn layout(nodes: &[Node], edges: &[Edge]) -> Layout {
    let pairs: Vec<(usize, usize)> = edges.iter().map(|edge| (edge.from, edge.to)).collect();
    let columns = columns(nodes.len(), &pairs);
    let count = columns.iter().max().map_or(0, |last| last + 1);
    let sizes: Vec<(usize, usize)> = nodes
        .iter()
        .map(|node| {
            let widest = node.lines.iter().map(|line| line_width(line)).max().unwrap_or(0);
            (widest + 2 + 2 * node.pad, node.lines.len() + 2)
        })
        .collect();
    let mut widths = vec![0; count];
    for (node, &column) in columns.iter().enumerate() {
        widths[column] = widths[column].max(sizes[node].0);
    }
    let ys = rows(&columns, &sizes, &pairs, count);
    let ends = ends(edges, &ys, &sizes);
    let (ways, taken, labels, buses) = ways(edges, &columns, &ys, &sizes, &ends, count);
    let gaps: Vec<usize> = (0..=count)
        .map(|gap| {
            let inner = gap > 0 && gap < count;
            if taken[gap] == 0 && !inner { 0 } else { GAP + 2 * taken[gap] + labels[gap] }
        })
        .collect();
    let mut starts = vec![0; count + 1];
    let mut left = vec![0; count];
    for column in 0..count {
        left[column] = starts[column] + gaps[column];
        starts[column + 1] = left[column] + widths[column];
    }
    let boxes: Vec<Rect> = (0..nodes.len())
        .map(|node| Rect { x: left[columns[node]], y: ys[node], w: sizes[node].0, h: sizes[node].1 })
        .collect();
    let below = boxes.iter().map(|at| at.y + at.h).max().unwrap_or(0);
    let height = below + 2 * buses;
    let width = starts[count] + gaps[count];
    Layout { columns, boxes, starts, ways, ends, below, width, height }
}

/// Each edge's way (see [`Way`]), how many lanes each gap has, the widest label ending in each, and the rows under
/// the boxes taken.
fn ways(
    edges: &[Edge],
    columns: &[usize],
    ys: &[usize],
    sizes: &[(usize, usize)],
    ends: &[(usize, usize)],
    count: usize,
) -> (Vec<Way>, Vec<usize>, Vec<usize>, usize) {
    // No box of a column strictly between `from` and `to` on `row`.
    let free = |row: usize, from: usize, to: usize| {
        (0..columns.len()).all(|node| {
            let column = columns[node];
            column <= from || column >= to || row < ys[node] || row >= ys[node] + sizes[node].1
        })
    };
    let mut taken = vec![0; count + 1];
    let mut labels = vec![0; count + 1];
    let mut buses = 0;
    let mut lane = |gap: usize| {
        taken[gap] += 1;
        (gap, taken[gap] - 1)
    };
    let mut ways = vec![Way::default(); edges.len()];
    for (at, edge) in edges.iter().enumerate() {
        if edge.from == edge.to {
            continue;
        }
        let (from, to) = (columns[edge.from], columns[edge.to]);
        let (sy, ty) = (ys[edge.from] + ends[at].0, ys[edge.to] + ends[at].1);
        let label = edge.label.width() + 1;
        ways[at] = if to > from && free(sy, from, to) {
            labels[to] = labels[to].max(label);
            Way { last: Some(lane(to)), ..Way::default() }
        } else if to > from && free(ty, from, to) {
            Way { first: Some(lane(from + 1)), ..Way::default() }
        } else {
            labels[to] = labels[to].max(label);
            buses += 1;
            Way { first: Some(lane(from + 1)), last: Some(lane(to)), bus: Some(buses - 1) }
        };
    }
    (ways, taken, labels, buses)
}

/// The `at`th of `count` edges on a side of a box `height` rows tall: spread over the rows inside it, the middle for
/// one.
fn slot(at: usize, count: usize, height: usize) -> usize {
    let inside = height.saturating_sub(2).max(1);
    if count <= 1 { height / 2 } else { 1 + (at * inside / count).min(inside - 1) }
}

/// Each edge's row on its source and its target: the edges leaving a box, and those entering one, each a row of
/// their own while there are rows - in the order of where their other end is, top first, so they don't cross.
fn ends(edges: &[Edge], ys: &[usize], sizes: &[(usize, usize)]) -> Vec<(usize, usize)> {
    let drawn = |at: usize| edges[at].from != edges[at].to;
    let middle = |node: usize| ys[node] + sizes[node].1 / 2;
    (0..edges.len())
        .map(|at| {
            let (from, to) = (edges[at].from, edges[at].to);
            let mut leaving: Vec<usize> = (0..edges.len()).filter(|&i| drawn(i) && edges[i].from == from).collect();
            let mut entering: Vec<usize> = (0..edges.len()).filter(|&i| drawn(i) && edges[i].to == to).collect();
            leaving.sort_by_key(|&i| (middle(edges[i].to), i));
            entering.sort_by_key(|&i| (middle(edges[i].from), i));
            let out = leaving.iter().position(|&i| i == at).unwrap_or(0);
            let into = entering.iter().position(|&i| i == at).unwrap_or(0);
            (slot(out, leaving.len(), sizes[from].1), slot(into, entering.len(), sizes[to].1))
        })
        .collect()
}

/// Each box's top, column by column: in the first, the nodes with edges before those without; in the others, each
/// node as level as it can be with the nodes that point into it (so their edges run straight), in that order.
fn rows(columns: &[usize], sizes: &[(usize, usize)], pairs: &[(usize, usize)], count: usize) -> Vec<usize> {
    let mut ys = vec![0; columns.len()];
    let linked = |node: usize| pairs.iter().any(|&(from, to)| from != to && (from == node || to == node));
    for column in 0..count {
        let mut members: Vec<usize> = (0..columns.len()).filter(|&node| columns[node] == column).collect();
        let wanted: Vec<Option<usize>> = (0..columns.len())
            .map(|node| {
                let middles: Vec<usize> = pairs
                    .iter()
                    .filter(|&&(from, to)| to == node && columns[from] < column)
                    .map(|&(from, _)| ys[from] + sizes[from].1 / 2)
                    .collect();
                (!middles.is_empty()).then(|| middles.iter().sum::<usize>() / middles.len())
            })
            .collect();
        if column == 0 {
            members.sort_by_key(|&node| !linked(node));
        } else {
            members.sort_by_key(|&node| wanted[node].unwrap_or(usize::MAX));
        }
        let mut next = 0;
        for node in members {
            let h = sizes[node].1;
            ys[node] = wanted[node].map_or(next, |middle| middle.saturating_sub(h / 2).max(next));
            next = ys[node] + h + ROW_GAP;
        }
    }
    ys
}

/// An edge's cells, in order from its source to its target, the side its first cell touches the source from, and
/// the arrow its last cell is.
type Route = (Vec<(usize, usize)>, u8, &'static str);

fn route(layout: &Layout, edge: &Edge, at: usize) -> Option<Route> {
    if edge.from == edge.to {
        return None;
    }
    let (from, to) = (layout.boxes[edge.from], layout.boxes[edge.to]);
    let x = |(gap, lane): (usize, usize)| layout.starts[gap] + 2 + 2 * lane;
    let (sy, ty) = (from.y + layout.ends[at].0, to.y + layout.ends[at].1);
    let (start, end) = ((from.x + from.w, sy), (to.x.checked_sub(1)?, ty));
    let way = layout.ways[at];
    let points = match (way.first, way.last, way.bus) {
        (Some(first), Some(last), Some(bus)) => {
            let row = layout.below + 1 + 2 * bus;
            vec![start, (x(first), sy), (x(first), row), (x(last), row), (x(last), ty), end]
        }
        (_, Some(last), None) => vec![start, (x(last), sy), (x(last), ty), end],
        (Some(first), None, None) => vec![start, (x(first), sy), (x(first), ty), end],
        _ => return None,
    };
    Some((cells(&points), W, "▶"))
}

/// The cells along straight stretches between the points.
fn cells(points: &[(usize, usize)]) -> Vec<(usize, usize)> {
    let mut out: Vec<(usize, usize)> = Vec::new();
    for pair in points.windows(2) {
        let ((x0, y0), (x1, y1)) = (pair[0], pair[1]);
        let steps = x0.abs_diff(x1).max(y0.abs_diff(y1));
        for step in 0..=steps {
            let x = if x1 >= x0 { x0 + step.min(x1 - x0) } else { x0 - step.min(x0 - x1) };
            let y = if y1 >= y0 { y0 + step.min(y1 - y0) } else { y0 - step.min(y0 - y1) };
            if out.last() != Some(&(x, y)) {
                out.push((x, y));
            }
        }
    }
    out
}

/// The way from one cell to the next.
fn toward(from: (usize, usize), to: (usize, usize)) -> u8 {
    match (to.0.cmp(&from.0), to.1.cmp(&from.1)) {
        (std::cmp::Ordering::Greater, _) => E,
        (std::cmp::Ordering::Less, _) => W,
        (_, std::cmp::Ordering::Greater) => S,
        _ => N,
    }
}

fn glyph(mask: u8, dashed: bool) -> &'static str {
    match mask {
        0 => " ",
        N | S | 5 if dashed => "┆",
        N | S | 5 => "│",
        E | W | 10 if dashed => "┄",
        E | W | 10 => "─",
        6 => "╭",
        12 => "╮",
        3 => "╰",
        9 => "╯",
        7 => "├",
        13 => "┤",
        14 => "┬",
        11 => "┴",
        _ => "┼",
    }
}

impl Map {
    /// The graph: the edges (each with its motion), then the boxes over them.
    pub fn draw(nodes: &[Node], edges: &[Edge], motion: &[Motion]) -> Self {
        let layout = layout(nodes, edges);
        let mut map = Self { cells: vec![vec![Cell::default(); layout.width + 2]; layout.height + 2], layout };
        for (at, edge) in edges.iter().enumerate() {
            map.edge(edge, at, motion.get(at).copied().unwrap_or_default());
        }
        for (at, node) in nodes.iter().enumerate() {
            map.node(node, map.layout.boxes[at]);
        }
        map
    }

    fn cell(&mut self, x: usize, y: usize) -> Option<&mut Cell> {
        self.cells.get_mut(y)?.get_mut(x)
    }

    fn edge(&mut self, edge: &Edge, at: usize, motion: Motion) {
        let Some((path, start, arrow)) = route(&self.layout, edge, at) else { return };
        let shown = ((motion.shown.clamp(0.0, 1.0) * path.len() as f64).ceil() as usize).min(path.len());
        let color = motion.color.unwrap_or(edge.color);
        for i in 0..shown {
            let mut mask = if i == 0 { start } else { toward(path[i], path[i - 1]) };
            if i + 1 < path.len() {
                mask |= toward(path[i], path[i + 1]);
            }
            let last = i + 1 == path.len();
            let Some(cell) = self.cell(path[i].0, path[i].1) else { continue };
            cell.mask |= mask;
            cell.dashed = edge.dashed;
            cell.color = color;
            if last {
                cell.text = Some(arrow.into());
            }
        }
        if shown < path.len() {
            return;
        }
        if let Some((moved, dot)) = motion.dot {
            let at = (moved.max(0.0) as usize) % path.len().saturating_sub(1).max(1);
            if let Some(cell) = self.cell(path[at].0, path[at].1) {
                cell.text = Some("•".into());
                cell.color = dot;
            }
        }
        self.label(edge, &path, color); // over the dot: it passes behind the label
    }

    /// The label on the edge's last straight stretch, after its bend, when it fits.
    fn label(&mut self, edge: &Edge, path: &[(usize, usize)], color: C) {
        if edge.label.is_empty() || path.len() < 2 {
            return;
        }
        let y = path[path.len() - 1].1;
        let run = path.iter().rev().take_while(|(_, row)| *row == y).count();
        let room = run.saturating_sub(3);
        if edge.label.width() > room {
            return;
        }
        let start = path[path.len() - run].0 + 1 + (room - edge.label.width()) / 2;
        for (offset, grapheme) in edge.label.graphemes(true).enumerate() {
            if let Some(cell) = self.cell(start + offset, y) {
                cell.text = Some(grapheme.into());
                cell.color = color;
            }
        }
    }

    fn put(&mut self, x: usize, y: usize, text: &str, color: C, bg: Option<C>, target: Option<Target>) {
        if let Some(cell) = self.cell(x, y) {
            *cell = Cell { text: Some(text.into()), mask: 0, dashed: false, color, bg, target };
        }
    }

    fn node(&mut self, node: &Node, at: Rect) {
        let (side, line) = if node.dashed { ("┆", "┄") } else { ("│", "─") };
        let last = at.y + at.h - 1;
        for x in at.x..at.x + at.w {
            let (top, bottom) = match x {
                _ if x == at.x => ("╭", "╰"),
                _ if x == at.x + at.w - 1 => ("╮", "╯"),
                _ => (line, line),
            };
            self.put(x, at.y, top, node.border, None, node.target);
            self.put(x, last, bottom, node.border, None, node.target);
        }
        for y in at.y + 1..last {
            self.put(at.x, y, side, node.border, None, node.target);
            self.put(at.x + at.w - 1, y, side, node.border, None, node.target);
            for x in at.x + 1..at.x + at.w - 1 {
                self.put(x, y, " ", C::Gray, None, node.target);
            }
        }
        for (row, parts) in node.lines.iter().enumerate() {
            let mut x = at.x + 1 + node.pad;
            for part in parts {
                for grapheme in part.text.graphemes(true) {
                    self.put(x, at.y + 1 + row, grapheme, part.fg, part.bg, node.target);
                    let wide = grapheme.width();
                    for extra in 1..wide {
                        self.put(x + extra, at.y + 1 + row, "", part.fg, part.bg, node.target);
                    }
                    x += wide.max(1);
                }
            }
        }
    }

    /// Its rows from column `from` on, `cells` wide at most.
    pub fn rows(&self, from: usize, cells: usize) -> Vec<Vec<Seg>> {
        self.cells
            .iter()
            .map(|row| {
                let mut segs: Vec<Seg> = Vec::new();
                let mut used = 0;
                for cell in row.iter().skip(from) {
                    let text = cell.text.as_deref().unwrap_or_else(|| glyph(cell.mask, cell.dashed));
                    if text.is_empty() {
                        continue;
                    }
                    if used + text.width() > cells {
                        break;
                    }
                    used += text.width();
                    match segs.last_mut() {
                        Some(last) if last.fg == cell.color && last.bg == cell.bg && last.target == cell.target => {
                            last.text.push_str(text);
                        }
                        _ => {
                            let mut part = seg(text, cell.color);
                            part.bg = cell.bg;
                            part.target = cell.target;
                            segs.push(part);
                        }
                    }
                }
                segs
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(name: &str) -> Node {
        Node { lines: vec![vec![seg(name, C::White)]], pad: 1, border: C::Gray, dashed: false, target: None }
    }

    fn edge(from: usize, to: usize, label: &str) -> Edge {
        Edge { from, to, color: C::Cyan, dashed: false, label: label.into() }
    }

    fn text(map: &Map) -> String {
        let rows = map.rows(0, 200);
        let line =
            |row: &Vec<Seg>| row.iter().map(|part| part.text.as_str()).collect::<String>().trim_end().to_string();
        rows.iter().map(line).collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn a_node_comes_after_every_node_with_an_edge_into_it_and_cycles_are_broken() {
        assert_eq!(columns(4, &[(0, 1), (1, 2), (0, 2)]), [0, 1, 2, 0]);
        assert_eq!(columns(3, &[(0, 1), (1, 0), (1, 2)]), [0, 1, 2], "the edge back to 0 is a cycle's");
    }

    #[test]
    fn edges_run_from_a_boxs_right_side_into_the_next_ones_left_side_with_their_label() {
        let nodes = [node("Shop"), node("Orders"), node("Billing")];
        let edges = [edge(0, 1, "2.0/s"), edge(1, 2, ""), edge(0, 2, "")];
        let drawn = text(&Map::draw(&nodes, &edges, &[]));
        let expected = [
            "╭──────╮              ╭────────╮         ╭─────────╮",
            "│ Shop │────2.0/s────▶│ Orders │────┬───▶│ Billing │",
            "╰──────╯    │         ╰────────╯    │    ╰─────────╯",
            "            │                       │",
            "            ╰───────────────────────╯",
        ];
        assert_eq!(drawn.trim_end(), expected.join("\n"), "Shop to Billing goes around Orders, never behind it");
    }

    #[test]
    fn a_box_is_level_with_what_points_into_it_and_the_linked_ones_come_first() {
        let nodes = [node("WebAPI"), node("Admin"), node("Frontend"), node("db")];
        let map = Map::draw(&nodes, &[edge(2, 0, ""), edge(0, 3, "")], &[]);
        let drawn = text(&map);
        let first = drawn.lines().nth(1).unwrap_or_default();
        assert!(
            first.contains("Frontend") && first.contains("WebAPI") && first.contains("db"),
            "in a line: 
{drawn}"
        );
        assert!(
            drawn.lines().nth(5).is_some_and(|line| line.contains("Admin")),
            "the unlinked one after: 
{drawn}"
        );
    }

    #[test]
    fn edges_into_a_tall_box_each_have_a_row_of_their_own() {
        let tall = |name: &str| Node {
            lines: vec![vec![seg(name, C::White)], vec![seg("up", C::Green)], vec![seg("2 errors", C::Red)]],
            pad: 2,
            border: C::Gray,
            dashed: false,
            target: None,
        };
        let nodes = [tall("Admin"), tall("Frontend"), tall("WebAPI")];
        let drawn = text(&Map::draw(&nodes, &[edge(0, 2, ""), edge(1, 2, "")], &[]));
        let arrows: Vec<usize> =
            drawn.lines().enumerate().filter(|(_, line)| line.contains('▶')).map(|(at, _)| at).collect();
        assert_eq!(arrows.len(), 2, "two rows into WebAPI: \n{drawn}");
        assert!(drawn.contains("│  WebAPI"), "two cells of padding: \n{drawn}");
    }

    #[test]
    fn an_edge_back_to_an_earlier_column_runs_under_the_boxes() {
        let nodes = [node("A"), node("B")];
        let map = Map::draw(&nodes, &[edge(0, 1, ""), edge(1, 0, "")], &[]);
        let drawn = text(&map);
        let expected = [
            "       ╭───╮       ╭───╮",
            "  ╭───▶│ A │──────▶│ B │──╮",
            "  │    ╰───╯       ╰───╯  │",
            "  │                       │",
            "  ╰───────────────────────╯",
        ];
        assert_eq!(drawn.trim_end(), expected.join("\n"), "out of B's right side, under, into A's left side");
        assert_eq!(map.layout.columns, [0, 1]);
    }

    #[test]
    fn an_edge_that_skips_a_column_runs_straight_where_nothing_is_in_the_way() {
        let tall = |name: &str| Node {
            lines: vec![vec![seg(name, C::White)], vec![seg("up", C::Green)]],
            pad: 1,
            border: C::Gray,
            dashed: false,
            target: None,
        };
        let nodes = [tall("WebAPI"), tall("Admin"), tall("Frontend"), tall("db")];
        let drawn = text(&Map::draw(&nodes, &[edge(2, 0, ""), edge(0, 3, ""), edge(1, 3, "")], &[]));
        let expected = [
            "╭───────╮",
            "│ Admin │",
            "│ up    │────────────────────────╮    ╭────╮",
            "╰───────╯                        ╰───▶│ db │",
            "                               ╭─────▶│ up │",
            "╭──────────╮       ╭────────╮  │      ╰────╯",
            "│ Frontend │       │ WebAPI │  │",
            "│ up       │──────▶│ up     │──╯",
            "╰──────────╯       ╰────────╯",
        ];
        let why = "Admin to db straight over WebAPI's column, the edges into db in their order, none crossing";
        assert_eq!(drawn.trim_end(), expected.join("\n"), "{why}");
    }

    #[test]
    fn half_shown_an_edge_has_no_arrow_yet_and_a_dot_moves_along_it() {
        let nodes = [node("A"), node("B")];
        let half = Map::draw(&nodes, &[edge(0, 1, "")], &[Motion { shown: 0.5, ..Motion::default() }]);
        assert!(!text(&half).contains('▶'), "\n{}", text(&half));
        let dotted =
            Map::draw(&nodes, &[edge(0, 1, "")], &[Motion { dot: Some((2.0, C::White)), ..Motion::default() }]);
        assert!(text(&dotted).contains('•'), "\n{}", text(&dotted));
    }
}
