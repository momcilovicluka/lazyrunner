//! A frontend's browser errors in its column (#112): with `"browserErrors"` on, the Node hook lazyrunner loads into
//! an npm app's dev server puts a small script of lazyrunner's in the pages it serves. The script reports what goes
//! wrong in the page - an uncaught error, an unhandled rejection, `console.error` - back to the dev server, and the hook
//! writes each report as a `lazyrunner-browser {...}` line; lazyrunner reads those out of the app's output and shows each
//! as an entry of its own, in the error colour, so it counts wherever an error counts.

use serde::Deserialize;

/// What lazyrunner sets for an npm app it starts while the setting is on: the hook does nothing without it.
pub const ENV: &str = "LAZYRUNNER_BROWSER_ERRORS";
/// The value it has then.
pub const ON: &str = "1";
/// How the hook's lines start.
const MARK: &str = "lazyrunner-browser ";
/// How a report's first line starts in the column; the column puts a globe before it.
pub const PREFIX: &str = "browser ✗ ";
/// A report's entry's category in a parsed column.
pub const CATEGORY: &str = "browser";
/// A report's message, its stack and the page's address, at most (the hook bounds them too).
const MESSAGE_CHARS: usize = 1000;
const LINE_CHARS: usize = 300;
const PAGE_CHARS: usize = 300;
/// The lines under a report's first one, at most.
const MOST_LINES: usize = 12;
/// How a report's lines under its first one are indented.
const INDENT: &str = "    ";

/// What a report is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// An error nothing caught.
    Error,
    /// A promise rejected with nothing to handle it.
    Rejection,
    /// The page's `console.error`.
    Console,
    /// Reports the script didn't send: too many at once.
    Dropped,
}

/// One report from a page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    pub kind: Kind,
    pub message: String,
    /// Its stack's lines, and the message's lines after its first.
    pub stack: Vec<String>,
    /// The page's path (its address without the query or hash).
    pub page: Option<String>,
    /// How many times it came: the same message again within a moment, or the reports dropped.
    pub count: u64,
    /// It stands for the same message again, after the first was shown.
    pub again: bool,
}

/// A line of an app's output, as far as the browser errors go.
#[derive(Debug, PartialEq, Eq)]
pub enum Line {
    /// Not the hook's.
    Other,
    /// The hook's, but unreadable: it stays out of the column all the same.
    Unreadable,
    Report(Report),
}

/// What the hook writes, as it writes it.
#[derive(Deserialize)]
struct Sent {
    level: Option<String>,
    message: Option<String>,
    stack: Option<String>,
    page: Option<String>,
    count: Option<u64>,
    again: Option<bool>,
}

/// Reads an output line: a report of the hook's, or not one.
pub fn read(text: &str) -> Line {
    let Some(at) = text.find(MARK) else { return Line::Other };
    let Ok(sent) = serde_json::from_str::<Sent>(text[at + MARK.len()..].trim()) else { return Line::Unreadable };
    let kind = match sent.level.as_deref() {
        Some("rejection") => Kind::Rejection,
        Some("console") => Kind::Console,
        Some("dropped") => Kind::Dropped,
        _ => Kind::Error,
    };
    let message = sent.message.unwrap_or_default();
    let mut said = message.lines().map(str::trim_end).filter(|line| !line.trim().is_empty());
    let first = clean(said.next().unwrap_or_default(), MESSAGE_CHARS);
    let mut stack: Vec<String> = said.map(|line| clean(line, LINE_CHARS)).collect();
    let stack_lines = sent.stack.unwrap_or_default();
    stack.extend(stack_lines.lines().filter_map(frame));
    Line::Report(Report {
        kind,
        message: first,
        stack,
        page: sent.page.as_deref().map(path).filter(|page| !page.is_empty()),
        count: sent.count.unwrap_or(1).max(1),
        again: sent.again.unwrap_or(false),
    })
}

