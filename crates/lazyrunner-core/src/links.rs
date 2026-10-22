//! Links in output: URLs, and places in files - a build error's `Orders.cs(12,5)`, a .NET stack frame's
//! `in C:\src\Orders.cs:line 42`, a node or jest frame's `src/cart.ts:12:5` - so a click can open them: a URL in the
//! browser, a file at its line in the editor (`"editor"`).

use std::collections::VecDeque;
use std::path::{Component, Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;

use crate::run::tools::find_on_path;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Url(String),
    File { path: String, line: Option<u32>, column: Option<u32> },
}

/// A link in a line: its bytes `start..end`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    pub start: usize,
    pub end: usize,
    pub target: Target,
}

/// Folders a file name is never looked for in: build output, packages, tools' own.
const SKIPPED: [&str; 9] = [".git", "node_modules", "bin", "obj", "dist", "target", ".vs", ".idea", ".angular"];
/// How many folder entries a search for a bare file name reads at most (a click must not hang on a huge tree).
const SEARCH_BUDGET: usize = 20_000;
/// Lines longer than this aren't looked through for links (minified output, base64).
const LONGEST_SCANNED: usize = 2_000;
/// The files a link may name: source, markup, styles, data and text - never anything the system would run.
const EXTENSIONS: &str = "cs|fs|vb|cshtml|razor|xaml|csproj|props|targets|ts|tsx|js|jsx|mjs|cjs|vue|svelte|css|scss|less|html|json|xml|yml|yaml|md|sql|txt|config|log";

fn pattern(regex: &str) -> Regex {
    Regex::new(regex).expect("a valid pattern")
}

