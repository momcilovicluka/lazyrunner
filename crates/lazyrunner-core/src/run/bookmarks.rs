//! Bookmarks (#84): marks that are kept - per project, in the state folder's bookmarks.json - with their line, the
//! lines either side of it, their app, when and on which branch, a name and a note. After a restart a bookmark finds
//! its line again when its app prints one of the same shape (`output::shape`, as `Alt+d` compares runs: times, ids
//! and ports don't count); until then it's from an earlier run, and one that hasn't found its line in 30 days says
//! so. They export to Markdown for a ticket or the team's notes.

use std::path::Path;
use std::time::Duration;

use anyhow::Result;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::output;

/// The file in the project's state folder, and the Markdown export beside it.
pub const FILE: &str = "bookmarks.json";
pub const MARKDOWN: &str = "bookmarks.md";
/// The lines kept either side of a bookmark's own.
pub const AROUND: usize = 2;
/// A bookmark that hasn't found its line for this long says so.
pub const STALE: Duration = Duration::from_secs(30 * 24 * 3600);
const DAY: u64 = 24 * 3600;

/// A kept mark.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Bookmark {
    /// Tells it from the others (when it was kept, to the nanosecond).
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub note: String,
    /// Its app's key.
    pub app: String,
    /// Its line, and the lines before and after it as they were.
    pub line: String,
    #[serde(default)]
    pub before: Vec<String>,
    #[serde(default)]
    pub after: Vec<String>,
    /// When it was kept (RFC 3339), and on which branch.
    pub kept: String,
    #[serde(default)]
    pub branch: String,
    /// When it last found its line (RFC 3339); none since it was kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub found: Option<String>,
}

impl Bookmark {
    /// Whether `text` is its line in another run: the same shape.
    pub fn finds(&self, text: &str) -> bool {
        output::shape(text.trim()) == output::shape(self.line.trim())
    }

    /// When it last had its line: found, else kept.
    pub fn seen(&self) -> &str {
        self.found.as_deref().unwrap_or(&self.kept)
    }

    /// How many whole days it hasn't found its line, once that's 30 or more.
    pub fn stale_days(&self, now: Timestamp) -> Option<u64> {
        let seen = self.seen().parse::<Timestamp>().ok()?;
        let unseen = u64::try_from(now.duration_since(seen).as_secs()).unwrap_or(0);
        (unseen >= STALE.as_secs()).then_some(unseen / DAY)
    }
}

/// The bookmarks kept in the state folder `state_dir`, oldest first; none when there's no file or it doesn't read.
pub fn load(state_dir: &Path) -> Vec<Bookmark> {
    crate::json::read(&state_dir.join(FILE)).and_then(|value| serde_json::from_value(value).ok()).unwrap_or_default()
}

/// Keeps `bookmarks` in the state folder `state_dir`.
pub fn save(state_dir: &Path, bookmarks: &[Bookmark]) -> Result<()> {
    crate::config::write(&state_dir.join(FILE), &serde_json::to_value(bookmarks)?)
}

/// `bookmarks` as Markdown: each one's name, app (by `name_of` its key), when it was kept and on which branch, its
/// note, and its lines in a code block - its own marked `>`.
pub fn markdown(bookmarks: &[Bookmark], name_of: impl Fn(&str) -> String) -> String {
    let mut text = String::from("# Bookmarks\n");
    for bookmark in bookmarks {
        let kept = crate::time::as_written(&bookmark.kept)
            .map_or_else(|| bookmark.kept.clone(), |at| at.strftime(crate::time::DAY_CLOCK).to_string());
        text.push_str(&format!("\n## {}\n\n", bookmark.name));
        text.push_str(&format!("- **App:** {}\n", name_of(&bookmark.app)));
        text.push_str(&format!("- **Kept:** {kept}\n"));
        if !bookmark.branch.is_empty() {
            text.push_str(&format!("- **Branch:** {}\n", bookmark.branch));
        }
        if !bookmark.note.is_empty() {
            text.push_str(&format!("- **Note:** {}\n", bookmark.note));
        }
        let lines: Vec<String> = bookmark
            .before
            .iter()
            .map(|line| format!("  {line}"))
            .chain([format!("> {}", bookmark.line)])
            .chain(bookmark.after.iter().map(|line| format!("  {line}")))
            .collect();
        let fence = fence(&lines);
        text.push_str(&format!("\n{fence}text\n{}\n{fence}\n", lines.join("\n")));
    }
    text
}

