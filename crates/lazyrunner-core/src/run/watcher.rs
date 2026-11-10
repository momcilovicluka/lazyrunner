//! Watching the .NET sources (the profile's watch folder) and each command app's code (its `watch` globs) for "restart
//! on code change", and a watched npm or Nx test suite's folder for test watch: a saved source file is an event;
//! build output, packages and virtual environments are not.

use std::path::{Component, Path, PathBuf};

use regex::Regex;

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::mpsc::UnboundedSender;

use super::Event;

const SOURCE_EXTENSIONS: [&str; 9] = ["cs", "csproj", "json", "config", "props", "targets", "cshtml", "razor", "resx"];
const IGNORED_FOLDERS: [&str; 3] = ["bin", "obj", "node_modules"];
/// A front end's sources, and what its tests' tools leave behind.
const TEST_EXTENSIONS: [&str; 13] =
    ["ts", "tsx", "js", "jsx", "mjs", "cjs", "mts", "cts", "vue", "html", "json", "scss", "css"];
const TEST_IGNORED_FOLDERS: [&str; 9] =
    ["bin", "obj", "node_modules", ".git", "dist", ".nx", ".angular", "coverage", "TestResults"];

/// Folders a command app's code is never in: build output, packages, virtual environments, caches.
const COMMAND_IGNORED_FOLDERS: [&str; 12] = [
    ".git",
    ".venv",
    "venv",
    "node_modules",
    "target",
    "__pycache__",
    ".mypy_cache",
    ".pytest_cache",
    "bin",
    "obj",
    "dist",
    "build",
];

/// Watches while it lives.
pub struct CodeWatcher {
    _watcher: RecommendedWatcher,
}

/// Starts watching `dir` (nothing when it isn't a folder or can't be watched).
pub fn watch(dir: &Path, events: UnboundedSender<Event>) -> Option<CodeWatcher> {
    watch_with(dir, events, is_source, Event::FileChanged)
}

/// Starts watching a test suite's folder for its sources.
pub fn watch_tests(dir: &Path, events: UnboundedSender<Event>) -> Option<CodeWatcher> {
    watch_with(dir, events, is_test_source, Event::TestFileChanged)
}

/// Starts watching a command app's folder for the files its `watch` globs name.
pub fn watch_command(dir: &Path, globs: &[String], key: &str, events: UnboundedSender<Event>) -> Option<CodeWatcher> {
    let patterns: Vec<Regex> = globs.iter().filter_map(|glob| glob_pattern(glob)).collect();
    let folder = dir.to_path_buf();
    let wanted = move |path: &Path| is_command_source(&folder, &patterns, path);
    let key = key.to_string();
    watch_with(dir, events, wanted, move |path| Event::CommandFileChanged { key: key.clone(), path })
}

/// A file of a command app's code: one its globs name, from its folder, outside the folders its code is never in.
fn is_command_source(folder: &Path, patterns: &[Regex], path: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(folder) else { return false };
    let ignored = relative.components().any(|part| match part {
        Component::Normal(name) => COMMAND_IGNORED_FOLDERS.iter().any(|ignored| name.eq_ignore_ascii_case(ignored)),
        _ => false,
    });
    let relative = relative.to_string_lossy().replace('\\', "/");
    !ignored && patterns.iter().any(|pattern| pattern.is_match(&relative))
}

/// A glob (`**/*.py`, `src/*.go`) as a pattern for a path from its folder, `/` between folders: `**` any folders,
/// `*` and `?` within one name; any case.
pub fn glob_pattern(glob: &str) -> Option<Regex> {
    let glob: Vec<char> = glob.replace('\\', "/").chars().collect();
    let mut pattern = String::from("(?i)^");
    let mut at = 0;
    while at < glob.len() {
        match glob[at] {
            '*' if glob.get(at + 1) == Some(&'*') && glob.get(at + 2) == Some(&'/') => {
                pattern.push_str("(?:.*/)?");
                at += 3;
                continue;
            }
            '*' if glob.get(at + 1) == Some(&'*') => {
                pattern.push_str(".*");
                at += 2;
                continue;
            }
            '*' => pattern.push_str("[^/]*"),
            '?' => pattern.push_str("[^/]"),
            other => pattern.push_str(&regex::escape(&other.to_string())),
        }
        at += 1;
    }
    pattern.push('$');
    Regex::new(&pattern).ok()
}

