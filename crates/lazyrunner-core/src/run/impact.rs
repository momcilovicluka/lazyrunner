//! What a change affects (#78): which test suites' code changed since they last ran whole, and why. A whole run
//! keeps what it ran on - the commit, and the files changed on top of it with when each was last written - so what
//! changed since is git's diff from that commit plus whatever differs in the working tree now. A .NET suite is
//! affected by a source of its own project or of one it references, directly or not, and by a build file no project
//! owns (Directory.Build.props, global.json); an npm or Nx suite by a source in its own folder.

use std::collections::hash_map::Entry;
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::build;
use super::suites::Runner;
use super::tools::git;
use super::watcher;

/// What a whole run ran on: the commit, and the files changed on top of it (from the root, with `/`) with when each
/// was last written.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Baseline {
    pub head: String,
    #[serde(default)]
    pub dirty: BTreeMap<String, u128>,
}

/// Why a suite runs for a change: the projects from it to the changed one (its own first) and the changed files
/// there; with no projects, build files no project owns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Why {
    pub chain: Vec<String>,
    pub files: Vec<String>,
}

/// What a change means for a suite.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Impact {
    /// Nothing it depends on changed since its last whole run.
    Unchanged,
    /// Something did, for these reasons.
    Affected(Vec<Why>),
    /// It can't be told (it hasn't run whole since this is kept, the commit it ran on is gone, no git): it runs with
    /// the affected ones, better once too often than once missed.
    Unknown(String),
}

impl Impact {
    /// Whether it runs with the affected suites.
    pub fn runs(&self) -> bool {
        !matches!(self, Self::Unchanged)
    }
}

/// Build files that change every project that's under them.
const BUILD_FILES: [&str; 4] = ["props", "targets", "global.json", "nuget.config"];

/// Where the root is in its repo (`""`, `"app/"`): git's paths are from the repo's top.
fn prefix(root: &Path) -> Option<String> {
    git(root, &["rev-parse", "--show-prefix"])
}

/// A path git gave, from the root; `None` outside it.
fn from_root(path: &str, prefix: &str) -> Option<String> {
    path.trim().trim_matches('"').replace('\\', "/").strip_prefix(prefix).map(str::to_string)
}

/// The working tree's changes on top of the commit: each path and when it was last written.
fn dirty(root: &Path, prefix: &str) -> BTreeMap<String, u128> {
    let status = git(root, &["--no-optional-locks", "status", "--porcelain=v1", "-uall", "--", "."]);
    status
        .unwrap_or_default()
        .lines()
        .filter_map(|line| line.get(3..))
        .filter_map(|path| from_root(path.rsplit(" -> ").next().unwrap_or_default(), prefix))
        .filter(|path| !built(path))
        .map(|path| {
            let ticks = build::modified_ticks(&root.join(&path));
            (path, ticks)
        })
        .collect()
}

/// A build's output or packages, never a source.
fn built(path: &str) -> bool {
    let lower = format!("/{}", path.to_lowercase());
    ["/bin/", "/obj/", "/node_modules/"].iter().any(|part| lower.contains(part))
}

/// What the code is now, for a whole run to keep; `None` outside git.
pub fn baseline(root: &Path) -> Option<Baseline> {
    let prefix = prefix(root)?;
    Some(Baseline { head: git(root, &["rev-parse", "HEAD"])?, dirty: dirty(root, &prefix) })
}

/// The files changed since `base` (from the root, with `/`), the working tree now being `now`; `None` when git
/// can't say (the commit isn't in this checkout any more).
fn changed_since(root: &Path, prefix: &str, base: &Baseline, now: &BTreeMap<String, u128>) -> Option<BTreeSet<String>> {
    let committed = git(root, &["diff", "--name-only", &base.head, "HEAD", "--", "."])?;
    let mut changed: BTreeSet<String> =
        committed.lines().filter_map(|line| from_root(line, prefix)).filter(|line| !line.is_empty()).collect();
    changed
        .extend(now.iter().filter(|(path, ticks)| base.dirty.get(*path) != Some(ticks)).map(|(path, _)| path.clone()));
    // Changed when it ran and not now: committed or undone since, either way not what it ran on.
    changed.extend(base.dirty.keys().filter(|path| !now.contains_key(*path)).cloned());
    Some(changed)
}

