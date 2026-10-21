//! lazyrunner, the Rust version: `lazyrunner [start]` runs the project here, `stop` / `restart` tell an open one,
//! `update` / `version` keep it current, and `profile check|init` look after project profiles.

mod profile_cli;
mod start;
mod update_cli;

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use lazyrunner_core::ipc;

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
        /// Something the lazyrunner that opened this one had to say: shown first in the log.
        #[arg(long = "note")]
        notes: Vec<String>,
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
    /// Sends a control request to a running lazyrunner and prints the answer (the tests use it).
    #[command(hide = true)]
    Ctl { name: String, request: String },
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
    let cli = Cli::parse_from(profile_cli::powershell_flags(std::env::args_os()));
    match cli.command {
        Some(Command::Profile { action, path, json, save }) => {
            std::process::exit(profile_cli::run(action.as_deref(), path.as_deref(), json, save))
        }
        Some(Command::Ctl { name, request }) => {
            let body = serde_json::from_str(&request).context("the request must be JSON")?;
            let answer = runtime()?.block_on(ipc::request(&name, &body)).context("no lazyrunner answered")?;
            println!("{answer}");
            Ok(())
        }
        Some(Command::Start) => start::start(true),
        None => start::start(false),
        Some(Command::Update) => update_cli::update(),
        Some(Command::Version) => update_cli::version(),
        Some(Command::Changelog { after }) => update_cli::changelog_after(&after),
        Some(Command::Stop) => start::send("stop"),
        Some(Command::Restart) => start::send("restart"),
        Some(Command::Run { root, headless, ipc, start, handover, handover_handles, notes }) => {
            let size = headless.as_deref().map(parse_size).transpose()?;
            start::run_root(&root, size, ipc, start, handover.map(|file| (file, handover_handles)), notes)
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
    }

    #[test]
    fn the_command_line_is_consistent() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }
}
