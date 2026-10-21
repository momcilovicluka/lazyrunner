//! Segments: runs of text in one colour, as the PowerShell version builds every row. A row is a list of segments,
//! cut to the width it has.

use ratatui::buffer::Buffer;
use ratatui::style::Style;
use serde_json::Value;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use super::mouse::{Hit, Target};
use super::theme::{C, Theme};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Seg {
    pub text: String,
    pub fg: C,
    pub bg: Option<C>,
    /// A style of its own instead of the theme's for `fg` / `bg` (the theme picker's swatches show other themes).
    pub style: Option<Style>,
    /// What a click there means (drawing records where it landed).
    pub target: Option<Target>,
}

impl Seg {
    pub fn at(mut self, target: Target) -> Self {
        self.target = Some(target);
        self
    }
}

pub fn seg(text: impl Into<String>, fg: C) -> Seg {
    Seg { text: text.into(), fg, bg: None, style: None, target: None }
}

pub fn on(text: impl Into<String>, fg: C, bg: C) -> Seg {
    Seg { text: text.into(), fg, bg: Some(bg), style: None, target: None }
}

/// Text in a style of its own, whatever theme is showing.
pub fn styled(text: impl Into<String>, style: Style) -> Seg {
    Seg { text: text.into(), fg: C::Gray, bg: None, style: Some(style), target: None }
}

/// The row's segments that have no target yet get this one.
pub fn aim(segs: Vec<Seg>, target: Target) -> Vec<Seg> {
    segs.into_iter().map(|seg| if seg.target.is_some() { seg } else { seg.at(target) }).collect()
}

/// The cells a row takes.
pub fn width(segs: &[Seg]) -> usize {
    segs.iter().map(|seg| seg.text.width()).sum()
}

/// The first `cells` cells of `text`.
pub fn take_cells(text: &str, cells: usize) -> String {
    let mut out = String::new();
    let mut used = 0;
    for grapheme in text.graphemes(true) {
        let w = grapheme.width();
        if used + w > cells {
            break;
        }
        used += w;
        out.push_str(grapheme);
    }
    out
}

/// `text` padded with spaces (or cut) to exactly `cells` cells.
pub fn pad(text: &str, cells: usize) -> String {
    let cut = take_cells(text, cells);
    let room = cells.saturating_sub(cut.width());
    format!("{cut}{}", " ".repeat(room))
}

/// Cuts a row to `cells`, ending in … when something was cut off.
pub fn limit(segs: &[Seg], cells: usize) -> Vec<Seg> {
    if width(segs) <= cells {
        return segs.to_vec();
    }
    let mut out = Vec::new();
    let mut used = 0;
    for seg in segs {
        let room = cells.saturating_sub(used);
        if room == 0 {
            break;
        }
        let text =
            if seg.text.width() <= room { seg.text.clone() } else { format!("{}…", take_cells(&seg.text, room - 1)) };
        used += text.width();
        out.push(Seg { text, ..seg.clone() });
    }
    out
}

/// Draws a row at (x, y), cut to `cells`; where segments with a target landed goes to `hits`.
pub fn draw(buf: &mut Buffer, x: u16, y: u16, cells: u16, segs: &[Seg], theme: &Theme, hits: &mut Vec<Hit>) {
    let right = x.saturating_add(cells).min(buf.area.right());
    if y >= buf.area.bottom() {
        return;
    }
    let mut cx = x;
    for seg in segs {
        if cx >= right || seg.text.is_empty() {
            if cx >= right {
                break;
            }
            continue;
        }
        let (next, _) = buf.set_stringn(
            cx,
            y,
            &seg.text,
            usize::from(right - cx),
            seg.style.unwrap_or_else(|| theme.style(seg.fg, seg.bg)),
        );
        if let Some(target) = seg.target {
            hits.push(Hit { x: cx, y, width: next.saturating_sub(cx), target });
        }
        cx = next;
    }
}

/// A JSON value as text (a string as it is, anything else as JSON).
pub fn text_of(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_are_cut_with_an_ellipsis() {
        let row = [seg("hello ", C::Gray), seg("world", C::Cyan)];
        assert_eq!(width(&row), 11);
        let cut = limit(&row, 8);
        assert_eq!(cut.iter().map(|s| s.text.as_str()).collect::<Vec<_>>(), ["hello ", "w…"]);
        assert_eq!(pad("ab", 4), "ab  ");
        assert_eq!(pad("abcdef", 3), "abc");
    }
}