/// Each suite's impact: `suites` are each one's key, what runs it, and what its last whole run ran on.
pub fn assess(root: &Path, source_dir: &str, suites: &[(String, Runner, Option<Baseline>)]) -> HashMap<String, Impact> {
    let Some(prefix) = prefix(root).filter(|_| git(root, &["rev-parse", "HEAD"]).is_some()) else {
        let why = "the repo isn't in git, so what changed can't be told";
        return suites.iter().map(|(key, ..)| (key.clone(), Impact::Unknown(why.into()))).collect();
    };
    let now = dirty(root, &prefix);
    let source = root.join(source_dir);
    let mut changes: HashMap<Baseline, Option<BTreeSet<String>>> = HashMap::new();
    let mut owners: HashMap<PathBuf, Option<PathBuf>> = HashMap::new();
    suites
        .iter()
        .map(|(key, runner, base)| {
            let impact = match base {
                None => Impact::Unknown("it hasn't run whole since lazyrunner keeps what a run ran on".into()),
                Some(base) => {
                    match changes.entry(base.clone()).or_insert_with(|| changed_since(root, &prefix, base, &now)) {
                        None => Impact::Unknown("the commit its last run ran on isn't in this checkout".into()),
                        Some(changed) => {
                            let found = Found { root, source: &source, changed };
                            found.impact(runner, &mut owners)
                        }
                    }
                }
            };
            (key.clone(), impact)
        })
        .collect()
}

/// What changed, and where.
struct Found<'a> {
    root: &'a Path,
    source: &'a Path,
    changed: &'a BTreeSet<String>,
}

impl Found<'_> {
    /// What it means for the suite `runner` runs.
    fn impact(&self, runner: &Runner, owners: &mut HashMap<PathBuf, Option<PathBuf>>) -> Impact {
        let whys = match runner {
            Runner::Dotnet { project } => self.dotnet(project, owners),
            Runner::Npm { dir, .. } | Runner::Nx { workspace: dir, .. } => self.folder(dir),
        };
        if whys.is_empty() { Impact::Unchanged } else { Impact::Affected(whys) }
    }

    /// A .NET suite: the changed sources of the projects it references, each with the projects it's through.
    fn dotnet(&self, project: &Path, owners: &mut HashMap<PathBuf, Option<PathBuf>>) -> Vec<Why> {
        let graph = references(project);
        let mut by_chain: BTreeMap<Vec<String>, Vec<String>> = BTreeMap::new();
        for file in self.changed {
            let path = self.root.join(file);
            if !path.starts_with(self.source) {
                continue;
            }
            let chain = match owner(&path, self.root, owners) {
                Some(owner) if watcher::is_source(&path) => chain(&graph, &owner),
                None if is_build_file(&path) => Some(Vec::new()),
                _ => None,
            };
            if let Some(chain) = chain {
                by_chain.entry(chain).or_default().push(file.clone());
            }
        }
        by_chain.into_iter().map(|(chain, files)| Why { chain, files }).collect()
    }

    /// An npm or Nx suite: the changed sources in its folder.
    fn folder(&self, dir: &Path) -> Vec<Why> {
        let files: Vec<String> = self
            .changed
            .iter()
            .filter(|file| {
                let path = self.root.join(file);
                path.starts_with(dir) && watcher::is_test_source(&path)
            })
            .cloned()
            .collect();
        let name = dir.file_name().map_or(String::new(), |name| name.to_string_lossy().into_owned());
        if files.is_empty() { Vec::new() } else { vec![Why { chain: vec![name], files }] }
    }
}

/// A build file no project owns that changes every project under it.
fn is_build_file(path: &Path) -> bool {
    let name = path.file_name().map(|name| name.to_string_lossy().to_lowercase()).unwrap_or_default();
    let extension = path.extension().map(|extension| extension.to_string_lossy().to_lowercase()).unwrap_or_default();
    BUILD_FILES.contains(&name.as_str()) || BUILD_FILES.contains(&extension.as_str())
}