/// A stack's line as a frame - Chrome's `at f (url:1:2)`, Firefox's and Safari's `f@url:1:2` - or `None` for the
/// lines that only say the message again.
fn frame(line: &str) -> Option<String> {
    let line = line.trim();
    if let Some(rest) = line.strip_prefix("at ") {
        return Some(format!("at {}", clean(rest, LINE_CHARS)));
    }
    let numbered =
        line.rsplit(':').next().is_some_and(|end| !end.is_empty() && end.bytes().all(|b| b.is_ascii_digit()));
    (line.contains('@') && numbered).then(|| format!("at {}", clean(line, LINE_CHARS)))
}

/// A page's address as its path: no scheme, host, query or hash.
fn path(page: &str) -> String {
    let page = page.split(['?', '#']).next().unwrap_or_default();
    let rest = page.split_once("://").map_or(page, |(_, rest)| rest.find('/').map_or("/", |at| &rest[at..]));
    clean(rest, PAGE_CHARS)
}

/// Text from a page as a column shows it: no colours or control characters, tabs as spaces, at most `chars`
/// characters.
fn clean(text: &str, chars: usize) -> String {
    let text = crate::output::strip_ansi(text).replace('\t', " ");
    let mut kept: String = text.chars().filter(|c| !c.is_control()).take(chars).collect();
    if text.chars().filter(|c| !c.is_control()).nth(chars).is_some() {
        kept.push('…');
    }
    kept.trim().to_string()
}

impl Report {
    /// Its lines in the column: the first says what happened, the rest - its stack, then the page - are indented
    /// under it.
    pub fn lines(&self) -> Vec<String> {
        let said = match self.kind {
            Kind::Error => self.message.clone(),
            Kind::Rejection => format!("Unhandled rejection: {}", self.message),
            Kind::Console => format!("console.error: {}", self.message),
            Kind::Dropped => {
                let errors = if self.count == 1 { "error" } else { "errors" };
                format!("{} more {errors} in the page not sent: too many at once", self.count)
            }
        };
        let times = match (self.again, self.count) {
            _ if self.kind == Kind::Dropped => String::new(),
            (true, count) => format!(" (again ×{count})"),
            (false, count) if count > 1 => format!(" ×{count}"),
            _ => String::new(),
        };
        let mut lines = vec![format!("{PREFIX}{said}{times}")];
        let under = self.stack.len().min(MOST_LINES);
        lines.extend(self.stack.iter().take(under).map(|line| format!("{INDENT}{line}")));
        if self.stack.len() > under {
            lines.push(format!("{INDENT}… {} more lines", self.stack.len() - under));
        }
        if let Some(page) = &self.page {
            lines.push(format!("{INDENT}page {page}"));
        }
        lines
    }

