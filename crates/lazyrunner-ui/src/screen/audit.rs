//! O: outdated and vulnerable packages, per app (#79), in three titled panes: the apps, each with its worst finding;
//! the selected app's packages - the vulnerable first, the worst first, their severity in capitals, what a transitive
//! one comes in through, then the outdated ones, and what the last scan fixed as `resolved ✓`; and the picked
//! package's details: its advisory, the versions it affects and the one that fixes it, its link (`Enter` opens it) and
//! what `i` will run. `y` (or `u`) copies that, `i` runs it after a yes - the app stops first and starts again after,
//! then it's scanned again. `/` filters the packages by name, `v` shows only the vulnerable ones. The details sit
//! beside the table on a wide terminal, under it on a narrower one.
//!
//! `r` scans the selected app, `R` every one: `dotnet list package` and `npm audit` / `npm outdated`, in the background,
//! one app at a time, kept in the state folder so the view opens on the last scan. They ask the package feeds, so
//! nothing is scanned until asked. A scan marks what's `new` since the one before.
//!
//! It moves too (not with reduced motion): the rows come in one after another as it opens, the table dims while its app
//! is scanned, the rows that changed light up as the scan lands, what it fixed flashes green, and a new high or
//! critical finding pulses once. With reduced motion a changed row keeps an accent edge instead.

use std::time::Duration;

use crossterm::event::KeyEvent;
use lazyrunner_core::output::Hue;
use lazyrunner_core::profile::App;
use lazyrunner_core::run::Controller;
use lazyrunner_core::run::audit::{self, AppAudit, Package, Severity};

use super::columns::{chunk, fit, spinner};
use super::icons::{self, Icon};
use super::mouse::Target;
use super::seg::{Seg, aim, on, pad, seg};
use super::theme::C;
use super::{Overlay, Ui};

/// The apps' list, at least and at most this wide.
const LIST_WIDTH: (usize, usize) = (24, 36);
/// From this wide the picked package's details go beside its table (this wide), else under it.
const BESIDE_FROM: usize = 110;
const DETAIL_WIDTH: usize = 44;
/// The table's columns after the package's name, and the `new` chip after a name.
const VERSION_CELLS: usize = 12;
const SEVERITY_CELLS: usize = 18;
const KIND_CELLS: usize = 11;
const NEW: &str = " new ";
/// As the view opens its rows come in this far apart, all of them within `STAGGER_ALL`.
const STAGGER: Duration = Duration::from_millis(15);
const STAGGER_ALL: Duration = Duration::from_millis(200);
/// As a scan lands: how long the rows that changed stay lit, and a new high or critical finding's pulse.
const LAND: Duration = Duration::from_millis(200);
const PULSE: Duration = Duration::from_millis(300);

/// O: the audit view, on the app with the worst finding.
pub fn open(ui: &mut Ui, controller: &Controller) {
    let worst = |at: usize| controller.apps.get(at).and_then(|app| controller.audits().get(&app.key)?.worst());
    let app = (0..controller.apps.len())
        .max_by_key(|&at| (worst(at), std::cmp::Reverse(at)))
        .filter(|&at| worst(at).is_some())
        .unwrap_or(0);
    let opened = ui.now;
    ui.overlay = Some(Overlay::Audit {
        app,
        index: 0,
        packages: false,
        only: false,
        confirm: false,
        filter: String::new(),
        typing: false,
        opened,
    });
}

fn scan_of(controller: &Controller, app: usize) -> Option<&AppAudit> {
    controller.apps.get(app).and_then(|app| controller.audits().get(&app.key))
}

/// Whether a package's name holds what `/` typed (any case).
fn named(name: &str, filter: &str) -> bool {
    filter.is_empty() || name.to_lowercase().contains(&filter.to_lowercase())
}

