//! Builds: whether a .NET app needs one (a fingerprint of its sources, a stamp of the last good build), what its
//! output means (progress, warnings, errors), and how long things usually take (for the progress estimates).

use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::UNIX_EPOCH;

use regex::Regex;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use super::tools::git;
use crate::json;
use crate::output::Hue;

fn pattern(regex: &str) -> Regex {
    Regex::new(regex).expect("a valid pattern")
}

/// The committed sources under `source_dir` plus every uncommitted change there (with its time): equal
/// fingerprints mean nothing changed. `None` outside git (then every build runs).
pub fn fingerprint(root: &Path, source_dir: &str) -> Option<String> {
    let tree = git(root, &["rev-parse", &format!("HEAD:{}", source_dir.replace('\\', "/"))])?;
    let mut parts = vec![tree];
    let changes =
        git(root, &["--no-optional-locks", "status", "--porcelain=v1", "-uall", "--", source_dir]).unwrap_or_default();
    static BUILD_OUTPUT: LazyLock<Regex> = LazyLock::new(|| pattern(r"(?i)[\\/](bin|obj)[\\/]"));
    for line in changes.lines().filter(|line| line.len() > 3 && !BUILD_OUTPUT.is_match(line)) {
        let path = line[3..].rsplit(" -> ").next().unwrap_or_default().trim_matches('"');
        let ticks = modified_ticks(&root.join(path));
        parts.push(format!("{line}|{ticks}"));
    }
    Some(hex(&Sha256::digest(parts.join("\n").as_bytes())))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02X}")).collect()
}

/// When a file was last written, in nanoseconds (0 when it isn't there).
pub fn modified_ticks(path: &Path) -> u128 {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |since| since.as_nanos())
}

/// The newest build output of a project (its .dll under bin/Debug), so a stamp is only trusted while that output
/// is still the one it describes.
pub fn output_ticks(root: &Path, project: &str) -> u128 {
    let path = root.join(project);
    let Some(name) = path.file_stem().map(|stem| format!("{}.dll", stem.to_string_lossy())) else { return 0 };
    let debug = path.parent().map(|dir| dir.join("bin").join("Debug")).unwrap_or_default();
    let mut newest = 0;
    let mut pending = vec![(debug, 0)];
    while let Some((dir, depth)) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() && depth < 2 {
                pending.push((path, depth + 1));
            } else if path.file_name().is_some_and(|file| file.to_string_lossy().eq_ignore_ascii_case(&name)) {
                newest = newest.max(modified_ticks(&path));
            }
        }
    }
    newest
}

fn stamp_path(state: &Path, key: &str) -> PathBuf {
    state.join(format!("build-{key}.stamp.json"))
}

/// Whether the last good build of `key` is of these sources and its output is untouched since.
pub fn is_up_to_date(state: &Path, root: &Path, key: &str, project: &str, fingerprint: Option<&str>) -> bool {
    let Some(fingerprint) = fingerprint else { return false };
    let Some(stamp) = json::read(&stamp_path(state, key)) else { return false };
    let ticks = output_ticks(root, project);
    json::text(json::field(&stamp, "root")) == root.display().to_string()
        && json::text(json::field(&stamp, "fingerprint")) == fingerprint
        && ticks != 0
        && json::text(json::field(&stamp, "outputTicks")) == ticks.to_string()
}

pub fn save_stamp(state: &Path, root: &Path, key: &str, project: &str, fingerprint: Option<&str>) {
    let Some(fingerprint) = fingerprint else { return };
    let stamp = json!({
        "root": root.display().to_string(),
        "fingerprint": fingerprint,
        "outputTicks": output_ticks(root, project).to_string(),
    });
    let _ = crate::config::write(&stamp_path(state, key), &stamp);
}

