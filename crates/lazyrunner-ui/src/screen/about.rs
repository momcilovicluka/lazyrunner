//! The About box (`,` then `A`, or `:` about): lazyrunner's version, who makes it, and where to go next - the
//! README, the changelog, the releases, the license, a bug report or an idea, the source, an update check. Its logo
//! shimmers in rainbow colours over a flowing rule, the title sparkles and the heart beats (all still with reduced
//! motion).

use lazyrunner_core::changelog;
use lazyrunner_core::run::Controller;
use lazyrunner_core::update::DEFAULT_REPO;
use unicode_width::UnicodeWidthStr;

use super::actions::Run;
use super::icons::{self, Icon};
use super::mouse::Target;
use super::overlays::{LOGO, boxed};
use super::seg::{Seg, aim, on, pad, seg};
use super::theme::C;
use super::{Overlay, Ui, keys};

const VERSION: &str = changelog::VERSION;
const INNER: usize = 72;
/// Who makes lazyrunner, and what they do for it.
const CONTRIBUTORS: [(&str, &str); 1] = [("Luka Momčilović", "creator · maintainer")];
/// Who reported the bugs and asked for the features that shaped lazyrunner (their GitHub names).
const REPORTERS: [(&str, &str); 1] = [("Ognjen Maričić", "@ognjen004028")];
const RAINBOW: [C; 6] = [C::Red, C::Yellow, C::Green, C::Cyan, C::Blue, C::Magenta];
const SPARKLES: [&str; 4] = ["✦", "✧", "⋆", "✧"];
/// How fast the rainbow flows across the logo and its rule.
const RAINBOW_BANDS_PER_SECOND: f64 = 5.0;
const LABEL_CELLS: usize = 22;
/// The names of the contributors, padded so what they do lines up.
const NAME_CELLS: usize = 18;

/// What a link does: opens a page in the browser, or runs one of lazyrunner's own actions.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Does {
    Open(String),
    Run(Run),
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
        link("u", Icon::Update, "Check for an update", "the startup checklist, now", Does::Run(Run::Checklist)),
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

pub fn open(ui: &mut Ui) {
    ui.overlay = Some(Overlay::About { index: 0 });
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
            *index = (*index + 1).min(last);
            return;
        }
        "k" | "UpArrow" | "Ctrl+P" => {
            *index = index.saturating_sub(1);
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
        _ => all.iter().find(|link| link.key == name),
    };
    let Some(link) = chosen else { return };
    match &link.does {
        Does::Open(url) => super::links::open_url(ui, controller, url),
        Does::Run(run) => {
            ui.overlay = None;
            keys::run(ui, controller, run);
        }
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
    let inner = cells.saturating_sub(8).clamp(20, INNER);
    let mut lines = vec![
        version_line(controller, nerd),
        vec![seg("  Runs a project's .NET and npm apps side by side.", C::Gray)],
        Vec::new(),
        vec![seg("  Core contributors", C::DarkCyan)],
    ];
    for (name, role) in CONTRIBUTORS {
        lines.push(vec![
            seg(format!("   {} ", icons::icon(nerd, Icon::Person)), C::Magenta),
            seg(format!("{name:<NAME_CELLS$}"), C::White),
            seg(role, C::DarkGray),
        ]);
    }
    for (name, github) in REPORTERS {
        lines.push(vec![
            seg(format!("   {} ", icons::icon(nerd, Icon::Idea)), C::Yellow),
            seg(format!("{name:<NAME_CELLS$}"), C::White),
            seg(format!("bug reports & ideas · {github}"), C::DarkGray),
        ]);
    }
    lines.push(Vec::new());
    lines.push(vec![seg("  Links", C::DarkCyan)]);
    for (at, link) in all.iter().enumerate() {
        let icon = icons::icon(nerd, link.icon);
        let label = pad(link.label, LABEL_CELLS);
        let line = if at == index {
            vec![on(pad(&format!(" ▸ {}  {icon}  {label}{}", link.key, link.hint), inner), C::Black, C::Cyan)]
        } else {
            vec![
                seg(format!("   {}  ", link.key), C::Cyan),
                seg(format!("{icon}  "), RAINBOW[at % RAINBOW.len()]),
                seg(label, C::White),
                seg(link.hint, C::DarkGray),
            ]
        };
        lines.push(aim(line, Target::Item(at)));
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
    boxed(&format!("{sparkle} About lazyrunner"), lines, footer, cells, rows, INNER)
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