    /// How many errors it stands for.
    pub fn errors(&self) -> u64 {
        self.count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(text: &str) -> Report {
        match read(text) {
            Line::Report(report) => report,
            other => panic!("not a report: {other:?}"),
        }
    }

    #[test]
    fn an_uncaught_error_is_its_message_its_frames_and_the_page_without_its_query() {
        let line = r#"lazyrunner-browser {"level":"error","message":"Uncaught TypeError: Cannot read properties of undefined (reading 'id')","stack":"TypeError: Cannot read properties of undefined (reading 'id')\n    at Orders (http://localhost:5173/src/Orders.tsx:12:5)\n    at renderWithHooks (http://localhost:5173/node_modules/.vite/deps/react-dom.js:1:2)","page":"http://localhost:5173/orders/42?token=secret#top","count":1}"#;
        let found = report(line);
        assert_eq!(found.kind, Kind::Error);
        assert_eq!(
            found.lines(),
            [
                "browser ✗ Uncaught TypeError: Cannot read properties of undefined (reading 'id')",
                "    at Orders (http://localhost:5173/src/Orders.tsx:12:5)",
                "    at renderWithHooks (http://localhost:5173/node_modules/.vite/deps/react-dom.js:1:2)",
                "    page /orders/42",
            ]
        );
        assert_eq!(found.errors(), 1);
    }

    #[test]
    fn each_kind_says_what_it_was() {
        let first = |line: &str| report(line).lines()[0].clone();
        assert_eq!(
            first(
                r#"lazyrunner-browser {"level":"rejection","message":"Error: Request failed","stack":"Error: Request failed\n    at load (http://localhost:4200/main.js:3:9)"}"#
            ),
            "browser ✗ Unhandled rejection: Error: Request failed"
        );
        assert_eq!(
            first(r#"lazyrunner-browser {"level":"console","message":"ERROR HttpErrorResponse: 500 for /api/orders"}"#),
            "browser ✗ console.error: ERROR HttpErrorResponse: 500 for /api/orders"
        );
        assert_eq!(
            first(r#"lazyrunner-browser {"level":"dropped","message":"","count":37}"#),
            "browser ✗ 37 more errors in the page not sent: too many at once"
        );
        assert_eq!(first(r#"lazyrunner-browser {"level":"strange","message":"x"}"#), "browser ✗ x", "an error");
    }

    #[test]
    fn the_same_message_again_says_how_many_times() {
        let again =
            report(r#"lazyrunner-browser {"level":"console","message":"Billing failed","count":5,"again":true}"#);
        assert_eq!(again.lines(), ["browser ✗ console.error: Billing failed (again ×5)"]);
        assert_eq!(again.errors(), 5);
        let grouped = report(r#"lazyrunner-browser {"level":"error","message":"Boom","count":3}"#);
        assert_eq!(grouped.lines(), ["browser ✗ Boom ×3"]);
        let none = report(r#"lazyrunner-browser {"level":"error","message":"Boom","count":0}"#);
        assert_eq!(none.errors(), 1, "a report is one at least");
    }

    #[test]
    fn firefox_frames_and_a_messages_own_lines_go_under_it() {
        let found = report(
            r#"lazyrunner-browser {"level":"console","message":"Warning: Each child needs a key\n    at li\n    at Orders","stack":"Error: no user ada@example.com\nOrders@http://localhost:5173/src/Orders.tsx:12:5\n@http://localhost:5173/src/main.tsx:3:1"}"#,
        );
        assert_eq!(
            found.lines(),
            [
                "browser ✗ console.error: Warning: Each child needs a key",
                "    at li",
                "    at Orders",
                "    at Orders@http://localhost:5173/src/Orders.tsx:12:5",
                "    at @http://localhost:5173/src/main.tsx:3:1",
            ]
        );
    }

    #[test]
    fn a_report_is_bounded_and_has_no_control_characters() {
        let long = "x".repeat(5000);
        let frames: Vec<String> = (0..40).map(|n| format!("    at f{n} (http://localhost:5173/a.js:{n}:1)")).collect();
        let line = serde_json::json!({
            "level": "error",
            "message": format!("Boom\u{1b}[31m red\u{7}\t{long}"),
            "stack": frames.join("\n"),
            "page": format!("http://localhost:5173/{long}"),
        });
        let found = report(&format!("lazyrunner-browser {line}"));
        let lines = found.lines();
        assert!(lines[0].chars().count() <= PREFIX.chars().count() + MESSAGE_CHARS + 1, "{}", lines[0]);
        assert!(lines[0].starts_with("browser ✗ Boom red x") && lines[0].ends_with('…'), "{}", lines[0]);
        assert!(lines.iter().all(|line| !line.chars().any(char::is_control)));
        assert_eq!(lines.len(), 1 + MOST_LINES + 2, "the first, its frames, how many more, the page");
        assert_eq!(lines[MOST_LINES + 1], "    … 28 more lines");
        assert!(lines.last().is_some_and(|page| page.chars().count() <= INDENT.len() + 5 + PAGE_CHARS + 1));
    }

    #[test]
    fn other_lines_are_not_reports_and_a_broken_one_stays_out_of_the_column() {
        assert_eq!(read("  VITE v5.4.0  ready in 312 ms"), Line::Other);
        assert_eq!(read("lazyrunner-call GET http://localhost:5000/api/x 200 3"), Line::Other);
        assert_eq!(read(r#"lazyrunner-browser {"level":"error","message":"#), Line::Unreadable);
        assert_eq!(read("lazyrunner-browser not json"), Line::Unreadable);
        let after_a_spinner = report(r#"⠋ building lazyrunner-browser {"message":"Boom"}"#);
        assert_eq!(after_a_spinner.lines(), ["browser ✗ Boom"], "a line another write began");
    }
}