/// The selected app's packages as listed: only the vulnerable ones with `only`, only those `/` names.
fn shown<'a>(controller: &'a Controller, app: usize, only: bool, filter: &str) -> Vec<&'a Package> {
    let Some(scan) = scan_of(controller, app) else { return Vec::new() };
    let listed = scan.packages.iter().filter(|package| !only || package.severity.is_some());
    listed.filter(|package| named(&package.name, filter)).collect()
}

/// j k choose (the apps, or their packages once l or Enter went into them), h back, r scan the app, R every app, /
/// filter by name, v only the vulnerable, y or u copy the fix, i install it (after a yes: y or Enter, n or Esc not),
/// Enter or o open the advisory, Esc back out of the packages, then the filter, then close; false for a key it
/// doesn't use.
pub fn key(ui: &mut Ui, controller: &mut Controller, name: &str, key: &KeyEvent) -> bool {
    let chosen = picked(ui, controller);
    let apps = controller.apps.len();
    let Some(Overlay::Audit { app, index, packages, only, confirm, filter, typing, .. }) = &mut ui.overlay else {
        return false;
    };
    if *typing {
        match name {
            "Enter" => *typing = false,
            "Escape" => (*typing, *filter) = (false, String::new()),
            "Backspace" => {
                filter.pop();
            }
            _ => {
                if let Some(c) = super::keys::typed(key) {
                    filter.push(c);
                }
            }
        }
        *index = 0;
        return true;
    }
    if *confirm {
        match name {
            "y" | "Enter" => {
                *confirm = false;
                if let Some((app, package)) = chosen {
                    controller.audit_install(&app.key, &package);
                }
            }
            "n" | "Escape" | "q" => *confirm = false,
            _ => {}
        }
        return true;
    }
    let count = shown(controller, *app, *only, filter).len();
    *index = (*index).min(count.saturating_sub(1));
    match name {
        "j" | "DownArrow" | "k" | "UpArrow" => {
            let step = if matches!(name, "j" | "DownArrow") { 1 } else { -1 };
            if *packages {
                *index = super::keys::wrapped(*index, step, count.max(1));
            } else {
                (*app, *index) = (super::keys::wrapped(*app, step, apps.max(1)), 0);
            }
        }
        "Enter" | "o" if *packages => open_advisory(ui, controller),
        "l" | "RightArrow" | "Enter" => *packages = count > 0,
        "h" | "LeftArrow" => *packages = false,
        "Tab" => *packages = !*packages && count > 0,
        "/" => (*typing, *index) = (true, 0),
        "v" => (*only, *index) = (!*only, 0),
        "r" => {
            if let Some(key) = controller.apps.get(*app).map(|app| app.key.clone()) {
                controller.audit(Some(&[key]));
            }
        }
        "R" => controller.audit(None),
        "i" => match chosen.as_ref().and_then(|(app, package)| audit::command(app, package)) {
            Some(_) => *confirm = true,
            None => {
                controller.add_log("There's nothing to install for that - no fixed version is known", Hue::DarkGray)
            }
        },
        "y" | "u" => copy_fix(ui, controller),
        "o" => open_advisory(ui, controller),
        "Escape" if *packages => *packages = false,
        "Escape" if !filter.is_empty() => filter.clear(),
        "Escape" | "q" | "O" => ui.overlay = None,
        _ => return false,
    }
    true
}

/// The picked package, with its app.
fn picked(ui: &Ui, controller: &Controller) -> Option<(App, Package)> {
    let Some(Overlay::Audit { app, index, only, filter, .. }) = &ui.overlay else { return None };
    let package = shown(controller, *app, *only, filter).get(*index).map(|package| (*package).clone())?;
    Some((controller.apps.get(*app)?.clone(), package))
}

/// y: the picked package's fix, or the upgrade to its latest version, for its app's folder.
fn copy_fix(ui: &mut Ui, controller: &mut Controller) {
    let Some((app, package)) = picked(ui, controller) else { return };
    match audit::command(&app, &package) {
        Some(command) => {
            let what = format!("the fix for {} - run it in {}", package.name, folder(controller, &app));
            super::copy::put(ui.clipboard, controller, &command, &what);
        }
        None => controller.add_log(format!("{}: no fixed version is known yet", package.name), Hue::DarkGray),
    }
}

