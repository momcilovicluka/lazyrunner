//! The commands lazyrunner runs (an app's server, a build, npm ci) and the tools behind them: finding a program on
//! PATH, npm's launcher, and git.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Result, bail};

use super::child::Spec;
use super::suites::{Runner, dotnet_filter, name_pattern};
use crate::apptypes::{self, Mode};
use crate::profile::{App, AppKind, join};

/// A program on PATH (with Windows' extensions: .exe, .cmd, ...).
pub fn find_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let extensions: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".EXE;.CMD;.BAT;.COM".into())
            .split(';')
            .map(str::to_lowercase)
            .collect()
    } else {
        vec![String::new()]
    };
    std::env::split_paths(&path).find_map(|dir| {
        extensions.iter().map(|extension| dir.join(format!("{name}{extension}"))).find(|candidate| candidate.is_file())
    })
}

/// How to run npm: on Windows `node npm-cli.js` (npm.cmd adds a cmd.exe layer), else `npm`.
pub fn npm_launcher() -> Result<(PathBuf, Vec<OsString>)> {
    let Some(npm) = find_on_path("npm") else { bail!("npm was not found on PATH - install Node.js") };
    if cfg!(windows) {
        let cli = npm.parent().map(|dir| dir.join("node_modules").join("npm").join("bin").join("npm-cli.js"));
        if let (Some(node), Some(cli)) = (find_on_path("node"), cli.filter(|cli| cli.is_file())) {
            return Ok((node, vec![cli.into_os_string()]));
        }
    }
    Ok((npm, Vec::new()))
}

/// How an app runs: what to start, and what its column says about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerCommand {
    pub spec: Spec,
    pub label: String,
    /// The npm script, to notice when a branch needs another one.
    pub script: Option<String>,
}

/// What the controller runs for an app: its server, its build, its npm ci. `Tools` runs dotnet and npm; tests put
/// quick scripts in their place (`Controller::commands`) to drive builds and restarts without those tools.
pub trait Commands: Send + Sync {
    fn server(&self, root: &Path, app: &App, active: &[App]) -> Result<ServerCommand>;
    fn build(&self, root: &Path, project: &str) -> Spec;
    fn npm_ci(&self, dir: &Path) -> Result<Spec>;

    /// A test suite's run, its results written to `results` (only the tests in `names`, when there are any).
    fn test(&self, runner: &Runner, results: &Path, names: &[String]) -> Result<Spec> {
        test_command(runner, results, names)
    }

    /// The build of the test projects in the MSBuild project `project`.
    fn test_build(&self, project: &Path) -> Spec {
        test_build_command(project)
    }
}

/// The real tools: dotnet and npm.
pub struct Tools;

impl Commands for Tools {
    fn server(&self, root: &Path, app: &App, active: &[App]) -> Result<ServerCommand> {
        server_command(root, app, active)
    }

    fn build(&self, root: &Path, project: &str) -> Spec {
        build_command(root, project)
    }

    fn npm_ci(&self, dir: &Path) -> Result<Spec> {
        npm_ci_command(dir)
    }
}

pub fn server_command(root: &Path, app: &App, active: &[App]) -> Result<ServerCommand> {
    match &app.kind {
        AppKind::Dotnet { project, launch_profile, .. } => {
            let dir = root.join(project).parent().map_or_else(|| root.to_path_buf(), Path::to_path_buf);
            // After --: ASP.NET logs each request it finished, for the requests view (H).
            let args = ["run", "--no-build", "--launch-profile", launch_profile, "--", super::requests::LOG_REQUESTS]
                .map(OsString::from)
                .to_vec();
            Ok(ServerCommand {
                spec: Spec {
                    program: find_on_path("dotnet").unwrap_or_else(|| "dotnet".into()),
                    args,
                    dir,
                    env: Vec::new(),
                },
                label: format!("dotnet run --launch-profile {launch_profile}"),
                script: None,
            })
        }
        AppKind::Npm { dir, .. } => {
            let Some(mode) = apptypes::mode(root, app, active) else {
                bail!("{} on this branch", apptypes::no_mode_reason(root, app, active));
            };
            let folder = root.join(dir);
            if !folder.join("node_modules").exists() {
                bail!("{} is missing - run 'npm install' in {} first", join(dir, "node_modules"), folder.display());
            }
            let (program, mut args) = npm_launcher()?;
            args.extend(["run".into(), mode.script.clone().into(), "--verbose".into()]);
            Ok(ServerCommand {
                spec: Spec { program, args, dir: folder, env: Vec::new() },
                label: format!("npm run {} --verbose ({})", mode.script, mode.label),
                script: Some(mode.script),
            })
        }
    }
}

