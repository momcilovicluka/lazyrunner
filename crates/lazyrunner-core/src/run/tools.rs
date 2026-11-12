//! The commands lazyrunner runs (an app's server, a build, npm ci) and the tools behind them: finding a program on
//! PATH, npm's launcher, and git.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Result, anyhow, bail};

use super::child::Spec;
use super::suites::{Runner, dotnet_filter, name_pattern};
use crate::apptypes::{self, Mode};
use crate::profile::{
    App, AppKind, CWD_PLACEHOLDER, Command as AppCommand, HOME_PREFIX, PORT_PLACEHOLDER, Task, join, native,
};

/// What a program's file name may end with: on Windows its extensions (.exe, .cmd, ...), elsewhere nothing.
fn executable_extensions() -> Vec<String> {
    if cfg!(windows) {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".EXE;.CMD;.BAT;.COM".into())
            .split(';')
            .map(str::to_lowercase)
            .collect()
    } else {
        vec![String::new()]
    }
}

/// A program on PATH (with Windows' extensions: .exe, .cmd, ...).
pub fn find_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let extensions = executable_extensions();
    std::env::split_paths(&path).find_map(|dir| {
        extensions.iter().map(|extension| dir.join(format!("{name}{extension}"))).find(|candidate| candidate.is_file())
    })
}

/// A file as named, or with one of Windows' extensions - on Windows those first: a name without an extension there is
/// a shell script beside its `.cmd` (`mvnw` and `mvnw.cmd`), which Windows can't run.
fn executable(path: &Path) -> Option<PathBuf> {
    let mut extended = executable_extensions().into_iter().map(|extension| {
        let mut file = path.as_os_str().to_os_string();
        file.push(extension);
        PathBuf::from(file)
    });
    let as_named = || (!cfg!(windows) || path.extension().is_some()) && path.is_file();
    extended.find(|candidate| candidate.is_file()).or_else(|| as_named().then(|| path.to_path_buf()))
}

/// A command's program: a path (from the command's folder) as written or with Windows' extensions; else in one of its
/// `path` folders, else on PATH.
pub fn program_path(dir: &Path, folders: &[PathBuf], name: &str) -> Option<PathBuf> {
    if !name.contains(['/', '\\']) && !Path::new(name).is_absolute() {
        return folders.iter().find_map(|folder| executable(&folder.join(name))).or_else(|| find_on_path(name));
    }
    executable(&dir.join(name))
}

/// The user's home folder.
fn home() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from)
}

/// A command app's folder, and the folders of its `path` that are there (`~/` ones from the home folder).
pub fn command_folders(root: &Path, command: &AppCommand) -> (PathBuf, Vec<PathBuf>) {
    let dir = root.join(&command.cwd);
    let place = |folder: &String| match folder.strip_prefix(HOME_PREFIX) {
        Some(rest) => home().map(|home| home.join(native(rest))),
        None => Some(dir.join(native(folder))),
    };
    let folders = command.path.iter().filter_map(place).filter(|folder| folder.is_dir()).collect();
    (dir, folders)
}

/// A command app's own `path` folders (not the home folder's): what its install or setup makes.
pub fn own_folders(command: &AppCommand) -> Vec<&String> {
    command.path.iter().filter(|folder| !folder.starts_with(HOME_PREFIX)).collect()
}

/// Whether none of a command app's own `path` folders is there yet (a `.venv` to make).
pub fn lacks_own_folders(root: &Path, command: &AppCommand) -> bool {
    let own = own_folders(command);
    let dir = root.join(&command.cwd);
    !own.is_empty() && !own.iter().any(|folder| dir.join(native(folder)).is_dir())
}

