//! `y`: copies the focused app's last error with its stack trace (a failed build's errors with their full paths), or
//! the search match with the lines that belong to it - ready to paste into a ticket or a chat. In the database picker
//! it copies the highlighted secrets.json line.

use std::path::Path;

use lazyrunner_core::output::{Hue, Level, OutputLine};
use lazyrunner_core::run::Controller;
use lazyrunner_core::run::secrets::{self, Entry};

use super::Ui;

/// What is copied at most: an entry with a long stack trace, or a build with many errors.
const MAX_LINES: usize = 200;
pub(super) const NEWLINE: &str = if cfg!(windows) { "\r\n" } else { "\n" };
const BUILD_HEADER: &str = "──── ⚙";

/// Copies text to the system clipboard (the tests swap this for one that only records).
pub type Clipboard = fn(&str) -> Result<(), String>;

pub fn to_clipboard(text: &str) -> Result<(), String> {
    arboard::Clipboard::new()
        .and_then(|mut clipboard| clipboard.set_text(text.to_string()))
        .map_err(|error| error.to_string())
}

/// `y` on the columns.
pub fn copy(ui: &Ui, controller: &mut Controller) {
    let Some(app) = controller.apps.get(ui.focus) else { return };
    let (key, name) = (app.key.clone(), app.name.clone());
    let Some(state) = controller.states.get(ui.focus) else { return };
    let mut lines: Vec<&OutputLine> = state.output.newest_first(0).collect();
    lines.reverse();
    let from_search = ui
        .search
        .as_ref()
        .filter(|search| !search.typing && search.key == key)
        .and_then(|search| search.index)
        .filter(|&index| index < lines.len());
    let (block, what) = match from_search {
        Some(index) => (block(&lines, index), "the search match"),
        None => (last_error(&lines, controller.failed_build_errors.get(&key)), "the last error"),
    };
    if block.is_empty() {
        controller
            .add_log(format!("Nothing to copy: no error in {name}'s output (/ finds a line to copy)."), Hue::DarkGray);
        return;
    }
    let count = block.len();
    let what = format!("{what} from {name} ({count} line{})", if count == 1 { "" } else { "s" });
    put(ui.clipboard, controller, &block.join(NEWLINE), &what);
}

/// `y` in the database picker: the entry's whole line from secrets.json as it is there (without its indentation),
/// password included. Read again, in case the file changed since the picker opened.
pub fn copy_row(clipboard: Clipboard, controller: &mut Controller, path: &Path, entry: &Entry) {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    match secrets::line_at(&text, entry.line) {
        Some((key, line)) if key == entry.key => {
            let what = format!("the {} row from secrets.json (password included)", entry.label);
            put(clipboard, controller, line.trim(), &what);
        }
        _ => controller
            .add_log("secrets.json changed since the picker opened - press d again to copy that row.", Hue::Yellow),
    }
}

pub(super) fn put(clipboard: Clipboard, controller: &mut Controller, text: &str, what: &str) {
    match clipboard(text) {
        Ok(()) => controller.add_log(format!("Copied {what} to the clipboard"), Hue::Cyan),
        Err(error) => controller.add_log(format!("Couldn't copy to the clipboard: {error}"), Hue::Red),
    }
}

/// An indented line (the message, exception or stack frame of the entry above it). The .NET console log prints an
/// entry as "fail: Category[id]" with its message, exception and stack trace indented below.
fn continues(text: &str) -> bool {
    (text.starts_with(char::is_whitespace) && !text.trim().is_empty()) || text.starts_with("--- ")
}

/// A line that reports an error: red, a .NET fail / crit entry, an exception, or a compiler error.
fn is_error(line: &OutputLine) -> bool {
    let text = line.text.as_str();
    line.hue == Some(Hue::Red)
        || line.level == Level::Error
        || text.starts_with("fail: ")
        || text.starts_with("crit: ")
        || text.contains(": error ")
        || mentions_exception(text)
}

/// "Exception" as the end of a word (NullReferenceException, Exception:), not "Exceptions".
fn mentions_exception(text: &str) -> bool {
    text.match_indices("Exception").any(|(at, word)| {
        text[at + word.len()..].chars().next().is_none_or(|next| !(next.is_alphanumeric() || next == '_'))
    })
}

/// The last error: the errors of the app's failed build (full paths) when the error is part of it, else the last
/// error line's entry with its indented lines. Nothing when there is none.
fn last_error(lines: &[&OutputLine], failed_build: Option<&Vec<String>>) -> Vec<String> {
    let Some(found) = lines.iter().rposition(|line| is_error(line)) else { return Vec::new() };
    let build_started = lines.iter().rposition(|line| line.text.starts_with(BUILD_HEADER));
    if let Some(errors) = failed_build.filter(|errors| !errors.is_empty())
        && build_started.is_some_and(|start| start <= found)
    {
        return errors.clone();
    }
    let mut start = found;
    while start > 0 && continues(&lines[start].text) {
        start -= 1;
    }
    block(lines, start)
}

/// A line and the indented lines below it, at most `MAX_LINES`.
fn block(lines: &[&OutputLine], start: usize) -> Vec<String> {
    let mut block = vec![lines[start].text.clone()];
    block.extend(
        lines[start + 1..]
            .iter()
            .take(MAX_LINES - 1)
            .take_while(|line| continues(&line.text))
            .map(|line| line.text.clone()),
    );
    block
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exceptions_are_whole_words() {
        assert!(mentions_exception("System.NullReferenceException: Object reference not set"));
        assert!(mentions_exception("Unhandled Exception"));
        assert!(!mentions_exception("Exceptions are logged elsewhere"));
    }

    #[test]
    fn indented_lines_and_dashes_belong_to_the_entry_above() {
        assert!(continues("      at Shop.Orders.Get() in Orders.cs:line 12"));
        assert!(continues("--- End of stack trace from previous location ---"));
        assert!(!continues("info: Microsoft.Hosting.Lifetime[0]"));
        assert!(!continues("   "));
    }

    /// The real clipboard (`cargo test -p lazyrunner-ui real_clipboard -- --ignored`): what was there is put back.
    #[test]
    #[ignore = "uses the desktop's clipboard"]
    fn real_clipboard_gets_the_text() {
        let mut clipboard = arboard::Clipboard::new().unwrap();
        let before = clipboard.get_text().ok();
        to_clipboard("lazyrunner clipboard check\r\n  second line").unwrap();
        let got = clipboard.get_text().unwrap();
        if let Some(before) = before {
            let _ = clipboard.set_text(before);
        }
        assert_eq!(got, "lazyrunner clipboard check\r\n  second line");
    }
}