/// The project a file belongs to: the nearest .csproj in its folder or one above it, inside the repo.
pub fn owning_project(path: &Path, root: &Path) -> Option<PathBuf> {
    path.ancestors().skip(1).take_while(|dir| dir.starts_with(root)).find_map(project_in)
}

/// The .csproj in a folder.
fn project_in(dir: &Path) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .find(|file| file.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("csproj")))
}

/// [`owning_project`], each folder looked in once.
fn owner(path: &Path, root: &Path, owners: &mut HashMap<PathBuf, Option<PathBuf>>) -> Option<PathBuf> {
    path.ancestors()
        .skip(1)
        .take_while(|dir| dir.starts_with(root))
        .find_map(|dir| owners.entry(dir.to_path_buf()).or_insert_with(|| project_in(dir)).clone())
}

/// A path as it compares with another: one kind of slash, any case.
fn same(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/").to_lowercase()
}

/// A test project and the projects it references, directly or not: each with the one it was first found through.
fn references(project: &Path) -> HashMap<String, (PathBuf, Option<String>)> {
    let mut graph = HashMap::from([(same(project), (project.to_path_buf(), None))]);
    let mut queue = VecDeque::from([project.to_path_buf()]);
    while let Some(path) = queue.pop_front() {
        for reference in build::references(&path) {
            if let Entry::Vacant(entry) = graph.entry(same(&reference)) {
                entry.insert((reference.clone(), Some(same(&path))));
                queue.push_back(reference);
            }
        }
    }
    graph
}