/// Enter or o: the picked package's advisory, in the browser.
fn open_advisory(ui: &mut Ui, controller: &mut Controller) {
    let Some((_, package)) = picked(ui, controller) else { return };
    match package.advisories.first() {
        Some(advisory) => super::links::open_url(ui, controller, &advisory.url),
        None => controller.add_log(format!("{} has no advisory - it's outdated only", package.name), Hue::DarkGray),
    }
}

/// Where an app's fix runs, from the repo.
fn folder(controller: &Controller, app: &App) -> String {
    let folder = audit::folder(&controller.root, app);
    let shown = folder.strip_prefix(&controller.root).unwrap_or(&folder).display().to_string().replace('\\', "/");
    if shown.is_empty() { ".".into() } else { shown }
}

/// A click on an app selects it, on a package picks it.
pub fn click(ui: &mut Ui, target: Option<Target>) -> bool {
    let Some(Overlay::Audit { app, index, packages, confirm, .. }) = &mut ui.overlay else { return false };
    match target {
        Some(Target::Item(at)) => (*app, *index, *packages) = (at, 0, false),
        Some(Target::Package(at)) => (*index, *packages) = (at, true),
        _ => return false,
    }
    *confirm = false;
    true
}

/// Whether its motion runs: not with reduced motion, nor in the screen tests' frozen frames.
fn moving(ui: &Ui) -> bool {
    ui.motion.enabled && !ui.reduced_motion
}

/// How long ago app `key`'s scan landed, while its motion runs.
fn landed(ui: &Ui, controller: &Controller, key: &str, within: Duration) -> Option<Duration> {
    let since = ui.now.saturating_duration_since(controller.audit_landed(key)?);
    (moving(ui) && since < within).then_some(since)
}

/// How many of `count` table rows have come in since the view opened.
fn arrived(ui: &Ui, count: usize) -> usize {
    let Some(Overlay::Audit { opened, .. }) = &ui.overlay else { return count };
    if !moving(ui) {
        return count;
    }
    let step = STAGGER.min(STAGGER_ALL / count.max(1) as u32).max(Duration::from_millis(1));
    let since = ui.now.saturating_duration_since(*opened);
    ((since.as_millis() / step.as_millis()) as usize + 1).min(count)
}

/// Whether the view moves: its rows still coming in, or a scan just landed.
pub fn animating(ui: &Ui, controller: &Controller) -> bool {
    let Some(Overlay::Audit { opened, .. }) = &ui.overlay else { return false };
    let opening = moving(ui) && ui.now.saturating_duration_since(*opened) < STAGGER_ALL;
    opening || controller.apps.iter().any(|app| landed(ui, controller, &app.key, LAND.max(PULSE)).is_some())
}

/// A severity as the table and the details show it: in capitals, a critical one filled.
fn pill(severity: Severity) -> Seg {
    let name = severity.name().to_uppercase();
    match severity {
        Severity::Critical => on(format!(" {name} "), C::Black, C::Red),
        Severity::High => seg(format!(" {name} "), C::Red),
        Severity::Moderate => seg(format!(" {name} "), C::Yellow),
        Severity::Low => seg(format!(" {name} "), C::DarkGray),
        Severity::Info => seg(format!(" {name} "), C::Cyan),
    }
}

/// A severity in the apps' list: in its colour, a critical one filled.
fn badge(severity: Severity, text: &str) -> Seg {
    match severity {
        Severity::Critical => on(format!(" {text} "), C::Black, C::Red),
        Severity::High => seg(text, C::Red),
        Severity::Moderate => seg(text, C::Yellow),
        Severity::Low => seg(text, C::DarkGray),
        Severity::Info => seg(text, C::Cyan),
    }
}

fn spin(ui: &Ui) -> &str {
    if ui.reduced_motion { "●" } else { spinner(ui) }
}

