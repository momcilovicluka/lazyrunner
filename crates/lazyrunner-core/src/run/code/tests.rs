//! The code map's tests (#89): the .NET projects, the apps on top and the rules, the file cache, cycles.

use super::*;
use crate::testing::{TempDir, write};

fn project(path: &Path, refs: &[&str], extra: &str) {
    let items: String = refs.iter().map(|to| format!("<ProjectReference Include=\"{to}\" />")).collect();
    write(
        path,
        &format!(
            "<Project Sdk=\"Microsoft.NET.Sdk\"><PropertyGroup><TargetFramework>net8.0</TargetFramework></PropertyGroup><ItemGroup>{items}{extra}</ItemGroup></Project>"
        ),
    );
}

fn dotnet_app(key: &str, project: &str) -> App {
    App {
        key: key.into(),
        name: key.into(),
        icon: None,
        nerd_icon: None,
        kind: AppKind::Dotnet {
            project: project.into(),
            launch_profile: String::new(),
            swagger: None,
            args: Vec::new(),
        },
        tests: Vec::new(),
        migrations: None,
        env: Vec::new(),
        depends_on: Vec::new(),
    }
}

fn orders(root: &Path) {
    project(&root.join("src/Orders.Core/Orders.Core.csproj"), &[], "<PackageReference Include=\"Dapper\" />");
    project(&root.join("src/Orders.Api/Orders.Api.csproj"), &["../Orders.Core/Orders.Core.csproj"], "");
    project(
        &root.join("tests/Orders.Tests/Orders.Tests.csproj"),
        &["../../src/Orders.Core/Orders.Core.csproj"],
        "<PackageReference Include=\"Microsoft.NET.Test.Sdk\" />",
    );
    write(
        root.join("src/Orders.Core/Pricing/Rate.cs"),
        "namespace Orders.Core.Pricing;\nusing Orders.Core.Tax;\nclass Rate {}",
    );
    write(root.join("src/Orders.Core/Tax/Vat.cs"), "namespace Orders.Core.Tax\n{\n    class Vat {}\n}");
    write(root.join("src/Orders.Core/obj/Gen.cs"), "namespace Generated;");
    write(root.join("src/Orders.Api/Program.cs"), "using Orders.Core.Pricing;\nusing System;\nnamespace Orders.Api;");
}

#[test]
fn a_library_with_its_tests_inside_is_a_library_and_a_test_project_nothing_uses_is_tests() {
    let temp = TempDir::new("code-untest");
    let root = temp.join("repo");
    let sdk = "<PackageReference Include=\"Microsoft.NET.Test.Sdk\" /><PackageReference Include=\"xunit\" />";
    project(&root.join("Data/Data.csproj"), &[], sdk);
    project(&root.join("Api/Api.csproj"), &["../Data/Data.csproj"], "");
    project(&root.join("Data.Tests/Data.Tests.csproj"), &["../Data/Data.csproj"], sdk);
    let map = scan(&root, &[], &[], &mut FileCache::default());
    let kind = |name: &str| map.parts[map.named(name).unwrap_or_else(|| panic!("{name}"))].kind;
    assert_eq!((kind("Data"), kind("Data.Tests")), (Kind::Library, Kind::Tests));
}

#[test]
fn the_dotnet_projects_their_references_their_namespaces_and_what_they_use() {
    let temp = TempDir::new("code-dotnet");
    let root = temp.join("repo");
    orders(&root);
    let map = scan(&root, &[], &[], &mut FileCache::default());
    let at = |name: &str| map.named(name).unwrap_or_else(|| panic!("{name} in {:?}", map.parts));
    let (core, api, tests) = (at("Orders.Core"), at("Orders.Api"), at("Orders.Tests"));
    assert_eq!((map.parts[core].kind, map.parts[tests].kind), (Kind::Library, Kind::Tests));
    assert_eq!((map.parts[core].files, map.parts[core].packages), (2, 1), "obj isn't its code");
    let vat = map.by_file("vat.cs").map(|at| map.parts[at].name.as_str());
    assert_eq!(vat, Some("Orders.Core.Tax"), "a file: its namespace");
    assert_eq!(map.by_file("Tax/Vat.cs"), map.by_file("vat.cs"), "or the end of its path");
    assert_eq!(map.parts[core].framework.as_deref(), Some("net8.0"));
    let mut users = map.reach(core, false, true);
    users.sort_unstable();
    let mut expected = vec![api, tests];
    expected.sort_unstable();
    assert_eq!(users, expected, "what uses Orders.Core");
    assert_eq!(map.way(api, core), Some(vec![api, core]));

    let (inside, links) = map.level(Some(&map.parts[core].id));
    let mut names: Vec<&str> = inside.iter().map(|&at| map.parts[at].name.as_str()).collect();
    names.sort_unstable();
    assert_eq!(names, ["Orders.Core.Pricing", "Orders.Core.Tax"]);
    assert_eq!(links.len(), 1, "Pricing uses Tax");
    let (api_inside, _) = map.level(Some(&map.parts[api].id));
    assert!(api_inside.contains(&core), "Orders.Api's namespace uses one of Orders.Core's: it shows, outside");
}

