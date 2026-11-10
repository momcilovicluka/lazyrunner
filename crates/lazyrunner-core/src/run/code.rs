//! The code map (#89): every part of the stack's code and what depends on what. On top, the apps, linked when one's
//! code uses another's; under them the projects - .NET ones by their `<ProjectReference>`s, TypeScript ones (an Nx
//! workspace's, or an Angular app) by their imports and Nx's implicit dependencies; inside a .NET project its
//! namespaces (their `using`s), inside a TypeScript project its folders, and inside a folder its NgModules and
//! components (their `imports: [...]`). Read with patterns, no compiler, each file once until it changes (kept in
//! the state folder): what references what, which parts go round in a cycle, which links a rule doesn't allow (the
//! profile's `layers`, Nx's module boundaries), and what a change to a part affects.

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::profile::{App, AppKind, Layer};

pub use super::code_ts::nx_links;

/// Folders no code of the project's own is in.
pub(crate) const NOT_CODE: [&str; 12] =
    ["bin", "obj", "node_modules", "dist", ".angular", ".git", ".nx", "coverage", "out", "build", ".vs", "TestResults"];
/// How deep under a folder its files are looked for.
const DEPTH: usize = 16;
/// The level the apps are on: the parts inside it are the apps.
pub const STACK: &str = "stack";
/// The name of a TypeScript project's top folder (its files outside any folder of its source).
pub const TOP_FOLDER: &str = "(root)";
/// The files read, by their time, in the state folder.
const CACHE_FILE: &str = "code-cache.json";

/// What a part is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A project that runs (a web app, an exe, an Nx application), or on the top level an app of the profile.
    App,
    Library,
    Tests,
    /// Inside a .NET project: a namespace.
    Namespace,
    /// Inside a TypeScript project: a folder of its source.
    Folder,
    /// Inside a folder: an NgModule, and a component.
    Module,
    Component,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Self::App => "app",
            Self::Library => "library",
            Self::Tests => "tests",
            Self::Namespace => "namespace",
            Self::Folder => "folder",
            Self::Module => "module",
            Self::Component => "component",
        }
    }
}

/// Which side of the stack a part is code of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stack {
    Dotnet,
    Node,
}

/// A part of the code: an app, a project, or a namespace, folder, module or component inside one.
#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    /// Unique in the map: "app:orders", "dotnet:services/Orders/Orders.csproj", "node:libs/shared-ui", and for a part
    /// inside another its id, `#`, then the part's name.
    pub id: String,
    pub name: String,
    pub kind: Kind,
    pub stack: Stack,
    /// What `o` opens: the project's file, the folder, the module's file.
    pub path: PathBuf,
    /// The part it's inside of (by its id): [`STACK`] for an app, `None` for a project.
    pub inside: Option<String>,
    /// The profile's app it belongs to (by its key).
    pub app: Option<String>,
    /// .NET: its target framework.
    pub framework: Option<String>,
    pub files: usize,
    /// The packages it references (.NET); an app's projects.
    pub packages: usize,
    /// Its NgModules and components (TypeScript).
    pub modules: usize,
    /// An Nx project's tags (`scope:shop`).
    pub tags: Vec<String>,
}

/// One part depending on another: a reference, a `using`, an import; `lazy` for a dynamic import (a lazy route),
/// `implicit` for one only Nx's project graph knows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Link {
    pub from: usize,
    pub to: usize,
    pub lazy: bool,
    pub implicit: bool,
}

/// A link a rule doesn't allow, and which rule.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Break {
    pub from: usize,
    pub to: usize,
    pub why: String,
}

/// A source file, and the part it's the code of: its namespace, its folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    pub path: PathBuf,
    pub part: usize,
}

/// The parts, their links, the links rules don't allow, and the source files.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CodeMap {
    pub parts: Vec<Part>,
    pub links: Vec<Link>,
    pub breaks: Vec<Break>,
    pub files: Vec<Source>,
}

impl CodeMap {
    pub fn find(&self, id: &str) -> Option<usize> {
        self.parts.iter().position(|part| part.id == id)
    }

    /// A part by its id or its name (any case): a project first, then an app, then the rest.
    pub fn named(&self, wanted: &str) -> Option<usize> {
        self.find(wanted).or_else(|| {
            let same = |part: &Part| part.name.eq_ignore_ascii_case(wanted);
            let projects = self.parts.iter().position(|part| part.inside.is_none() && same(part));
            let apps = || self.parts.iter().position(|part| part.inside.as_deref() == Some(STACK) && same(part));
            projects.or_else(apps).or_else(|| self.parts.iter().position(same))
        })
    }

