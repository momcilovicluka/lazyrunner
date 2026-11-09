//! A failure's likely cause (#34): the sources that changed between the run its test last passed in and the one it
//! failed in - git's diff between the two commits, and the files that differed on top of them - those in the
//! failure's stack first, then those of the test's project and the projects it references, then the rest.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use super::impact::{self, Baseline};
use super::suites::Runner;
use super::tools::git;
use super::watcher;

/// Why a changed file may have broken the test.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Lead {
    /// The failure's stack goes through it.
    Stack,
    /// It's in the test's project or one it references: the projects from the test's to its own.
    Chain(Vec<String>),
    /// It only changed.
    Changed,
}

/// A changed file and why it may be the cause.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cause {
    pub file: String,
    pub lead: Lead,
}

/// What changed between two runs' code (from the root, with `/`); `None` when git can't say.
fn changed_between(root: &Path, before: &Baseline, after: &Baseline) -> Option<BTreeSet<String>> {
    let prefix = impact::prefix(root)?;
    let committed = git(root, &["diff", "--name-only", &before.head, &after.head, "--", "."])?;
    let mut changed: BTreeSet<String> =
        committed.lines().filter_map(|line| impact::from_root(line, &prefix)).filter(|line| !line.is_empty()).collect();
    let dirty = before.dirty.keys().chain(after.dirty.keys());
    changed.extend(dirty.filter(|path| before.dirty.get(*path) != after.dirty.get(*path)).cloned());
    Some(changed)
}

/// The sources changed between the run that passed and the one that failed, most likely first; `None` when git
/// can't say (a commit isn't in this checkout any more): then it's left out rather than guessed.
pub fn likely_cause(
    root: &Path,
    source_dir: &str,
    runner: &Runner,
    passed: &Baseline,
    failed: &Baseline,
    stack: &str,
) -> Option<Vec<Cause>> {
    let changed = changed_between(root, passed, failed)?;
    let stack = stack.replace('\\', "/").to_lowercase();
    let lead = Leads::new(root, source_dir, runner);
    let mut causes: Vec<Cause> = changed
        .into_iter()
        .filter_map(|file| {
            let lead = lead.of(&file, &stack)?;
            Some(Cause { file, lead })
        })
        .collect();
    causes.sort_by_key(|cause| rank(&cause.lead));
    Some(causes)
}

fn rank(lead: &Lead) -> usize {
    match lead {
        Lead::Stack => 0,
        Lead::Chain(_) => 1,
        Lead::Changed => 2,
    }
}

/// What a changed file means for the test's suite.
struct Leads<'a> {
    root: &'a Path,
    source: PathBuf,
    runner: &'a Runner,
    graph: HashMap<String, (PathBuf, Option<String>)>,
    owners: std::cell::RefCell<HashMap<PathBuf, Option<PathBuf>>>,
}

impl<'a> Leads<'a> {
    fn new(root: &'a Path, source_dir: &str, runner: &'a Runner) -> Self {
        let graph = match runner {
            Runner::Dotnet { project } => impact::references(project),
            Runner::Npm { .. } | Runner::Nx { .. } => HashMap::new(),
        };
        Self { root, source: root.join(source_dir), runner, graph, owners: Default::default() }
    }

    /// Its lead, or `None` for a file that isn't a source (a readme, a lock file).
    fn of(&self, file: &str, stack: &str) -> Option<Lead> {
        let path = self.root.join(file);
        let source = watcher::is_source(&path) || watcher::is_test_source(&path);
        if !source {
            return None;
        }
        if in_stack(stack, &file.to_lowercase()) || self.in_stack_from_its_folder(stack, &path) {
            return Some(Lead::Stack);
        }
        Some(self.chain(&path).map_or(Lead::Changed, Lead::Chain))
    }

    /// An npm or Nx stack names files from the app's folder.
    fn in_stack_from_its_folder(&self, stack: &str, path: &Path) -> bool {
        let dir = match self.runner {
            Runner::Npm { dir, .. } | Runner::Nx { workspace: dir, .. } => dir,
            Runner::Dotnet { .. } => return false,
        };
        let Ok(inside) = path.strip_prefix(dir) else { return false };
        in_stack(stack, &inside.to_string_lossy().replace('\\', "/").to_lowercase())
    }

