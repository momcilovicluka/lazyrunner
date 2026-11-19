//! What collecting code coverage needs (#33), for the startup doctor while the `coverage` setting is on: a .NET test
//! project needs coverlet's collector referenced, a vitest suite a coverage provider among its packages; jest (and
//! Nx's jest) collects it itself. Each one missing says what adds it, and the doctor's `f` runs those commands.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::Result;

use super::checks::{Outcome, Status};
use super::child::Spec;
use super::coverage;
use super::suites::{self, Runner};
use crate::profile::App;

/// The package `dotnet test --collect "XPlat Code Coverage"` needs.
pub const COVERLET: &str = "coverlet.collector";
/// vitest's coverage providers; the first is the one the fix adds.
pub const VITEST_PROVIDERS: [&str; 2] = ["@vitest/coverage-v8", "@vitest/coverage-istanbul"];
/// The doctor's row.
pub const CHECK: &str = "Coverage";

/// A suite that can't collect coverage yet, and what it needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Missing {
    pub suite: String,
    pub need: Need,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Need {
    /// A .NET test project without coverlet's collector.
    Coverlet { project: PathBuf },
    /// A vitest suite's folder without a coverage provider.
    VitestProvider { dir: PathBuf },
}

impl Missing {
    /// What it lacks, in words.
    pub fn lacks(&self) -> String {
        match &self.need {
            Need::Coverlet { .. } => format!("no {COVERLET}"),
            Need::VitestProvider { .. } => "no vitest coverage provider".into(),
        }
    }

    /// The command that adds it, as the doctor shows it (paths from the repo's root).
    pub fn command(&self, root: &Path) -> String {
        let shown = |path: &Path| path.strip_prefix(root).unwrap_or(path).display().to_string().replace('\\', "/");
        match &self.need {
            Need::Coverlet { project } => format!("dotnet add {} package {COVERLET}", shown(project)),
            Need::VitestProvider { dir } => format!("npm install -D {} in {}", VITEST_PROVIDERS[0], shown(dir)),
        }
    }

    /// The command that adds it, to run.
    pub fn spec(&self) -> Result<Spec> {
        match &self.need {
            Need::Coverlet { project } => {
                let args = ["add".into(), project.into(), "package".into(), COVERLET.into()];
                let dir = project.parent().map_or_else(PathBuf::new, Path::to_path_buf);
                Ok(Spec { program: super::tools::dotnet(), args: args.to_vec(), dir, env: Vec::new() })
            }
            Need::VitestProvider { dir } => {
                let (program, mut args) = super::tools::npm_launcher()?;
                args.extend(["install", "-D", VITEST_PROVIDERS[0]].map(OsString::from));
                Ok(Spec { program, args, dir: dir.clone(), env: Vec::new() })
            }
        }
    }
}

/// Whether a folder's packages have a vitest coverage provider: in its package.json, or installed.
pub fn has_vitest_provider(dir: &Path) -> bool {
    let package = crate::json::read(&dir.join("package.json"));
    let listed = |name: &str| {
        package.as_ref().is_some_and(|package| {
            ["dependencies", "devDependencies"].iter().any(|deps| package[deps].get(name).is_some())
        })
    };
    VITEST_PROVIDERS.iter().any(|name| listed(name) || dir.join("node_modules").join(name).is_dir())
}

/// The project's suites that can't collect coverage, and how many suites there are to collect it from.
pub fn missing(root: &Path, apps: &[App]) -> (Vec<Missing>, usize) {
    let found = suites::discover(root, apps);
    let tested: Vec<&suites::Suite> = found.iter().filter(|suite| !suite.untested).collect();
    let mut missing: Vec<Missing> = Vec::new();
    for suite in &tested {
        let need = match &suite.runner {
            Runner::Dotnet { project } if !coverage::collects(&suite.runner, suite.coverage.coverlet) => {
                Need::Coverlet { project: project.clone() }
            }
            Runner::Npm { dir, vitest: true } if !has_vitest_provider(dir) => Need::VitestProvider { dir: dir.clone() },
            _ => continue,
        };
        if !missing.iter().any(|known| known.need == need) {
            missing.push(Missing { suite: suite.name.clone(), need });
        }
    }
    (missing, tested.len())
}