    /// The part a part is inside of, by its id: an app's is the stack, a project's none.
    pub fn parent(&self, at: usize) -> Option<&str> {
        self.parts[at].inside.as_deref().filter(|inside| *inside != STACK)
    }

    /// A level of the map: the apps ([`STACK`]), the projects (`inside` none), or the parts of one part - with the
    /// parts outside it they link to. The parts, and the links between them.
    pub fn level(&self, inside: Option<&str>) -> (Vec<usize>, Vec<Link>) {
        let mut shown: Vec<usize> =
            (0..self.parts.len()).filter(|&at| self.parts[at].inside.as_deref() == inside).collect();
        if inside.is_some_and(|inside| inside != STACK) {
            let outside: BTreeSet<usize> = self
                .links
                .iter()
                .filter(|link| shown.contains(&link.from) && self.parts[link.to].inside.as_deref() != inside)
                .map(|link| link.to)
                .collect();
            shown.extend(outside);
        }
        let links =
            self.links.iter().filter(|link| shown.contains(&link.from) && shown.contains(&link.to)).copied().collect();
        (shown, links)
    }

    /// What a part depends on (`forward`) or what depends on it, directly or (`all`) not - on its own level: a
    /// project's are projects; a part inside one's are its siblings, and the parts outside they use.
    pub fn reach(&self, from: usize, forward: bool, all: bool) -> Vec<usize> {
        let level = self.parts[from].inside.as_deref();
        let on_level = |at: usize| self.parts[at].inside.as_deref() == level;
        let outside = |_: usize| level.is_some_and(|level| level != STACK);
        let mut seen = BTreeSet::new();
        let mut queue = VecDeque::from([from]);
        while let Some(at) = queue.pop_front() {
            for link in &self.links {
                let (here, there) = if forward { (link.from, link.to) } else { (link.to, link.from) };
                let wanted = on_level(there) || outside(there);
                if here == at && there != from && wanted && seen.insert(there) && all && on_level(there) {
                    queue.push_back(there);
                }
            }
        }
        seen.into_iter().collect()
    }

    /// The shortest way from one part to another along the links: the parts on it, both ends included.
    pub fn way(&self, from: usize, to: usize) -> Option<Vec<usize>> {
        let mut before: HashMap<usize, usize> = HashMap::new();
        let mut queue = VecDeque::from([from]);
        while let Some(at) = queue.pop_front() {
            if at == to {
                let mut way = vec![to];
                while let Some(&back) = before.get(way.last()?) {
                    way.push(back);
                }
                way.reverse();
                return Some(way);
            }
            for link in self.links.iter().filter(|link| link.from == at) {
                if link.to != from && !before.contains_key(&link.to) {
                    before.insert(link.to, at);
                    queue.push_back(link.to);
                }
            }
        }
        None
    }

    /// The cycles among `parts` (each a set of parts that all reach each other), by Tarjan's strongly connected
    /// components; a part on its own isn't one.
    pub fn cycles(&self, parts: &[usize], links: &[Link]) -> Vec<Vec<usize>> {
        let index_of: HashMap<usize, usize> = parts.iter().enumerate().map(|(at, &part)| (part, at)).collect();
        let mut next: Vec<Vec<usize>> = vec![Vec::new(); parts.len()];
        for link in links {
            if let (Some(&from), Some(&to)) = (index_of.get(&link.from), index_of.get(&link.to))
                && from != to
            {
                next[from].push(to);
            }
        }
        let mut found = Vec::new();
        let mut tarjan =
            Tarjan { next: &next, index: vec![None; parts.len()], low: vec![0; parts.len()], ..Tarjan::default() };
        for at in 0..parts.len() {
            if tarjan.index[at].is_none() {
                tarjan.visit(at, &mut found);
            }
        }
        found
            .into_iter()
            .filter(|cycle| cycle.len() > 1)
            .map(|cycle| cycle.into_iter().map(|at| parts[at]).collect())
            .collect()
    }

