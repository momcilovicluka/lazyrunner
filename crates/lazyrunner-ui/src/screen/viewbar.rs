//! What every full-screen view starts with: its header - the view's name in a chip, the same in every view - and,
//! under it, its keys, on as many rows as they need (#90): wrapped between keys, never scrolling, so each key is in the
//! same place at a glance.

use unicode_width::UnicodeWidthStr;

use super::icons::{self, Icon};
use super::seg::{Seg, on, seg, take_cells};
use super::theme::C;

/// The most rows a view's keys take: past them the rest is cut with `…` (`?` lists every key).
const MOST_KEY_ROWS: usize = 4;
/// Between two keys in a view's keys.
const KEY_SEPARATOR: &str = " · ";
const MORE: &str = "…";

/// A view's name in its header's chip.
pub fn chip(nerd: bool, icon: Icon, name: &str) -> Seg {
    on(format!(" {} {} ", icons::icon(nerd, icon), name.to_uppercase()), C::Black, C::Cyan)
}

/// A view's keys in `cells`, on as many rows as they need - wrapped between keys, a key and what it does never apart.
pub fn keys(text: &str, cells: usize) -> Vec<Vec<Seg>> {
    wrapped_keys(text, cells).into_iter().map(|row| vec![seg(row, C::DarkGray)]).collect()
}

/// The keys' rows: each starts with a space, keys go on the row while they fit; a key longer than a row is cut, and
/// past [`MOST_KEY_ROWS`] the last row ends with `…`.
pub fn wrapped_keys(text: &str, cells: usize) -> Vec<String> {
    let mut rows: Vec<String> = Vec::new();
    let mut row = String::new();
    for key in text.trim().split(KEY_SEPARATOR).filter(|key| !key.trim().is_empty()) {
        let longer = format!("{row}{KEY_SEPARATOR}{key}");
        if row.is_empty() {
            row = format!(" {key}");
        } else if longer.width() <= cells {
            row = longer;
        } else {
            rows.push(std::mem::replace(&mut row, format!(" {key}")));
        }
    }
    if !row.is_empty() {
        rows.push(row);
    }
    let cut = rows.len() > MOST_KEY_ROWS;
    rows.truncate(MOST_KEY_ROWS);
    let mut rows: Vec<String> = rows.into_iter().map(|row| fitted(&row, cells)).collect();
    if let Some(last) = rows.last_mut().filter(|_| cut) {
        *last = format!("{}{MORE}", take_cells(last, cells.saturating_sub(MORE.width())));
    }
    rows
}

/// A row cut to `cells`, with `…` when it was longer.
fn fitted(row: &str, cells: usize) -> String {
    if row.width() <= cells { row.to_string() } else { format!("{}{MORE}", take_cells(row, cells.saturating_sub(1))) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_wrap_between_keys_and_never_scroll() {
        let keys = " h j k l choose · Enter its column · c its code · Tab the code map · Esc back";
        assert_eq!(wrapped_keys(keys, 200), [keys]);
        let rows = wrapped_keys(keys, 40);
        assert_eq!(rows, [" h j k l choose · Enter its column", " c its code · Tab the code map", " Esc back"]);
        assert!(rows.iter().all(|row| row.width() <= 40));
        let many = (1..=30).map(|at| format!("{at} key")).collect::<Vec<_>>().join(" · ");
        let rows = wrapped_keys(&many, 20);
        assert_eq!(rows.len(), MOST_KEY_ROWS);
        assert!(rows.last().is_some_and(|row| row.ends_with(MORE) && row.width() <= 20), "{rows:?}");
        assert_eq!(wrapped_keys(" a very long key that never fits", 10), [" a very l…"]);
    }
}
