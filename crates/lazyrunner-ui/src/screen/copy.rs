//! `y`: copies from the focused app's output - it asks what: the last error with its stack trace (a failed build's
//! errors with their full paths), every error, every warning, both in order, the search match with the lines that
//! belong to it, or all of its output - ready to paste into a ticket or a chat. In the database picker it copies the
//! highlighted secrets.json line.

use std::collections::HashSet;
use std::path::Path;

use lazyrunner_core::output::{Hue, Level, OutputLine};
use lazyrunner_core::run::Controller;
use lazyrunner_core::run::app::AppState;
use lazyrunner_core::run::secrets::{self, Entry};

use super::Ui;

/// What is copied at most: an entry with a long stack trace, or a build with many errors.
const MAX_LINES: usize = 200;
/// What every error, warning or both copy at most: the newest entries up to this many lines.
const MAX_COLLECTED: usize = 2000;
pub(super) const NEWLINE: &str = if cfg!(windows) { "\r\n" } else { "\n" };
const BUILD_HEADER: &str = "──── ⚙";

/// Copies text to the system clipboard (the tests swap this for one that only records).
pub type Clipboard = fn(&str) -> Result<(), String>;

pub fn to_clipboard(text: &str) -> Result<(), String> {
    arboard::Clipboard::new()
        .and_then(|mut clipboard| clipboard.set_text(text.to_string()))
        .map_err(|error| error.to_string())
}

/// What `y` copies from the focused app's output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pick {
    LastError,
    Errors,
    Warnings,
    Both,
    Search,
    Output,
}

/// `y`'s choices, as it lists them: the key, and what it copies.
pub const PICKS: [(Pick, &str, &str); 6] = [
    (Pick::LastError, "y", "the last error"),
    (Pick::Errors, "e", "every error"),
    (Pick::Warnings, "w", "every warning"),
    (Pick::Both, "a", "errors and warnings"),
    (Pick::Search, "s", "the search match"),
    (Pick::Output, "o", "all of its output"),
];

/// What a pick takes from the focused app's output now: its lines, and how many entries they are (errors and
/// warnings are separated by an empty line; the others are one entry).
pub fn gather(ui: &Ui, controller: &Controller, pick: Pick) -> (Vec<String>, usize) {
    if pick != Pick::Search {
        return gather_app(controller, ui.focus, pick);
    }
    let (Some(app), Some(state)) = (controller.apps.get(ui.focus), controller.states.get(ui.focus)) else {
        return (Vec::new(), 0);
    };
    let lines = oldest_first(state);
    let index = ui
        .search
        .as_ref()
        .filter(|search| !search.typing && search.key == app.key)
        .and_then(|search| search.index)
        .filter(|&index| index < lines.len());
    one(index.map_or_else(Vec::new, |index| block(&lines, index)))
}

/// What a pick other than the search match takes from the output of the app at `index` (the agent asks for any).
pub fn gather_app(controller: &Controller, index: usize, pick: Pick) -> (Vec<String>, usize) {
    let (Some(app), Some(state)) = (controller.apps.get(index), controller.states.get(index)) else {
        return (Vec::new(), 0);
    };
    let lines = oldest_first(state);
    match pick {
        Pick::LastError => one(last_error(&lines, controller.failed_build_errors.get(&app.key))),
        Pick::Search => (Vec::new(), 0),
        Pick::Output => one(lines.iter().map(|line| line.text.clone()).collect()),
        Pick::Errors => collect(&lines, |error, _| error),
        Pick::Warnings => collect(&lines, |error, warning| warning && !error),
        Pick::Both => collect(&lines, |error, warning| error || warning),
    }
}

fn oldest_first(state: &AppState) -> Vec<&OutputLine> {
    let mut lines: Vec<&OutputLine> = state.output.newest_first(0).collect();
    lines.reverse();
    lines
}

/// One entry, or none when it has no lines.
fn one(lines: Vec<String>) -> (Vec<String>, usize) {
    let count = usize::from(!lines.is_empty());
    (lines, count)
}

/// y, then what: copies it, or says there's nothing of it.
pub fn copy(ui: &Ui, controller: &mut Controller, pick: Pick) {
    let Some(name) = controller.apps.get(ui.focus).map(|app| app.name.clone()) else { return };
    let (lines, entries) = gather(ui, controller, pick);
    if lines.is_empty() {
        let why = match pick {
            Pick::LastError => format!("no error in {name}'s output (/ finds a line to copy)"),
            Pick::Errors => format!("no errors in {name}'s output"),
            Pick::Warnings => format!("no warnings in {name}'s output"),
            Pick::Both => format!("no errors or warnings in {name}'s output"),
            Pick::Search => format!("no search match in {name} (/ finds one)"),
            Pick::Output => format!("{name} has no output"),
        };
        controller.add_log(format!("Nothing to copy: {why}."), Hue::DarkGray);
        return;
    }
    let label = PICKS.iter().find(|(choice, ..)| *choice == pick).map_or("", |(_, _, label)| *label);
    let count = lines.iter().filter(|line| !line.is_empty()).count();
    let lines_said = format!("{count} line{}", if count == 1 { "" } else { "s" });
    let size = if matches!(pick, Pick::Errors | Pick::Warnings | Pick::Both) {
        format!("{entries} entr{}, {lines_said}", if entries == 1 { "y" } else { "ies" })
    } else {
        lines_said
    };
    put(ui.clipboard, controller, &lines.join(NEWLINE), &format!("{label} from {name} ({size})"));
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
pub fn is_error(line: &OutputLine) -> bool {
    let text = line.text.as_str();
    line.hue == Some(Hue::Red)
        || line.level == Level::Error
        || text.starts_with("fail: ")
        || text.starts_with("crit: ")
        || text.contains(": error ")
        || mentions_exception(text)
}

/// A line that reports a warning: yellow, a .NET warn entry, a compiler or bundler warning.
fn is_warning(line: &OutputLine) -> bool {
    let text = line.text.as_str();
    line.hue == Some(Hue::Yellow)
        || line.level == Level::Warn
        || text.starts_with("warn: ")
        || text.contains(": warning ")
        || text.contains("[WARNING]")
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

/// The entries `keep` wants (by whether they have an error, a warning), in order, an empty line between them. An
/// entry printed again (a dev server's rebuild repeats its errors) counts once, at its last place; when there are
/// more than `MAX_COLLECTED` lines, the newest.
fn collect(lines: &[&OutputLine], keep: impl Fn(bool, bool) -> bool) -> (Vec<String>, usize) {
    let mut entries: Vec<Vec<&OutputLine>> = Vec::new();
    for line in lines {
        match entries.last_mut() {
            Some(entry) if continues(&line.text) => entry.push(line),
            _ => entries.push(vec![line]),
        }
    }
    let mut seen = HashSet::new();
    let mut picked: Vec<Vec<String>> = Vec::new();
    let mut total = 0;
    for entry in entries.iter().rev() {
        let (error, warning) = (entry.iter().any(|line| is_error(line)), entry.iter().any(|line| is_warning(line)));
        if !keep(error, warning) {
            continue;
        }
        let texts: Vec<String> = entry.iter().take(MAX_LINES).map(|line| line.text.clone()).collect();
        if total + texts.len() > MAX_COLLECTED {
            break;
        }
        if seen.insert(texts.join("\n")) {
            total += texts.len();
            picked.push(texts);
        }
    }
    picked.reverse();
    let count = picked.len();
    let mut out = Vec::new();
    for (at, entry) in picked.into_iter().enumerate() {
        if at > 0 {
            out.push(String::new());
        }
        out.extend(entry);
    }
    (out, count)
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