    /// The part a source file is the code of, by its name or the end of its path (any case, either slash): a module
    /// or component declared in it, else its namespace or folder.
    pub fn by_file(&self, wanted: &str) -> Option<usize> {
        let wanted = wanted.replace('\\', "/").to_lowercase();
        let file = self.files.iter().find(|file| {
            let path = file.path.to_string_lossy().replace('\\', "/").to_lowercase();
            path == wanted || path.ends_with(&format!("/{wanted}"))
        })?;
        let declared = self
            .parts
            .iter()
            .position(|part| matches!(part.kind, Kind::Module | Kind::Component) && part.path == file.path);
        declared.or(Some(file.part))
    }

    /// How many projects two apps' code shares.
    pub fn shared_projects(&self, one: &str, other: &str) -> usize {
        let theirs: BTreeSet<usize> = self.app_projects(other).into_iter().collect();
        self.app_projects(one).iter().filter(|at| theirs.contains(at)).count()
    }

    /// The projects an app's code is: its own, and all they depend on.
    pub fn app_projects(&self, app: &str) -> Vec<usize> {
        let own: Vec<usize> = (0..self.parts.len())
            .filter(|&at| self.parts[at].inside.is_none() && self.parts[at].app.as_deref() == Some(app))
            .collect();
        let mut all: BTreeSet<usize> = own.iter().copied().collect();
        for &at in &own {
            all.extend(self.reach(at, true, true));
        }
        all.into_iter().collect()
    }
}

#[derive(Default)]
struct Tarjan<'a> {
    next: &'a [Vec<usize>],
    index: Vec<Option<usize>>,
    low: Vec<usize>,
    stack: Vec<usize>,
    counter: usize,
}

impl Tarjan<'_> {
    fn visit(&mut self, at: usize, found: &mut Vec<Vec<usize>>) {
        self.index[at] = Some(self.counter);
        self.low[at] = self.counter;
        self.counter += 1;
        self.stack.push(at);
        for &to in &self.next[at] {
            match self.index[to] {
                None => {
                    self.visit(to, found);
                    self.low[at] = self.low[at].min(self.low[to]);
                }
                Some(index) if self.stack.contains(&to) => self.low[at] = self.low[at].min(index),
                Some(_) => {}
            }
        }
        if Some(self.low[at]) == self.index[at] {
            let mut component = Vec::new();
            while let Some(top) = self.stack.pop() {
                component.push(top);
                if top == at {
                    break;
                }
            }
            component.reverse();
            found.push(component);
        }
    }
}

/// What a file says, once read: a C# file's namespace and `using`s, a TypeScript file's imports (each with whether
/// it's lazy), its decorated classes, and how many NgModules and components it has.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Parsed {
    #[serde(default)]
    pub namespace: Option<String>,
    #[serde(default)]
    pub usings: Vec<String>,
    #[serde(default)]
    pub imports: Vec<(String, bool)>,
    #[serde(default)]
    pub classes: Vec<Decorated>,
    #[serde(default)]
    pub modules: usize,
}

/// An NgModule or a component: its class, which, and what its `imports: [...]` names.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Decorated {
    pub name: String,
    pub module: bool,
    pub imports: Vec<String>,
}

/// The files read, each with its time then: one that hasn't changed isn't read again. Kept in the state folder.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct FileCache {
    files: HashMap<String, (u64, Parsed)>,
    #[serde(skip)]
    used: HashSet<String>,
    #[serde(skip)]
    changed: bool,
}

impl FileCache {
    pub fn load(state: &Path) -> Self {
        let text = std::fs::read_to_string(state.join(CACHE_FILE)).ok();
        text.and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
    }

    /// Writes it, when anything was read anew.
    pub fn save(&mut self, state: &Path) {
        if !std::mem::take(&mut self.changed) {
            return;
        }
        if let Ok(text) = serde_json::to_string(self) {
            let _ = std::fs::create_dir_all(state);
            let _ = std::fs::write(state.join(CACHE_FILE), text);
        }
    }

    /// How many files it knows.
    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// What a file says: as read before when it hasn't changed since, else read now.
    pub(crate) fn parsed(&mut self, file: &Path, parse: fn(&str) -> Parsed) -> Option<Parsed> {
        let key = same(file);
        let ticks = super::build::modified_ticks(file) as u64;
        self.used.insert(key.clone());
        if let Some((known, parsed)) = self.files.get(&key)
            && *known == ticks
        {
            return Some(parsed.clone());
        }
        let parsed = parse(&std::fs::read_to_string(file).ok()?);
        self.files.insert(key, (ticks, parsed.clone()));
        self.changed = true;
        Some(parsed)
    }

