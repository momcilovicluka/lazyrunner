//! The database screen's copies, backups, exports and imports (#110), drawn: a job's wash across its database's row -
//! its edge an eighth of a cell at a time, a shimmer running along what's filled - folding into a file icon when it's
//! done; a new database's ghost row, its dashed rule filling solid from the left with the restore's progress; a row
//! that goes, dimming and closing up; and the files section, each file's size a bar of beads and its age fading it.
//! Everything here is drawn from plain values the screen works out each frame, motion included.

use unicode_width::UnicodeWidthStr;

use super::seg::{Seg, band, cells, eighth, focus_seg, gauge, on, pad, seg, width};
use super::theme::C;

/// The cells a shimmer lights.
const SHIMMER_CELLS: usize = 4;
/// A ghost row's rule, still dashed and filled solid.
const DASHED: &str = "╌";
const SOLID: &str = "━";
/// A file's size bar is this many beads at most.
pub const FILE_BEADS: usize = 10;
/// The files' names get at most this many cells.
const NAME_MOST: usize = 40;
/// The folder in the files' title: its last this many names.
const TAIL_NAMES: usize = 2;

/// A row a job washes across: its active mark, its name in `label_cells`, what the job says, how far it's come (0 to
/// 1), where the shimmer is along what's filled (0 to 1), how far it has folded away into `icon` once done (0 to 1),
/// and whether the selection bar is on it.
#[derive(Clone, Debug, PartialEq)]
pub struct Wash {
    pub mark: String,
    pub label: String,
    pub label_cells: usize,
    pub status: String,
    pub share: f64,
    pub shimmer: Option<f64>,
    pub fold: Option<f64>,
    pub icon: String,
    pub picked: bool,
}

/// A washed row in `cells`: filled from the left to its share on the accent, its edge an eighth block, a shimmer on
/// the filled part; folding, the wash draws back to the right into the file icon.
pub fn wash(wash: &Wash, cells_wide: usize) -> Vec<Seg> {
    let room = cells_wide.saturating_sub(wash.mark.width());
    let label = pad(&wash.label, wash.label_cells.min(room));
    let status = pad(&wash.status, room.saturating_sub(label.width()));
    let plain = vec![seg(label, C::White), seg(status, C::Yellow)];
    let plain: Vec<Seg> = if wash.picked { plain.into_iter().map(focus_seg).collect() } else { plain };
    let mut row = vec![seg(wash.mark.clone(), C::Green)];
    if let Some(fold) = wash.fold {
        let from = ((fold.clamp(0.0, 1.0)) * room as f64).round() as usize;
        let mut drawn = band(plain, from, room, C::Black, C::DarkCyan);
        let icon_at = room.saturating_sub(wash.icon.width().max(1) + 1);
        drawn = cells(&drawn, 0, icon_at);
        drawn.push(seg(" ".repeat(icon_at.saturating_sub(width(&drawn))), C::Gray));
        drawn.push(on(format!("{} ", wash.icon), C::Black, C::Cyan));
        row.extend(drawn);
        return row;
    }
    let eighths = (wash.share.clamp(0.0, 1.0) * (room * 8) as f64).round() as usize;
    let (whole, part) = (eighths / 8, eighths % 8);
    let mut filled = band(plain.clone(), 0, whole, C::Black, C::DarkCyan);
    if let Some(at) = wash.shimmer.filter(|_| whole > SHIMMER_CELLS) {
        let start = (at.clamp(0.0, 1.0) * (whole + SHIMMER_CELLS) as f64) as usize;
        filled = band(filled, start.saturating_sub(SHIMMER_CELLS), start.min(whole), C::Black, C::Cyan);
    }
    row.extend(cells(&filled, 0, whole));
    if part > 0 && whole < room {
        row.push(seg(eighth(part), C::DarkCyan));
        row.extend(cells(&plain, whole + 1, room));
    } else {
        row.extend(cells(&plain, whole, room));
    }
    row
}