/// `dotnet build` for an app that builds.
pub fn build_command(root: &Path, project: &str) -> Spec {
    let path = root.join(project);
    let dir = path.parent().map_or_else(|| root.to_path_buf(), Path::to_path_buf);
    let mut args: Vec<OsString> = vec!["build".into(), path.into_os_string()];
    args.extend(["-nologo", "-v", "m", "-tl:off", "-clp:NoSummary"].map(OsString::from));
    Spec { program: find_on_path("dotnet").unwrap_or_else(|| "dotnet".into()), args, dir, env: Vec::new() }
}

/// `npm ci` in an npm app's folder.
pub fn npm_ci_command(dir: &Path) -> Result<Spec> {
    let (program, mut args) = npm_launcher()?;
    args.push("ci".into());
    Ok(Spec { program, args, dir: dir.to_path_buf(), env: Vec::new() })
}

fn dotnet() -> PathBuf {
    find_on_path("dotnet").unwrap_or_else(|| "dotnet".into())
}

/// A test suite's run: `dotnet test --no-build` writing TRX, or jest / vitest (through npm or Nx) writing their
/// JSON; `names` narrows it to those tests.
pub fn test_command(runner: &Runner, results: &Path, names: &[String]) -> Result<Spec> {
    let folder = results.parent().map_or_else(PathBuf::new, Path::to_path_buf);
    let file = results.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let output = format!("--outputFile={}", results.display());
    let pattern = (!names.is_empty()).then(|| name_pattern(names));
    match runner {
        Runner::Dotnet { project } => {
            let mut args: Vec<OsString> = vec!["test".into(), project.into(), "--no-build".into(), "-nologo".into()];
            args.extend(["--logger".into(), format!("trx;LogFileName={file}").into()]);
            args.extend(["--results-directory".into(), folder.into_os_string()]);
            args.extend(["--logger", "console;verbosity=normal"].map(OsString::from));
            if !names.is_empty() {
                args.extend(["--filter".into(), dotnet_filter(names).into()]);
            }
            let dir = project.parent().map_or_else(PathBuf::new, Path::to_path_buf);
            Ok(Spec { program: dotnet(), args, dir, env: Vec::new() })
        }
        Runner::Npm { dir, vitest } => {
            if let Some(problem) = packages_problem(runner) {
                bail!("{problem} - i installs them");
            }
            let (program, mut args) = npm_launcher()?;
            args.extend(["test", "--"].map(OsString::from));
            if *vitest {
                args.extend(["--run", "--reporter=default", "--reporter=json"].map(OsString::from));
                args.push(format!("--outputFile.json={}", results.display()).into());
                args.extend(pattern.into_iter().flat_map(|pattern| ["-t".into(), OsString::from(pattern)]));
            } else {
                args.extend(["--ci".into(), "--json".into(), output.into()]);
                args.extend(pattern.map(|pattern| format!("--testNamePattern={pattern}").into()));
            }
            Ok(Spec { program, args, dir: dir.clone(), env: Vec::new() })
        }
        Runner::Nx { workspace, project } => {
            let Some(nx) = nx_script(workspace) else {
                bail!("{} - i installs them", packages_problem(runner).unwrap_or_default());
            };
            let Some(node) = find_on_path("node") else { bail!("node was not found on PATH - install Node.js") };
            let mut args: Vec<OsString> = vec![nx.into_os_string(), "test".into(), project.into()];
            args.extend(["--skip-nx-cache".into(), "--ci".into(), "--json".into(), output.into()]);
            args.extend(pattern.map(|pattern| format!("--testNamePattern={pattern}").into()));
            // Without its daemon: it would live in this suite's process tree and go when the suite ends, under
            // another suite of the workspace running beside it.
            let env = vec![("NX_DAEMON".into(), "false".into())];
            Ok(Spec { program: node, args, dir: workspace.clone(), env })
        }
    }
}

/// Nx's command line in a workspace: where its package.json's `bin` says (Nx 16 and on: dist/bin/nx.js), else
/// where older ones kept it.
pub fn nx_script(workspace: &Path) -> Option<PathBuf> {
    let package = workspace.join("node_modules").join("nx");
    let manifest = crate::json::read(&package.join("package.json"));
    let bin =
        manifest.as_ref().and_then(|manifest| manifest["bin"].as_str().or_else(|| manifest["bin"]["nx"].as_str()));
    bin.map(|bin| package.join(bin))
        .into_iter()
        .chain([package.join("dist").join("bin").join("nx.js"), package.join("bin").join("nx.js")])
        .find(|script| script.is_file())
}

