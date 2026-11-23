//! The database screen's save points (#110), drawn: a rail under the databases - the live database the glowing
//! `◉ now` node on top, each save point a node below it (`◆` a snapshot, `◇` a backup, `◌` one dropped outside
//! lazyrunner), the stretch down to the picked one lit and labelled with what changed since - and beside it what
//! changed, as bars diverging from an axis: rows added growing right in green, removed growing left in red, a table
//! whose rows changed in place an amber `◆`. The rail is the migrations rail's braille track; "you are here" is its
//! bead. Everything here is drawn from a [`Rail`] and a [`Panel`] the screen works out each frame, motion included.

use unicode_width::UnicodeWidthStr;

use super::columns::fit;
use super::seg::{Seg, on, pad, seg, take_cells};
use super::theme::C;

/// The rail's braille, as the migrations rail draws it: its line (the left column of dots), the same dashed (the
/// stretch a rewind crossed), a row's four dots on the left, and the bead's (both columns).
const BRAILLE: u32 = 0x2800;
const LINE: u32 = 0x47;
const DASHED: u32 = 0x05;
const BOTH: [u32; 4] = [0x09, 0x12, 0x24, 0xC0];
/// The rows a save point takes on the rail: the stretch above it, then its node.
pub const ROWS_EACH: usize = 2;
/// The glyphs: the live database, a snapshot, a backup, one that's gone.
pub const NOW: &str = "◉";
const SNAPSHOT: &str = "◆";
const BACKUP: &str = "◇";
const GONE: &str = "◌";
/// A ripple's ring reaches this many quarter rows from its node.
const RIPPLE_REACH: f64 = 6.0;
/// The changes' table names get at most this many cells, the bars on each side of the axis between these.
const NAME_MOST: usize = 24;
const SIDE: (usize, usize) = (8, 16);
/// A bar's cell, and the axis it grows from (an amber diamond on it: changed in place).
const BEAD: &str = "▰";
const AXIS: &str = "│";
const IN_PLACE: &str = "◆";
/// Between the parts of what the changes' title line says on its right: the later ones go first when it's narrow.
const KEY_SEPARATOR: &str = " · ";

/// A save point on the rail.
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub label: String,
    pub clock: String,
    /// `snapshot · ⟲ pinned`, `backup`, `gone - dropped outside lazyrunner`.
    pub detail: String,
    pub backup: bool,
    pub gone: bool,
    pub pinned: bool,
}

/// What the rail shows: its database, its save points (the newest first), the picked one and whether the rail has
/// the keys, what changed since the picked one, the bead ("you are here", in rows from the top: 0 is now) and its
/// colour and label, the stretch a rewind crossed, a new node dropping in (where it is) and its ripple (its node,
/// how far), and a line under the title (a job, a hint).
#[derive(Clone, Debug, PartialEq)]
pub struct Rail {
    pub database: String,
    pub nodes: Vec<Node>,
    pub picked: usize,
    pub focused: bool,
    pub since: Option<String>,
    pub bead: f64,
    pub bead_color: C,
    pub here: String,
    pub drained: bool,
    pub dropping: Option<f64>,
    pub ripple: Option<(usize, f64)>,
    pub note: Option<(String, C)>,
}

impl Rail {
    pub fn new(database: &str) -> Self {
        Self {
            database: database.to_string(),
            nodes: Vec::new(),
            picked: 0,
            focused: false,
            since: None,
            bead: 0.0,
            bead_color: C::Cyan,
            here: "you are here".into(),
            drained: false,
            dropping: None,
            ripple: None,
            note: None,
        }
    }
}

/// The rail row save point `at` (the newest first) is on.
pub fn node_row(at: usize) -> usize {
    1 + ROWS_EACH * at + 1
}

/// A row on the rail as a dot: its row, and which of the row's four dots.
fn dot(row: f64) -> (usize, usize) {
    let sub = (row * 4.0 + 1.5).floor().max(0.0) as usize;
    (sub / 4, sub % 4)
}

