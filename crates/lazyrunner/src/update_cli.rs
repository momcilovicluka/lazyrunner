//! `lazyrunner update` and `lazyrunner version`, as the PowerShell version prints them, and the hidden
//! `__changelog --after <version>` that an updated binary runs for the one that installed it (whose own changelog
//! stops at its own version).

use std::io::{self, IsTerminal, Write};

use anyhow::{Context, Result};
use lazyrunner_core::changelog::{self, Line};
use lazyrunner_core::config::ConfigFile;
use lazyrunner_core::output::Hue;
use lazyrunner_core::paths::Paths;
use lazyrunner_core::update::{self, Progress, Status};
use ratatui::crossterm::queue;
use ratatui::crossterm::style::{Color, Print, ResetColor, SetForegroundColor};

use crate::profile_cli::install_dir;
use crate::start;

/// A line in colour (when stdout is a terminal and NO_COLOR isn't set).
fn say(hue: Hue, text: &str) {
    let colour = io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none_or(|value| value.is_empty());
    let mut out = io::stdout().lock();
    let _ = if colour {
        queue!(out, SetForegroundColor(color_of(hue)), Print(text), ResetColor, Print("\n"))
    } else {
        writeln!(out, "{text}")
    };
    let _ = out.flush();
}

fn color_of(hue: Hue) -> Color {
    match hue {
        Hue::Gray => Color::Grey,
        Hue::DarkGray => Color::DarkGrey,
        Hue::Cyan => Color::Cyan,
        Hue::DarkCyan => Color::DarkCyan,
        Hue::Green => Color::Green,
        Hue::DarkGreen => Color::DarkGreen,
        Hue::Yellow => Color::Yellow,
        Hue::DarkYellow => Color::DarkYellow,
        Hue::Red => Color::Red,
        Hue::Magenta => Color::Magenta,
        Hue::White => Color::White,
    }
}

fn say_lines(lines: &[Line]) {
    for line in lines {
        say(line.hue, &line.text);
    }
}

/// `lazyrunner update`: checks (even with auto-update off) and installs a newer version.
pub fn update() -> Result<()> {
    let paths = Paths::from_env(&install_dir());
    let config = ConfigFile::new(&paths.config_file).load(None);
    let version = changelog::VERSION;
    if update::source(&config).is_none() {
        let file = paths.config_file.file_name().map_or_else(String::new, |name| name.to_string_lossy().into_owned());
        say(Hue::DarkGray, &format!("lazyrunner v{version} - no release repo configured (updateRepoUrl in {file})."));
        return Ok(());
    }
    // Before the update: afterwards this path holds the new binary.
    let exe = update::exe().context("can't find lazyrunner's own path")?;
    say(Hue::Cyan, &format!("lazyrunner v{version}"));
    let mut last_step = String::new();
    let mut report = |line: &str| {
        let step = line.split(" ▕").next().unwrap_or(line).split(" · ").next().unwrap_or(line).to_string();
        if step != last_step {
            let dots = if step == "downloading" { "…" } else { "" };
            say(Hue::DarkGray, &format!("  {step}{dots}"));
            last_step = step;
        }
    };
    let outcome = update::update(&config, &paths.state_dir, true, &mut Progress::new(&mut report));
    let hue = match outcome.status {
        Status::Ok => Hue::Green,
        Status::Warn => Hue::Yellow,
        Status::Skip => Hue::DarkGray,
    };
    say(hue, &outcome.detail);
    if outcome.updated_to.is_none() {
        return Ok(());
    }
    // What's new is in the new binary's changelog.
    let _ = std::process::Command::new(&exe).args(["__changelog", "--after", version]).status();
    if start::is_open(&paths) {
        say(Hue::Yellow, "lazyrunner is open - , then p there reopens it on the new version.");
    }
    Ok(())
}

/// `lazyrunner version`: the version, where updates come from, and the newest changelog entry.
pub fn version() -> Result<()> {
    let paths = Paths::from_env(&install_dir());
    let config = ConfigFile::new(&paths.config_file).load(None);
    say(Hue::Cyan, &format!("lazyrunner v{} ({})", changelog::VERSION, update::TARGET));
    let on = lazyrunner_core::json::truthy(lazyrunner_core::json::field(&config, "autoUpdate"));
    let from = match update::source(&config) {
        Some(source) => format!("updates from {}, auto-update {}", source.url, if on { "on" } else { "off" }),
        None => "no release repo configured".to_string(),
    };
    say(Hue::DarkGray, &from);
    let newest: Vec<_> = changelog::entries(changelog::CHANGELOG, None, None).into_iter().take(1).collect();
    say_lines(&changelog::format(&newest));
    Ok(())
}

/// What changed after `after`, up to this version.
pub fn changelog_after(after: &str) -> Result<()> {
    say_lines(&changelog::format(&changelog::entries(changelog::CHANGELOG, Some(after), Some(changelog::VERSION))));
    Ok(())
}
