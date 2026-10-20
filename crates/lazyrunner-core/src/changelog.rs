//! The changelog (Keep a Changelog: `## [1.2.0] - 2026-10-14`, then `### Added` and bullets): what's new since the
//! version this machine last ran, and the lines the checklist and the changelog view show.

use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::json;

use crate::output::Hue;

/// CHANGELOG.md, as shipped.
pub const CHANGELOG: &str = include_str!("../../../CHANGELOG.md");
/// This lazyrunner's version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub version: String,
    pub date: String,
    pub lines: Vec<String>,
}

/// A line to show, in its colour.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub text: String,
    pub hue: Hue,
}

/// "1.2.3", "v1.2.3", "1.2.3-beta" as comparable numbers (four places); `None` when it isn't a version.
pub fn version(text: &str) -> Option<[u64; 4]> {
    static VERSION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^v?(\d+(?:\.\d+){1,3})").expect("a valid pattern"));
    let found = VERSION.captures(text.trim())?;
    let mut parts = [0u64; 4];
    for (slot, part) in parts.iter_mut().zip(found[1].split('.')) {
        *slot = part.parse().ok()?;
    }
    Some(parts)
}

/// The entries newer than `after` and up to `up_to` (either may be open), newest first as the file lists them.
pub fn entries(text: &str, after: Option<&str>, up_to: Option<&str>) -> Vec<Entry> {
    static HEADING: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^##\s+\[?v?(\d+(?:\.\d+){1,3}[^\]\s]*)\]?\s*(?:[-–]\s*(.+))?$").expect("a valid pattern")
    });
    static OTHER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^#\s|^\[.+\]:\s").expect("a valid pattern"));
    let mut all: Vec<Entry> = Vec::new();
    let mut open = false;
    for line in text.lines() {
        if let Some(found) = HEADING.captures(line) {
            let date = found.get(2).map_or("", |m| m.as_str()).trim().to_string();
            all.push(Entry { version: found[1].to_string(), date, lines: Vec::new() });
            open = true;
            continue;
        }
        if OTHER.is_match(line) {
            open = false; // the title, link references
            continue;
        }
        if open
            && !line.trim().is_empty()
            && let Some(entry) = all.last_mut()
        {
            entry.lines.push(line.trim_end().to_string());
        }
    }
    let (from, to) = (after.and_then(version), up_to.and_then(version));
    all.into_iter()
        .filter(|entry| {
            version(&entry.version).is_some_and(|v| from.is_none_or(|from| v > from) && to.is_none_or(|to| v <= to))
        })
        .collect()
}

/// "v1.2.0 · 2026-10-14" headings, then their bullets.
pub fn format(entries: &[Entry]) -> Vec<Line> {
    static BULLET: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*[-*]\s+").expect("a valid pattern"));
    let mut lines = Vec::new();
    for entry in entries {
        let date = if entry.date.is_empty() { String::new() } else { format!(" · {}", entry.date) };
        lines.push(Line { text: format!("v{}{date}", entry.version), hue: Hue::Cyan });
        for line in &entry.lines {
            let heading = line.starts_with("###");
            let text = if heading {
                line.trim_start_matches('#').trim().to_string()
            } else {
                BULLET.replace(line, "  • ").into_owned()
            };
            let text = text.replace("**", "").replace('`', "");
            lines.push(Line { text, hue: if heading { Hue::White } else { Hue::Gray } });
        }
    }
    lines
}

