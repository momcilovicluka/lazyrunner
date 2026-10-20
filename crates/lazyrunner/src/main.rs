//! lazyrunner, the Rust version: `lazyrunner [start]` runs the project here (P2), `stop` / `restart` tell an open
//! one, `profile check|init` (P1), and the look-and-feel spike (P0) with its benchmark mode.

mod profile_cli;
mod start;
mod update_cli;

use std::ffi::OsString;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use lazyrunner_core::emitter::{self, EmitterOptions};
use lazyrunner_core::ipc;
use lazyrunner_core::process::Launch;
use lazyrunner_ui::model::AppSpec;
use lazyrunner_ui::run::{RunOptions, Surface, run};
use lazyrunner_ui::theme::ColorMode;
use ratatui::Terminal;
use ratatui::backend::TestBackend;

const APPS: [(&str, &str); 9] = [
    ("Orders", "◆"),
    ("Billing", "●"),
    ("Users", "▲"),
    ("Cart", "■"),
    ("Search", "★"),
    ("Reports", "✦"),
    ("Gateway", "⬢"),
    ("Mailer", "✉"),
    ("Docs", "✎"),
];
/// Each spike app takes this much longer than the one before it to "compile", so they get ready one by one.
const STAGGER: Duration = Duration::from_millis(350);

#[derive(Parser)]
#[command(name = "lazyrunner", version, about = "Runs your project's apps side by side in one terminal")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Open lazyrunner on the project here and start everything (plain `lazyrunner` starts unless auto-start is off).
    Start,
    /// Stop everything in the open lazyrunner.
    Stop,
    /// Restart everything in the open lazyrunner (rebuilding what changed).
    Restart,
    /// Install a newer lazyrunner if there is one (also when auto-update is off).
    Update,
    /// This lazyrunner's version, where its updates come from, and what's new in it.
    Version,
    /// What changed after a version (the updated binary prints it for the one that installed it).
    #[command(name = "__changelog", hide = true)]
    Changelog {
        #[arg(long)]
        after: String,
    },
    /// Run the project in a folder without asking (the tests use it; the profile must be trusted already).
    #[command(hide = true)]
    Run {
        #[arg(long)]
        root: PathBuf,
        /// Draw into memory at this size (e.g. 160x45) instead of the terminal.
        #[arg(long, value_name = "WIDTHxHEIGHT")]
        headless: Option<String>,
        #[arg(long)]
        ipc: Option<String>,
        /// Start everything right away.
        #[arg(long)]
        start: bool,
        /// Take over the running apps another lazyrunner handed over (the file it wrote).
        #[arg(long)]
        handover: Option<PathBuf>,
        /// The handle values it passed on (closed if the file can't be read).
        #[arg(long, value_delimiter = ',')]
        handover_handles: Vec<u64>,
    },
    /// Kills what lazyrunner started when lazyrunner itself dies (Linux, macOS).
    #[command(name = "__guard", hide = true)]
    Guard,
    /// Check a project profile, or propose one for a checkout (projects/README.md describes profiles).
    Profile {
        /// check: its fields, the schema, and the checkout it is for. init: a .lazyrunner.json for a checkout.
        action: Option<String>,
        /// check: the profile file (default: the one that applies here). init: the checkout (default: this repo).
        path: Option<PathBuf>,
        /// check: print the result as JSON.
        #[arg(long)]
        json: bool,
        /// init: write .lazyrunner.json instead of printing it.
        #[arg(long)]
        save: bool,
    },
    /// The look-and-feel spike: synthetic apps streaming output.
    Spike(SpikeArgs),
    /// Prints synthetic app output; the spike's apps are this.
    #[command(hide = true)]
    Emit {
        #[arg(long, default_value_t = 2000)]
        rate: u32,
        #[arg(long, default_value_t = 1500)]
        ready_after_ms: u64,
        #[arg(long, default_value_t = 1)]
        seed: u64,
        #[arg(long)]
        limit: Option<u64>,
    },
    /// Sends a control request to a running lazyrunner and prints the answer (the tests use it).
    #[command(hide = true)]
    Ctl { name: String, request: String },
}

#[derive(Args, Clone)]
struct SpikeArgs {
    /// How many apps (1-9).
    #[arg(long, default_value_t = 5, value_parser = clap::value_parser!(u8).range(1..=9))]
    apps: u8,
    /// Lines per second each app prints once ready (0: idle apps).
    #[arg(long, default_value_t = 2000)]
    rate: u32,
    /// How long the first app takes to get ready; each next one takes a bit longer.
    #[arg(long, default_value_t = 1500)]
    ready_after_ms: u64,
    /// Run this many seconds, quit, and print the numbers as JSON.
    #[arg(long, value_name = "SECONDS", value_parser = parse_seconds)]
    bench: Option<Duration>,
    /// Also write the benchmark numbers to this file.
    #[arg(long, requires = "bench")]
    report: Option<PathBuf>,
    /// Draw into memory instead of the terminal, at this size (e.g. 160x45); keys come over --ipc.
    #[arg(long, value_name = "WIDTHxHEIGHT")]
    headless: Option<String>,
    /// Skip the logo at startup.
    #[arg(long)]
    no_splash: bool,
    /// Listen for control requests on this channel name.
    #[arg(long, env = "LAZYRUNNER_IPC")]
    ipc: Option<String>,
}