/// A new database's ghost row: its name and what it says, how far the restore has come (0 to 1), and how far the row
/// has slid open (0 to 1). Once the database is there, its own row takes its place, glinting.
#[derive(Clone, Debug, PartialEq)]
pub struct Ghost {
    pub label: String,
    pub label_cells: usize,
    pub status: String,
    pub share: f64,
    pub opened: f64,
}

/// A ghost row in `cells`: `  ┆ Shop_copy @ localhost ╌╌╌ restoring · 40% ╌╌`, the rule and the words solid and lit
/// from the left to its share, dashed and dim after; while it opens, only its left part.
pub fn ghost(ghost: &Ghost, cells_wide: usize) -> Vec<Seg> {
    let lead = "   ";
    let room = cells_wide.saturating_sub(lead.width());
    let name = format!("{} ", ghost.label);
    let status = format!(" {} ", ghost.status);
    let rule = room.saturating_sub(name.width() + status.width());
    let before = ghost.label_cells.saturating_sub(name.width()).min(rule);
    let parts = [(name, true), (DASHED.repeat(before), false), (status, true), (DASHED.repeat(rule - before), false)];
    let lit_to = (ghost.share.clamp(0.0, 1.0) * room as f64).round() as usize;
    let mut row = vec![seg(lead, C::DarkGray)];
    let mut at = 0;
    for (text, words) in parts {
        for grapheme in unicode_segmentation::UnicodeSegmentation::graphemes(text.as_str(), true) {
            let lit = at < lit_to;
            let drawn = if !words && lit { SOLID } else { grapheme };
            let color = match (words, lit) {
                (true, true) => C::White,
                (false, true) => C::Cyan,
                _ => C::DarkGray,
            };
            row.push(seg(drawn, color));
            at += grapheme.width();
        }
    }
    let open = lead.width() + (ghost.opened.clamp(0.0, 1.0) * room as f64).round() as usize;
    merge(cells(&row, 0, open))
}

/// Neighbouring segments of the same colours joined: a row drawn a cell at a time, as few segments as it needs.
fn merge(segs: Vec<Seg>) -> Vec<Seg> {
    let mut out: Vec<Seg> = Vec::new();
    for part in segs {
        match out.last_mut() {
            Some(last) if last.fg == part.fg && last.bg == part.bg && last.style == part.style => {
                last.text.push_str(&part.text);
            }
            _ => out.push(part),
        }
    }
    out
}

/// A row that goes: dim, closing up towards its middle as `share` (0 to 1) goes by.
pub fn collapse(row: Vec<Seg>, share: f64) -> Vec<Seg> {
    let row = super::seg::dim(row);
    let wide = width(&row);
    let keep = ((1.0 - share.clamp(0.0, 1.0)) * wide as f64).round() as usize;
    let from = (wide - keep) / 2;
    let mut out = vec![seg(" ".repeat(from), C::DarkGray)];
    out.extend(cells(&row, from, from + keep));
    out
}

/// A row sliding in from the right: `share` (0 to 1) of the way in.
pub fn slide_in(row: Vec<Seg>, share: f64, cells_wide: usize) -> Vec<Seg> {
    let offset = ((1.0 - share.clamp(0.0, 1.0)) * cells_wide as f64).round() as usize;
    if offset == 0 {
        return row;
    }
    let mut out = vec![seg(" ".repeat(offset), C::DarkGray)];
    out.extend(cells(&row, 0, cells_wide.saturating_sub(offset)));
    out
}

/// A file's row: its icon, name, size and age, its bar's share of the biggest file's, its colours (older: dimmer),
/// where it is (a server's backup folder), a job on it, whether it's picked, and its motion: how far it has slid in,
/// how far it has gone.
#[derive(Clone, Debug, PartialEq)]
pub struct FileRow {
    pub icon: String,
    pub name: String,
    pub size: String,
    pub share: f64,
    pub age: String,
    pub color: C,
    pub bar: C,
    pub on: Option<String>,
    pub doing: Option<String>,
    pub picked: bool,
    pub slide: f64,
    pub leaving: Option<f64>,
}

