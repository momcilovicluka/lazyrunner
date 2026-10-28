//! The About box (`,` then `A`, or `:` about): lazyrunner's version, who makes it - from the repo, by way of
//! contributors.json - and where to go next: the README, the changelog, the releases, the license, a bug report or an
//! idea, the source, an update check (run right there, its outcome under its link), everyone who contributed. Its logo
//! shimmers in rainbow colours over a flowing rule, the title sparkles and the heart beats (all still with reduced
//! motion).

use std::sync::mpsc::{Receiver, TryRecvError, channel};

use lazyrunner_core::changelog;
use lazyrunner_core::contributors;
use lazyrunner_core::output::Hue;
use lazyrunner_core::run::Controller;
use lazyrunner_core::update::{self, DEFAULT_REPO};
use unicode_width::UnicodeWidthStr;

use super::actions::Run;
use super::columns::spinner;
use super::icons::{self, Icon};
use super::mouse::Target;
use super::overlays::{LOGO, boxed};
use super::seg::{Seg, aim, on, pad, seg};
use super::theme::C;
use super::{Overlay, Ui, keys};

const VERSION: &str = changelog::VERSION;
const INNER: usize = 72;
const RAINBOW: [C; 6] = [C::Red, C::Yellow, C::Green, C::Cyan, C::Blue, C::Magenta];
const SPARKLES: [&str; 4] = ["✦", "✧", "⋆", "✧"];
/// How fast the rainbow flows across the logo and its rule.
const RAINBOW_BANDS_PER_SECOND: f64 = 5.0;
const LABEL_CELLS: usize = 22;
/// The names of the contributors, padded so what they do lines up.
const NAME_CELLS: usize = 18;

/// What a link does: opens a page in the browser, runs one of lazyrunner's own actions, or checks for an update
/// with the box kept open.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Does {
    Open(String),
    Run(Run),
    CheckUpdate,
}

/// `u`: the update check, the startup checklist's own, running in the background; what it is doing and how it ended
/// show under its link.
pub struct UpdateCheck {
    results: Receiver<Message>,
    pub line: String,
    /// None while it runs.
    pub status: Option<update::Status>,
}

enum Message {
    Progress(String),
    Done(update::Outcome),
}

struct Link {
    key: &'static str,
    icon: Icon,
    label: &'static str,
    hint: &'static str,
    does: Does,
}

fn link(key: &'static str, icon: Icon, label: &'static str, hint: &'static str, does: Does) -> Link {
    Link { key, icon, label, hint, does }
}

fn links() -> Vec<Link> {
    let page = |path: &str| Does::Open(format!("{DEFAULT_REPO}{path}"));
    // The repository's issue forms, the bug report's version and operating system filled in.
    let os = match std::env::consts::OS {
        "windows" => "Windows",
        "linux" => "Linux",
        "macos" => "macOS",
        _ => "Other",
    };
    let bug = format!("/issues/new?template=bug_report.yml&version={}&os={}", encode(VERSION), encode(os));
    vec![
        link("r", Icon::Readme, "README", "setting it up, and every key", page("#readme")),
        link("c", Icon::Changelog, "What's new", "the changelog, right here", Does::Run(Run::Changelog)),
        link("e", Icon::Releases, "Releases", "every version, on GitHub", page("/releases")),
        link("l", Icon::License, "License", "source-available; free to use, at work too", page("/blob/main/LICENSE")),
        link("b", Icon::Bug, "Report a bug", "the bug form, this version filled in", page(&bug)),
        link(
            "f",
            Icon::Idea,
            "Suggest a feature",
            "the feature form, for your idea",
            page("/issues/new?template=feature_request.yml"),
        ),
        link("s", Icon::Source, "Source", "the code, on GitHub", page("")),
        link("u", Icon::Update, "Check for an update", "right here, and install it", Does::CheckUpdate),
        link(
            "p",
            Icon::Person,
            "Contributors",
            "everyone, with their commits, on GitHub",
            page("/graphs/contributors"),
        ),
    ]
}