/// The view's first row: the project, what the scans found, and what's scanning or installing (or when the last scan
/// was).
fn header(ui: &Ui, controller: &Controller) -> Vec<Seg> {
    let mut header = vec![
        on(format!(" {} AUDIT ", icons::icon(ui.nerd, Icon::Shield)), C::Black, C::Cyan),
        seg(format!("  {}", controller.project.name), C::White),
    ];
    let scans = controller.audits();
    let all: Vec<Package> = scans.values().flat_map(|scan| scan.packages.iter().cloned()).collect();
    for severity in Severity::ALL.into_iter().filter(|severity| *severity >= Severity::High) {
        let count = all.iter().filter(|package| package.severity == Some(severity)).count();
        if count > 0 {
            header.extend([seg("  ", C::Gray), badge(severity, &format!("{count} {}", severity.name()))]);
        }
    }
    if !scans.is_empty() {
        header.push(seg(format!("  {} packages", all.len()), C::Gray));
    }
    if let Some((_, command)) = controller.audit_installing() {
        header.push(seg(format!(" · {} installing: {command}", spin(ui)), C::Yellow));
    } else if let Some(key) = controller.auditing() {
        let name = controller.app(key).map_or(key, |app| app.name.as_str());
        let more = controller.audit_queue().len();
        let more = if more > 0 { format!(" · {more} more") } else { String::new() };
        header.push(seg(format!(" · {} scanning {name}{more}", spin(ui)), C::Yellow));
    } else if let Some(at) = scans.values().map(|scan| scan.at).max() {
        header.push(seg(format!(" · last scan {}", audit::age(at)), C::DarkGray));
    }
    header
}

fn hint(packages: bool, only: bool, typing: bool) -> String {
    if typing {
        return " type to filter the packages by name · Enter keeps it · Esc drops it".into();
    }
    let only = if only { "v everything" } else { "v only vulnerable" };
    if packages {
        format!(" j k · y copy the fix · i install it · Enter advisory · / filter · {only} · r rescan · h apps · Esc")
    } else {
        format!(" j k choose · l its packages · r scan it · R scan every app · / filter · {only} · Esc back")
    }
}

/// The question before `i` installs the picked package's fix: the keys first, a long command cut at the end.
fn question(controller: &Controller, app: usize, package: Option<&Package>, view_width: usize) -> Vec<Seg> {
    let (Some(app), Some(package)) = (controller.apps.get(app), package) else { return Vec::new() };
    let Some(command) = audit::command(app, package) else { return Vec::new() };
    let stops = if controller.is_running(&app.key) { " - it stops the app and starts it again" } else { "" };
    let text = format!(" y installs · n doesn't - {command} in {}{stops} ", folder(controller, app));
    vec![on(pad(&text, view_width), C::Black, C::Yellow)]
}

/// What an app's scan says, in a few cells; a new high or critical finding pulses once as the scan lands.
fn state(ui: &Ui, controller: &Controller, key: &str) -> Seg {
    if controller.auditing() == Some(key) {
        return seg(format!("{} scanning", spin(ui)), C::Yellow);
    }
    if controller.audit_queue().iter().any(|queued| queued == key) {
        return seg("queued", C::DarkGray);
    }
    let Some(scan) = controller.audits().get(key) else { return seg("not scanned", C::DarkGray) };
    if scan.error.is_some() {
        return seg("✗ couldn't scan", C::Red);
    }
    if let Some(worst) = scan.worst() {
        let mut shown = badge(worst, &format!("{} {}", scan.count(worst), worst.name()));
        let first_found = worst >= Severity::High && !scan.new_advisories.is_empty();
        if first_found && landed(ui, controller, key, PULSE).is_some_and(|since| since < PULSE / 2) {
            (shown.fg, shown.bg) = (C::Black, Some(C::White));
        }
        return shown;
    }
    match scan.packages.iter().filter(|package| package.outdated()).count() {
        0 => seg("up to date", C::Green),
        outdated => seg(format!("↑ {outdated} outdated"), C::Gray),
    }
}