/// The files section: its folder, a line under its title (a hint, a job), its files, and whether it has the keys.
#[derive(Clone, Debug, PartialEq)]
pub struct Files {
    pub folder: String,
    pub note: Option<(String, C)>,
    pub rows: Vec<FileRow>,
    pub focused: bool,
}

/// A folder by its last names: `…\shop\databases` - the log and the README say where it is in full.
fn tail(folder: &str) -> String {
    let separator = if folder.contains('\\') { "\\" } else { "/" };
    let names: Vec<&str> = folder.split(['\\', '/']).filter(|name| !name.is_empty()).collect();
    if names.len() <= TAIL_NAMES {
        return folder.to_string();
    }
    format!("…{separator}{}", names[names.len() - TAIL_NAMES..].join(separator))
}

/// The files section in `cells`, `rows` rows at most (the picked file kept in view).
pub fn files(files: &Files, cells_wide: usize, rows: usize) -> Vec<Vec<Seg>> {
    let title = format!(" FILES · {}", tail(&files.folder));
    let mut lines = vec![vec![seg(title, C::DarkCyan)]];
    if let Some((note, color)) = &files.note {
        lines.push(super::columns::fit(vec![seg(format!(" {note}"), *color)], cells_wide));
    }
    if files.rows.is_empty() {
        lines.truncate(rows);
        return lines;
    }
    let name = files.rows.iter().map(|row| row.name.width()).max().unwrap_or(0).min(NAME_MOST);
    let size = files.rows.iter().map(|row| row.size.width()).max().unwrap_or(0);
    let age = files.rows.iter().map(|row| row.age.width()).max().unwrap_or(0);
    let room = rows.saturating_sub(lines.len()).max(1);
    let picked = files.rows.iter().position(|row| row.picked).unwrap_or(0);
    let top = (picked + 1).saturating_sub(room);
    for row in files.rows.iter().skip(top).take(room) {
        lines.push(file_row(row, (name, size, age), files.focused, cells_wide));
    }
    lines.truncate(rows);
    lines
}

