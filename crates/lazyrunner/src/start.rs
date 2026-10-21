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
use lazyrunner_core::run::controller::Passed;
use lazyrunner_ui::project_run::{Finished, ProjectOptions, ProjectRun, run_project};
use lazyrunner_ui::run::Surface;
use lazyrunner_ui::screen::Exit;
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

/// Asks the open lazyrunner to do `action` (on `root`: another worktree switches, another project reopens);
/// false when none is open.
fn send_to_open(paths: &Paths, action: &str, root: Option<&Path>) -> bool {
    let Ok(runtime) = runtime() else { return false };
    let mut request = json!({ "action": action });
    if let Some(root) = root {
        request["root"] = json!(root.display().to_string());
    }
    runtime.block_on(ipc::request(&control_name(paths), &request)).is_ok_and(|answer| answer["ok"] == true)
}

/// Whether a lazyrunner is open on this machine (it answers on its control channel).
pub fn is_open(paths: &Paths) -> bool {
    let Ok(runtime) = runtime() else { return false };
    runtime
        .block_on(ipc::request(&control_name(paths), &json!({ "state": true })))
        .is_ok_and(|answer| answer["ok"] == true)
}

/// `lazyrunner stop` and `lazyrunner restart`.
pub fn send(action: &str) -> Result<()> {
    let paths = Paths::from_env(&install_dir());
    if send_to_open(&paths, action, None) {
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
    if !confirm_trust(&project, &paths)? {
        std::process::exit(1);
    }
    if send_to_open(&paths, "start", project.root.as_deref()) {
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

/// The trust question on the terminal (answered already when the profile is trusted as it is).
fn confirm_trust(project: &Project, paths: &Paths) -> Result<bool> {
    let mut ask = |prompt: &str| -> String {
        print!("{prompt} ");
        let _ = io::stdout().flush();
        let mut answer = String::new();
        let _ = io::stdin().lock().read_line(&mut answer);
        answer
    };
    Ok(trust::confirm(project, &paths.trust_store(), &mut io::stdout(), &mut ask)?)
}

/// The project lazyrunner reopens on (`interactive`: the trust question may be asked; else it has to be trusted).
fn reopen_project(root: &Path, paths: &Paths, interactive: bool) -> Result<Option<Project>> {
    let found = profile::find_project_profile(root, &paths.profiles)?
        .ok_or_else(|| anyhow!("lazyrunner: {} is not a project (no profile matches it).", root.display()))?;
    let project = profile::resolve(&found, &paths.profiles)?;
    let trusted =
        if interactive { confirm_trust(&project, paths)? } else { trust::is_trusted(&project, &paths.trust_store()) };
    Ok(trusted.then_some(project))
}

/// The hidden `lazyrunner run --root <dir>` the tests use: no trust question (an untrusted repo profile is an
/// error), optionally headless.
pub fn run_root(
    root: &Path,
    headless: Option<(u16, u16)>,
    ipc: Option<String>,
    start_now: bool,
    handover: Option<(PathBuf, Vec<u64>)>,
    notes: Vec<String>,
) -> Result<()> {
    let paths = Paths::from_env(&install_dir());
    let found = profile::find_project_profile(root, &paths.profiles)?
        .ok_or_else(|| anyhow!("lazyrunner: {} is not a project (no profile matches it).", root.display()))?;
    let project = profile::resolve(&found, &paths.profiles)?;
    if !trust::is_trusted(&project, &paths.trust_store()) {
        bail!("lazyrunner: {} isn't trusted yet - run lazyrunner in that repo to review it.", project.path.display());
    }
    run_with(project, paths, headless, ipc, start_now, handover, notes)
}

fn run(project: Project, paths: Paths, ipc: Option<String>, start_now: bool) -> Result<()> {
    run_with(project, paths, None, ipc, start_now, None, Vec::new())
}

/// Runs the project, and again each time it asks to be reopened (another project, or its profile reloaded).
fn run_with(
    project: Project,
    paths: Paths,
    headless: Option<(u16, u16)>,
    ipc: Option<String>,
    start_now: bool,
    handover: Option<(PathBuf, Vec<u64>)>,
    notes: Vec<String>,
) -> Result<()> {
    let (mut project, mut start_now, mut notes, mut handover) = (project, start_now, notes, handover);
    let binary = Binary::now();
    loop {
        let Finished { exit, handover: passed, notes: carried } =
            run_once(project.clone(), paths.clone(), headless, ipc.clone(), start_now, notes, handover.take())?;
        match exit {
            Exit::Quit => return Ok(()),
            Exit::Reopen { root, start } => {
                start_now = start;
                notes = carried;
                if let Some(passed) = passed {
                    // The next lazyrunner (the new binary after an update, or this one again) takes the running
                    // apps over. Only comes back when it couldn't be started: the apps stopped, so they start again.
                    let error = binary.run_next(&root, start, Some(passed), &notes, headless, ipc.as_deref());
                    notes.push(format!("✗ Couldn't start the next lazyrunner ({error:#}) - the apps start again."));
                    start_now = true;
                } else if binary.replaced() {
                    // Only comes back when the new binary couldn't be started: this one carries on.
                    let error = binary.run_next(&root, start, None, &notes, headless, ipc.as_deref());
                    notes.push(format!("✗ Couldn't open the new lazyrunner ({error:#}) - this one carries on."));
                }
                match reopen_project(&root, &paths, headless.is_none()) {
                    Ok(Some(next)) => project = next,
                    Ok(None) => notes.push(format!("{} isn't trusted - kept {}.", root.display(), project.name)),
                    Err(error) => {
                        notes.push(format!("✗ Couldn't reopen {}: {error:#} - kept {}.", root.display(), project.name))
                    }
                }
            }
        }
    }
}

/// This lazyrunner's binary as it started, to tell when an update replaced it (here, or by `lazyrunner update`).
struct Binary {
    path: Option<PathBuf>,
    modified: Option<std::time::SystemTime>,
}

impl Binary {
    fn now() -> Self {
        let path = lazyrunner_core::update::exe();
        let modified = path.as_deref().and_then(|path| std::fs::metadata(path).and_then(|meta| meta.modified()).ok());
        Self { path, modified }
    }

    fn replaced(&self) -> bool {
        let now = self.path.as_deref().and_then(|path| std::fs::metadata(path).and_then(|meta| meta.modified()).ok());
        lazyrunner_core::update::installed().is_some() || (now.is_some() && now != self.modified)
    }

    /// Opens `root` in the next lazyrunner - the new binary after an update, or this one taking the running apps
    /// over - in this terminal (on Unix in this very process; on Windows a process that inherits the handed-over
    /// apps' handles, and this one exits with it). Returns only when it couldn't be started.
    fn run_next(
        &self,
        root: &Path,
        start: bool,
        handover: Option<Passed>,
        notes: &[String],
        headless: Option<(u16, u16)>,
        ipc: Option<&str>,
    ) -> anyhow::Error {
        let Some(path) = self.path.as_deref() else { return anyhow!("can't find lazyrunner's own path") };
        let mut command = std::process::Command::new(path);
        command.arg("run").arg("--root").arg(root);
        if start {
            command.arg("--start");
        }
        for note in notes {
            command.arg("--note").arg(note);
        }
        if let Some(passed) = &handover {
            command.arg("--handover").arg(&passed.file);
            let values: Vec<String> = passed.handles.iter().map(u64::to_string).collect();
            command.arg("--handover-handles").arg(values.join(","));
        }
        if let Some((width, height)) = headless {
            command.arg("--headless").arg(format!("{width}x{height}"));
        }
        if let Some(ipc) = ipc {
            command.arg("--ipc").arg(ipc);
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            drop(handover);
            command.exec().into()
        }
        #[cfg(not(unix))]
        {
            match command.spawn() {
                Ok(mut next) => {
                    drop(handover); // it inherited its own copies of the apps' handles; these are this process's
                    match next.wait() {
                        Ok(status) => std::process::exit(status.code().unwrap_or(1)),
                        Err(error) => error.into(),
                    }
                }
                Err(error) => {
                    if let Some(passed) = &handover {
                        let _ = std::fs::remove_file(&passed.file); // nobody will read it
                    }
                    error.into()
                }
            }
        }
    }
}

fn run_once(
    project: Project,
    paths: Paths,
    headless: Option<(u16, u16)>,
    ipc: Option<String>,
    start_now: bool,
    notes: Vec<String>,
    handover: Option<(PathBuf, Vec<u64>)>,
) -> Result<Finished> {
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
        notes,
        handover_handles: handover.as_ref().map(|(_, handles)| handles.clone()).unwrap_or_default(),
        handover: handover.map(|(file, _)| file),
    };
    let run = ProjectRun { project, root, paths, guard };
    let runtime = runtime()?;
    let result = match headless {
        Some((width, height)) => {
            let mut terminal = Terminal::new(TestBackend::new(width, height))?;
            lazyrunner_ui::panics::keep_off_screen();
            runtime.block_on(run_project(&mut terminal as &mut dyn Surface, run, options))
        }
        None => {
            let mut terminal = ratatui::try_init().context("this needs a terminal")?;
            lazyrunner_ui::panics::keep_off_screen(); // after ratatui's hook, which it keeps for this thread
            let result = runtime.block_on(run_project(&mut terminal as &mut dyn Surface, run, options));
            ratatui::restore();
            result
        }
    };
    runtime.shutdown_timeout(std::time::Duration::from_millis(500));
    result
}