/// The track's cells: its line, lit from now down to the picked node, the stretch a rewind crossed drained, the bead,
/// a node dropping in, a ripple's ring.
fn track(rail: &Rail, rows: usize) -> Vec<(u32, C)> {
    let lit_to = rail.nodes.get(rail.picked).map(|_| node_row(rail.picked));
    let mut track: Vec<(u32, C)> = (0..rows)
        .map(|row| match lit_to {
            Some(to) if row <= to && !rail.nodes.is_empty() => (LINE, C::Cyan),
            _ => (LINE, C::DarkGray),
        })
        .collect();
    if rail.drained {
        let (cell, _) = dot(rail.bead);
        for slot in track.iter_mut().take(cell) {
            *slot = (DASHED, C::DarkGray);
        }
    }
    if let Some((at, share)) = rail.ripple.filter(|(_, share)| *share < 1.0) {
        let center = (node_row(at) * 4 + 1) as i64;
        let reach = (share * RIPPLE_REACH).round() as i64;
        let color = if share < 0.5 { C::White } else { C::DarkCyan };
        for sub in [center - reach, center + reach].into_iter().filter(|sub| reach >= 1 && *sub >= 0) {
            let (cell, k) = (sub as usize / 4, sub as usize % 4);
            if let Some(slot) = track.get_mut(cell) {
                *slot = (slot.0 | BOTH[k], color);
            }
        }
    }
    if let Some(row) = rail.dropping {
        let (cell, k) = dot(row);
        if let Some(slot) = track.get_mut(cell) {
            *slot = (LINE | BOTH[k], C::White);
        }
    }
    let (cell, k) = dot(rail.bead);
    if let Some(slot) = track.get_mut(cell) {
        *slot = (LINE | BOTH[k], rail.bead_color);
    }
    track
}

/// The rail in `width`, `rows` rows at most (the older save points cut when they don't fit).
pub fn rail(rail: &Rail, width: usize, rows: usize) -> Vec<Vec<Seg>> {
    let mut lines = vec![vec![seg(format!(" SAVE POINTS · {}", rail.database), C::DarkCyan)]];
    if let Some((note, color)) = &rail.note {
        lines.push(fit(vec![seg(format!(" {note}"), *color)], width));
    }
    let room = rows.saturating_sub(lines.len());
    let tall = (1 + ROWS_EACH * rail.nodes.len()).min(room.max(1));
    let track = track(rail, tall);
    let (bead_row, _) = dot(rail.bead);
    for (row, (dots, color)) in track.iter().enumerate() {
        let mark = char::from_u32(BRAILLE + dots).unwrap_or(' ');
        let mut segs = vec![seg(format!("  {mark} "), *color)];
        if row == 0 {
            let leaving = rail.drained || rail.bead >= 0.5;
            segs.push(seg(NOW, if leaving { C::DarkGray } else { C::Cyan }));
            segs.push(seg("  now", if leaving { C::DarkGray } else { C::White }));
        } else if row % ROWS_EACH == 1 {
            if row == 1
                && let Some(since) = &rail.since
            {
                segs.push(seg(format!("   {since}"), C::DarkGray));
            }
        } else if let Some(node) = rail.nodes.get((row - 2) / ROWS_EACH) {
            let at = (row - 2) / ROWS_EACH;
            let landed = rail.dropping.is_none() || at != 0;
            segs.extend(node_segs(rail, node, at, landed));
        }
        if row == bead_row {
            segs.push(seg(format!("  ◂ {}", rail.here), rail.bead_color));
        }
        lines.push(fit(segs, width));
    }
    if rail.nodes.is_empty() {
        lines.push(fit(
            vec![seg("     no save points yet - s makes a snapshot (a moment), S a backup (slower)", C::DarkGray)],
            width,
        ));
    }
    lines.truncate(rows);
    lines
}

/// A save point's node and its words: its glyph, label, time and what it is - picked, it's lit.
fn node_segs(rail: &Rail, node: &Node, at: usize, landed: bool) -> Vec<Seg> {
    let (glyph, color) = match (node.gone, node.backup) {
        (true, _) => (GONE, C::DarkGray),
        (false, true) => (BACKUP, C::Cyan),
        (false, false) => (SNAPSHOT, C::Cyan),
    };
    let rippling = rail.ripple.is_some_and(|(node, share)| node == at && share < 0.5);
    let glyph_color = if rippling { C::White } else { color };
    let mut segs = vec![seg(if landed { glyph } else { " " }, glyph_color)];
    let text = match node.label.ends_with(&node.clock) {
        true => format!("  {}", node.label),
        false => format!("  {}  {}", node.label, node.clock),
    };
    let picked = at == rail.picked;
    segs.push(match (picked, rail.focused) {
        (true, true) => on(text, C::FocusText, C::FocusBack),
        (true, false) => seg(text, C::White),
        _ => seg(text, if node.gone { C::DarkGray } else { C::Gray }),
    });
    let detail_color = if node.pinned { C::Cyan } else { C::DarkGray };
    segs.push(seg(format!("  {}", node.detail), detail_color));
    segs
}