#[test]
fn the_apps_are_on_top_with_their_projects_and_a_rule_breaks_a_link() {
    let temp = TempDir::new("code-apps");
    let root = temp.join("repo");
    orders(&root);
    let apps = [dotnet_app("orders", "src/Orders.Api/Orders.Api.csproj")];
    let layers = [Layer { parts: "*.Api".into(), must_not_reference: vec!["Orders.Core".into()] }];
    let map = scan(&root, &apps, &layers, &mut FileCache::default());
    let (top, _) = map.level(Some(STACK));
    assert_eq!(top.len(), 1);
    let app = &map.parts[top[0]];
    assert_eq!((app.name.as_str(), app.packages), ("orders", 2), "Orders.Api and what it references");
    let names: Vec<&str> = map.app_projects("orders").iter().map(|&at| map.parts[at].name.as_str()).collect();
    assert!(names.contains(&"Orders.Api") && names.contains(&"Orders.Core") && !names.contains(&"Orders.Tests"));
    assert_eq!(map.breaks.len(), 1, "{:?}", map.breaks);
    assert!(map.breaks[0].why.contains("Orders.Api must not reference Orders.Core"), "{:?}", map.breaks);
}

#[test]
fn a_file_is_read_again_only_when_it_changed() {
    let temp = TempDir::new("code-cache");
    let root = temp.join("repo");
    orders(&root);
    let mut cache = FileCache::default();
    scan(&root, &[], &[], &mut cache);
    let read = cache.len();
    assert!(read >= 3, "the .cs files: {read}");
    let state = temp.join("state");
    cache.save(&state);
    let mut again = FileCache::load(&state);
    assert_eq!(again.len(), read, "kept in the state folder");
    std::fs::remove_file(root.join("src/Orders.Core/Tax/Vat.cs")).expect("removed");
    let map = scan(&root, &[], &[], &mut again);
    assert_eq!(again.len(), read - 1, "a gone file is forgotten");
    assert!(map.named("Orders.Core.Tax").is_none());
}

#[test]
fn a_glob_takes_any_case_and_stars() {
    assert!(glob("*.Api", "orders.api") && glob("Orders.*", "Orders.Core") && glob("*", "x"));
    assert!(glob("Orders.*.Tests", "Orders.Core.Tests") && !glob("*.Api", "Orders.Core"));
}

#[test]
fn a_cycle_is_parts_that_all_reach_each_other() {
    let part = |name: &str| Part {
        id: name.into(),
        name: name.into(),
        kind: Kind::Library,
        stack: Stack::Node,
        path: PathBuf::new(),
        inside: None,
        app: None,
        framework: None,
        files: 0,
        packages: 0,
        modules: 0,
        tags: Vec::new(),
    };
    let link = |from, to| Link { from, to, lazy: false, implicit: false };
    let map = CodeMap {
        parts: ["a", "b", "c", "d"].map(part).to_vec(),
        links: vec![link(0, 1), link(1, 2), link(2, 0), link(2, 3)],
        ..CodeMap::default()
    };
    let (parts, links) = map.level(None);
    let mut cycles = map.cycles(&parts, &links);
    cycles.iter_mut().for_each(|cycle| cycle.sort_unstable());
    assert_eq!(cycles, [vec![0, 1, 2]]);
}