/// The apps, each with what its scan says; the selected one lit while the apps have the keys.
fn app_rows(ui: &Ui, controller: &Controller, selected: usize, list_width: usize, focus: bool) -> Vec<Vec<Seg>> {
    let apps = controller.apps.iter().enumerate();
    apps.map(|(at, app)| {
        let state = state(ui, controller, &app.key);
        let icon = icons::app(ui.nerd, app, controller.framework(&app.key));
        let room = list_width.saturating_sub(state.text.chars().count() + 7).max(4);
        let marker = if at == selected { "▸" } else { " " };
        let mut row =
            vec![seg(format!(" {marker} {icon} "), C::Gray), seg(pad(&app.name, room), C::White), seg(" ", C::Gray)];
        row.push(state);
        if at == selected && !focus {
            for part in row.iter_mut().filter(|part| part.bg.is_none()) {
                (part.fg, part.bg) = (C::FocusText, Some(C::FocusBack));
            }
        }
        aim(row, Target::Item(at))
    })
    .collect()
}

/// How the table looks right now: dimmed while its app is scanned, its changed rows lit as the scan lands, and with
/// reduced motion an accent edge on them instead.
struct Look {
    dim: bool,
    landing: bool,
    edge: bool,
}

/// The table: its column titles, then a package a row - its name with what it comes in through and `new` when the
/// scan before didn't have it, its versions, its severity (or `↑ outdated only`) and whether the app references it
/// itself - and what the last scan fixed, as `resolved ✓`; the picked one lit while the packages have the keys.
fn table_rows(
    list: &[&Package],
    scan: Option<&AppAudit>,
    filter: &str,
    (index, focus): (usize, bool),
    look: &Look,
    table_width: usize,
) -> Vec<Vec<Seg>> {
    let name_cells = table_width.saturating_sub(VERSION_CELLS * 2 + SEVERITY_CELLS + KIND_CELLS).max(10);
    let titles = format!(
        "  {}{}{}{}{}",
        pad("Package", name_cells - 2),
        pad("Current", VERSION_CELLS),
        pad("Latest", VERSION_CELLS),
        pad(" Severity", SEVERITY_CELLS),
        pad("Type", KIND_CELLS)
    );
    let mut rows = vec![vec![seg(titles, C::DarkGray)]];
    let new = scan.map_or(&[][..], |scan| &scan.new);
    for (at, package) in list.iter().enumerate() {
        let changed = new.contains(&package.name);
        let edge = if changed && look.edge { seg("▌", C::Cyan) } else { seg(" ", C::Gray) };
        let mut name = vec![seg(format!(" {}", package.name), C::White)];
        if let Some(first) = package.via.first() {
            let more = if package.via.len() > 1 { format!(" +{}", package.via.len() - 1) } else { String::new() };
            name.push(seg(format!(" via {first}{more}"), C::DarkGray));
        }
        let chip = if changed { NEW.len() } else { 0 };
        let mut row = vec![edge];
        row.extend(fit(name, name_cells - 1 - chip));
        if changed {
            row.push(on(NEW, C::Black, C::Yellow));
        }
        let latest = package.latest.clone().filter(|_| package.outdated()).unwrap_or_default();
        let latest_color = if package.major_behind() { C::Yellow } else { C::Gray };
        row.extend([
            seg(pad(&package.current, VERSION_CELLS), C::Gray),
            seg(pad(&latest, VERSION_CELLS), latest_color),
        ]);
        let severity = match package.severity {
            Some(severity) => vec![pill(severity)],
            None => vec![seg(" ↑ outdated only", C::DarkGray)],
        };
        row.extend(fit(severity, SEVERITY_CELLS));
        row.push(seg(pad(if package.direct { "direct" } else { "transitive" }, KIND_CELLS), C::DarkGray));
        let lit = (at == index && focus).then_some(C::FocusBack).or((changed && look.landing).then_some(C::DarkCyan));
        for part in row.iter_mut().skip(1).filter(|part| part.bg.is_none()) {
            if let Some(back) = lit {
                part.bg = Some(back);
                if matches!(part.fg, C::Gray | C::DarkGray | C::White) {
                    part.fg = C::FocusText;
                }
            } else if look.dim {
                part.fg = C::DarkGray;
            }
        }
        rows.push(aim(row, Target::Package(at)));
    }
    let resolved = scan.into_iter().flat_map(|scan| &scan.fixed).filter(|fixed| named(&fixed.name, filter));
    for fixed in resolved {
        let mut row = vec![
            seg(" ", C::Gray),
            seg(pad(&format!(" {}", fixed.name), name_cells - 1), C::Gray),
            seg(pad(&fixed.from, VERSION_CELLS), C::DarkGray),
            seg(pad(fixed.to.as_deref().unwrap_or(""), VERSION_CELLS), C::Gray),
            seg(pad(" resolved ✓", SEVERITY_CELLS), C::Green),
        ];
        if look.landing {
            for part in row.iter_mut().skip(1) {
                (part.fg, part.bg) = (C::Black, Some(C::Green));
            }
        }
        rows.push(row);
    }
    rows
}

