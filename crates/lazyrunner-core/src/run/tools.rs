//! The commands lazyrunner runs (an app's server, a build, npm ci) and the tools behind them: finding a program on
//! PATH, npm's launcher, and git.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Result, bail};

use super::child::Spec;
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
            let args = ["run", "--no-build", "--launch-profile", launch_profile].map(OsString::from).to_vec();
            Ok(ServerCommand {
                spec: Spec { program: find_on_path("dotnet").unwrap_or_else(|| "dotnet".into()), args, dir },
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
                spec: Spec { program, args, dir: folder },
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
    Spec { program: find_on_path("dotnet").unwrap_or_else(|| "dotnet".into()), args, dir }
}

/// `npm ci` in an npm app's folder.
pub fn npm_ci_command(dir: &Path) -> Result<Spec> {
    let (program, mut args) = npm_launcher()?;
    args.push("ci".into());
    Ok(Spec { program, args, dir: dir.to_path_buf() })
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
    fn a_dotnet_app_runs_its_launch_profile_without_building() {
        let app = App {
            key: "orders".into(),
            name: "Orders".into(),
            icon: None,
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
        assert_eq!(command.spec.args, ["run", "--no-build", "--launch-profile", "Orders"].map(OsString::from));
    }
}
