//! Watching the .NET sources (the profile's watch folder) for "restart on code change": a saved source file is an
//! event; build output and packages are not.

use std::path::{Component, Path};

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::mpsc::UnboundedSender;

use super::Event;

const SOURCE_EXTENSIONS: [&str; 9] = ["cs", "csproj", "json", "config", "props", "targets", "cshtml", "razor", "resx"];
const IGNORED_FOLDERS: [&str; 3] = ["bin", "obj", "node_modules"];

/// Watches while it lives.
pub struct CodeWatcher {
    _watcher: RecommendedWatcher,
}

/// Starts watching `dir` (nothing when it isn't a folder or can't be watched).
pub fn watch(dir: &Path, events: UnboundedSender<Event>) -> Option<CodeWatcher> {
    if !dir.is_dir() {
        return None;
    }
    let mut watcher = notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
        let Ok(event) = result else { return };
        if matches!(event.kind, EventKind::Access(_)) {
            return;
        }
        for path in event.paths.into_iter().filter(|path| is_source(path)) {
            let _ = events.send(Event::FileChanged(path));
        }
    })
    .ok()?;
    watcher.watch(dir, RecursiveMode::Recursive).ok()?;
    Some(CodeWatcher { _watcher: watcher })
}

/// A source file a rebuild depends on (not build output or packages).
pub fn is_source(path: &Path) -> bool {
    let in_ignored = path.components().any(|part| match part {
        Component::Normal(name) => IGNORED_FOLDERS.iter().any(|ignored| name.eq_ignore_ascii_case(ignored)),
        _ => false,
    });
    let extension = path.extension().map(|extension| extension.to_string_lossy().to_lowercase()).unwrap_or_default();
    !in_ignored && SOURCE_EXTENSIONS.contains(&extension.as_str())
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