/// The project names from the test project to `changed`, when it references it at all.
fn chain(graph: &HashMap<String, (PathBuf, Option<String>)>, changed: &Path) -> Option<Vec<String>> {
    let mut at = same(changed);
    let mut names = Vec::new();
    loop {
        let (path, parent) = graph.get(&at)?;
        names.push(path.file_stem().map_or(String::new(), |stem| stem.to_string_lossy().into_owned()));
        match parent {
            Some(parent) => at.clone_from(parent),
            None => break,
        }
    }
    names.reverse();
    Some(names)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{TempDir, write};

    fn git_in(dir: &Path, args: &[&str]) {
        let _ = std::process::Command::new("git")
            .args(["-c", "commit.gpgsign=false", "-c", "user.name=tests", "-c", "user.email=tests@localhost"])
            .args(args)
            .current_dir(dir)
            .output();
    }

    fn project(path: &Path, references: &[&str]) {
        let items: String =
            references.iter().map(|reference| format!("<ProjectReference Include=\"{reference}\" />")).collect();
        write(path, &format!("<Project Sdk=\"Microsoft.NET.Sdk\"><ItemGroup>{items}</ItemGroup></Project>"));
    }

    /// Orders.Tests references Orders.Api, which references Orders.Core; Billing.Tests references nothing; a shop.
    fn repo(temp: &TempDir) -> PathBuf {
        let root = temp.join("repo");
        project(&root.join("src/Orders.Core/Orders.Core.csproj"), &[]);
        write(root.join("src/Orders.Core/Pricing.cs"), "class Pricing {}");
        project(&root.join("src/Orders.Api/Orders.Api.csproj"), &["../Orders.Core/Orders.Core.csproj"]);
        project(&root.join("tests/Orders.Tests/Orders.Tests.csproj"), &["../../src/Orders.Api/Orders.Api.csproj"]);
        project(&root.join("tests/Billing.Tests/Billing.Tests.csproj"), &[]);
        write(root.join("web/shop/app.ts"), "export {};");
        write(root.join(".gitignore"), "bin/\nobj/\n");
        git_in(&root, &["init", "-q", "-b", "main"]);
        git_in(&root, &["add", "-A"]);
        git_in(&root, &["commit", "-q", "-m", "start"]);
        root
    }

    fn suites(root: &Path, base: Option<&Baseline>) -> Vec<(String, Runner, Option<Baseline>)> {
        let dotnet = |path: &str| Runner::Dotnet { project: root.join(path) };
        vec![
            ("orders".into(), dotnet("tests/Orders.Tests/Orders.Tests.csproj"), base.cloned()),
            ("billing".into(), dotnet("tests/Billing.Tests/Billing.Tests.csproj"), base.cloned()),
            ("shop".into(), Runner::Npm { dir: root.join("web/shop"), vitest: false }, base.cloned()),
        ]
    }

    #[test]
    fn a_change_affects_the_suites_that_reference_it_and_says_through_what() {
        let temp = TempDir::new("impact");
        let root = repo(&temp);
        let base = baseline(&root).expect("in git");
        let found = assess(&root, "", &suites(&root, Some(&base)));
        assert!(found.values().all(|impact| *impact == Impact::Unchanged), "nothing changed: {found:?}");

        write(root.join("src/Orders.Core/Pricing.cs"), "class Pricing { int Rate; }");
        write(root.join("src/Orders.Core/obj/Pricing.g.cs"), "// built");
        let found = assess(&root, "", &suites(&root, Some(&base)));
        let why = Why {
            chain: vec!["Orders.Tests".into(), "Orders.Api".into(), "Orders.Core".into()],
            files: vec!["src/Orders.Core/Pricing.cs".into()],
        };
        assert_eq!(found["orders"], Impact::Affected(vec![why]), "through the projects it references");
        assert_eq!(found["billing"], Impact::Unchanged);
        assert_eq!(found["shop"], Impact::Unchanged, "not its folder");

        git_in(&root, &["commit", "-qam", "rate"]);
        let found = assess(&root, "", &suites(&root, Some(&base)));
        assert!(found["orders"].runs(), "committed since its run: still affected");
        let ran = baseline(&root).expect("in git");
        assert!(!assess(&root, "", &suites(&root, Some(&ran)))["orders"].runs(), "ran on it since");

        write(root.join("Directory.Build.props"), "<Project />");
        write(root.join("notes.json"), "{}");
        let found = assess(&root, "", &suites(&root, Some(&ran)));
        let Impact::Affected(whys) = &found["billing"] else { panic!("a build file no project owns: {found:?}") };
        assert_eq!(whys[0], Why { chain: Vec::new(), files: vec!["Directory.Build.props".into()] });
        assert_eq!(found["shop"], Impact::Unchanged);

        let props = baseline(&root).expect("in git");
        assert!(!assess(&root, "", &suites(&root, Some(&props)))["billing"].runs(), "ran with the props as they are");
        std::fs::remove_file(root.join("Directory.Build.props")).expect("removed");
        assert!(assess(&root, "", &suites(&root, Some(&props)))["billing"].runs(), "undone since: not what it ran on");
    }

    #[test]
    fn a_suite_that_never_ran_whole_or_whose_commit_is_gone_runs_with_the_affected() {
        let temp = TempDir::new("impact-unknown");
        let root = repo(&temp);
        let found = assess(&root, "", &suites(&root, None));
        assert!(matches!(found["orders"], Impact::Unknown(_)));
        let gone = Baseline { head: "0".repeat(40), dirty: BTreeMap::new() };
        let found = assess(&root, "", &suites(&root, Some(&gone)));
        assert!(matches!(&found["billing"], Impact::Unknown(why) if why.contains("isn't in this checkout")));

        let base = baseline(&root).expect("in git");
        write(root.join("web/shop/app.ts"), "export const x = 1;");
        let found = assess(&root, "", &suites(&root, Some(&base)));
        let shop = Why { chain: vec!["shop".into()], files: vec!["web/shop/app.ts".into()] };
        assert_eq!(found["shop"], Impact::Affected(vec![shop]));
        assert_eq!(found["orders"], Impact::Unchanged, "not a .NET source");
    }

    #[test]
    fn a_folder_outside_git_cant_tell() {
        let temp = TempDir::new("impact-no-git");
        let found = assess(temp.path(), "", &suites(temp.path(), None));
        assert!(found.values().all(|impact| matches!(impact, Impact::Unknown(why) if why.contains("isn't in git"))));
    }
}