    /// Forgets the files not read this time: gone, or no longer any project's.
    fn prune(&mut self) {
        let used = std::mem::take(&mut self.used);
        let before = self.files.len();
        self.files.retain(|file, _| used.contains(file));
        self.changed |= self.files.len() != before;
    }
}

fn pattern(regex: &str) -> Regex {
    Regex::new(regex).expect("a valid pattern")
}

static FRAMEWORK: LazyLock<Regex> = LazyLock::new(|| pattern(r"<TargetFrameworks?>\s*([^<;\s]+)"));
static PACKAGE: LazyLock<Regex> = LazyLock::new(|| pattern(r"(?i)<PackageReference\s+Include="));
static NAMESPACE: LazyLock<Regex> = LazyLock::new(|| pattern(r"(?m)^\s*namespace\s+([\w.]+)"));
static USING: LazyLock<Regex> = LazyLock::new(|| pattern(r"(?m)^\s*(?:global\s+)?using\s+(?:static\s+)?([\w.]+)\s*;"));

/// What a C# file says: its namespace and its `using`s.
fn csharp(text: &str) -> Parsed {
    Parsed {
        namespace: NAMESPACE.captures(text).map(|found| found[1].to_string()),
        usings: USING.captures_iter(text).map(|found| found[1].to_string()).collect(),
        ..Parsed::default()
    }
}

/// A path as it compares with another: one kind of slash, any case.
pub(crate) fn same(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/").to_lowercase()
}

/// A path under the root, with `/`.
pub(crate) fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root).unwrap_or(path).to_string_lossy().replace('\\', "/")
}

/// The files under `dir` whose name `wanted` takes - not in [`NOT_CODE`] folders, nor in the folders `stop` says
/// are another project's.
pub(crate) fn files(dir: &Path, wanted: &dyn Fn(&str) -> bool, stop: &dyn Fn(&Path) -> bool) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut queue = VecDeque::from([(dir.to_path_buf(), 0)]);
    while let Some((at, depth)) = queue.pop_front() {
        let Ok(entries) = std::fs::read_dir(&at) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if path.is_dir() {
                let skip = NOT_CODE.iter().any(|skip| name.eq_ignore_ascii_case(skip));
                if !skip && depth < DEPTH && !stop(&path) {
                    queue.push_back((path, depth + 1));
                }
            } else if wanted(&name) {
                found.push(path);
            }
        }
    }
    found
}

/// The whole map: the .NET projects under the root, the TypeScript ones in the npm apps' folders, the apps on top,
/// and the links the profile's `layers` and Nx's module boundaries don't allow. Each file is read only when it
/// changed since `cache` read it.
pub fn scan(root: &Path, apps: &[App], layers: &[Layer], cache: &mut FileCache) -> CodeMap {
    let mut map = CodeMap::default();
    dotnet(root, apps, &mut map, cache);
    let constraints = super::code_ts::node(root, apps, &mut map, cache);
    stack(root, apps, &mut map);
    map.breaks = breaks(&map, layers, &constraints);
    cache.prune();
    map
}

/// The apps on top: each with the projects its code is and their files, linked to another when its code uses that
/// app's own projects.
fn stack(root: &Path, apps: &[App], map: &mut CodeMap) {
    let first = map.parts.len();
    let mut closures = Vec::new();
    let apps: Vec<&App> = apps.iter().filter(|app| app.command().is_none()).collect();
    for app in &apps {
        let projects = map.app_projects(&app.key);
        let (stack, path) = match &app.kind {
            AppKind::Dotnet { project, .. } => (Stack::Dotnet, root.join(project)),
            AppKind::Npm { dir, .. } => (Stack::Node, root.join(dir)),
            AppKind::Command(_) => continue,
        };
        map.parts.push(Part {
            id: format!("app:{}", app.key),
            name: app.name.clone(),
            kind: Kind::App,
            stack,
            path,
            inside: Some(STACK.into()),
            app: Some(app.key.clone()),
            framework: None,
            files: projects.iter().map(|&at| map.parts[at].files).sum(),
            packages: projects.len(),
            modules: 0,
            tags: Vec::new(),
        });
        closures.push(projects);
    }
    for (at, closure) in closures.iter().enumerate() {
        for (other_at, other) in apps.iter().enumerate() {
            let uses = closure.iter().any(|&part| map.parts[part].app.as_deref() == Some(&other.key));
            if other_at != at && uses {
                map.links.push(Link { from: first + at, to: first + other_at, lazy: false, implicit: false });
            }
        }
    }
}

