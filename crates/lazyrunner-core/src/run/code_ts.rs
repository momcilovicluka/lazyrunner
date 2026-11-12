//! The code map's TypeScript side (#89): in each npm app's folder its projects - an Nx workspace's (each folder with a
//! `project.json`, with its tags), or the app itself - linked by their imports: through the `tsconfig` `paths`
//! aliases (`@shop/shared-ui`), or a relative path into another project's folder; lazy for a dynamic `import()` (a
//! lazy route). Inside each project its source folders, linked the same way; inside each folder its NgModules and
//! components, linked by their `imports: [...]`. Nx's implicit dependencies come from its own project graph
//! (`nx graph`, run in the background after the rest), its module boundaries from `.eslintrc.json`.

use std::collections::{BTreeSet, HashMap};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use regex::Regex;
use serde_json::Value;

use super::code::{CodeMap, Constraint, Decorated, FileCache, Kind, Link, Parsed, Part, Stack, files, relative, same};
use crate::profile::{App, AppKind};

/// How long `nx graph` may take.
const NX_TIMEOUT: Duration = Duration::from_secs(120);

fn pattern(regex: &str) -> Regex {
    Regex::new(regex).expect("a valid pattern")
}

/// `import { X } from '…'`, `export * from '…'`.
static FROM: LazyLock<Regex> =
    LazyLock::new(|| pattern(r#"(?m)(?:^|[;}\s])(?:import|export)\s[^'"`;]*?\bfrom\s*['"]([^'"]+)['"]"#));
/// `import '…'` (for its side effects).
static BARE: LazyLock<Regex> = LazyLock::new(|| pattern(r#"(?m)^\s*import\s*['"]([^'"]+)['"]"#));
/// `import('…')`: lazy.
static DYNAMIC: LazyLock<Regex> = LazyLock::new(|| pattern(r#"\bimport\(\s*['"]([^'"]+)['"]\s*\)"#));
/// An Angular module or component, the class it decorates, and the `imports: [...]` in its decorator.
static DECORATOR: LazyLock<Regex> = LazyLock::new(|| pattern(r"@(NgModule|Component)\s*\("));
static CLASS: LazyLock<Regex> = LazyLock::new(|| pattern(r"\bclass\s+([A-Za-z_$][\w$]*)"));
static IMPORTS: LazyLock<Regex> = LazyLock::new(|| pattern(r"\bimports\s*:\s*\[([^\]]*)\]"));
static NAME: LazyLock<Regex> = LazyLock::new(|| pattern(r"^\s*(?:\.\.\.)?([A-Za-z_$][\w$]*)"));

/// A TypeScript project: its folder, name, kind, tags, and the file `o` opens.
struct Project {
    root: PathBuf,
    name: String,
    kind: Kind,
    tags: Vec<String>,
    file: PathBuf,
}

/// A `tsconfig` `paths` alias: its name (without a trailing `*`), whether it ends in one, and where it points.
struct Alias {
    name: String,
    wildcard: bool,
    target: String,
}

/// The npm apps' folders, each once, with the first app in each.
fn workspaces(root: &Path, apps: &[App]) -> Vec<(PathBuf, String)> {
    let mut seen = BTreeSet::new();
    apps.iter()
        .filter_map(|app| match &app.kind {
            AppKind::Npm { dir, .. } => Some((root.join(dir), app.key.clone())),
            AppKind::Dotnet { .. } | AppKind::Command(_) => None,
        })
        .filter(|(workspace, _)| workspace.is_dir() && seen.insert(same(workspace)))
        .collect()
}

/// The TypeScript projects of every npm app's folder; the module boundaries their `.eslintrc.json` sets.
pub(crate) fn node(root: &Path, apps: &[App], map: &mut CodeMap, cache: &mut FileCache) -> Vec<Constraint> {
    let mut constraints = Vec::new();
    for (workspace, app) in workspaces(root, apps) {
        workspace_map(root, &workspace, &app, map, cache);
        constraints.extend(boundaries(&workspace));
    }
    constraints
}

/// A path with `.` and `..` resolved, without touching the disk.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

fn projects(workspace: &Path) -> Vec<Project> {
    let found = files(workspace, &|name| name == "project.json", &|_| false);
    let mut projects: Vec<Project> = found
        .into_iter()
        .filter_map(|file| {
            let json: Value = serde_json::from_str(&crate::json::read_text(&file).ok()?).ok()?;
            let root = file.parent()?.to_path_buf();
            let folder = root.file_name().map_or(String::new(), |name| name.to_string_lossy().into_owned());
            let name = json["name"].as_str().map_or(folder, str::to_string);
            let kind = match json["projectType"].as_str() {
                _ if name.ends_with("-e2e") => Kind::Tests,
                Some("application") => Kind::App,
                _ => Kind::Library,
            };
            let tags = json["tags"]
                .as_array()
                .map_or_else(Vec::new, |tags| tags.iter().filter_map(|tag| tag.as_str().map(str::to_string)).collect());
            Some(Project { root, name, kind, tags, file })
        })
        .collect();
    if projects.is_empty() {
        let name = workspace.file_name().map_or(String::new(), |name| name.to_string_lossy().into_owned());
        let file = workspace.join("package.json");
        projects.push(Project { root: workspace.to_path_buf(), name, kind: Kind::App, tags: Vec::new(), file });
    }
    projects
}

/// The `tsconfig` `paths` of the workspace (`tsconfig.base.json`, else `tsconfig.json`).
fn aliases(workspace: &Path) -> Vec<Alias> {
    let config = ["tsconfig.base.json", "tsconfig.json"]
        .iter()
        .map(|name| workspace.join(name))
        .find_map(|file| crate::json::read_text(&file).ok());
    let Some(json) = config.and_then(|text| serde_json::from_str::<Value>(&super::secrets::strip_jsonc(&text)).ok())
    else {
        return Vec::new();
    };
    let base = json["compilerOptions"]["baseUrl"].as_str().unwrap_or(".");
    let Some(paths) = json["compilerOptions"]["paths"].as_object() else { return Vec::new() };
    paths
        .iter()
        .filter_map(|(name, targets)| {
            let target = targets.as_array()?.first()?.as_str()?;
            let target = workspace.join(base).join(target).to_string_lossy().replace('\\', "/");
            let wildcard = name.ends_with('*');
            Some(Alias { name: name.trim_end_matches('*').to_string(), wildcard, target })
        })
        .collect()
}

/// The module boundaries of an Nx workspace: every `depConstraints` its `.eslintrc.json` has.
fn boundaries(workspace: &Path) -> Vec<Constraint> {
    fn gather(value: &Value, found: &mut Vec<Constraint>) {
        match value {
            Value::Object(fields) => {
                for (name, inner) in fields {
                    if name == "depConstraints"
                        && let Some(rules) = inner.as_array()
                    {
                        found.extend(rules.iter().filter_map(constraint));
                    } else {
                        gather(inner, found);
                    }
                }
            }
            Value::Array(items) => items.iter().for_each(|item| gather(item, found)),
            _ => {}
        }
    }
    fn tags(value: &Value) -> Vec<String> {
        value
            .as_array()
            .map_or_else(Vec::new, |tags| tags.iter().filter_map(|tag| Some(tag.as_str()?.to_string())).collect())
    }
    fn constraint(rule: &Value) -> Option<Constraint> {
        Some(Constraint {
            source: rule["sourceTag"].as_str()?.to_string(),
            only: tags(&rule["onlyDependOnLibsWithTags"]),
            not: tags(&rule["notDependOnLibsWithTags"]),
        })
    }
    let Ok(text) = crate::json::read_text(&workspace.join(".eslintrc.json")) else { return Vec::new() };
    let Ok(json) = serde_json::from_str::<Value>(&super::secrets::strip_jsonc(&text)) else { return Vec::new() };
    let mut found = Vec::new();
    gather(&json, &mut found);
    found
}

/// Where an import goes, when it's this workspace's code; `None` for a package's.
fn resolve(spec: &str, file: &Path, aliases: &[Alias]) -> Option<PathBuf> {
    if spec.starts_with('.') {
        return Some(normalize(&file.parent()?.join(spec)));
    }
    let alias = aliases
        .iter()
        .filter(|alias| if alias.wildcard { spec.starts_with(&alias.name) } else { spec == alias.name })
        .max_by_key(|alias| alias.name.len())?;
    let target =
        if alias.wildcard { alias.target.replacen('*', &spec[alias.name.len()..], 1) } else { alias.target.clone() };
    Some(normalize(Path::new(&target)))
}

/// A file's folder in its project's source: under `src/`, then `lib/` or `app/`, its first folder; `(root)` for the
/// files at the top.
fn folder_of(project: &Path, file: &Path) -> String {
    let inside = relative(project, file);
    let mut rest = inside.as_str();
    for skip in ["src/", "lib/", "app/"] {
        rest = rest.strip_prefix(skip).unwrap_or(rest);
    }
    match rest.split_once('/') {
        Some((folder, _)) => folder.to_string(),
        None => super::code::TOP_FOLDER.to_string(),
    }
}

/// What a TypeScript file says: its imports, and its NgModules and components with what their `imports: [...]`
/// names.
fn typescript(text: &str) -> Parsed {
    let eager = FROM.captures_iter(text).chain(BARE.captures_iter(text)).map(|found| (found[1].to_string(), false));
    let lazy = DYNAMIC.captures_iter(text).map(|found| (found[1].to_string(), true));
    let mut classes = Vec::new();
    for found in DECORATOR.captures_iter(text) {
        let start = found.get(0).map_or(0, |whole| whole.end());
        let Some(class) = CLASS.captures(&text[start..]) else { continue };
        let body = &text[start..start + class.get(0).map_or(0, |whole| whole.start())];
        let imports = IMPORTS.captures(body).map_or_else(Vec::new, |list| {
            list[1].split(',').filter_map(|item| NAME.captures(item).map(|name| name[1].to_string())).collect()
        });
        classes.push(Decorated { name: class[1].to_string(), module: &found[1] == "NgModule", imports });
    }
    Parsed { imports: eager.chain(lazy).collect(), modules: classes.len(), classes, ..Parsed::default() }
}

/// A file's decorated classes, waiting for every class's part to be made before they're linked.
struct Classes {
    folder: usize,
    file: PathBuf,
    classes: Vec<Decorated>,
}

/// One workspace's projects, their folders and their folders' modules and components, and their links.
fn workspace_map(root: &Path, workspace: &Path, app: &str, map: &mut CodeMap, cache: &mut FileCache) {
    let projects = projects(workspace);
    let aliases = aliases(workspace);
    let roots: Vec<String> = projects.iter().map(|project| same(&project.root)).collect();
    let owner = |path: &Path| {
        let path = same(path);
        (0..roots.len())
            .filter(|&at| path == roots[at] || path.starts_with(&format!("{}/", roots[at])))
            .max_by_key(|&at| roots[at].len())
    };
    let first = map.parts.len();
    let mut sources = Vec::new();
    for (at, project) in projects.iter().enumerate() {
        let own = &roots[at];
        let nested = |path: &Path| {
            let path = same(path);
            roots.iter().any(|other| other != own && (path == *other || path.starts_with(&format!("{other}/"))))
        };
        let ts = files(&project.root, &|name| name.ends_with(".ts") && !name.ends_with(".d.ts"), &nested);
        map.parts.push(Part {
            id: format!("node:{}", relative(root, &project.root)),
            name: project.name.clone(),
            kind: project.kind,
            stack: Stack::Node,
            path: project.file.clone(),
            inside: None,
            app: Some(app.to_string()),
            framework: None,
            files: ts.len(),
            packages: 0,
            modules: 0,
            tags: project.tags.clone(),
        });
        sources.push(ts);
    }
    let mut folders: HashMap<(usize, String), usize> = HashMap::new();
    let mut links: HashMap<(usize, usize), bool> = HashMap::new();
    let mut link = |from: usize, to: usize, lazy: bool| {
        let all_lazy = links.entry((from, to)).or_insert(lazy);
        *all_lazy &= lazy;
    };
    let mut decorated = Vec::new();
    for (at, ts) in sources.iter().enumerate() {
        let project = first + at;
        for file in ts {
            let Some(parsed) = cache.parsed(file, typescript) else { continue };
            let from = folder(map, &mut folders, project, &projects[at].root, file);
            map.parts[from].files += 1;
            map.files.push(super::code::Source { path: file.clone(), part: from });
            map.parts[from].modules += parsed.modules;
            map.parts[project].modules += parsed.modules;
            for (spec, lazy) in &parsed.imports {
                let Some(target) = resolve(spec, file, &aliases) else { continue };
                let Some(to) = owner(&target) else { continue };
                if to == at {
                    let inner = folder(map, &mut folders, project, &projects[at].root, &target);
                    if inner != from {
                        link(from, inner, *lazy);
                    }
                } else {
                    link(project, first + to, *lazy);
                    link(from, first + to, *lazy);
                }
            }
            if !parsed.classes.is_empty() {
                decorated.push(Classes { folder: from, file: file.clone(), classes: parsed.classes });
            }
        }
    }
    let mut links: Vec<Link> =
        links.into_iter().map(|((from, to), lazy)| Link { from, to, lazy, implicit: false }).collect();
    links.sort_by_key(|link| (link.from, link.to));
    map.links.extend(links);
    modules(map, decorated);
}

/// The NgModules and components inside their folders, linked by their `imports: [...]` to the others of the
/// workspace they name.
fn modules(map: &mut CodeMap, decorated: Vec<Classes>) {
    let mut by_name: HashMap<String, usize> = HashMap::new();
    let mut made = Vec::new();
    for found in &decorated {
        for class in &found.classes {
            let parent = map.parts[found.folder].clone();
            let at = map.parts.len();
            map.parts.push(Part {
                id: format!("{}#{}", parent.id, class.name),
                name: class.name.clone(),
                kind: if class.module { Kind::Module } else { Kind::Component },
                path: found.file.clone(),
                inside: Some(parent.id.clone()),
                files: 1,
                modules: 0,
                ..parent
            });
            by_name.entry(class.name.clone()).or_insert(at);
            made.push((at, &class.imports));
        }
    }
    let mut linked = BTreeSet::new();
    for (from, imports) in made {
        for name in imports {
            if let Some(&to) = by_name.get(name)
                && to != from
                && linked.insert((from, to))
            {
                map.links.push(Link { from, to, lazy: false, implicit: false });
            }
        }
    }
}

/// The part of a file's folder in its project, made when it's the first file of it seen.
fn folder(
    map: &mut CodeMap,
    folders: &mut HashMap<(usize, String), usize>,
    project: usize,
    root: &Path,
    file: &Path,
) -> usize {
    let name = folder_of(root, file);
    if let Some(&at) = folders.get(&(project, name.clone())) {
        return at;
    }
    let parent = map.parts[project].clone();
    let path = if name == super::code::TOP_FOLDER { root.to_path_buf() } else { find_folder(root, &name) };
    map.parts.push(Part {
        id: format!("{}#{name}", parent.id),
        name: name.clone(),
        kind: Kind::Folder,
        path,
        inside: Some(parent.id.clone()),
        files: 0,
        modules: 0,
        tags: Vec::new(),
        ..parent
    });
    folders.insert((project, name), map.parts.len() - 1);
    map.parts.len() - 1
}

/// Where a source folder is: under `src/lib/`, `src/app/` or `src/`, as [`folder_of`] took it.
fn find_folder(root: &Path, name: &str) -> PathBuf {
    ["src/lib", "src/app", "src", "lib", "app", "."]
        .iter()
        .map(|under| root.join(under).join(name))
        .find(|path| path.is_dir())
        .unwrap_or_else(|| root.join(name))
}

/// Runs a command to its end, `limit` at most (then it's stopped): whether it went well.
fn run_for(mut command: Command, limit: Duration) -> bool {
    let Ok(mut child) = command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() else {
        return false;
    };
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if started.elapsed() < limit => std::thread::sleep(Duration::from_millis(200)),
            _ => {
                let _ = child.kill();
                return false;
            }
        }
    }
}

/// Nx's implicit dependencies, from its own project graph (`nx graph --file`, the workspace's own Nx, no daemon, no
/// Nx Cloud): the links between the map's TypeScript projects that no import says. How many it added.
pub fn nx_links(root: &Path, apps: &[App], map: &mut CodeMap) -> usize {
    let mut added = 0;
    for (workspace, _) in workspaces(root, apps) {
        let nx = workspace.join("node_modules").join(".bin").join(if cfg!(windows) { "nx.cmd" } else { "nx" });
        if !workspace.join("nx.json").is_file() || !nx.is_file() {
            continue;
        }
        let out = std::env::temp_dir().join(format!("lazyrunner-nx-graph-{}.json", std::process::id()));
        let mut command = Command::new(&nx);
        command
            .arg("graph")
            .arg(format!("--file={}", out.display()))
            .current_dir(&workspace)
            .env("NX_DAEMON", "false")
            .env("NX_NO_CLOUD", "true");
        let ran = run_for(command, NX_TIMEOUT);
        let graph = std::fs::read_to_string(&out).ok().filter(|_| ran);
        let _ = std::fs::remove_file(&out);
        if let Some(graph) = graph.and_then(|text| serde_json::from_str::<Value>(&text).ok()) {
            added += implicit(map, &workspace_id(root, &workspace), &graph);
        }
    }
    added
}

/// The start of the ids of a workspace's projects.
fn workspace_id(root: &Path, workspace: &Path) -> String {
    let rel = relative(root, workspace);
    if rel.is_empty() { "node:".into() } else { format!("node:{rel}") }
}

/// The implicit dependencies of an Nx project graph (`graph.dependencies`, those of type `implicit`) between the
/// workspace's projects, by name.
fn implicit(map: &mut CodeMap, prefix: &str, graph: &Value) -> usize {
    let project = |name: &str| {
        map.parts.iter().position(|part| part.inside.is_none() && part.id.starts_with(prefix) && part.name == name)
    };
    let mut found = Vec::new();
    let Some(dependencies) = graph["graph"]["dependencies"].as_object() else { return 0 };
    for edges in dependencies.values().filter_map(Value::as_array) {
        for edge in edges.iter().filter(|edge| edge["type"] == "implicit") {
            let (Some(source), Some(target)) = (edge["source"].as_str(), edge["target"].as_str()) else { continue };
            if let (Some(from), Some(to)) = (project(source), project(target)) {
                found.push((from, to));
            }
        }
    }
    let mut added = 0;
    for (from, to) in found {
        if from != to && !map.links.iter().any(|link| link.from == from && link.to == to) {
            map.links.push(Link { from, to, lazy: false, implicit: true });
            added += 1;
        }
    }
    added
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{TempDir, write};

    fn npm(key: &str, dir: &str) -> App {
        App {
            key: key.into(),
            name: key.into(),
            icon: None,
            nerd_icon: None,
            kind: AppKind::Npm { dir: dir.into(), variants: Vec::new() },
            tests: Vec::new(),
            migrations: None,
            env: Vec::new(),
            depends_on: Vec::new(),
            health: crate::profile::Health::default(),
        }
    }

    fn workspace(root: &Path) {
        let web = root.join("web");
        write(
            web.join("tsconfig.base.json"),
            r#"{ // the aliases
              "compilerOptions": { "paths": { "@shop/ui": ["libs/ui/src/index.ts"], "@shop/data/*": ["libs/data/src/*"] } } }"#,
        );
        write(
            web.join(".eslintrc.json"),
            r#"{ "overrides": [{ "rules": { "@nx/enforce-module-boundaries": ["error", { "depConstraints": [
              { "sourceTag": "type:ui", "onlyDependOnLibsWithTags": ["type:ui"] } ] }] } }] }"#,
        );
        write(web.join("apps/shop/project.json"), r#"{ "name": "shop", "projectType": "application" }"#);
        write(web.join("libs/ui/project.json"), r#"{ "name": "ui", "projectType": "library", "tags": ["type:ui"] }"#);
        write(
            web.join("libs/data/project.json"),
            r#"{ "name": "data", "projectType": "library", "tags": ["type:data"] }"#,
        );
        write(
            web.join("apps/shop/src/app/app.routes.ts"),
            "import { Button } from '@shop/ui';\nexport const routes = [{ loadChildren: () => import('@shop/data/cart') }];",
        );
        write(
            web.join("apps/shop/src/app/home/home.module.ts"),
            "import { Shared } from '../shared/shared.module';\n@NgModule({\n  declarations: [Home],\n  imports: [CommonModule, Shared, RouterModule.forChild(routes)]\n})\nexport class HomeModule {}\n@Component({ selector: 'x', template: '[a]' })\nexport class Home {}",
        );
        write(
            web.join("apps/shop/src/app/shared/shared.module.ts"),
            "@NgModule({ imports: [] })\nexport class Shared {}",
        );
        write(web.join("libs/ui/src/index.ts"), "export * from './lib/button/button.component';");
        write(
            web.join("libs/ui/src/lib/button/button.component.ts"),
            "import { Cart } from '@shop/data/cart';\n@Component({ standalone: true, imports: [] })\nexport class Button {}",
        );
        write(web.join("libs/data/src/cart.ts"), "import { Button } from '@shop/ui';\nexport class Cart {}");
        write(web.join("libs/ui/node_modules/x/index.ts"), "import '@shop/data/cart';");
    }

    #[test]
    fn an_nx_workspaces_projects_are_linked_by_their_imports_and_their_folders_inside_them() {
        let temp = TempDir::new("code-ts");
        let root = temp.join("repo");
        workspace(&root);
        let mut map = CodeMap::default();
        node(&root, &[npm("shop", "web")], &mut map, &mut FileCache::default());
        let at = |name: &str| map.named(name).unwrap_or_else(|| panic!("{name} in {:?}", map.parts));
        let (shop, ui, data) = (at("shop"), at("ui"), at("data"));
        assert_eq!((map.parts[shop].kind, map.parts[ui].kind), (Kind::App, Kind::Library));
        assert_eq!(map.parts[ui].tags, ["type:ui"]);
        assert_eq!(map.parts[shop].modules, 3, "two modules and a component");
        let link = |from, to| map.links.iter().find(|link| link.from == from && link.to == to).copied();
        assert_eq!(link(shop, ui).map(|link| link.lazy), Some(false));
        assert_eq!(link(shop, data).map(|link| link.lazy), Some(true), "a lazy route");
        let (projects, links) = map.level(None);
        let mut cycle = map.cycles(&projects, &links).concat();
        cycle.sort_unstable();
        let mut expected = vec![ui, data];
        expected.sort_unstable();
        assert_eq!(cycle, expected, "ui and data import each other");

        let (inside, inner) = map.level(Some(&map.parts[shop].id));
        let names: BTreeSet<&str> = inside.iter().map(|&at| map.parts[at].name.as_str()).collect();
        assert!(names.contains("home") && names.contains("shared") && names.contains("(root)"), "{names:?}");
        assert!(inner.iter().any(|link| map.parts[link.from].name == "home" && map.parts[link.to].name == "shared"));

        let home = at("home");
        let (classes, class_links) = map.level(Some(&map.parts[home].id));
        let named: BTreeSet<&str> = classes.iter().map(|&at| map.parts[at].name.as_str()).collect();
        assert!(named.contains("HomeModule") && named.contains("Home") && named.contains("Shared"), "{named:?}");
        let class = |name: &str| map.parts.iter().position(|part| part.name == name && part.kind == Kind::Module);
        let (module, shared) = (at("HomeModule"), class("Shared").expect("the Shared module"));
        assert_eq!(map.parts[module].kind, Kind::Module);
        assert!(class_links.iter().any(|link| link.from == module && link.to == shared), "its imports: [...]");
    }

    #[test]
    fn nx_module_boundaries_and_implicit_dependencies() {
        let temp = TempDir::new("code-nx");
        let root = temp.join("repo");
        workspace(&root);
        let apps = [npm("shop", "web")];
        let map = super::super::code::scan(&root, &apps, &[], &mut FileCache::default());
        let (ui, data) = (map.named("ui").expect("ui"), map.named("data").expect("data"));
        let broken: Vec<(usize, usize)> = map.breaks.iter().map(|rule| (rule.from, rule.to)).collect();
        assert_eq!(broken, [(ui, data)], "type:ui may only depend on type:ui: {:?}", map.breaks);

        let mut map = map;
        let graph = serde_json::json!({ "graph": { "dependencies": {
            "shop": [{ "source": "shop", "target": "ui", "type": "static" }, { "source": "data", "target": "shop", "type": "implicit" }]
        } } });
        assert_eq!(implicit(&mut map, "node:web", &graph), 1);
        let shop = map.named("shop").expect("shop");
        assert!(map.links.iter().any(|link| link.from == data && link.to == shop && link.implicit));
    }
}
