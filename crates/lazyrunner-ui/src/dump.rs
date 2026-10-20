//! The frame as plain text (one line per terminal row), for tests and the frame dump file.

use ratatui::buffer::Buffer;

pub fn text(buf: &Buffer) -> String {
    let width = usize::from(buf.area.width);
    let mut out = String::with_capacity(buf.content.len() + usize::from(buf.area.height));
    for row in buf.content.chunks(width.max(1)) {
        let start = out.len();
        for cell in row {
            out.push_str(cell.symbol());
        }
        let trimmed = out[start..].trim_end().len();
        out.truncate(start + trimmed);
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;
    use ratatui::style::Style;

    #[test]
    fn one_trimmed_line_per_row() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 8, 2));
        buf.set_string(1, 0, "hi ▸", Style::new());
        assert_eq!(text(&buf), " hi ▸\n\n");
    }
}