/// The doctor's coverage row: every suite can collect it, or which can't and what adds it (`f` runs that).
pub fn report(root: &Path, apps: &[App]) -> Outcome {
    let (missing, suites) = missing(root, apps);
    if suites == 0 {
        return (Status::Skip, "no test suites to collect it from".into());
    }
    if missing.is_empty() {
        let plural = if suites == 1 { "" } else { "s" };
        return (Status::Ok, format!("all {suites} suite{plural} can collect it (jest needs nothing more)"));
    }
    let each: Vec<String> = missing
        .iter()
        .map(|missing| format!("{}: {} - `{}`", missing.suite, missing.lacks(), missing.command(root)))
        .collect();
    (Status::Warn, format!("{} - f adds {}", each.join(" · "), if missing.len() == 1 { "it" } else { "them" }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::AppKind;
    use crate::testing::{TempDir, write};

    const TEST_PROJECT: &str = r#"<Project Sdk="Microsoft.NET.Sdk"><ItemGroup><PackageReference Include="Microsoft.NET.Test.Sdk" Version="17.0.0" /></ItemGroup></Project>"#;

    fn web(dir: &str) -> App {
        App {
            key: "web".into(),
            name: "web".into(),
            icon: None,
            nerd_icon: None,
            tests: Vec::new(),
            migrations: None,
            env: Vec::new(),
            depends_on: Vec::new(),
            health: crate::profile::Health::default(),
            restart_on_crash: false,
            kind: AppKind::Npm { dir: dir.into(), variants: Vec::new() },
        }
    }

    #[test]
    fn a_test_project_without_coverlet_and_a_vitest_folder_without_a_provider_need_theirs() {
        let temp = TempDir::new("coverage-needs");
        let root = temp.path();
        write(root.join("tests/Orders.Tests/Orders.Tests.csproj"), TEST_PROJECT);
        write(root.join("web/package.json"), r#"{ "scripts": { "test": "vitest run" } }"#);
        let (status, detail) = report(root, &[web("web")]);
        assert_eq!(status, Status::Warn, "{detail}");
        assert!(detail.contains("Orders.Tests: no coverlet.collector - `dotnet add tests/Orders.Tests/Orders.Tests.csproj package coverlet.collector`"), "{detail}");
        assert!(
            detail.contains("web: no vitest coverage provider - `npm install -D @vitest/coverage-v8 in web`"),
            "{detail}"
        );
        assert!(detail.ends_with("f adds them"), "{detail}");
        let (missing, _) = missing(root, &[web("web")]);
        let spec = missing[0].spec().unwrap();
        let args: Vec<String> = spec.args.iter().map(|arg| arg.to_string_lossy().into_owned()).collect();
        assert_eq!(args[0], "add");
        assert_eq!(args[2..], ["package", "coverlet.collector"]);
    }

    #[test]
    fn suites_with_what_they_need_and_jest_ones_pass() {
        let temp = TempDir::new("coverage-needs-ok");
        let root = temp.path();
        let with_coverlet =
            TEST_PROJECT.replace("</ItemGroup>", r#"<PackageReference Include="coverlet.collector" /></ItemGroup>"#);
        write(root.join("tests/Orders.Tests/Orders.Tests.csproj"), &with_coverlet);
        write(
            root.join("web/package.json"),
            r#"{ "scripts": { "test": "vitest run" }, "devDependencies": { "@vitest/coverage-istanbul": "^3.0.0" } }"#,
        );
        write(root.join("admin/package.json"), r#"{ "scripts": { "test": "jest" } }"#);
        let mut admin = web("admin");
        admin.key = "admin".into();
        assert_eq!(
            report(root, &[web("web"), admin]),
            (Status::Ok, "all 3 suites can collect it (jest needs nothing more)".into())
        );
        let installed = TempDir::new("coverage-needs-installed");
        write(installed.join("node_modules/@vitest/coverage-v8/package.json"), "{}");
        assert!(has_vitest_provider(installed.path()), "installed, if not listed");
        assert_eq!(report(&root.join("nowhere"), &[]).0, Status::Skip, "no suites: nothing to check");
    }
}
