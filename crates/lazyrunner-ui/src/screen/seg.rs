//! Segments: runs of text in one colour, as the PowerShell version builds every row. A row is a list of segments,
//! cut to the width it has.

use ratatui::buffer::Buffer;
use ratatui::style::{Modifier, Style};
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
    /// Underlined: a link.
    pub underline: bool,
}

impl Seg {
    pub fn at(mut self, target: Target) -> Self {
        self.target = Some(target);
        self
    }
}

pub fn seg(text: impl Into<String>, fg: C) -> Seg {
    Seg { text: text.into(), fg, bg: None, style: None, target: None, underline: false }
}

/// A key as keycaps: each part of it (", f", "i c") a pill, a space between them.
pub fn keycaps(key: &str, fg: C, bg: C) -> Vec<Seg> {
    let mut caps = Vec::new();
    for part in key.split(' ').filter(|part| !part.is_empty()) {
        if !caps.is_empty() {
            caps.push(seg(" ", fg));
        }
        caps.push(on(format!(" {part} "), fg, bg));
    }
    caps
}

pub fn on(text: impl Into<String>, fg: C, bg: C) -> Seg {
    Seg { text: text.into(), fg, bg: Some(bg), style: None, target: None, underline: false }
}

/// Text in a style of its own, whatever theme is showing.
pub fn styled(text: impl Into<String>, style: Style) -> Seg {
    Seg { text: text.into(), fg: C::Gray, bg: None, style: Some(style), target: None, underline: false }
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

/// `segs` with the cells `from..to` drawn in `fg` on `bg`: a band, sweeping across a row, say.
pub fn band(segs: Vec<Seg>, from: usize, to: usize, fg: C, bg: C) -> Vec<Seg> {
    let mut out = Vec::new();
    let mut at = 0;
    for seg in segs {
        let mut part = String::new();
        let mut lit = None;
        for grapheme in seg.text.graphemes(true) {
            let inside = (from..to).contains(&at);
            if lit.is_some_and(|was| was != inside) {
                out.push(banded(&seg, std::mem::take(&mut part), lit == Some(true), fg, bg));
            }
            lit = Some(inside);
            part.push_str(grapheme);
            at += grapheme.width();
        }
        if !part.is_empty() {
            out.push(banded(&seg, part, lit == Some(true), fg, bg));
        }
    }
    out
}

fn banded(seg: &Seg, text: String, lit: bool, fg: C, bg: C) -> Seg {
    if lit { Seg { text, fg, bg: Some(bg), style: None, ..seg.clone() } } else { Seg { text, ..seg.clone() } }
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
/// What the pointer touches: what it's over (lit up), and what was just clicked (shown pressed).
#[derive(Clone, Copy, Debug, Default)]
pub struct Touch {
    pub hover: Option<Target>,
    pub pressed: Option<Target>,
}

/// Whether a target is something to click (a title, an item, a link…), not the background of a box or a column.
fn clickable(target: Target) -> bool {
    !matches!(target, Target::Box | Target::Output(_))
}

#[allow(clippy::too_many_arguments)]
pub fn draw(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    cells: u16,
    segs: &[Seg],
    theme: &Theme,
    hits: &mut Vec<Hit>,
    touch: Touch,
) {
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
        let mut style = seg.style.unwrap_or_else(|| theme.style(seg.fg, seg.bg));
        if seg.underline {
            style = style.add_modifier(Modifier::UNDERLINED);
        }
        if let Some(target) = seg.target.filter(|target| clickable(*target)) {
            if touch.pressed == Some(target) {
                style = style.add_modifier(Modifier::REVERSED);
            } else if touch.hover == Some(target) {
                style = style.add_modifier(Modifier::BOLD);
                if matches!(target, Target::Link { .. } | Target::Tests | Target::Reveal(_)) {
                    style = style.add_modifier(Modifier::UNDERLINED);
                }
            }
        }
        let (next, _) = buf.set_stringn(cx, y, &seg.text, usize::from(right - cx), style);
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

/// How fast a selected row too long for its box scrolls sideways, and how long it rests at either end.
const MARQUEE_CELLS_PER_SECOND: f64 = 10.0;
const MARQUEE_REST: f64 = 1.2;

/// `text` in exactly `cells`: as it is when it fits; otherwise `seconds` into scrolling sideways through it - resting
/// at its start, moving along to its end, resting there, then from the start again.
pub fn marquee(text: &str, cells: usize, seconds: f64) -> String {
    let overflow = text.width().saturating_sub(cells);
    if overflow == 0 {
        return pad(text, cells);
    }
    let travel = overflow as f64 / MARQUEE_CELLS_PER_SECOND;
    let at = seconds % (2.0 * MARQUEE_REST + travel);
    let offset = (((at - MARQUEE_REST).max(0.0) * MARQUEE_CELLS_PER_SECOND) as usize).min(overflow);
    let mut skipped = 0;
    let rest: String = text
        .graphemes(true)
        .skip_while(|grapheme| {
            let skip = skipped < offset;
            if skip {
                skipped += grapheme.width();
            }
            skip
        })
        .collect();
    pad(&rest, cells)
}

/// `text` in exactly `cells`, cut with … when it doesn't fit.
pub fn ellipsis(text: &str, cells: usize) -> String {
    if text.width() <= cells {
        return pad(text, cells);
    }
    format!("{}…", take_cells(text, cells.saturating_sub(1)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_row_too_long_scrolls_sideways_resting_at_both_ends() {
        let text = "abcdefghij";
        assert_eq!(marquee("short", 8, 5.0), "short   ", "a row that fits stays put");
        assert_eq!(marquee(text, 6, 0.0), "abcdef", "resting at its start");
        assert_eq!(marquee(text, 6, MARQUEE_REST + 0.25), "cdefgh", "moving along");
        assert_eq!(marquee(text, 6, MARQUEE_REST + 1.0), "efghij", "resting at its end");
        assert_eq!(marquee(text, 6, 2.0 * MARQUEE_REST + 0.5), "abcdef", "from the start again");
        assert_eq!(ellipsis(text, 6), "abcde…");
        assert_eq!(ellipsis("abc", 6), "abc   ");
    }

    #[test]
    fn a_band_lights_its_cells_across_segments() {
        let row = vec![seg("abc", C::Gray), seg("defg", C::Red)];
        let lit = band(row, 2, 5, C::Black, C::Cyan);
        let parts: Vec<(&str, Option<C>)> = lit.iter().map(|part| (part.text.as_str(), part.bg)).collect();
        assert_eq!(parts, [("ab", None), ("c", Some(C::Cyan)), ("de", Some(C::Cyan)), ("fg", None)]);
    }

    #[test]
    fn rows_are_cut_with_an_ellipsis() {
        let row = [seg("hello ", C::Gray), seg("world", C::Cyan)];
        assert_eq!(width(&row), 11);
        let cut = limit(&row, 8);
        assert_eq!(cut.iter().map(|s| s.text.as_str()).collect::<Vec<_>>(), ["hello ", "w…"]);
        assert_eq!(pad("ab", 4), "ab  ");
        assert_eq!(pad("abcdef", 3), "abc");
    }

    #[test]
    fn what_the_pointer_is_over_lights_up_and_what_was_clicked_shows_pressed() {
        let row = [seg("Orders", C::Gray).at(Target::Title(0)), seg(" box", C::Gray).at(Target::Box)];
        let theme = crate::screen::theme::Theme::no_color();
        let style_at = |touch: Touch, x: u16| {
            let mut buf = Buffer::empty(ratatui::layout::Rect::new(0, 0, 20, 1));
            draw(&mut buf, 0, 0, 20, &row, &theme, &mut Vec::new(), touch);
            buf[(x, 0)].modifier
        };
        let over = Touch { hover: Some(Target::Title(0)), pressed: None };
        assert!(style_at(over, 0).contains(Modifier::BOLD));
        let pressed = Touch { hover: Some(Target::Title(0)), pressed: Some(Target::Title(0)) };
        assert!(style_at(pressed, 0).contains(Modifier::REVERSED));
        let over_the_box = Touch { hover: Some(Target::Box), pressed: Some(Target::Box) };
        assert!(style_at(over_the_box, 8).is_empty(), "a box's background isn't something to click");
        assert!(style_at(Touch::default(), 0).is_empty());
    }
}