/// A command to start: its program found in `dir` or its `folders`, or on PATH (a PowerShell script run by
/// PowerShell); `{cwd}` in it and in `env` the folder, `{port}` the port; the folders first on PATH.
fn spec_in(
    dir: PathBuf,
    folders: &[PathBuf],
    port: Option<u16>,
    env: &[(String, String)],
    words: &[String],
) -> Result<Spec> {
    let port = port.map(|port| port.to_string());
    let folder = dir.display().to_string();
    let fill = |word: &str| {
        let word = word.replace(CWD_PLACEHOLDER, &folder);
        port.as_ref().map_or_else(|| word.clone(), |port| word.replace(PORT_PLACEHOLDER, port))
    };
    let Some((first, rest)) = words.split_first() else { bail!("there is no command to run") };
    let found = program_path(&dir, folders, first)
        .ok_or_else(|| anyhow!("{first} was not found on PATH or in {}", dir.display()))?;
    let mut args: Vec<OsString> = rest.iter().map(|word| fill(word).into()).collect();
    let script = found.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("ps1"));
    let program = if script {
        let mut before: Vec<OsString> =
            ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"].map(OsString::from).to_vec();
        before.push(found.into_os_string());
        args.splice(0..0, before);
        find_on_path("pwsh").or_else(|| find_on_path("powershell")).unwrap_or_else(|| "powershell".into())
    } else {
        found
    };
    let mut vars: Vec<(OsString, OsString)> =
        env.iter().map(|(name, value)| (name.into(), fill(value).into())).collect();
    let has_path = env.iter().any(|(name, _)| name.eq_ignore_ascii_case("PATH"));
    if !folders.is_empty() && !has_path {
        let current = std::env::var_os("PATH").unwrap_or_default();
        let all = folders.iter().cloned().chain(std::env::split_paths(&current));
        if let Ok(joined) = std::env::join_paths(all) {
            vars.push(("PATH".into(), joined));
        }
    }
    Ok(Spec { program, args, dir, env: vars })
}

/// A command app's command (its run, build or install) to start, in its folder with its port, env and `path`.
pub fn command_spec(root: &Path, app: &App, words: &[String]) -> Result<Spec> {
    let Some(command) = app.command() else { bail!("{} isn't a command app", app.name) };
    let (dir, folders) = command_folders(root, command);
    spec_in(dir, &folders, apptypes::command_port(command), &app.env, words)
}

/// A task's command to start, in its folder with its env.
pub fn task_spec(root: &Path, task: &Task) -> Result<Spec> {
    spec_in(root.join(&task.cwd), &[], None, &task.env, &task.run)
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

    /// `dotnet ef` with these arguments, in the repo.
    fn ef(&self, root: &Path, args: Vec<OsString>) -> Spec {
        Spec { program: dotnet(), args, dir: root.to_path_buf(), env: Vec::new() }
    }

    /// `npm install`: packages added and updated to match package.json (the lock file may change).
    fn npm_install(&self, dir: &Path) -> Result<Spec> {
        npm_install_command(dir)
    }

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
        AppKind::Dotnet { project, launch_profile, args: extra, .. } => {
            let dir = root.join(project).parent().map_or_else(|| root.to_path_buf(), Path::to_path_buf);
            // After --: ASP.NET logs each request it finished, for the requests view (H); then the profile's own.
            let mut args = [
                "run",
                "--no-build",
                "--launch-profile",
                launch_profile,
                "--",
                super::requests::LOG_REQUESTS,
                super::requests::LOG_SCOPES,
                super::calls::LOG_CALLS,
            ]
            .map(OsString::from)
            .to_vec();
            args.extend(extra.iter().map(OsString::from));
            let shown: String = extra.iter().map(|arg| format!(" {arg}")).collect();
            Ok(ServerCommand {
                spec: Spec {
                    program: find_on_path("dotnet").unwrap_or_else(|| "dotnet".into()),
                    args,
                    dir,
                    env: env(app),
                },
                label: format!(
                    "dotnet run --launch-profile {launch_profile}{}",
                    if shown.is_empty() { String::new() } else { format!(" --{shown}") }
                ),
                script: None,
            })
        }
        AppKind::Command(command) => {
            let spec = command_spec(root, app, &command.run)?;
            Ok(ServerCommand { spec, label: command.run.join(" "), script: None })
        }
        AppKind::Npm { dir, .. } => {
            let Some(mode) = apptypes::mode(root, app, active) else {
                bail!("{} on this branch", apptypes::no_mode_reason(root, app, active));
            };
            let folder = root.join(dir);
            if !has_packages(&folder) {
                bail!("{} is missing - run 'npm install' in {} first", join(dir, "node_modules"), folder.display());
            }
            let (program, mut args) = npm_launcher()?;
            args.extend(["run".into(), mode.script.clone().into(), "--verbose".into()]);
            Ok(ServerCommand {
                spec: Spec { program, args, dir: folder, env: env(app) },
                label: format!("npm run {} --verbose ({})", mode.script, mode.label),
                script: Some(mode.script),
            })
        }
    }
}