/// A name against a pattern: any case, `*` for anything.
fn glob(pattern: &str, name: &str) -> bool {
    let (pattern, name) = (pattern.to_lowercase(), name.to_lowercase());
    let pieces: Vec<&str> = pattern.split('*').collect();
    let [first, middle @ .., last] = pieces.as_slice() else { return pattern == name };
    if !name.starts_with(first) || !name[first.len()..].ends_with(last) || name.len() < first.len() + last.len() {
        return false;
    }
    let mut rest = &name[first.len()..name.len() - last.len()];
    for piece in middle {
        match rest.find(piece) {
            Some(at) => rest = &rest[at + piece.len()..],
            None => return false,
        }
    }
    true
}

/// An Nx module boundary: projects tagged `source` may only depend on those with one of `only`'s tags (`*` any),
/// and on none with one of `not`'s.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Constraint {
    pub source: String,
    pub only: Vec<String>,
    pub not: Vec<String>,
}

/// The project links a rule doesn't allow: the profile's `layers`, Nx's module boundaries.
fn breaks(map: &CodeMap, layers: &[Layer], constraints: &[Constraint]) -> Vec<Break> {
    let mut found = Vec::new();
    for link in &map.links {
        let (from, to) = (&map.parts[link.from], &map.parts[link.to]);
        if from.inside.is_some() || to.inside.is_some() {
            continue;
        }
        let layered = layers.iter().find(|layer| {
            glob(&layer.parts, &from.name) && layer.must_not_reference.iter().any(|name| glob(name, &to.name))
        });
        let why = layered.map(|_| format!("{} must not reference {} (the profile's layers)", from.name, to.name));
        let bounded = || {
            constraints.iter().filter(|_| from.stack == Stack::Node && to.stack == Stack::Node).find_map(|rule| {
                let applies = rule.source == "*" || from.tags.contains(&rule.source);
                let allowed = rule.only.is_empty() || rule.only.iter().any(|tag| tag == "*" || to.tags.contains(tag));
                let banned = rule.not.iter().find(|tag| to.tags.contains(tag));
                match (applies, allowed, banned) {
                    (false, ..) => None,
                    (true, _, Some(tag)) => {
                        Some(format!("{} ({}) must not depend on {} ({tag})", from.name, rule.source, to.name))
                    }
                    (true, false, None) => Some(format!(
                        "{} ({}) may only depend on libraries tagged {} - {} isn't",
                        from.name,
                        rule.source,
                        rule.only.join(", "),
                        to.name
                    )),
                    (true, true, None) => None,
                }
            })
        };
        if let Some(why) = why.or_else(bounded) {
            found.push(Break { from: link.from, to: link.to, why });
        }
    }
    found
}

/// A .NET project, as its file says.
struct Project {
    path: PathBuf,
    text: String,
}

/// The .NET projects: every `.csproj` under the root, and those the apps reference, with their namespaces.
fn dotnet(root: &Path, apps: &[App], map: &mut CodeMap, cache: &mut FileCache) {
    let mut found: Vec<PathBuf> = files(root, &|name| name.to_lowercase().ends_with(".csproj"), &|_| false);
    for app in apps {
        if let AppKind::Dotnet { project, .. } = &app.kind {
            found.extend(super::build::projects(&root.join(project)));
        }
    }
    let mut seen = BTreeSet::new();
    let projects: Vec<Project> = found
        .into_iter()
        .filter(|path| seen.insert(same(path)))
        .filter_map(|path| Some(Project { text: crate::json::read_text(&path).ok()?, path }))
        .collect();
    let dirs: Vec<String> = projects.iter().filter_map(|project| project.path.parent().map(same)).collect();
    let first = map.parts.len();
    let mut sources: Vec<Vec<PathBuf>> = Vec::new();
    for project in &projects {
        let dir = project.path.parent().map_or_else(|| root.to_path_buf(), Path::to_path_buf);
        let own = same(&dir);
        // A folder with a project of its own is that project's code.
        let other_project = |path: &Path| {
            let path = same(path);
            dirs.iter().any(|other| *other != own && (path == *other || path.starts_with(&format!("{other}/"))))
        };
        let cs = files(&dir, &|name| name.to_lowercase().ends_with(".cs"), &other_project);
        map.parts.push(dotnet_part(root, apps, project, cs.len()));
        sources.push(cs);
    }
    for (at, project) in projects.iter().enumerate() {
        for reference in super::build::references(&project.path) {
            if let Some(to) = projects.iter().position(|other| same(&other.path) == same(&reference)) {
                map.links.push(Link { from: first + at, to: first + to, lazy: false, implicit: false });
            }
        }
    }
    untest_libraries(map, first);
    namespaces(map, first, &sources, cache);
}