/// What the table says when it has no packages: not scanned yet, nothing found, nothing named so, or why the scan
/// failed.
fn empty(controller: &Controller, app: &App, only: bool, filter: &str) -> Vec<Vec<Seg>> {
    match controller.audits().get(&app.key) {
        None => vec![
            vec![seg("  not scanned yet - r scans it, R scans every app", C::Gray)],
            vec![seg("  (dotnet list package / npm audit and npm outdated: they ask the package feeds)", C::DarkGray)],
        ],
        Some(_) if !filter.is_empty() => vec![vec![seg(format!("  no package named \"{filter}\""), C::Gray)]],
        Some(scan) if scan.packages.is_empty() && scan.error.is_none() => {
            vec![vec![seg("  ✓ nothing vulnerable or outdated", C::Green)]]
        }
        Some(_) if only => vec![vec![seg("  ✓ nothing vulnerable - v shows the outdated too", C::Green)]],
        Some(_) => Vec::new(),
    }
}

/// The picked package's details, as the concept has them: its name and version, its severity, advisory id and how
/// the app gets it; its advisory with the versions it affects, the one that fixes it and its link; the others; and
/// what `i` will run.
fn details(controller: &Controller, app: &App, package: Option<&Package>, detail_width: usize) -> Vec<Vec<Seg>> {
    let Some(package) = package else { return Vec::new() };
    let new = controller.audits().get(&app.key).map_or(&[][..], |scan| &scan.new_advisories);
    let room = detail_width.saturating_sub(2).max(10);
    let mut out = vec![vec![seg(format!(" {} ", package.name), C::White), seg(package.current.clone(), C::Gray)]];
    let how = match (package.direct, package.via.first()) {
        (true, _) => format!("direct dependency of {}", app.name),
        (false, Some(via)) => format!("transitive, via {via}"),
        (false, None) => "transitive".into(),
    };
    let (first, others) = match package.advisories.split_first() {
        Some((first, others)) => (Some(first), others),
        None => (None, &[][..]),
    };
    out.push(match (package.severity, first) {
        (Some(severity), Some(advisory)) => {
            vec![pill(severity), seg(format!(" {} · {how}", advisory.id), C::DarkGray)]
        }
        (Some(severity), None) => vec![pill(severity), seg(format!(" {how}"), C::DarkGray)],
        (None, _) => vec![seg(" ↑ outdated only", C::Gray), seg(format!(" · {how}"), C::DarkGray)],
    });
    let title =
        |out: &mut Vec<Vec<Seg>>, text: &str| out.extend([Vec::new(), vec![seg(format!(" {text}"), C::DarkCyan)]]);
    let label = |out: &mut Vec<Vec<Seg>>, text: &str| out.push(vec![seg(format!(" {text}"), C::DarkGray)]);
    let value = |out: &mut Vec<Vec<Seg>>, text: &str, color: C| {
        out.extend(chunk(text, room).into_iter().map(|piece| vec![seg(format!(" {piece}"), color)]));
    };
    if let Some(advisory) = first {
        let mut heading = vec![seg(" ADVISORY", C::DarkCyan)];
        if new.contains(&advisory.id) {
            heading.extend([seg(" ", C::Gray), on(NEW, C::Black, C::Yellow)]);
        }
        out.extend([Vec::new(), heading]);
        if !advisory.title.is_empty() {
            value(&mut out, &advisory.title, C::Gray);
        }
        if !advisory.range.is_empty() {
            label(&mut out, "affected versions");
            value(&mut out, &advisory.range, C::White);
        }
        if let Some(fix) = package.fix.as_ref().filter(|fix| !fix.version.is_empty()) {
            label(&mut out, "fixed in");
            let name = if fix.package == package.name { String::new() } else { format!("{} ", fix.package) };
            value(&mut out, &format!("{name}{}", fix.version), C::Green);
        }
        label(&mut out, "link");
        value(&mut out, &format!("{} (Enter opens)", advisory.url.trim_start_matches("https://")), C::Cyan);
    }
    if !others.is_empty() {
        title(&mut out, &format!("ALSO ({})", others.len()));
        for advisory in others {
            let text = if advisory.title.is_empty() { advisory.id.clone() } else { advisory.title.clone() };
            let mut line = vec![seg(" ", C::Gray), pill(advisory.severity)];
            line.push(seg(format!(" {text}"), C::Gray));
            if new.contains(&advisory.id) {
                line.push(on(NEW, C::Black, C::Yellow));
            }
            out.push(line);
        }
    }
    match audit::command(app, package) {
        Some(command) => {
            title(&mut out, "WHAT `i` WILL RUN");
            value(&mut out, &command, C::Cyan);
            label(&mut out, &format!("in {} · y copies it", folder(controller, app)));
            if package.fix.as_ref().is_some_and(|fix| fix.major) || (package.fix.is_none() && package.major_behind()) {
                out.push(vec![seg(" a new major version: check what changed first", C::Yellow)]);
            }
        }
        None if package.severity.is_some() => {
            title(&mut out, "FIX");
            out.push(vec![seg(" no fixed version is known yet", C::Gray)]);
        }
        None => {}
    }
    out
}