/// Why a suite can't run for want of packages (npm, Nx), if it can't.
pub fn packages_problem(runner: &Runner) -> Option<String> {
    let folder = runner.npm_dir()?;
    if !folder.join("node_modules").exists() {
        return Some(format!("{} has no node_modules", folder.display()));
    }
    let nx = matches!(runner, Runner::Nx { .. });
    (nx && nx_script(folder).is_none()).then(|| format!("{} has no Nx in node_modules", folder.display()))
}

/// `dotnet build` of the MSBuild project that lists the test projects.
pub fn test_build_command(project: &Path) -> Spec {
    let mut args: Vec<OsString> = vec!["build".into(), project.into()];
    args.extend(["-nologo", "-m", "-v", "m", "-tl:off", "-clp:NoSummary"].map(OsString::from));
    let dir = project.parent().map_or_else(PathBuf::new, Path::to_path_buf);
    Spec { program: dotnet(), args, dir, env: Vec::new() }
}

/// An npm app's mode, for callers that hold only its key.
pub fn mode_of(root: &Path, app: &App, active: &[App]) -> Option<Mode> {
    apptypes::mode(root, app, active)
}

/// `git <args>` in `dir`: its output, or `None` when git fails or isn't there.
pub fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git").arg("-C").arg(dir).args(args).output().ok()?;
    output.status.success().then(|| String::from_utf8_lossy(&output.stdout).trim_end().to_string())
}

/// The repo's git folder (a worktree's own one).
pub fn git_dir(root: &Path) -> Option<PathBuf> {
    git(root, &["rev-parse", "--absolute-git-dir"]).map(|dir| PathBuf::from(crate::profile::native(&dir)))
}

/// The checked-out branch, read from HEAD (cheap enough to do every second): "main", or "1a2b3c4d (detached)".
pub fn branch(git_dir: &Path) -> Option<String> {
    let head = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let head = head.trim();
    Some(match head.strip_prefix("ref: refs/heads/") {
        Some(branch) => branch.to_string(),
        None => format!("{} (detached)", head.chars().take(8).collect::<String>()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{TempDir, write};

    #[test]
    fn the_branch_comes_from_head() {
        let dir = TempDir::new("branch");
        write(dir.join("HEAD"), "ref: refs/heads/feature/x\n");
        assert_eq!(branch(dir.path()).as_deref(), Some("feature/x"));
        write(dir.join("HEAD"), "0123456789abcdef\n");
        assert_eq!(branch(dir.path()).as_deref(), Some("01234567 (detached)"));
    }

    #[test]
    fn nx_is_found_where_its_package_json_says_and_missing_packages_are_named() {
        let dir = TempDir::new("nx-script");
        let workspace = dir.join("spa");
        let runner = Runner::Nx { workspace: workspace.clone(), project: "core".into() };
        assert!(packages_problem(&runner).is_some_and(|why| why.ends_with("has no node_modules")));
        write(workspace.join("node_modules/.package-lock.json"), "{}");
        assert!(packages_problem(&runner).is_some_and(|why| why.ends_with("has no Nx in node_modules")));
        write(workspace.join("node_modules/nx/package.json"), r#"{ "bin": { "nx": "./dist/bin/nx.js" } }"#);
        write(workspace.join("node_modules/nx/dist/bin/nx.js"), "");
        assert_eq!(nx_script(&workspace), Some(workspace.join("node_modules/nx/./dist/bin/nx.js")));
        assert_eq!(packages_problem(&runner), None);
    }

    #[test]
    fn a_dotnet_app_runs_its_launch_profile_without_building() {
        let app = App {
            key: "orders".into(),
            name: "Orders".into(),
            icon: None,
            tests: Vec::new(),
            kind: AppKind::Dotnet {
                project: crate::profile::native("services/Orders/Orders.csproj"),
                launch_profile: "Orders".into(),
                swagger: None,
            },
        };
        let root = Path::new("repo");
        let command = server_command(root, &app, &[]).unwrap();
        assert_eq!(command.label, "dotnet run --launch-profile Orders");
        assert_eq!(command.spec.dir, root.join("services").join("Orders"));
        let logs = crate::run::requests::LOG_REQUESTS;
        assert_eq!(
            command.spec.args,
            ["run", "--no-build", "--launch-profile", "Orders", "--", logs].map(OsString::from)
        );
    }
}