/// A project with the test SDK that projects other than tests reference is a library with its tests inside.
fn untest_libraries(map: &mut CodeMap, first: usize) {
    let used: Vec<usize> = (first..map.parts.len())
        .filter(|&at| map.parts[at].kind == Kind::Tests)
        .filter(|&at| map.links.iter().any(|link| link.to == at && map.parts[link.from].kind != Kind::Tests))
        .collect();
    for at in used {
        map.parts[at].kind = Kind::Library;
    }
}

fn dotnet_part(root: &Path, apps: &[App], project: &Project, files: usize) -> Part {
    let text = &project.text;
    let lower = text.to_lowercase();
    let tests = text.contains("Microsoft.NET.Test.Sdk") || lower.contains("<istestproject>true");
    let runs = text.contains("Microsoft.NET.Sdk.Web") || lower.contains("<outputtype>exe");
    let kind = if tests {
        Kind::Tests
    } else if runs {
        Kind::App
    } else {
        Kind::Library
    };
    let app = apps.iter().find_map(|app| match &app.kind {
        AppKind::Dotnet { project: own, .. } if same(&root.join(own)) == same(&project.path) => Some(app.key.clone()),
        _ => None,
    });
    Part {
        id: format!("dotnet:{}", relative(root, &project.path)),
        name: project.path.file_stem().map_or(String::new(), |stem| stem.to_string_lossy().into_owned()),
        kind,
        stack: Stack::Dotnet,
        path: project.path.clone(),
        inside: None,
        app,
        framework: FRAMEWORK.captures(text).map(|found| found[1].to_string()),
        files,
        packages: PACKAGE.find_iter(text).count(),
        modules: 0,
        tags: Vec::new(),
    }
}

/// Inside each .NET project its namespaces, linked by the `using`s of their files: to the project's other namespaces,
/// and to the projects whose namespaces they are.
fn namespaces(map: &mut CodeMap, first: usize, sources: &[Vec<PathBuf>], cache: &mut FileCache) {
    let mut declared: Vec<Vec<(PathBuf, String, Vec<String>)>> = Vec::new();
    let mut owner: HashMap<String, usize> = HashMap::new();
    for (at, cs) in sources.iter().enumerate() {
        let mut files = Vec::new();
        for file in cs {
            let Some(parsed) = cache.parsed(file, csharp) else { continue };
            let Some(namespace) = parsed.namespace else {
                map.files.push(Source { path: file.clone(), part: first + at });
                continue;
            };
            owner.entry(namespace.clone()).or_insert(first + at);
            files.push((file.clone(), namespace, parsed.usings));
        }
        declared.push(files);
    }
    for (at, files) in declared.iter().enumerate() {
        let project = first + at;
        let mut ids: HashMap<String, usize> = HashMap::new();
        for (file, namespace, _) in files {
            if let Some(&id) = ids.get(namespace) {
                map.parts[id].files += 1;
                map.files.push(Source { path: file.clone(), part: id });
                continue;
            }
            map.files.push(Source { path: file.clone(), part: map.parts.len() });
            let parent = map.parts[project].clone();
            ids.insert(namespace.clone(), map.parts.len());
            map.parts.push(Part {
                id: format!("{}#{namespace}", parent.id),
                name: namespace.clone(),
                kind: Kind::Namespace,
                inside: Some(parent.id.clone()),
                files: 1,
                packages: 0,
                modules: 0,
                framework: None,
                ..parent
            });
        }
        let mut linked = BTreeSet::new();
        for (_, namespace, usings) in files {
            let from = ids[namespace];
            for used in usings {
                let to = match (ids.get(used), owner.get(used)) {
                    (Some(&inner), _) => inner,
                    (None, Some(&other)) if other != project => other,
                    _ => continue,
                };
                if to != from && linked.insert((from, to)) {
                    map.links.push(Link { from, to, lazy: false, implicit: false });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