fn file_row(row: &FileRow, (name, size, age): (usize, usize, usize), focused: bool, cells_wide: usize) -> Vec<Seg> {
    let (beads, _) = gauge(row.share, FILE_BEADS);
    let beads = if row.share > 0.0 && beads.is_empty() { gauge(1.0, 1).0 } else { beads };
    let shown = if row.name.width() > name {
        format!("{}…", super::seg::take_cells(&row.name, name - 1))
    } else {
        row.name.clone()
    };
    let mut segs = vec![
        seg(format!("  {} ", row.icon), row.bar),
        seg(pad(&shown, name), row.color),
        seg(format!("  {:>size$}  ", row.size), row.color),
        seg(pad(&beads, FILE_BEADS), row.bar),
        seg(format!("  {}", pad(&row.age, age)), row.color),
    ];
    if let Some(on) = &row.on {
        segs.push(seg(format!("  {on}"), C::DarkGray));
    }
    if let Some(doing) = &row.doing {
        segs.push(seg(format!("  {doing}"), C::Yellow));
    }
    let mut segs = super::columns::fit(segs, cells_wide);
    if row.picked && focused {
        segs = super::seg::focus_row(segs, cells_wide);
    }
    if let Some(gone) = row.leaving {
        return collapse(segs, gone);
    }
    slide_in(segs, row.slide, cells_wide)
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

    fn sample_wash() -> Wash {
        Wash {
            mark: " ● ".into(),
            label: "Shop @ localhost".into(),
            label_cells: 26,
            status: "backing up · 64% · 1.5 GB of 2.4 GB · about 20s left".into(),
            share: 0.64,
            shimmer: None,
            fold: None,
            icon: "▤".into(),
            picked: false,
        }
    }

    #[test]
    fn a_backup_washes_across_its_row_its_edge_an_eighth_at_a_time() {
        let row = wash(&sample_wash(), 100);
        insta::assert_snapshot!(text(std::slice::from_ref(&row)));
        let filled: usize = row.iter().filter(|part| part.bg == Some(C::DarkCyan)).map(|part| part.text.width()).sum();
        assert_eq!(filled, 62, "64% of the 97 cells after the mark, whole");
        let edge = row.iter().find(|part| part.fg == C::DarkCyan && part.bg.is_none()).unwrap();
        assert_eq!(edge.text, "▏", "the next eighth");
        assert_eq!(width(&row), 100);
        let shimmering = wash(&Wash { shimmer: Some(0.5), ..sample_wash() }, 100);
        assert!(shimmering.iter().any(|part| part.bg == Some(C::Cyan)), "the shimmer lights part of the fill");
        let folding = wash(&Wash { fold: Some(0.5), ..sample_wash() }, 100);
        assert!(
            text(std::slice::from_ref(&folding)).ends_with("▤"),
            "it folds into the file icon: {}",
            text(&[folding])
        );
    }

    #[test]
    fn a_copys_ghost_row_fills_solid_from_the_left_as_it_restores() {
        let ghost_row = Ghost {
            label: "Shop_copy @ localhost".into(),
            label_cells: 26,
            status: "restoring · 40% · 1.0 GB of 2.4 GB · about 12s left".into(),
            share: 0.4,
            opened: 1.0,
        };
        let drawn = ghost(&ghost_row, 100);
        insta::assert_snapshot!(text(std::slice::from_ref(&drawn)));
        assert!(drawn.iter().any(|part| part.fg == C::Cyan && part.text.contains(SOLID)));
        assert!(drawn.iter().any(|part| part.fg == C::DarkGray && part.text.contains(DASHED)));
        let opening = ghost(&Ghost { opened: 0.25, ..ghost_row.clone() }, 100);
        assert!(width(&opening) < 30, "it slides open from the left");
        let done = ghost(&Ghost { share: 1.0, ..ghost_row }, 100);
        assert!(!text(&[done]).contains(DASHED), "all solid once restored");
    }

    #[test]
    fn the_files_show_their_size_as_a_bar_and_fade_with_age() {
        let row = |name: &str, size: &str, share: f64, age: &str, color: C| FileRow {
            icon: "▤".into(),
            name: name.into(),
            size: size.into(),
            share,
            age: age.into(),
            color,
            bar: if color == C::DarkGray { C::DarkGray } else { C::Cyan },
            on: None,
            doing: None,
            picked: false,
            slide: 1.0,
            leaving: None,
        };
        let section = Files {
            folder: r"C:\Users\me\AppData\Local\lazyrunner\shop\databases".into(),
            note: Some(("Tab picks a file · i imports it · Del deletes it".into(), C::DarkGray)),
            rows: vec![
                FileRow {
                    on: Some("in localhost's backup folder".into()),
                    ..row("Shop_20261123_1042.bak", "2.4 GB", 1.0, "just now", C::White)
                },
                FileRow { picked: true, ..row("Shop_qa_20261120.bacpac", "612 MB", 0.25, "3 d ago", C::Gray) },
                row("Shop_20261102_0900.bak", "2.2 GB", 0.92, "21 d ago", C::DarkGray),
            ],
            focused: true,
        };
        insta::assert_snapshot!(text(&files(&section, 110, 8)));
        let leaving = FileRow { leaving: Some(0.5), ..section.rows[0].clone() };
        let gone = file_row(&leaving, (24, 6, 8), false, 110);
        assert!(gone.iter().all(|part| part.fg == C::DarkGray), "a file that goes dims");
        assert!(text(&[gone]).starts_with("   "), "and closes towards its middle");
        let coming = slide_in(vec![seg("x", C::Gray)], 0.5, 10);
        assert_eq!(text(&[coming]), "     x");
        assert_eq!(tail(r"C:\Users\me\AppData\Local\lazyrunner\shop\databases"), r"…\shop\databases");
        assert_eq!(tail("/home/me/.local/state/lazyrunner/shop/databases"), "…/shop/databases");
        assert_eq!(tail("databases"), "databases");
    }
}