/// A code fence longer than any run of backticks in `lines`.
fn fence(lines: &[String]) -> String {
    let longest = lines.iter().flat_map(|line| line.split(|c| c != '`').map(str::len)).max().unwrap_or(0);
    "`".repeat(longest.max(2) + 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TempDir;

    fn bookmark(name: &str, line: &str) -> Bookmark {
        Bookmark {
            id: format!("id-{name}"),
            name: name.into(),
            note: String::new(),
            app: "orders".into(),
            line: line.into(),
            before: vec!["info: Checkout started".into()],
            after: vec!["      at Shop.Orders.Checkout()".into()],
            kept: "2026-11-20T09:15:00+01:00".into(),
            branch: "main".into(),
            found: None,
        }
    }

    #[test]
    fn bookmarks_are_kept_and_read_back_in_their_order() {
        let temp = TempDir::new("bookmarks-kept");
        assert!(load(temp.path()).is_empty(), "none yet");
        let mut first = bookmark("Payment timeout", "warn: Payment timed out after 30000 ms");
        first.note = "the first sign".into();
        let mut second = bookmark("Config", "info: Using the Billing service at http://localhost:47202");
        second.found = Some("2026-11-21T10:00:00+01:00".into());
        save(temp.path(), &[first.clone(), second.clone()]).unwrap();
        assert_eq!(load(temp.path()), [first, second]);
    }

    #[test]
    fn a_bookmark_finds_its_line_whatever_the_times_ids_and_ports() {
        let kept =
            bookmark("Timeout", "12:01:07 warn: Request 4f2a9c1e to http://localhost:47201/orders took 30012 ms");
        assert!(kept.finds("09:44:51 warn: Request 77bd0e12 to http://localhost:51234/orders took 30087 ms"));
        assert!(kept.finds("  12:01:07 warn: Request 4f2a9c1e to http://localhost:47201/orders took 30012 ms  "));
        assert!(!kept.finds("09:44:51 warn: Request 77bd0e12 to http://localhost:51234/billing took 30087 ms"));
        assert!(!kept.finds("09:44:51 info: Request 77bd0e12 to http://localhost:51234/orders took 30087 ms"));
    }

    #[test]
    fn a_bookmark_that_has_not_found_its_line_for_30_days_says_how_long() {
        let mut kept = bookmark("Old", "info: Application started.");
        let now = |text: &str| text.parse::<Timestamp>().unwrap();
        assert_eq!(kept.stale_days(now("2026-12-19T09:15:00+01:00")), None, "29 days");
        assert_eq!(kept.stale_days(now("2026-12-20T09:15:00+01:00")), Some(30));
        kept.found = Some("2026-12-10T09:15:00+01:00".into());
        assert_eq!(kept.stale_days(now("2026-12-20T09:15:00+01:00")), None, "it found its line since");
        assert_eq!(kept.stale_days(now("2027-01-12T09:15:00+01:00")), Some(33));
    }

    #[test]
    fn bookmarks_export_to_markdown_with_their_lines() {
        let mut timeout = bookmark("Payment timeout", "warn: Payment timed out after 30000 ms");
        timeout.note = "comes before every failed checkout".into();
        let mut quoted = bookmark("Quoted", "info: ran ``` and ```` in a line");
        (quoted.branch, quoted.before, quoted.after) = (String::new(), Vec::new(), Vec::new());
        let text = markdown(&[timeout, quoted], |key| if key == "orders" { "Orders".into() } else { key.into() });
        let expected = "# Bookmarks\n\
            \n## Payment timeout\n\n\
            - **App:** Orders\n\
            - **Kept:** 2026-11-20 09:15:00\n\
            - **Branch:** main\n\
            - **Note:** comes before every failed checkout\n\
            \n```text\n  info: Checkout started\n> warn: Payment timed out after 30000 ms\n        at Shop.Orders.Checkout()\n```\n\
            \n## Quoted\n\n\
            - **App:** Orders\n\
            - **Kept:** 2026-11-20 09:15:00\n\
            \n`````text\n> info: ran ``` and ```` in a line\n`````\n";
        assert_eq!(text, expected);
    }
}