fn watch_with(
    dir: &Path,
    events: UnboundedSender<Event>,
    wanted: impl Fn(&Path) -> bool + Send + 'static,
    event: impl Fn(PathBuf) -> Event + Send + 'static,
) -> Option<CodeWatcher> {
    if !dir.is_dir() {
        return None;
    }
    let mut watcher = notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
        let Ok(change) = result else { return };
        if matches!(change.kind, EventKind::Access(_)) {
            return;
        }
        for path in change.paths.into_iter().filter(|path| wanted(path)) {
            let _ = events.send(event(path));
        }
    })
    .ok()?;
    watcher.watch(dir, RecursiveMode::Recursive).ok()?;
    Some(CodeWatcher { _watcher: watcher })
}

/// A front-end source file a test suite's run depends on (not build output, packages or test results).
pub fn is_test_source(path: &Path) -> bool {
    sourced(path, &TEST_IGNORED_FOLDERS, &TEST_EXTENSIONS)
}

/// A source file a rebuild depends on (not build output or packages).
pub fn is_source(path: &Path) -> bool {
    sourced(path, &IGNORED_FOLDERS, &SOURCE_EXTENSIONS)
}

fn sourced(path: &Path, ignored: &[&str], extensions: &[&str]) -> bool {
    let in_ignored = path.components().any(|part| match part {
        Component::Normal(name) => ignored.iter().any(|ignored| name.eq_ignore_ascii_case(ignored)),
        _ => false,
    });
    let extension = path.extension().map(|extension| extension.to_string_lossy().to_lowercase()).unwrap_or_default();
    !in_ignored && extensions.contains(&extension.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn only_sources_count() {
        assert!(is_source(Path::new("services/Orders/Program.cs")));
        assert!(is_source(Path::new("services/Orders/appsettings.json")));
        assert!(!is_source(Path::new("services/Orders/obj/project.assets.json")));
        assert!(!is_source(Path::new("services/Orders/BIN/Debug/x.cs")));
        assert!(!is_source(Path::new("services/Orders/readme.md")));
        assert!(is_test_source(Path::new("web/shop/src/cart.spec.ts")));
        assert!(!is_test_source(Path::new("web/shop/coverage/lcov.json")));
        assert!(!is_test_source(Path::new("web/shop/src/cart.cs")));
    }

    #[test]
    fn a_command_apps_globs_name_its_code_outside_virtual_environments_and_build_output() {
        let any = glob_pattern("**/*.py").unwrap();
        assert!(any.is_match("main.py") && any.is_match("app/api.py") && !any.is_match("main.pyc"));
        let one = glob_pattern("src/*.go").unwrap();
        assert!(one.is_match("src/a.go") && one.is_match("SRC/A.GO") && !one.is_match("src/x/a.go"));
        let folder = Path::new("repo").join("pricing");
        let patterns = vec![any];
        assert!(is_command_source(&folder, &patterns, &folder.join("app").join("api.py")));
        assert!(!is_command_source(&folder, &patterns, &folder.join(".venv").join("Lib").join("x.py")));
        assert!(!is_command_source(&folder, &patterns, &folder.join("__pycache__").join("x.py")));
        assert!(!is_command_source(&folder, &patterns, Path::new("elsewhere").join("x.py").as_path()));
    }

    #[tokio::test]
    async fn a_saved_source_file_is_an_event() {
        let dir = crate::testing::TempDir::new("watch");
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let _watcher = watch(dir.path(), tx).expect("watching");
        tokio::time::sleep(Duration::from_millis(200)).await;
        crate::testing::write(dir.join("obj/x.cs"), "ignored");
        crate::testing::write(dir.join("Program.cs"), "// changed");
        let event = tokio::time::timeout(Duration::from_secs(10), rx.recv()).await.expect("an event in time");
        let Some(Event::FileChanged(path)) = event else { panic!("{event:?}") };
        assert!(path.ends_with("Program.cs"), "{}", path.display());
    }
}