/// The variables the profile gives the app (`"env"`).
fn env(app: &App) -> Vec<(OsString, OsString)> {
    app.env.iter().map(|(name, value)| (name.into(), value.into())).collect()
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

pub(crate) fn dotnet() -> PathBuf {
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

/// Whether an npm app's packages are there: its node_modules, or nothing to install - npm ci makes no node_modules
/// for a package.json without dependencies (a script that needs only Node).
pub fn has_packages(folder: &Path) -> bool {
    folder.join("node_modules").exists() || !wants_packages(folder)
}

/// A package.json that lists packages (or can't be read, so it may).
fn wants_packages(folder: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(folder.join("package.json")) else { return true };
    let Ok(package) = serde_json::from_str::<serde_json::Value>(&text) else { return true };
    ["dependencies", "devDependencies", "optionalDependencies", "peerDependencies"]
        .iter()
        .any(|kind| package[kind].as_object().is_some_and(|packages| !packages.is_empty()))
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

/// `npm install` in an npm app's folder.
pub fn npm_install_command(dir: &Path) -> Result<Spec> {
    let (program, mut args) = npm_launcher()?;
    args.push("install".into());
    Ok(Spec { program, args, dir: dir.to_path_buf(), env: Vec::new() })
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

/// Whether a program with this name (`lazygit`, its `.exe` or not) runs with one of `dirs` as its working folder.
/// Processes whose folder can't be read (another user's, elevated) don't count.
pub fn running_in(name: &str, dirs: &[&Path]) -> bool {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
    let stem = |text: &str| {
        let lower = text.to_lowercase();
        lower.strip_suffix(".exe").map_or(lower.clone(), str::to_string)
    };
    // Windows' folders differ in case and a trailing separator; `/` and `\` are the same.
    let folder = |path: &Path| {
        let text = path.to_string_lossy().replace('\\', "/");
        let text = text.trim_end_matches('/');
        if cfg!(windows) { text.to_lowercase() } else { text.to_string() }
    };
    let (name, dirs): (String, Vec<String>) = (stem(name), dirs.iter().map(|dir| folder(dir)).collect());
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing().with_cwd(UpdateKind::OnlyIfNotSet),
    );
    system.processes().values().any(|process| {
        stem(&process.name().to_string_lossy()) == name && process.cwd().is_some_and(|cwd| dirs.contains(&folder(cwd)))
    })
}

#[cfg(test)]
mod home_tests {
    use super::*;

    #[test]
    fn a_wrapper_is_its_cmd_on_windows_and_itself_elsewhere() {
        let temp = crate::testing::TempDir::new("wrapper");
        crate::testing::write(temp.join("mvnw"), "#!/bin/sh");
        crate::testing::write(temp.join("mvnw.cmd"), "@echo off");
        let found = program_path(temp.path(), &[], "./mvnw").expect("the wrapper");
        let name = found.file_name().map(|name| name.to_string_lossy().to_lowercase()).unwrap_or_default();
        assert_eq!(name, if cfg!(windows) { "mvnw.cmd" } else { "mvnw" });
    }

    #[test]
    fn a_path_folder_from_home_is_looked_for_there_and_isnt_one_an_install_makes() {
        let temp = crate::testing::TempDir::new("path-home");
        let command = AppCommand {
            run: vec!["x".into()],
            cwd: String::new(),
            port: None,
            build: Vec::new(),
            install: Vec::new(),
            setup: Vec::new(),
            install_from: Vec::new(),
            ready: crate::profile::Ready::Started,
            watch: Vec::new(),
            errors: Vec::new(),
            warnings: Vec::new(),
            frames: None,
            preset: None,
            path: vec!["~/.".into(), ".venv/Scripts".into()],
            tools: Vec::new(),
            compose: None,
        };
        let (_, folders) = command_folders(temp.path(), &command);
        assert_eq!(folders, [home().unwrap().join(".")], "the home folder's, the .venv isn't made yet");
        assert_eq!(own_folders(&command), [".venv/Scripts"]);
        assert!(lacks_own_folders(temp.path(), &command));
        std::fs::create_dir_all(temp.join(".venv/Scripts")).unwrap();
        assert!(!lacks_own_folders(temp.path(), &command));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{TempDir, write};

    fn command_app(run: &[&str]) -> App {
        let command = crate::profile::Command {
            run: run.iter().map(|word| word.to_string()).collect(),
            cwd: "svc".into(),
            port: Some(8123),
            build: Vec::new(),
            install: Vec::new(),
            ready: crate::profile::Ready::Port(8123),
            watch: Vec::new(),
            errors: Vec::new(),
            warnings: Vec::new(),
            frames: None,
            preset: None,
            path: vec![".venv/Scripts".into()],
            tools: Vec::new(),
            compose: None,
            setup: Vec::new(),
            install_from: Vec::new(),
        };
        App {
            key: "svc".into(),
            name: "Svc".into(),
            icon: None,
            nerd_icon: None,
            kind: AppKind::Command(Box::new(command)),
            tests: Vec::new(),
            migrations: None,
            env: vec![("APP_URL".into(), "http://localhost:{port}".into())],
            depends_on: Vec::new(),
        }
    }

    #[test]
    fn a_command_apps_program_is_found_from_its_folder_and_its_port_fills_in() {
        let temp = TempDir::new("command-spec");
        let tool = if cfg!(windows) { "svc/bin/tool.exe" } else { "svc/bin/tool" };
        write(temp.join(tool), "");
        let app = command_app(&["bin/tool", "--port", "{port}"]);
        let spec = command_spec(temp.path(), &app, &app.command().unwrap().run).unwrap();
        assert_eq!(spec.program, temp.join(tool), "by its path from its folder, with Windows' extension");
        assert_eq!(spec.args, ["--port", "8123"].map(OsString::from));
        assert_eq!(spec.dir, temp.join("svc"));
        assert_eq!(spec.env, [(OsString::from("APP_URL"), OsString::from("http://localhost:8123"))]);
        write(temp.join("svc/start.ps1"), "");
        let script = command_app(&["./start.ps1", "-Fast"]);
        let spec = command_spec(temp.path(), &script, &script.command().unwrap().run).unwrap();
        assert!(spec.args.contains(&OsString::from("-File")) && spec.args.last() == Some(&OsString::from("-Fast")));
        let venv = if cfg!(windows) { "svc/.venv/Scripts/uvicorn.exe" } else { "svc/.venv/Scripts/uvicorn" };
        write(temp.join(venv), "");
        let uvicorn = command_app(&["uvicorn", "--root", "{cwd}"]);
        let spec = command_spec(temp.path(), &uvicorn, &uvicorn.command().unwrap().run).unwrap();
        assert_eq!(spec.program, temp.join(venv), "a path folder first");
        assert_eq!(spec.args[1], temp.join("svc").into_os_string(), "{{cwd}}: its folder");
        let path = spec.env.iter().find(|(name, _)| name == "PATH").map(|(_, value)| value.clone()).unwrap_or_default();
        assert!(std::env::split_paths(&path).next() == Some(temp.join("svc").join(".venv/Scripts")), "{path:?}");
        let missing = command_app(&["no-such-tool-anywhere"]);
        let error = command_spec(temp.path(), &missing, &missing.command().unwrap().run).unwrap_err().to_string();
        assert!(error.contains("no-such-tool-anywhere was not found"), "{error}");
    }

    #[test]
    fn an_npm_app_without_dependencies_needs_no_node_modules() {
        let temp = TempDir::new("has-packages");
        let app = |name: &str, package: &str| {
            let folder = temp.join(name);
            write(folder.join("package.json"), package);
            folder
        };
        let scripts = app("scripts", r#"{ "scripts": { "dev": "node server.mjs" }, "dependencies": {} }"#);
        assert!(has_packages(&scripts), "npm ci makes no node_modules for it");
        let vite = app("vite", r#"{ "devDependencies": { "vite": "^7.0.0" } }"#);
        assert!(!has_packages(&vite), "its packages aren't installed");
        write(vite.join("node_modules").join(".package-lock.json"), "{}");
        assert!(has_packages(&vite));
        assert!(!has_packages(&app("broken", "{ not json")), "a package.json it can't read may list packages");
    }

    #[test]
    fn a_program_runs_in_its_working_folder_and_not_elsewhere() {
        let me = std::env::current_exe().unwrap();
        let name = me.file_name().unwrap().to_string_lossy().to_uppercase();
        let here = std::env::current_dir().unwrap();
        let elsewhere = here.join("no-such-folder");
        assert!(running_in(&name, &[&here]));
        assert!(running_in(&name, &[Path::new(&format!("{}/", here.display()))]), "a trailing separator is the same");
        assert!(running_in(&name, &[&elsewhere, &here]), "any of the folders");
        assert!(!running_in(&name, &[&elsewhere]));
        assert!(!running_in("no-such-program-here", &[&here]));
    }

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
    fn a_dotnet_app_runs_its_launch_profile_without_building_and_its_own_args_follow() {
        let mut app = App {
            key: "orders".into(),
            name: "Orders".into(),
            icon: None,
            nerd_icon: None,
            tests: Vec::new(),
            migrations: None,
            env: Vec::new(),
            depends_on: Vec::new(),
            kind: AppKind::Dotnet {
                project: crate::profile::native("services/Orders/Orders.csproj"),
                launch_profile: "Orders".into(),
                swagger: None,
                args: Vec::new(),
            },
        };
        let root = Path::new("repo");
        let command = server_command(root, &app, &[]).unwrap();
        assert_eq!(command.label, "dotnet run --launch-profile Orders");
        assert_eq!(command.spec.dir, root.join("services").join("Orders"));
        let (logs, scopes) = (crate::run::requests::LOG_REQUESTS, crate::run::requests::LOG_SCOPES);
        assert_eq!(
            command.spec.args,
            ["run", "--no-build", "--launch-profile", "Orders", "--", logs, scopes, crate::run::calls::LOG_CALLS]
                .map(OsString::from)
        );
        if let AppKind::Dotnet { args, .. } = &mut app.kind {
            *args = vec!["--Seq:ApiKey=".into()];
        }
        let command = server_command(root, &app, &[]).unwrap();
        assert_eq!(command.spec.args.last(), Some(&OsString::from("--Seq:ApiKey=")), "after lazyrunner's own");
        assert_eq!(command.label, "dotnet run --launch-profile Orders -- --Seq:ApiKey=");
        app.env = vec![("ASPNETCORE_ENVIRONMENT".into(), "Local".into())];
        let command = server_command(root, &app, &[]).unwrap();
        assert_eq!(command.spec.env, [(OsString::from("ASPNETCORE_ENVIRONMENT"), OsString::from("Local"))]);
    }
}