/// A table's bar in the changes panel: its name, rows then, and - as drawn now, tweening - its rows added or removed
/// and whether its rows changed in place; how far its bar has grown (0 to 1).
#[derive(Clone, Debug, PartialEq)]
pub struct Bar {
    pub table: String,
    pub then: u64,
    pub delta: f64,
    pub changed: bool,
    pub grown: f64,
}

/// What the changes panel shows: its title, what it says on the right, a line under it (counting, an error), and
/// the tables' bars, the biggest change first.
#[derive(Clone, Debug, PartialEq)]
pub struct Panel {
    pub title: String,
    pub right: String,
    pub note: Option<(String, C)>,
    pub bars: Vec<Bar>,
}

/// `88020` as `88 020`.
pub fn thousands(number: u64) -> String {
    let digits = number.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (at, digit) in digits.chars().enumerate() {
        if at > 0 && (digits.len() - at).is_multiple_of(3) {
            out.push(' ');
        }
        out.push(digit);
    }
    out
}

/// The changes panel in `width`, `rows` rows at most.
pub fn panel(panel: &Panel, width: usize, rows: usize) -> Vec<Vec<Seg>> {
    let title = format!(" {}", panel.title);
    let room = width.saturating_sub(title.width() + 2);
    let first = panel.right.split(KEY_SEPARATOR).next().unwrap_or_default();
    let right = [panel.right.as_str(), first].into_iter().find(|text| text.width() <= room).unwrap_or_default();
    let gap = width.saturating_sub(title.width() + right.width() + 1);
    let mut lines =
        vec![vec![seg(title, C::DarkCyan), seg(" ".repeat(gap), C::DarkGray), seg(right.to_string(), C::DarkGray)]];
    if let Some((note, color)) = &panel.note {
        lines.push(fit(vec![seg(format!(" {note}"), *color)], width));
    }
    if panel.bars.is_empty() {
        lines.truncate(rows);
        return lines;
    }
    let name = panel.bars.iter().map(|bar| bar.table.width()).max().unwrap_or(0).clamp(6, NAME_MOST);
    let numbers = panel.bars.iter().map(|bar| figures(bar).width()).max().unwrap_or(0);
    let side = (width.saturating_sub(2 + name + 2 + 1 + 2 + numbers) / 2).clamp(SIDE.0, SIDE.1);
    let most = panel.bars.iter().map(|bar| bar.delta.abs()).fold(0.0, f64::max).max(1.0);
    let axis = format!("{}removed {AXIS} added", " ".repeat(2 + name + 2 + side.saturating_sub("removed ".width())));
    lines.push(fit(vec![seg(axis, C::DarkGray)], width));
    for bar in &panel.bars {
        lines.push(fit(bar_row(bar, name, side, most, numbers), width));
    }
    lines.truncate(rows);
    lines
}

/// What a bar's row says after it: `+12   88 020 → 88 032`, `~ changed   412`.
fn figures(bar: &Bar) -> String {
    let delta = bar.delta.round() as i64;
    let now = (bar.then as i64 + delta).max(0) as u64;
    match delta {
        0 if bar.changed => format!("~ changed  {:>9}", thousands(bar.then)),
        0 => format!("{:>4}  {:>9} → {}", "0", thousands(bar.then), thousands(now)),
        _ => {
            let sign = if delta > 0 { format!("+{delta}") } else { format!("−{}", -delta) };
            format!("{sign:>4}  {:>9} → {}", thousands(bar.then), thousands(now))
        }
    }
}