/// `text` for a URL's query: letters, digits and `-._~` as they are, everything else percent-encoded.
fn encode(text: &str) -> String {
    text.bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => (byte as char).to_string(),
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

/// Opens it with the newest list of contributors there is (the startup checklist may have fetched one since), and
/// without the outcome of an update check that ended before.
pub fn open(ui: &mut Ui, controller: &Controller) {
    ui.contributors = contributors::load(&controller.paths().state_dir);
    if ui.update_check.as_ref().is_some_and(|check| check.status.is_some()) {
        ui.update_check = None;
    }
    ui.overlay = Some(Overlay::About { index: 0 });
}

/// `u`: starts the update check, unless one is running.
fn check_update(ui: &mut Ui, controller: &Controller) {
    if ui.update_check.as_ref().is_some_and(|check| check.status.is_none()) {
        return;
    }
    let (sender, results) = channel();
    let (config, state_dir) = (controller.config.clone(), controller.paths().state_dir.clone());
    std::thread::spawn(move || {
        let progress = sender.clone();
        let mut report = |text: &str| {
            let _ = progress.send(Message::Progress(text.to_string()));
        };
        let outcome = update::update(&config, &state_dir, false, &mut update::Progress::new(&mut report));
        let _ = sender.send(Message::Done(outcome));
    });
    ui.update_check = Some(UpdateCheck { results, line: "checking…".into(), status: None });
}

/// Takes what the update check said since, the outcome to the log too; the version it installed, once.
pub fn poll_update(ui: &mut Ui, controller: &mut Controller) -> Option<String> {
    let check = ui.update_check.as_mut().filter(|check| check.status.is_none())?;
    loop {
        match check.results.try_recv() {
            Ok(Message::Progress(text)) => check.line = text,
            Ok(Message::Done(outcome)) => {
                let hue = match outcome.status {
                    update::Status::Ok => Hue::Green,
                    update::Status::Warn => Hue::Yellow,
                    update::Status::Skip => Hue::DarkGray,
                };
                controller.add_log(format!("lazyrunner: {}", outcome.detail), hue);
                (check.status, check.line) = (Some(outcome.status), outcome.detail);
                return outcome.updated_to;
            }
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => {
                (check.status, check.line) = (Some(update::Status::Warn), "the update check didn't finish".into());
                return None;
            }
        }
    }
}

/// The About box's keys: a link's letter, or j k (< >) + Enter; Esc or q closes it. A page opens with the box kept
/// open; lazyrunner's own actions close it.
pub fn key(ui: &mut Ui, controller: &mut Controller, name: &str) {
    let all = links();
    let Some(Overlay::About { index }) = &mut ui.overlay else { return };
    let last = all.len() - 1;
    let chosen = match name {
        "Escape" | "q" => {
            ui.overlay = None;
            return;
        }
        "j" | "DownArrow" | "Ctrl+N" => {
            *index = keys::wrapped(*index, 1, all.len());
            return;
        }
        "k" | "UpArrow" | "Ctrl+P" => {
            *index = keys::wrapped(*index, -1, all.len());
            return;
        }
        "<" | "Home" => {
            *index = 0;
            return;
        }
        ">" | "End" => {
            *index = last;
            return;
        }
        "Enter" | "Space" => all.get(*index),
        // The link a letter opens gets the highlight too, as if chosen with j k.
        _ => match all.iter().position(|link| link.key == name) {
            Some(at) => {
                *index = at;
                all.get(at)
            }
            None => return,
        },
    };
    let Some(link) = chosen else { return };
    match &link.does {
        Does::Open(url) => super::links::open_url(ui, controller, url),
        Does::Run(run) => {
            ui.overlay = None;
            keys::run(ui, controller, run);
        }
        Does::CheckUpdate => check_update(ui, controller),
    }
}

pub fn rows(ui: &mut Ui, controller: &Controller, cells: usize, rows: usize) -> Vec<Vec<Seg>> {
    let all = links();
    let index = match &mut ui.overlay {
        Some(Overlay::About { index }) => {
            *index = (*index).min(all.len() - 1);
            *index
        }
        _ => 0,
    };
    let (nerd, animated, tick, seconds) = (ui.nerd, !ui.reduced_motion, ui.tick(), ui.seconds());
    let widest = all
        .iter()
        .map(|link| 8 + icons::icon(nerd, link.icon).width() + LABEL_CELLS + link.hint.width())
        .max()
        .unwrap_or(0);
    let inner = super::overlays::fit_inner(cells, INNER, widest);
    let mut lines = vec![
        version_line(controller, nerd),
        vec![seg("  Runs a project's .NET and npm apps side by side.", C::Gray)],
        Vec::new(),
        vec![seg("  Core contributors", C::DarkCyan)],
    ];
    for person in &ui.contributors.people {
        let (icon, color) = if person.builds() { (Icon::Person, C::Magenta) } else { (Icon::Idea, C::Yellow) };
        let name = pad(&person.name, NAME_CELLS);
        lines.push(vec![
            seg(format!("   {} ", icons::icon(nerd, icon)), color),
            seg(format!("{name} "), C::White),
            seg(format!("{} · @{}", person.summary(), person.login), C::DarkGray),
        ]);
    }
    lines.push(Vec::new());
    lines.push(vec![seg("  Links", C::DarkCyan)]);
    for (at, link) in all.iter().enumerate() {
        let icon = icons::icon(nerd, link.icon);
        let label = pad(link.label, LABEL_CELLS);
        let line = if at == index {
            vec![on(ui.scrolling(&format!(" ▸ {}  {icon}  {label}{}", link.key, link.hint), inner), C::Black, C::Cyan)]
        } else {
            vec![
                seg(format!("   {}  ", link.key), C::Cyan),
                seg(format!("{icon}  "), RAINBOW[at % RAINBOW.len()]),
                seg(label, C::White),
                seg(link.hint, C::DarkGray),
            ]
        };
        lines.push(aim(line, Target::Item(at)));
        if link.does == Does::CheckUpdate
            && let Some(check) = &ui.update_check
        {
            lines.push(update_line(check, animated, tick));
        }
    }
    lines.push(Vec::new());
    lines.push(made_with(nerd, animated, tick));
    // A short terminal loses the logo first, then the blank line above the rest.
    let logo = logo(inner, animated, seconds);
    let borders = 3;
    if lines.len() + logo.len() + 2 + borders <= rows {
        lines.splice(0..0, [vec![Vec::new()], logo, vec![Vec::new()]].concat());
    } else if lines.len() + 1 + borders <= rows {
        lines.insert(0, Vec::new());
    }
    let sparkle = if animated { SPARKLES[(tick / 3) as usize % SPARKLES.len()] } else { SPARKLES[0] };
    let footer = "a letter, or j k + Enter · Esc closes";
    boxed(&format!("{sparkle} About lazyrunner"), lines, footer, cells, rows, inner)
}

/// The logo, centred, in rainbow colours flowing across it, a rainbow rule flowing the other way under it. Still:
/// the theme's cyans.
fn logo(inner: usize, animated: bool, seconds: f64) -> Vec<Vec<Seg>> {
    let wide = LOGO[0].width();
    let indent = " ".repeat(inner.saturating_sub(wide) / 2);
    // How far the colours have flowed, in bands: fractions too, so a band's edge moves a cell at a time.
    let shift = seconds * RAINBOW_BANDS_PER_SECOND;
    let rainbow = |band: f64| RAINBOW[band.floor().rem_euclid(RAINBOW.len() as f64) as usize];
    let mut lines: Vec<Vec<Seg>> = LOGO
        .iter()
        .enumerate()
        .map(|(row, line)| {
            let mut segs = vec![seg(indent.clone(), C::Gray)];
            segs.extend(line.chars().enumerate().map(|(at, symbol)| {
                let color = match (animated, row) {
                    (true, _) => rainbow(at as f64 / 3.0 + row as f64 - shift),
                    (false, 0) => C::Cyan,
                    (false, _) => C::DarkCyan,
                };
                seg(symbol.to_string(), color)
            }));
            segs
        })
        .collect();
    let mut rule = vec![seg(indent, C::Gray)];
    rule.extend((0..wide).map(|at| {
        let color = if animated { rainbow(at as f64 / 2.0 + shift) } else { C::DarkGray };
        seg("━", color)
    }));
    lines.push(rule);
    lines
}

/// The version, when it was released (by the changelog), what it runs on and the project it runs.
fn version_line(controller: &Controller, nerd: bool) -> Vec<Seg> {
    let released = changelog::entries(changelog::CHANGELOG, None, None)
        .into_iter()
        .find(|entry| entry.version == VERSION)
        .map_or(String::new(), |entry| format!(" · released {}", entry.date));
    let (os, arch) = (std::env::consts::OS, std::env::consts::ARCH);
    vec![
        seg(format!("  {} ", icons::icon(nerd, Icon::Releases)), C::Yellow),
        seg(format!("v{VERSION}"), C::White),
        seg(format!("{released} · {os} {arch} · running {}", controller.project.name), C::DarkGray),
    ]
}

/// Under the update link: a spinner and what the check is doing, then how it ended.
fn update_line(check: &UpdateCheck, animated: bool, tick: u64) -> Vec<Seg> {
    let (mark, color) = match check.status {
        None if animated => (spinner(tick).to_string(), C::Yellow),
        None => ("…".to_string(), C::Yellow),
        Some(update::Status::Ok) => ("✓".to_string(), C::Green),
        Some(update::Status::Warn) => ("!".to_string(), C::Yellow),
        Some(update::Status::Skip) => ("-".to_string(), C::DarkGray),
    };
    let text = if check.status == Some(update::Status::Warn) { C::Yellow } else { C::Gray };
    vec![seg(format!("      ↳ {mark} "), color), seg(check.line.clone(), text)]
}

/// "made with ♥ and Rust", the heart beating.
fn made_with(nerd: bool, animated: bool, tick: u64) -> Vec<Seg> {
    let beat = animated && tick % 10 < 2;
    vec![
        seg("  made with ", C::DarkGray),
        seg(icons::icon(nerd, Icon::Heart), if beat { C::Magenta } else { C::Red }),
        seg(" and ", C::DarkGray),
        seg(format!("{} Rust", icons::icon(nerd, Icon::Rust)), C::Yellow),
        seg(" · lazyrunner License 1.1", C::DarkGray),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bug_reports_text_is_percent_encoded() {
        assert_eq!(encode("v1.1.0 on windows\n\nWhat?"), "v1.1.0%20on%20windows%0A%0AWhat%3F");
        assert_eq!(encode("Momčilović"), "Mom%C4%8Dilovi%C4%87");
    }

    #[test]
    fn every_link_has_its_own_letter() {
        let all = links();
        for link in &all {
            assert_eq!(all.iter().filter(|other| other.key == link.key).count(), 1, "{}", link.key);
        }
    }
}