static URL: LazyLock<Regex> = LazyLock::new(|| pattern(r#"https?://[^\s<>"'`]+"#));
/// `Orders.cs(12,5)` and `C:\src\Orders.cs(12)`: MSBuild's errors and warnings.
static PARENS: LazyLock<Regex> = LazyLock::new(|| {
    pattern(&format!(r"(?P<path>(?:[A-Za-z]:)?[\w.\-\\/]+\.(?i:{EXTENSIONS}))\((?P<line>\d+)(?:,(?P<column>\d+))?\)"))
});
/// `in C:\src\Orders.cs:line 42`: a .NET stack frame (its path may have spaces).
static DOTNET_FRAME: LazyLock<Regex> = LazyLock::new(|| {
    pattern(&format!(r"\bin (?P<path>(?:[A-Za-z]:)?[^:*?<>|\r\n]+?\.(?i:{EXTENSIONS})):line (?P<line>\d+)"))
});
/// `src/cart.ts:12:5`: node, jest, vite, eslint and tsc's places.
static COLONS: LazyLock<Regex> = LazyLock::new(|| {
    pattern(&format!(r"(?P<path>(?:[A-Za-z]:)?[\w.\-\\/@]+\.(?i:{EXTENSIONS})):(?P<line>\d+)(?::(?P<column>\d+))?"))
});

/// The links in `text`, in order, none overlapping (a URL first, then the earliest place in a file).
pub fn find(text: &str) -> Vec<Link> {
    if text.len() > LONGEST_SCANNED {
        return Vec::new();
    }
    let mut links: Vec<Link> = Vec::new();
    if text.contains("://") {
        links.extend(URL.find_iter(text).filter_map(|found| {
            let trimmed = trim_url(found.as_str());
            let host = trimmed.split_once("://").map_or("", |(_, rest)| rest);
            (!host.is_empty()).then(|| Link {
                start: found.start(),
                end: found.start() + trimmed.len(),
                target: Target::Url(trimmed.into()),
            })
        }));
    }
    if !(text.contains('.') && (text.contains('(') || text.contains(':'))) {
        return links;
    }
    for (regex, frame) in [(&*DOTNET_FRAME, true), (&*PARENS, false), (&*COLONS, false)] {
        for found in regex.captures_iter(text) {
            let (Some(whole), Some(path)) = (found.get(0), found.name("path")) else { continue };
            let start = if frame { path.start() } else { whole.start() };
            let path = path.as_str().trim();
            if !openable(path) || links.iter().any(|link| start < link.end && whole.end() > link.start) {
                continue;
            }
            let number = |name| found.name(name).and_then(|value| value.as_str().parse().ok());
            let target = Target::File { path: path.into(), line: number("line"), column: number("column") };
            links.push(Link { start, end: whole.end(), target });
        }
    }
    links.sort_by_key(|link| link.start);
    links
}

/// A URL without the punctuation after it; a closing bracket stays when the URL opened one.
fn trim_url(url: &str) -> &str {
    let mut url = url.trim_end_matches(['.', ',', ';', ':', ']', '}', '\'', '"']);
    while url.ends_with(')') && url.matches('(').count() < url.matches(')').count() {
        url = url[..url.len() - 1].trim_end_matches(['.', ',', ';', ':']);
    }
    url
}

/// Whether a link may name this file: one with an extension in `EXTENSIONS` (never one the system would run), and
/// not on another machine (looking at a `\\server` path makes Windows sign in to it).
pub fn openable(path: &str) -> bool {
    if path.starts_with("\\\\") || path.starts_with("//") {
        return false;
    }
    let extension = Path::new(path).extension().and_then(|extension| extension.to_str()).unwrap_or_default();
    EXTENSIONS.split('|').any(|known| known.eq_ignore_ascii_case(extension))
}

/// Where a file a link names is: as given when absolute, else under `root`; a bare name (a build error shows only
/// the file's) is looked for under `root`, outside build output and packages.
pub fn resolve(root: &Path, path: &str) -> Option<PathBuf> {
    if !openable(path) {
        return None;
    }
    let given = Path::new(path);
    if given.is_absolute() {
        return given.is_file().then(|| given.to_path_buf());
    }
    let joined = root.join(given);
    if joined.is_file() {
        return Some(joined);
    }
    if given.components().any(|part| matches!(part, Component::ParentDir | Component::Prefix(_))) {
        return None; // never the end of a path under the root
    }
    let wanted: Vec<_> = given.components().collect();
    search(root, &wanted)
}

/// The first file under `root` whose path ends with `wanted`: breadth first, not into links (junctions, symlinks),
/// build output or packages, and only so far.
fn search(root: &Path, wanted: &[Component<'_>]) -> Option<PathBuf> {
    let mut budget = SEARCH_BUDGET;
    let mut folders = VecDeque::from([root.to_path_buf()]);
    while let Some(folder) = folders.pop_front() {
        let Ok(entries) = std::fs::read_dir(&folder) else { continue };
        for entry in entries.flatten() {
            if budget == 0 {
                return None;
            }
            budget -= 1;
            let Ok(kind) = entry.file_type() else { continue };
            let path = entry.path();
            if kind.is_dir() {
                if !SKIPPED.iter().any(|skipped| entry.file_name().eq_ignore_ascii_case(skipped)) {
                    folders.push_back(path);
                }
            } else if kind.is_file()
                && path
                    .components()
                    .rev()
                    .zip(wanted.iter().rev())
                    .all(|(have, want)| have.as_os_str().eq_ignore_ascii_case(want.as_os_str()))
            {
                return Some(path);
            }
        }
    }
    None
}

/// How the `"editor"` setting opens a file at a line: the program and its arguments; `None` is the default app.
/// `auto` (the default) is VS Code when it is on PATH, else Rider, else the default app; `code`, `rider` and
/// `default` pick one; anything else is a command with `{file}`, `{line}` and `{column}` in it.
pub fn editor_command(
    editor: &str,
    file: &Path,
    line: Option<u32>,
    column: Option<u32>,
) -> Option<(PathBuf, Vec<String>)> {
    let file_text = file.display().to_string();
    let (line, column) = (line.unwrap_or(1), column.unwrap_or(1));
    let code = || find_on_path("code").map(|code| (code, vec!["-g".into(), format!("{file_text}:{line}:{column}")]));
    let rider = || {
        find_on_path("rider").or_else(|| find_on_path("rider64")).map(|rider| {
            (rider, vec!["--line".into(), line.to_string(), "--column".into(), column.to_string(), file_text.clone()])
        })
    };
    match editor.trim() {
        "" | "auto" => code().or_else(rider),
        "code" => code(),
        "rider" => rider(),
        "default" => None,
        template => {
            let mut words = template.split_whitespace().map(|word| {
                word.replace("{file}", &file_text)
                    .replace("{line}", &line.to_string())
                    .replace("{column}", &column.to_string())
            });
            let program = words.next()?;
            let program = find_on_path(&program).unwrap_or_else(|| PathBuf::from(program));
            Some((program, words.collect()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{TempDir, write};

    fn targets(text: &str) -> Vec<(String, Target)> {
        find(text).into_iter().map(|link| (text[link.start..link.end].to_string(), link.target)).collect()
    }

    fn file(path: &str, line: u32, column: Option<u32>) -> Target {
        Target::File { path: path.into(), line: Some(line), column }
    }

    #[test]
    fn urls_and_places_in_files_are_found() {
        assert_eq!(
            targets("  ➜  Local:   http://localhost:58301/."),
            [("http://localhost:58301/".into(), Target::Url("http://localhost:58301/".into()))]
        );
        assert_eq!(
            targets("Orders.cs(12,5): error CS1002: ; expected"),
            [("Orders.cs(12,5)".into(), file("Orders.cs", 12, Some(5)))]
        );
        assert_eq!(
            targets(r"   at Shop.Orders.Checkout.Pay() in C:\src\shop app\Orders\Checkout.cs:line 42"),
            [(
                r"C:\src\shop app\Orders\Checkout.cs:line 42".into(),
                file(r"C:\src\shop app\Orders\Checkout.cs", 42, None)
            )]
        );
        assert_eq!(
            targets("    at Object.<anonymous> (src/cart/cart.test.ts:18:7)"),
            [("src/cart/cart.test.ts:18:7".into(), file("src/cart/cart.test.ts", 18, Some(7)))]
        );
        assert!(targets("info: Now listening on: localhost:58201 (12 ms)").is_empty());
    }

    #[test]
    fn nothing_that_runs_and_nothing_on_another_machine_is_a_link() {
        assert!(targets(r"   at X.Y() in C:\Users\me\Downloads\payload.exe:line 1").is_empty());
        assert!(targets("build.cmd(1): error").is_empty());
        assert!(
            targets(r"\\evil\share\cart.ts:1:1")
                .iter()
                .all(|(_, target)| !matches!(target, Target::File { path, .. } if path.starts_with("\\\\")))
        );
        assert!(targets("Thread.Sleep(1000); Retry.Attempt(3)").is_empty());
        assert!(!openable("tools/run.bat") && !openable(r"\\server\share\a.cs") && openable("src/App.tsx"));
        assert_eq!(resolve(Path::new("."), r"..\..\x.cs"), None);
    }

    #[test]
    fn a_url_keeps_its_own_brackets() {
        assert_eq!(
            targets("see https://en.wikipedia.org/wiki/Rust_(programming_language)."),
            [(
                "https://en.wikipedia.org/wiki/Rust_(programming_language)".into(),
                Target::Url("https://en.wikipedia.org/wiki/Rust_(programming_language)".into())
            )]
        );
        assert_eq!(targets("(at http://localhost:58201/)")[0].0, "http://localhost:58201/");
        assert!(targets("bare http://. here").is_empty());
    }

    #[test]
    fn a_url_with_a_file_in_it_is_one_link() {
        let found = targets("GET http://localhost:58301/src/main.ts:12 404");
        assert_eq!(found.len(), 1);
        assert!(matches!(found[0].1, Target::Url(_)));
    }

    #[test]
    fn a_bare_file_name_is_found_in_the_project_but_not_in_build_output() {
        let dir = TempDir::new("links-resolve");
        write(dir.join("services/Orders/bin/Debug/Orders.cs"), "");
        write(dir.join("services/Orders/Checkout.cs"), "");
        write(dir.join("web/src/cart/cart.ts"), "");
        assert_eq!(resolve(dir.path(), "Checkout.cs"), Some(dir.join("services/Orders/Checkout.cs")));
        assert_eq!(resolve(dir.path(), "src/cart/cart.ts"), Some(dir.join("web/src/cart/cart.ts")));
        assert_eq!(resolve(dir.path(), "Orders.cs"), None, "only in bin");
        assert_eq!(resolve(dir.path(), "Missing.cs"), None);
    }

    #[test]
    fn the_editor_opens_the_file_at_its_line() {
        let file = Path::new("/work/shop/Orders.cs");
        let (program, args) = editor_command("notepad++ -n{line} {file}", file, Some(12), Some(5)).unwrap();
        assert!(program.to_string_lossy().contains("notepad++"));
        assert_eq!(args, ["-n12", "/work/shop/Orders.cs"]);
        assert_eq!(editor_command("default", file, Some(12), None), None);
    }
}