    /// The projects from the test's to the file's, when the suite depends on it.
    fn chain(&self, path: &Path) -> Option<Vec<String>> {
        match self.runner {
            Runner::Dotnet { .. } => {
                if !path.starts_with(&self.source) || !watcher::is_source(path) {
                    return None;
                }
                let owner = impact::owner(path, self.root, &mut self.owners.borrow_mut())?;
                impact::chain(&self.graph, &owner)
            }
            Runner::Npm { dir, .. } | Runner::Nx { workspace: dir, .. } => {
                let name = dir.file_name().map_or(String::new(), |name| name.to_string_lossy().into_owned());
                path.starts_with(dir).then(|| vec![name])
            }
        }
    }
}

/// Whether the stack names this path (lowercase, with `/`) as a whole path, not the end of another file's name.
fn in_stack(stack: &str, path: &str) -> bool {
    stack.match_indices(path).any(|(at, _)| {
        let before = stack[..at].chars().next_back();
        before.is_none_or(|before| matches!(before, '/' | '(' | ' ' | '\t' | '\n' | '\'' | '"'))
    })
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

    #[test]
    fn the_files_in_the_stack_come_first_then_the_tests_projects_then_the_rest() {
        let temp = TempDir::new("cause");
        let root = temp.join("repo");
        let project = |path: &str, refs: &str| {
            let items =
                if refs.is_empty() { String::new() } else { format!("<ProjectReference Include=\"{refs}\" />") };
            write(root.join(path), &format!("<Project><ItemGroup>{items}</ItemGroup></Project>"));
        };
        project("src/Orders.Core/Orders.Core.csproj", "");
        project("src/Billing/Billing.csproj", "");
        project("tests/Orders.Tests/Orders.Tests.csproj", "../../src/Orders.Core/Orders.Core.csproj");
        for file in ["src/Orders.Core/Pricing.cs", "src/Orders.Core/Tax.cs", "src/Billing/Invoice.cs", "README.md"] {
            write(root.join(file), "1");
        }
        git_in(&root, &["init", "-q", "-b", "main"]);
        git_in(&root, &["add", "-A"]);
        git_in(&root, &["commit", "-q", "-m", "green"]);
        let passed = impact::baseline(&root).expect("in git");
        for file in ["src/Orders.Core/Pricing.cs", "src/Orders.Core/Tax.cs", "src/Billing/Invoice.cs", "README.md"] {
            write(root.join(file), "2");
        }
        git_in(&root, &["commit", "-qam", "edits"]);
        write(root.join("src/Billing/Invoice.cs"), "3");
        let failed = impact::baseline(&root).expect("in git");

        let runner = Runner::Dotnet { project: root.join("tests/Orders.Tests/Orders.Tests.csproj") };
        let stack = "at Orders.Core.Tax.Rate() in C:\\work\\repo\\src\\Orders.Core\\Tax.cs:line 9";
        let causes = likely_cause(&root, "", &runner, &passed, &failed, stack).expect("both commits are here");
        let chain = Lead::Chain(vec!["Orders.Tests".into(), "Orders.Core".into()]);
        assert_eq!(
            causes,
            [
                Cause { file: "src/Orders.Core/Tax.cs".into(), lead: Lead::Stack },
                Cause { file: "src/Orders.Core/Pricing.cs".into(), lead: chain },
                Cause { file: "src/Billing/Invoice.cs".into(), lead: Lead::Changed },
            ],
            "the readme isn't a source"
        );

        let gone = Baseline { head: "0".repeat(40), ..passed.clone() };
        assert_eq!(likely_cause(&root, "", &runner, &gone, &failed, stack), None, "left out rather than guessed");
        assert_eq!(likely_cause(&root, "", &runner, &failed, &failed, stack), Some(Vec::new()), "the same code");
    }

    #[test]
    fn a_stack_names_a_file_only_as_a_whole_path() {
        assert!(in_stack("at (src/cart.ts:3:1)", "src/cart.ts"));
        assert!(in_stack("c:/work/src/cart.ts", "src/cart.ts"));
        assert!(!in_stack("c:/work/src/minicart.ts", "cart.ts"));
    }
}