/// The projects a build compiles: the project and everything it references, directly or not.
pub fn project_count(project: &Path) -> usize {
    static REFERENCE: LazyLock<Regex> = LazyLock::new(|| pattern(r#"(?i)<ProjectReference\s+Include="([^"$]+)""#));
    let mut seen: HashSet<String> = HashSet::new();
    let mut queue = VecDeque::from([project.to_path_buf()]);
    while let Some(path) = queue.pop_front() {
        let Ok(text) = json::read_text(&path) else { continue };
        if !seen.insert(path.display().to_string().to_lowercase()) {
            continue;
        }
        let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
        for found in REFERENCE.captures_iter(&text) {
            queue.push_back(normalize(&dir.join(crate::profile::native(&found[1]))));
        }
    }
    seen.len()
}

/// A path with `.` and `..` resolved (without touching the disk).
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

/// A compiler message without its path and project: "Orders.cs(12,5): error CS1002: ; expected".
pub fn format_error(line: &str) -> String {
    static PATH: LazyLock<Regex> = LazyLock::new(|| pattern(r"^\s*.*[\\/]([^\\/]+\(\d+.*)$"));
    static PROJECT: LazyLock<Regex> = LazyLock::new(|| pattern(r"\s*\[[^\]]+\]\s*$"));
    let without_path = PATH.captures(line).map_or_else(|| line.to_string(), |found| found[1].to_string());
    PROJECT.replace(&without_path, "").trim().to_string()
}

/// What a line of build or npm ci output is.
#[derive(Debug, PartialEq, Eq)]
pub enum ToolLine {
    /// Counted, not shown (a big solution has hundreds).
    Warning,
    /// "Project -> output.dll": one project done.
    Built(String),
    Shown(String, Hue),
}

pub fn tool_line(text: &str, building: bool) -> ToolLine {
    static WARNING: LazyLock<Regex> = LazyLock::new(|| pattern(r"(?i): warning |^npm warn"));
    static BUILT: LazyLock<Regex> = LazyLock::new(|| pattern(r"^\s*([^\s:]+) -> "));
    static ERROR: LazyLock<Regex> = LazyLock::new(|| pattern(r"(?i): error |^npm (ERR!|error)"));
    static NPM_WARN: LazyLock<Regex> = LazyLock::new(|| pattern(r"(?i)^npm warn"));
    let warning = WARNING.is_match(text);
    if warning && building {
        return ToolLine::Warning;
    }
    if building && let Some(found) = BUILT.captures(text) {
        return ToolLine::Built(found[1].to_string());
    }
    let hue = if ERROR.is_match(text) {
        Hue::Red
    } else if NPM_WARN.is_match(text) {
        Hue::Yellow
    } else {
        Hue::DarkGray
    };
    ToolLine::Shown(format_error(text), hue)
}

/// The compiler errors of a failed build, each once.
pub fn errors(lines: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    lines.iter().filter(|line| line.contains(": error ")).filter(|line| seen.insert(line.as_str())).cloned().collect()
}

/// How long builds, starts and npm ci took the last few times, per app: the progress bars' estimates.
#[derive(Debug, Default)]
pub struct Timings {
    path: PathBuf,
    table: Map<String, Value>,
}

const RECENT_RUNS: usize = 5;

impl Timings {
    pub fn load(state: &Path) -> Self {
        let path = state.join("timings.json");
        let table = match json::read(&path) {
            Some(Value::Object(table)) => table,
            _ => Map::new(),
        };
        Self { path, table }
    }

    pub fn add(&mut self, kind: &str, key: &str, seconds: f64) {
        let kinds = self.table.entry(kind).or_insert_with(|| json!({}));
        if !kinds.is_object() {
            *kinds = json!({});
        }
        let mut recent: Vec<f64> = json::list(&kinds[key]).into_iter().filter_map(Value::as_f64).collect();
        recent.push((seconds * 10.0).round() / 10.0);
        let start = recent.len().saturating_sub(RECENT_RUNS);
        kinds[key] = json!(recent[start..]);
        let _ = crate::config::write(&self.path, &Value::Object(self.table.clone()));
    }

    /// The median of the last runs, in seconds.
    pub fn estimate(&self, kind: &str, key: &str) -> Option<f64> {
        let mut values: Vec<f64> =
            json::list(self.table.get(kind)?.get(key)?).into_iter().filter_map(Value::as_f64).collect();
        if values.is_empty() {
            return None;
        }
        values.sort_by(f64::total_cmp);
        Some(values[values.len() / 2])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{TempDir, write};

    #[test]
    fn compiler_messages_lose_their_path_and_project() {
        let line = r"C:\src\shop\services\Orders\Orders.cs(12,5): error CS1002: ; expected [C:\src\shop\services\Orders\Orders.csproj]";
        assert_eq!(format_error(line), "Orders.cs(12,5): error CS1002: ; expected");
        assert_eq!(
            format_error("/home/me/shop/Orders.cs(3,1): error CS0103: x [/home/me/shop/Orders.csproj]"),
            "Orders.cs(3,1): error CS0103: x"
        );
        assert_eq!(format_error("  Restore complete"), "Restore complete");
    }

    #[test]
    fn build_output_ticks_progress_counts_warnings_and_colours_errors() {
        assert_eq!(
            tool_line("  Orders -> C:\\src\\bin\\Debug\\net9.0\\Orders.dll", true),
            ToolLine::Built("Orders".into())
        );
        assert_eq!(tool_line("x.cs(1,1): warning CS0168: unused", true), ToolLine::Warning);
        assert_eq!(
            tool_line("x.cs(1,1): error CS1002: ; expected", true),
            ToolLine::Shown("x.cs(1,1): error CS1002: ; expected".into(), Hue::Red)
        );
        assert_eq!(
            tool_line("npm warn deprecated x", false),
            ToolLine::Shown("npm warn deprecated x".into(), Hue::Yellow)
        );
        assert_eq!(tool_line("added 3 packages", false), ToolLine::Shown("added 3 packages".into(), Hue::DarkGray));
    }

    #[test]
    fn a_build_counts_the_projects_it_references() {
        let dir = TempDir::new("project-count");
        write(
            dir.join("api/Api.csproj"),
            r#"<Project><ItemGroup><ProjectReference Include="..\lib\Lib.csproj" /></ItemGroup></Project>"#,
        );
        write(
            dir.join("lib/Lib.csproj"),
            r#"<Project><ItemGroup><ProjectReference Include="../core/Core.csproj" /></ItemGroup></Project>"#,
        );
        write(dir.join("core/Core.csproj"), "<Project />");
        assert_eq!(project_count(&dir.join("api/Api.csproj")), 3);
    }

    #[test]
    fn timings_keep_the_last_five_and_estimate_the_median() {
        let dir = TempDir::new("timings");
        let mut timings = Timings::load(dir.path());
        assert_eq!(timings.estimate("build", "orders"), None);
        for seconds in [9.0, 1.0, 2.0, 3.0, 4.0, 5.0] {
            timings.add("build", "orders", seconds);
        }
        assert_eq!(Timings::load(dir.path()).estimate("build", "orders"), Some(3.0), "9 fell out; median of 1-5");
    }

    #[test]
    fn a_stamp_is_trusted_only_for_the_same_sources_and_output() {
        let dir = TempDir::new("stamp");
        let (state, root) = (dir.join("state"), dir.join("repo"));
        let project = "api/Api.csproj";
        write(root.join("api/bin/Debug/net9.0/Api.dll"), "x");
        save_stamp(&state, &root, "api", project, Some("F1"));
        assert!(is_up_to_date(&state, &root, "api", project, Some("F1")));
        assert!(!is_up_to_date(&state, &root, "api", project, Some("F2")), "other sources");
        assert!(!is_up_to_date(&state, &root, "api", project, None), "outside git");
        std::thread::sleep(std::time::Duration::from_millis(20));
        write(root.join("api/bin/Debug/net9.0/Api.dll"), "y");
        assert!(!is_up_to_date(&state, &root, "api", project, Some("F1")), "the output changed since");
    }
}