/// Lines wrapped at word boundaries to `width`; a bullet's continuation lines line up under its text.
pub fn wrap(lines: &[Line], width: usize) -> Vec<Line> {
    static INDENT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*•\s|^\s+").expect("a valid pattern"));
    let width = width.max(8);
    let mut out = Vec::new();
    for line in lines {
        let indent = INDENT.find(&line.text).map_or(String::new(), |m| " ".repeat(m.as_str().chars().count()));
        let indent = if indent.len() * 2 > width { String::new() } else { indent };
        let mut text: Vec<char> = line.text.chars().collect();
        while text.len() > width {
            let cut = text[..=width.min(text.len() - 1)]
                .iter()
                .rposition(|c| *c == ' ')
                .filter(|&at| at > indent.len())
                .unwrap_or(width);
            out.push(Line { text: text[..cut].iter().collect::<String>().trim_end().to_string(), hue: line.hue });
            let rest: String = text[cut..].iter().collect();
            text = format!("{indent}{}", rest.trim_start()).chars().collect();
        }
        out.push(Line { text: text.into_iter().collect(), hue: line.hue });
    }
    out
}

/// What changed since the version this machine last ran (none on the first run); remembers this one.
pub fn whats_new(state_dir: &Path) -> Vec<Entry> {
    let path = state_dir.join("version.json");
    let seen = crate::json::read(&path).map(|value| crate::json::text(&value["seen"])).filter(|seen| !seen.is_empty());
    if seen.as_deref() != Some(VERSION) {
        let _ = crate::config::write(&path, &json!({ "seen": VERSION }));
    }
    let Some(seen) = seen else { return Vec::new() };
    if version(VERSION) <= version(&seen) {
        return Vec::new();
    }
    entries(CHANGELOG, Some(&seen), Some(VERSION))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = "# Changelog\n\nIntro.\n\n## [0.3.0] - 2026-11-01\n\n### Added\n- A **bold** `thing`\n\n## [0.2.0] - 2026-10-20\n- Second\n\n## 0.1.0\n- First\n\n[0.3.0]: https://x\n";

    #[test]
    fn versions_compare_by_number() {
        assert!(version("0.10.0") > version("0.9.9"));
        assert_eq!(version("v1.2"), Some([1, 2, 0, 0]));
        assert_eq!(version("0.2.0-dev"), Some([0, 2, 0, 0]));
        assert_eq!(version("next"), None);
    }

    #[test]
    fn entries_between_two_versions() {
        let all = entries(TEXT, None, None);
        assert_eq!(all.iter().map(|e| e.version.as_str()).collect::<Vec<_>>(), ["0.3.0", "0.2.0", "0.1.0"]);
        assert_eq!(all[0].date, "2026-11-01");
        let newer = entries(TEXT, Some("0.1.0"), Some("0.2.0"));
        assert_eq!(newer.iter().map(|e| e.version.as_str()).collect::<Vec<_>>(), ["0.2.0"]);
    }

    #[test]
    fn formatted_lines_read_like_the_panel_shows_them() {
        let lines = format(&entries(TEXT, Some("0.2.0"), None));
        let texts: Vec<&str> = lines.iter().map(|line| line.text.as_str()).collect();
        assert_eq!(texts, ["v0.3.0 · 2026-11-01", "Added", "  • A bold thing"]);
        assert_eq!((lines[0].hue, lines[1].hue, lines[2].hue), (Hue::Cyan, Hue::White, Hue::Gray));
    }

    #[test]
    fn wrapping_keeps_bullets_aligned() {
        let line = Line { text: "  • one two three four five six".into(), hue: Hue::Gray };
        let wrapped: Vec<String> = wrap(&[line], 14).into_iter().map(|line| line.text).collect();
        assert_eq!(wrapped, ["  • one two", "    three four", "    five six"]);
    }

    #[test]
    fn whats_new_only_after_an_upgrade() {
        let dir = crate::testing::TempDir::new("whats-new");
        assert!(whats_new(dir.path()).is_empty(), "first run: nothing");
        crate::config::write(&dir.join("version.json"), &json!({ "seen": "0.0.1" })).unwrap();
        let news = whats_new(dir.path());
        assert!(news.iter().all(|entry| version(&entry.version) <= version(VERSION)));
        assert!(whats_new(dir.path()).is_empty(), "said once");
    }
}
