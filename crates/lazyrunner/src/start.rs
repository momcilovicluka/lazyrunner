//! `lazyrunner [start]`, `stop`, `restart`, and the hidden `run` (tests) and `__guard`: finding the project, the
//! trust question, talking to a lazyrunner that is already open, and running the project's UI.

use std::io::{self, BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, anyhow, bail};
use lazyrunner_core::config::ConfigFile;
use lazyrunner_core::ipc;
use lazyrunner_core::json;
use lazyrunner_core::paths::Paths;
use lazyrunner_core::profile::{self, Project, REPO_PROFILE_FILE, trust};
use lazyrunner_core::run::child::Guard;
use lazyrunner_ui::project_run::{ProjectOptions, ProjectRun, run_project};
use lazyrunner_ui::run::Surface;
use lazyrunner_ui::theme::ColorMode;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::profile_cli::install_dir;

/// The control channel of the lazyrunner open on this machine (one per state folder).
fn control_name(paths: &Paths) -> String {
    let digest = Sha256::digest(paths.state_dir.display().to_string().to_lowercase().as_bytes());
    let short: String = digest.iter().take(6).map(|byte| format!("{byte:02x}")).collect();
    format!("ctl-{short}")
}

fn runtime() -> Result<tokio::runtime::Runtime> {
    Ok(tokio::runtime::Builder::new_multi_thread().enable_all().build()?)
}

/// Asks the open lazyrunner to do `action`; false when none is open.
fn send_to_open(paths: &Paths, action: &str) -> bool {
    let Ok(runtime) = runtime() else { return false };
    runtime
        .block_on(ipc::request(&control_name(paths), &json!({ "action": action })))
        .is_ok_and(|answer| answer["ok"] == true)
}

/// `lazyrunner stop` and `lazyrunner restart`.
pub fn send(action: &str) -> Result<()> {
    let paths = Paths::from_env(&install_dir());
    if send_to_open(&paths, action) {
        println!("lazyrunner is open in another terminal - sent it '{action}'.");
    } else {
        println!("lazyrunner is not running.");
    }
    Ok(())
}

/// The project this folder belongs to, else the configured repoRoot's.
fn find_project(paths: &Paths) -> Result<Project> {
    let here = std::env::current_dir()?;
    let dirs = &paths.profiles;
    let (_, warnings) = profile::known_profiles(dirs).unwrap_or_default();
    for warning in warnings {
        eprintln!("Skipped a profile: {warning}");
    }
    if let Some(found) = profile::find_project_profile(&here, dirs)? {
        return profile::resolve(&found, dirs);
    }
    let config = ConfigFile::new(&paths.config_file).load(None);
    let fallback = json::text(json::field(&config, "repoRoot"));
    if !fallback.is_empty()
        && Path::new(&fallback).is_dir()
        && let Some(found) = profile::find_project_profile(Path::new(&fallback), dirs)?
    {
        return profile::resolve(&found, dirs);
    }
    let place = if fallback.is_empty() {
        "and no repoRoot is configured".to_string()
    } else {
        format!("and repoRoot '{fallback}' (in {}) isn't one either", paths.config_file.display())
    };
    bail!(
        "lazyrunner: no project here - neither this folder nor a parent has a {REPO_PROFILE_FILE} or the marker file of a known profile, {place}. Add a {REPO_PROFILE_FILE} (projects/README.md; lazyrunner profile init writes a start)."
    )
}

/// `lazyrunner` and `lazyrunner start`: opens the project here (or tells the open lazyrunner to start it).
pub fn start(explicit: bool) -> Result<()> {
    let paths = Paths::from_env(&install_dir());
    let project = find_project(&paths)?;
    let store = paths.trust_store();
    let mut ask = |prompt: &str| -> String {
        print!("{prompt} ");
        let _ = io::stdout().flush();
        let mut answer = String::new();
        let _ = io::stdin().lock().read_line(&mut answer);
        answer
    };
    if !trust::confirm(&project, &store, &mut io::stdout(), &mut ask)? {
        std::process::exit(1);
    }
    if send_to_open(&paths, "start") {
        println!("lazyrunner is open in another terminal - sent it 'start'.");
        return Ok(());
    }
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        bail!("lazyrunner: needs an interactive terminal (input or output is redirected).");
    }
    let config = ConfigFile::new(&paths.config_file).load(Some(&project.id));
    let start_now = explicit || json::truthy(json::field(&config, "autoStartOnOpen"));
    run(project, paths, None, start_now)
}

/// The hidden `lazyrunner run --root <dir>` the tests use: no trust question (an untrusted repo profile is an
/// error), optionally headless.
pub fn run_root(root: &Path, headless: Option<(u16, u16)>, ipc: Option<String>, start_now: bool) -> Result<()> {
    let paths = Paths::from_env(&install_dir());
    let found = profile::find_project_profile(root, &paths.profiles)?
        .ok_or_else(|| anyhow!("lazyrunner: {} is not a project (no profile matches it).", root.display()))?;
    let project = profile::resolve(&found, &paths.profiles)?;
    if !trust::is_trusted(&project, &paths.trust_store()) {
        bail!("lazyrunner: {} isn't trusted yet - run lazyrunner in that repo to review it.", project.path.display());
    }
    run_with(project, paths, headless, ipc, start_now)
}

fn run(project: Project, paths: Paths, ipc: Option<String>, start_now: bool) -> Result<()> {
    run_with(project, paths, None, ipc, start_now)
}

fn run_with(
    project: Project,
    paths: Paths,
    headless: Option<(u16, u16)>,
    ipc: Option<String>,
    start_now: bool,
) -> Result<()> {
    let root = project.root.clone().unwrap_or_else(|| PathBuf::from("."));
    let exe = std::env::current_exe().context("can't find lazyrunner's own path")?;
    let guard = match Guard::start(&exe) {
        Ok(guard) => Some(Arc::new(guard)),
        Err(error) => {
            eprintln!(
                "lazyrunner: the guard didn't start ({error}) - if lazyrunner crashes, its apps may keep running"
            );
            None
        }
    };
    let mut names = vec![control_name(&paths)];
    names.extend(ipc);
    let options = ProjectOptions {
        ipc: names,
        frame_dump: std::env::var_os("LAZYRUNNER_FRAME_DUMP").map(PathBuf::from),
        colors: ColorMode::detect(),
        headless: headless.is_some(),
        start_now,
    };
    let run = ProjectRun { project, root, paths, guard };
    let runtime = runtime()?;
    let result = match headless {
        Some((width, height)) => {
            let mut terminal = Terminal::new(TestBackend::new(width, height))?;
            runtime.block_on(run_project(&mut terminal as &mut dyn Surface, run, options))
        }
        None => {
            let mut terminal = ratatui::try_init().context("this needs a terminal")?;
            let result = runtime.block_on(run_project(&mut terminal as &mut dyn Surface, run, options));
            ratatui::restore();
            result
        }
    };
    runtime.shutdown_timeout(std::time::Duration::from_millis(500));
    result
}