fn main() {
    // The binary as it starts: an update replaces the file at this path, and reopening runs that one.
    let _ = lazyrunner_core::update::exe();
    if let Err(error) = run_cli() {
        eprintln!("{error:#}");
        std::process::exit(1);
    }
}

fn run_cli() -> Result<()> {
    let process_started = Instant::now();
    let cli = Cli::parse_from(profile_cli::powershell_flags(std::env::args_os()));
    match cli.command {
        Some(Command::Profile { action, path, json, save }) => {
            std::process::exit(profile_cli::run(action.as_deref(), path.as_deref(), json, save))
        }
        Some(Command::Emit { rate, ready_after_ms, seed, limit }) => {
            let options = EmitterOptions { rate, ready_after: Duration::from_millis(ready_after_ms), seed, limit };
            Ok(emitter::run(&options)?)
        }
        Some(Command::Ctl { name, request }) => {
            let body = serde_json::from_str(&request).context("the request must be JSON")?;
            let answer = runtime()?.block_on(ipc::request(&name, &body)).context("no lazyrunner answered")?;
            println!("{answer}");
            Ok(())
        }
        Some(Command::Spike(args)) => spike(args, process_started),
        Some(Command::Start) => start::start(true),
        None => start::start(false),
        Some(Command::Update) => update_cli::update(),
        Some(Command::Version) => update_cli::version(),
        Some(Command::Changelog { after }) => update_cli::changelog_after(&after),
        Some(Command::Stop) => start::send("stop"),
        Some(Command::Restart) => start::send("restart"),
        Some(Command::Run { root, headless, ipc, start, handover, handover_handles }) => {
            let size = headless.as_deref().map(parse_size).transpose()?;
            start::run_root(&root, size, ipc, start, handover.map(|file| (file, handover_handles)))
        }
        Some(Command::Guard) => {
            lazyrunner_core::run::child::run_guard();
            Ok(())
        }
    }
}

fn runtime() -> Result<tokio::runtime::Runtime> {
    Ok(tokio::runtime::Builder::new_multi_thread().enable_all().build()?)
}

fn spike(args: SpikeArgs, process_started: Instant) -> Result<()> {
    let exe = std::env::current_exe().context("can't find lazyrunner's own path")?;
    let specs: Vec<AppSpec> = APPS
        .iter()
        .take(usize::from(args.apps))
        .enumerate()
        .map(|(index, (name, icon))| {
            let ready_after = Duration::from_millis(args.ready_after_ms) + STAGGER * index as u32;
            let launch_args: Vec<OsString> = vec![
                "emit".into(),
                "--rate".into(),
                args.rate.to_string().into(),
                "--ready-after-ms".into(),
                ready_after.as_millis().to_string().into(),
                "--seed".into(),
                (index + 1).to_string().into(),
            ];
            AppSpec {
                name: name.to_string(),
                icon: icon.to_string(),
                launch: Launch { program: exe.clone(), args: launch_args, dir: None },
            }
        })
        .collect();
    let options = RunOptions {
        splash: !args.no_splash,
        bench: args.bench,
        ipc: args.ipc.clone(),
        frame_dump: std::env::var_os("LAZYRUNNER_FRAME_DUMP").map(PathBuf::from),
        process_started,
        colors: ColorMode::detect(),
        headless: args.headless.is_some(),
    };
    let runtime = runtime()?;
    let report = match &args.headless {
        Some(size) => {
            let (width, height) = parse_size(size)?;
            let mut terminal = Terminal::new(TestBackend::new(width, height))?;
            runtime.block_on(run(&mut terminal as &mut dyn Surface, specs, options))?
        }
        None => {
            let mut terminal = ratatui::try_init().context("this needs a terminal (or --headless)")?;
            let result = runtime.block_on(run(&mut terminal as &mut dyn Surface, specs, options));
            ratatui::restore();
            result?
        }
    };
    // Don't wait for the emitters' pipes: they were killed with the run.
    runtime.shutdown_timeout(Duration::from_millis(200));
    if args.bench.is_some() {
        let text = serde_json::to_string_pretty(&report)?;
        println!("{text}");
        if let Some(path) = &args.report {
            std::fs::write(path, &text).with_context(|| format!("can't write {}", path.display()))?;
        }
    }
    Ok(())
}

fn parse_seconds(text: &str) -> Result<Duration, String> {
    let seconds: f32 = text.parse().map_err(|_| format!("'{text}' isn't a number of seconds"))?;
    match Duration::try_from_secs_f32(seconds) {
        Ok(length) if !length.is_zero() => Ok(length),
        _ => Err(format!("'{text}': give a positive number of seconds")),
    }
}

fn parse_size(size: &str) -> Result<(u16, u16)> {
    let parse = || -> Option<(u16, u16)> {
        let (width, height) = size.split_once(['x', 'X'])?;
        Some((width.parse().ok()?, height.parse().ok()?))
    };
    match parse() {
        Some((width, height)) if width > 0 && height > 0 => Ok((width, height)),
        _ => bail!("--headless wants WIDTHxHEIGHT, like 160x45 (got '{size}')"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_as_width_by_height() {
        assert_eq!(parse_size("160x45").unwrap(), (160, 45));
        assert!(parse_size("160").is_err());
        assert!(parse_size("0x10").is_err());
        assert_eq!(parse_seconds("2.5").unwrap(), Duration::from_millis(2500));
        assert!(parse_seconds("-1").is_err() && parse_seconds("nan").is_err() && parse_seconds("0").is_err());
    }

    #[test]
    fn the_command_line_is_consistent() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }
}