/// The whole view: the header, the keys (or the question before an install, or the filter being typed), the panes'
/// titles, then the apps beside the table and the picked package's details (beside the table when there's room,
/// under it when not).
pub fn rows(ui: &Ui, controller: &Controller, view_width: usize, rows: usize) -> Vec<Vec<Seg>> {
    let Some(Overlay::Audit { app, index, packages, only, confirm, filter, typing, .. }) = &ui.overlay else {
        return Vec::new();
    };
    let Some(app_at) = controller.apps.len().checked_sub(1).map(|last| (*app).min(last)) else { return Vec::new() };
    let the_app = &controller.apps[app_at];
    let scan = scan_of(controller, app_at);
    let list = shown(controller, app_at, *only, filter);
    let index = (*index).min(list.len().saturating_sub(1));
    let list_width = (view_width / 4).clamp(LIST_WIDTH.0, LIST_WIDTH.1).min(view_width.saturating_sub(30));
    let right = view_width.saturating_sub(list_width + 3);
    let beside = right >= BESIDE_FROM;
    let table_width = if beside { right.saturating_sub(DETAIL_WIDTH + 3) } else { right };
    let height = rows.saturating_sub(3);
    let look = Look {
        dim: moving(ui) && controller.auditing() == Some(the_app.key.as_str()),
        landing: landed(ui, controller, &the_app.key, LAND).is_some(),
        edge: ui.reduced_motion,
    };
    let mut table = table_rows(&list, scan, filter, (index, *packages), &look, table_width);
    let arrived = arrived(ui, table.len());
    table.truncate(arrived.max(1));
    if let Some(error) = scan.and_then(|scan| scan.error.as_ref()) {
        table.insert(0, vec![seg(format!("  ✗ the last scan failed: {error} - r tries again"), C::Red)]);
    }
    if list.is_empty() {
        table.extend(empty(controller, the_app, *only, filter));
    }
    let details = details(controller, the_app, list.get(index).copied(), if beside { DETAIL_WIDTH } else { right });
    let mut packages_title = vec![seg(format!(" PACKAGES · {}", the_app.name.to_uppercase()), C::DarkCyan)];
    if *typing || !filter.is_empty() {
        let cursor = if *typing { ui.cursor() } else { "" };
        packages_title.push(seg(format!("   / {filter}{cursor}"), C::Yellow));
    }
    let titles = if beside {
        let mut line = fit(packages_title, table_width);
        line.extend([seg(" │ ", C::DarkGray), seg(" DETAILS", C::DarkCyan)]);
        line
    } else {
        packages_title
    };
    let right_rows = if beside {
        let table = window(&table, index + 1, height);
        (0..height)
            .map(|row| {
                let mut line = fit(table.get(row).cloned().unwrap_or_default(), table_width);
                line.push(seg(" │ ", C::DarkGray));
                line.extend(fit(details.get(row).cloned().unwrap_or_default(), DETAIL_WIDTH));
                line
            })
            .collect()
    } else {
        // The table takes the rows it needs (up to what leaves the details half), the details the rest.
        let most = height.saturating_sub(details.len().min(height / 2) + 2).max(3);
        let table_room = table.len().clamp(3, most);
        let mut stacked = window(&table, index + 1, table_room);
        stacked.resize(table_room, Vec::new());
        stacked.push(vec![seg("─".repeat(right), C::DarkGray)]);
        stacked.push(vec![seg(" DETAILS", C::DarkCyan)]);
        stacked.extend(details);
        stacked
    };
    let left = app_rows(ui, controller, app_at, list_width, *packages);
    let keys = if *confirm {
        question(controller, app_at, list.get(index).copied(), view_width)
    } else {
        vec![seg(hint(*packages, *only, *typing), C::DarkGray)]
    };
    let mut pane_titles = fit(vec![seg(" APPS", C::DarkCyan)], list_width);
    pane_titles.push(seg(" │ ", C::DarkGray));
    pane_titles.extend(titles);
    let mut out = vec![fit(header(ui, controller), view_width), fit(keys, view_width), fit(pane_titles, view_width)];
    for row in 0..height {
        let mut line = fit(left.get(row).cloned().unwrap_or_default(), list_width);
        line.push(seg(" │ ", C::DarkGray));
        line.extend(fit(right_rows.get(row).cloned().unwrap_or_default(), right));
        out.push(line);
    }
    out
}

/// `rows` cut to `room`, the titles kept and the `selected` row in view.
fn window(rows: &[Vec<Seg>], selected: usize, room: usize) -> Vec<Vec<Seg>> {
    if rows.len() <= room || room < 2 {
        return rows.iter().take(room).cloned().collect();
    }
    let body = room - 1;
    let first = selected.saturating_sub(body / 2).max(1).min(rows.len() - body);
    std::iter::once(rows[0].clone()).chain(rows[first..first + body].iter().cloned()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_table_keeps_its_titles_and_the_picked_row_in_view() {
        let rows: Vec<Vec<Seg>> = (0..20).map(|at| vec![seg(at.to_string(), C::Gray)]).collect();
        let shown =
            |selected| -> Vec<String> { window(&rows, selected, 5).iter().map(|row| row[0].text.clone()).collect() };
        assert_eq!(shown(1), ["0", "1", "2", "3", "4"]);
        assert_eq!(shown(12), ["0", "10", "11", "12", "13"], "the titles stay, the picked one in the middle");
        assert_eq!(shown(19), ["0", "16", "17", "18", "19"]);
        assert!(named("Newtonsoft.Json", "json") && !named("lodash", "json") && named("x", ""));
    }
}
