//! `lazyrunner profile check|init`: runs the command in lazyrunner-core and prints what it says, in colour on a
//! terminal. init's notes go to stderr when its profile (stdout) goes to a file or a pipe.

use std::ffi::OsString;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};

use lazyrunner_core::paths::Paths;
use lazyrunner_core::profiletools::{self, Note, Outcome, Tone, init};
use ratatui::crossterm::queue;
use ratatui::crossterm::style::{Color, Print, ResetColor, SetForegroundColor};

/// Runs `lazyrunner profile <action>` and returns the exit code.
pub fn run(action: Option<&str>, path: Option<&Path>, json: bool, save: bool) -> i32 {
    let here = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let paths = Paths::from_env(&install_dir());
    let outcome = match action {
        Some("check") => profiletools::check(path, json, &here, &paths.profiles),
        Some("init") => init::init(path, save, &here, &paths.profiles),
        None => profiletools::usage(true),
        Some(_) => profiletools::usage(false),
    };
    print(&outcome, action == Some("init"));
    outcome.code
}

/// The folder lazyrunner is installed in: its bundled profiles are in `projects/` there.
pub fn install_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn print(outcome: &Outcome, notes_aside: bool) {
    let stdout_is_terminal = io::stdout().is_terminal();
    if let Some(data) = &outcome.data {
        let mut out = io::stdout().lock();
        let _ = writeln!(out, "{data}");
    }
    let to_stderr = notes_aside && !stdout_is_terminal;
    let terminal = if to_stderr { io::stderr().is_terminal() } else { stdout_is_terminal };
    let colour = terminal && std::env::var_os("NO_COLOR").is_none_or(|value| value.is_empty());
    let result = if to_stderr {
        write_notes(&mut io::stderr().lock(), &outcome.notes, colour)
    } else {
        write_notes(&mut io::stdout().lock(), &outcome.notes, colour)
    };
    let _ = result;
}

fn write_notes(out: &mut impl Write, notes: &[Note], colour: bool) -> io::Result<()> {
    for note in notes {
        match colour_of(note.tone).filter(|_| colour) {
            Some(color) => queue!(out, SetForegroundColor(color), Print(&note.text), ResetColor, Print("\n"))?,
            None => writeln!(out, "{}", note.text)?,
        }
    }
    out.flush()
}

fn colour_of(tone: Tone) -> Option<Color> {
    match tone {
        Tone::Accent => Some(Color::Cyan),
        Tone::Ok => Some(Color::Green),
        Tone::Error => Some(Color::Red),
        Tone::Warning => Some(Color::Yellow),
        Tone::Dim => Some(Color::DarkGrey),
        Tone::Plain => None,
    }
}

/// PowerShell's `-Json` and `-Save` as `--json` and `--save` after `profile`, so the old commands still work.
pub fn powershell_flags(args: impl Iterator<Item = OsString>) -> Vec<OsString> {
    let mut args: Vec<OsString> = args.collect();
    if args.get(1).is_some_and(|command| command == "profile") {
        for arg in args.iter_mut().skip(2) {
            match arg.to_str().map(str::to_ascii_lowercase).as_deref() {
                Some("-json") => *arg = "--json".into(),
                Some("-save") => *arg = "--save".into(),
                _ => {}
            }
        }
    }
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn powershell_flags_still_work() {
        let args = ["lazyrunner", "profile", "check", "-Json", "x.json"].map(OsString::from);
        assert_eq!(powershell_flags(args.into_iter()), ["lazyrunner", "profile", "check", "--json", "x.json"]);
        let other = ["lazyrunner", "version", "-json"].map(OsString::from);
        assert_eq!(powershell_flags(other.into_iter()), ["lazyrunner", "version", "-json"], "only after profile");
    }

    #[test]
    fn notes_without_colour_are_plain_lines() {
        let notes = [Note { text: "OK".into(), tone: Tone::Ok }, Note { text: "  - x".into(), tone: Tone::Dim }];
        let mut out = Vec::new();
        write_notes(&mut out, &notes, false).unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), "OK\n  - x\n");
    }
}