fn bar_row(bar: &Bar, name: usize, side: usize, most: f64, numbers: usize) -> Vec<Seg> {
    let cells = |delta: f64| {
        let share = (delta.abs() / most * side as f64 * bar.grown.clamp(0.0, 1.0)).ceil() as usize;
        if delta.abs() < 0.5 { 0 } else { share.clamp(1, side) }
    };
    let (removed, added) = if bar.delta < 0.0 { (cells(bar.delta), 0) } else { (0, cells(bar.delta)) };
    let table =
        if bar.table.width() > name { format!("{}…", take_cells(&bar.table, name - 1)) } else { bar.table.clone() };
    let in_place = bar.changed && bar.delta.round() == 0.0;
    let (axis, axis_color) = if in_place { (IN_PLACE, C::Yellow) } else { (AXIS, C::DarkGray) };
    let color = if in_place {
        C::Yellow
    } else if bar.delta < 0.0 {
        C::Red
    } else {
        C::Green
    };
    vec![
        seg(format!("  {}  ", pad(&table, name)), C::Gray),
        seg(" ".repeat(side - removed), C::DarkGray),
        seg(BEAD.repeat(removed), C::Red),
        seg(axis, axis_color),
        seg(BEAD.repeat(added), C::Green),
        seg(" ".repeat(side - added + 2), C::DarkGray),
        seg(pad(&figures(bar), numbers), color),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(lines: &[Vec<Seg>]) -> String {
        lines
            .iter()
            .map(|line| line.iter().map(|part| part.text.as_str()).collect::<String>().trim_end().to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn node(label: &str, clock: &str, detail: &str, backup: bool) -> Node {
        Node { label: label.into(), clock: clock.into(), detail: detail.into(), backup, gone: false, pinned: false }
    }

    fn sample() -> Rail {
        let mut rail = Rail::new("Shop");
        rail.nodes = vec![
            Node { pinned: true, ..node("before approval", "10:42", "snapshot · ⟲ pinned", false) },
            node("after import", "09:15", "backup", true),
            Node { gone: true, ..node("before 08:00", "08:00", "gone - dropped outside lazyrunner", false) },
        ];
        rail.since = Some("+16 −2 rows in 5 tables since \"before approval\"".into());
        rail
    }

    #[test]
    fn the_rail_shows_now_on_top_and_each_save_point_below_it() {
        insta::assert_snapshot!(text(&rail(&sample(), 90, 20)));
    }

    #[test]
    fn a_rewind_glides_you_are_here_down_the_rail_and_drains_what_it_crossed() {
        let mut rewinding = sample();
        rewinding.picked = 1;
        rewinding.focused = true;
        rewinding.bead = 2.5;
        rewinding.drained = true;
        rewinding.bead_color = C::White;
        rewinding.here = "you are here · rewinding 62%".into();
        insta::assert_snapshot!(text(&rail(&rewinding, 90, 20)));
        assert_eq!(dot(0.0), (0, 1), "now: its row's middle");
        assert_eq!(dot(node_row(1) as f64).0, 4);
    }

    #[test]
    fn no_save_points_says_how_to_make_one() {
        let empty = Rail::new("ShopLocal");
        let drawn = text(&rail(&empty, 90, 10));
        assert!(drawn.contains("◉  now  ◂ you are here") && drawn.contains("s makes a snapshot"), "{drawn}");
    }

    #[test]
    fn the_changes_diverge_from_an_axis_the_biggest_first() {
        let bar = |table: &str, then: u64, delta: f64, changed: bool| Bar {
            table: table.into(),
            then,
            delta,
            changed,
            grown: 1.0,
        };
        let changes = Panel {
            title: "SINCE \"before approval\" · 10:42".into(),
            right: "4 tables changed · u counts again".into(),
            note: None,
            bars: vec![
                bar("AuditLog", 88_020, 12.0, false),
                bar("OrderLines", 5_310, 3.0, false),
                bar("Drafts", 47, -2.0, true),
                bar("Orders", 1_204, 1.0, false),
                bar("Approvals", 412, 0.0, true),
            ],
        };
        insta::assert_snapshot!(text(&panel(&changes, 100, 12)));
        let half = Panel { bars: vec![Bar { grown: 0.5, ..changes.bars[0].clone() }], ..changes };
        let grown = text(&panel(&half, 100, 12));
        assert!(grown.contains(&format!("│{}", BEAD.repeat(8))) && !grown.contains(&BEAD.repeat(9)), "{grown}");
        assert_eq!(
            (thousands(88_032), thousands(412), thousands(1_000_000)),
            ("88 032".into(), "412".into(), "1 000 000".into())
        );
    }
}
