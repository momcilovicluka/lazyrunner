//! A: outdated and vulnerable packages, per app (#79), in three titled panes: the apps, each with its worst finding;
//! the selected app's packages - the vulnerable first, the worst first, an edge and their severity in its colour
//! (capitals, a critical one filled), what a transitive one comes in through, then the outdated ones dimmed, and what
//! the last scan fixed as `resolved ✓`; and the picked package's details: its advisory, the versions it affects and the
//! one that fixes it, its link (`Enter` opens it) and what `i` will run. `y` (or `u`) copies that, `i` runs it after a
//! yes - the app stops first and starts again after, then it's scanned again. `/` filters the packages by name (the
//! match lit), `v` shows only the vulnerable ones. The details sit beside the table on a wide terminal, under it on a
//! narrower one.
//!
//! `r` scans the selected app, `R` every one: `dotnet list package` and `npm audit` / `npm outdated`, in the background,
//! one app at a time (the header counts them off), kept in the state folder so the view opens on the last scan - its
//! age green today, red after a month. They ask the package feeds, so nothing is scanned until asked. A scan marks
//! what's `new` since the one before.
//!
//! It moves too (not with reduced motion): the rows come in one after another as it opens and the selection glides
//! between them; the table dims while its app is scanned, the rows that changed light up as the scan lands (their `new`
//! blinking once), what it fixed fills green from the left, and a new high or critical finding pulses - in the apps'
//! list and the header's count. A fix being installed spins in its row, and one that failed pulses it red; a copied
//! command lights up. With reduced motion a changed row keeps an accent edge instead.

use std::time::{Duration, Instant};

use crossterm::event::KeyEvent;
use lazyrunner_core::output::Hue;
use lazyrunner_core::profile::App;
use lazyrunner_core::run::Controller;
use lazyrunner_core::run::audit::{self, AppAudit, Package, Severity};

use super::columns::{chunk, fit, spinner};
use super::icons;
use super::motion::COPY_FLASH;
use super::mouse::Target;
use super::seg::{Seg, aim, on, pad, seg, width};
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
/// As a scan lands: how long the rows that changed stay lit (their `new` blinking once, `BLINK` off), what it fixed
/// takes to fill green, and a new high or critical finding's pulse.
const LAND: Duration = Duration::from_millis(200);
const BLINK: Duration = Duration::from_millis(60);
const SWEEP: Duration = Duration::from_millis(300);
const PULSE: Duration = Duration::from_millis(300);
/// A fix that failed to install pulses its row red, twice, over this long.
const FAILED: Duration = Duration::from_millis(400);
/// The last scan's age turns amber after this many days, red after `STALE_DAYS`.
const OLD_DAYS: i64 = 7;
const STALE_DAYS: i64 = 30;
/// The batch's bar in the header, up to this many apps.
const BATCH_BAR: usize = 12;
const FOOTNOTE: &str = "counting a package once per app that owns or references it";

/// A: the audit view, on the app with the worst finding.
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
        batch: false,
        expanded: false,
        filter: String::new(),
        typing: false,
        opened,
        copied: None,
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
/// filter by name, v only the vulnerable, y or u copy the fix, i install it (after a yes: y or Enter, n or Esc not), I
/// install many (c the critical, h high and up, m moderate and up, a all; n none), Enter or o open the advisory, Esc
/// back out of the packages, then the filter, then close; false for a key it doesn't use.
pub fn key(ui: &mut Ui, controller: &mut Controller, name: &str, key: &KeyEvent) -> bool {
    let chosen = picked(ui, controller);
    let apps = controller.apps.len();
    let Some(Overlay::Audit { app, index, packages, only, confirm, batch, expanded, filter, typing, .. }) =
        &mut ui.overlay
    else {
        return false;
    };
    if *batch {
        let floor = match name {
            "c" => Some(Severity::Critical),
            "h" => Some(Severity::High),
            "m" => Some(Severity::Moderate),
            "a" => Some(Severity::Info),
            _ => None,
        };
        match (name, floor) {
            (_, Some(floor)) => {
                *batch = false;
                if let Some(the_app) = controller.apps.get(*app).cloned() {
                    let chosen = fixable(controller, &the_app, floor);
                    if chosen.is_empty() {
                        let text = format!("{}: no {} fix to install", the_app.name, floor.name());
                        controller.add_log(text, Hue::DarkGray);
                    } else {
                        controller.audit_install_all(&the_app.key, chosen);
                    }
                }
            }
            ("n" | "Escape" | "q", _) => *batch = false,
            _ => {}
        }
        return true;
    }
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
        "e" => *expanded = !*expanded,
        "r" => {
            if let Some(key) = controller.apps.get(*app).map(|app| app.key.clone()) {
                controller.audit(Some(&[key]));
            }
        }
        "R" => controller.audit(None),
        "+" => controller.set_audit_parallelism(controller.audit_parallelism() + 1),
        "-" => controller.set_audit_parallelism(controller.audit_parallelism().saturating_sub(1)),
        "i" => match chosen.as_ref().and_then(|(app, package)| audit::command(app, package)) {
            Some(_) => *confirm = true,
            None => {
                controller.add_log("There's nothing to install for that - no fixed version is known", Hue::DarkGray)
            }
        },
        "I" => match controller.apps.get(*app).map(|the_app| fixable(controller, the_app, Severity::Info).len()) {
            Some(count) if count > 0 => *batch = true,
            _ => controller.add_log("Nothing vulnerable here has a fix to install", Hue::DarkGray),
        },
        "y" | "u" => copy_fix(ui, controller),
        "o" => open_advisory(ui, controller),
        "Escape" if *packages => *packages = false,
        "Escape" if !filter.is_empty() => filter.clear(),
        "Escape" | "q" | "A" => ui.overlay = None,
        _ => return false,
    }
    true
}

/// An app's vulnerable packages at `floor` or worse that have a fix, the worst first.
fn fixable(controller: &Controller, app: &App, floor: Severity) -> Vec<Package> {
    let Some(scan) = controller.audits().get(&app.key) else { return Vec::new() };
    let fixes = |package: &&Package| package.severity.is_some_and(|severity| severity >= floor);
    scan.packages.iter().filter(fixes).filter(|package| audit::command(app, package).is_some()).cloned().collect()
}

/// The picked package, with its app.
fn picked(ui: &Ui, controller: &Controller) -> Option<(App, Package)> {
    let Some(Overlay::Audit { app, index, only, filter, .. }) = &ui.overlay else { return None };
    let package = shown(controller, *app, *only, filter).get(*index).map(|package| (*package).clone())?;
    Some((controller.apps.get(*app)?.clone(), package))
}

/// y: the picked package's fix, or the upgrade to its latest version, for its app's folder; the command lights up.
fn copy_fix(ui: &mut Ui, controller: &mut Controller) {
    let Some((app, package)) = picked(ui, controller) else { return };
    match audit::command(&app, &package) {
        Some(command) => {
            let what = format!("the fix for {} - run it in {}", package.name, folder(controller, &app));
            super::copy::put(ui.clipboard, controller, &command, &what);
            let now = ui.now;
            if let Some(Overlay::Audit { copied, .. }) = &mut ui.overlay {
                *copied = Some(now);
            }
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
    let Some(Overlay::Audit { app, index, packages, confirm, batch, .. }) = &mut ui.overlay else { return false };
    match target {
        Some(Target::Item(at)) => (*app, *index, *packages) = (at, 0, false),
        Some(Target::Package(at)) => (*index, *packages) = (at, true),
        _ => return false,
    }
    (*confirm, *batch) = (false, false);
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

/// How long ago `at` was, while it's within `within` and motion runs.
fn within(ui: &Ui, at: Option<Instant>, within: Duration) -> Option<Duration> {
    let since = ui.now.saturating_duration_since(at?);
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

/// Whether the view moves: its rows still coming in, a scan just landed, a command just copied, a fix just failed.
pub fn animating(ui: &Ui, controller: &Controller) -> bool {
    let Some(Overlay::Audit { opened, copied, .. }) = &ui.overlay else { return false };
    let failed = controller.audit_install_failed().map(|(.., at)| at);
    within(ui, Some(*opened), STAGGER_ALL).is_some()
        || within(ui, *copied, COPY_FLASH).is_some()
        || within(ui, failed, FAILED).is_some()
        || controller.apps.iter().any(|app| landed(ui, controller, &app.key, SWEEP.max(PULSE)).is_some())
}

/// A severity as the table and the details show it, in capitals: a critical one filled, the others framed.
fn pill(severity: Severity) -> Seg {
    let name = severity.name().to_uppercase();
    match severity {
        Severity::Critical => on(format!(" {name} "), C::Black, C::Red),
        Severity::High => seg(format!("▕{name}▏"), C::Red),
        Severity::Moderate => seg(format!("▕{name}▏"), C::Yellow),
        Severity::Low => seg(format!("▕{name}▏"), C::DarkGray),
        Severity::Info => seg(format!("▕{name}▏"), C::Cyan),
    }
}

/// A severity in the apps' list and the header: in its colour, a critical one filled.
fn badge(severity: Severity, text: &str) -> Seg {
    match severity {
        Severity::Critical => on(format!(" {text} "), C::Black, C::Red),
        Severity::High => seg(text, C::Red),
        Severity::Moderate => seg(text, C::Yellow),
        Severity::Low => seg(text, C::DarkGray),
        Severity::Info => seg(text, C::Cyan),
    }
}

/// A pulse: shown inverted while it's on.
fn pulsed(mut shown: Seg, on: bool) -> Seg {
    if on {
        (shown.fg, shown.bg) = (C::Black, Some(C::White));
    }
    shown
}

fn spin(ui: &Ui) -> &str {
    if ui.reduced_motion { "●" } else { spinner(ui) }
}

/// The view's first row: the project, what the scans found (a count pulsing as a scan adds to it), and what's
/// installing or scanning - with the batch counted off - or when the last scan was, coloured by its age.
fn header(ui: &Ui, controller: &Controller) -> Vec<Seg> {
    let mut header = vec![seg(format!("  {}", controller.project.name), C::White)];
    let scans = controller.audits();
    let all: Vec<&Package> = scans.values().flat_map(|scan| &scan.packages).collect();
    let fresh: Vec<&AppAudit> = controller
        .apps
        .iter()
        .filter(|app| landed(ui, controller, &app.key, PULSE).is_some())
        .filter_map(|app| scans.get(&app.key))
        .collect();
    for severity in [Severity::Critical, Severity::High] {
        let count = all.iter().filter(|package| package.severity == Some(severity)).count();
        if count == 0 {
            continue;
        }
        let added = fresh.iter().any(|scan| {
            scan.packages.iter().any(|package| package.severity == Some(severity) && scan.new.contains(&package.name))
        });
        header.extend([seg("  ", C::Gray), pulsed(badge(severity, &format!("{count} {}", severity.name())), added)]);
    }
    if !scans.is_empty() {
        let plural = if all.len() == 1 { "" } else { "s" };
        header.push(seg(format!("  {} package{plural}", all.len()), C::Gray));
    }
    if let Some((_, command)) = controller.audit_installing() {
        header.push(seg(format!(" · {} installing: {command}", spin(ui)), C::Yellow));
    } else if !controller.auditing().is_empty() {
        let names: Vec<&str> = controller
            .auditing()
            .iter()
            .map(|key| controller.app(key).map_or(key.as_str(), |app| app.name.as_str()))
            .collect();
        header.push(seg(format!(" · {} scanning {}", spin(ui), names.join(", ")), C::Yellow));
        if let Some((done, total)) = controller.audit_batch().filter(|(_, total)| *total > 1) {
            header.push(seg(format!(" · {done} of {total} done "), C::Yellow));
            if total <= BATCH_BAR {
                header.extend([seg("█".repeat(done), C::Yellow), seg("░".repeat(total - done), C::DarkGray)]);
            }
        }
    } else if let Some(at) = scans.values().map(|scan| scan.at).max() {
        let days = (chrono::Local::now().timestamp() - at) / 86_400;
        let (color, nudge) = match days {
            0 => (C::Green, ""),
            _ if days < OLD_DAYS => (C::Gray, ""),
            _ if days < STALE_DAYS => (C::Yellow, ""),
            _ => (C::Red, " · R rescans"),
        };
        header.push(seg(format!(" · last scan {}{nudge}", audit::age(at)), color));
    }
    header.push(seg(format!(" · {} at once (+ -)", controller.audit_parallelism()), C::DarkGray));
    header
}

fn hint(packages: bool, only: bool, typing: bool) -> String {
    if typing {
        return " type to filter the packages by name · Enter keeps it · Esc drops it".into();
    }
    let only = if only { "v everything" } else { "v vulnerable" };
    if packages {
        format!(
            " i install · I install many · y copy · Enter advisory · e details · / filter · {only} · r rescan · h apps ·              Esc"
        )
    } else {
        format!(" j k choose · l its packages · I install many · r scan it · R scan all · / filter · {only} · Esc back")
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

/// The question before `I` installs many fixes together: how many each choice takes, the keys first; how many are a
/// new major, and that the app stops.
fn batch_question(controller: &Controller, app: usize, view_width: usize) -> Vec<Seg> {
    let Some(app) = controller.apps.get(app) else { return Vec::new() };
    let count = |floor| fixable(controller, app, floor).len();
    let all = fixable(controller, app, Severity::Info);
    let major = |package: &&Package| {
        package.fix.as_ref().is_some_and(|fix| fix.major) || (package.fix.is_none() && package.major_behind())
    };
    let majors = match all.iter().filter(major).count() {
        0 => String::new(),
        majors => format!(", {majors} a new major - check what changed"),
    };
    let stops = if controller.is_running(&app.key) { "; it stops the app and starts it again" } else { "" };
    let text = format!(
        " c critical ({}) · h high and up ({}) · m moderate and up ({}) · a all ({}) · n none - installs {}'s fixes \
         together{majors}{stops} ",
        count(Severity::Critical),
        count(Severity::High),
        count(Severity::Moderate),
        all.len(),
        app.name
    );
    vec![on(pad(&text, view_width), C::Black, C::Yellow)]
}

/// What an app's scan says, in a few cells; a new high or critical finding pulses once as the scan lands.
fn state(ui: &Ui, controller: &Controller, key: &str) -> Seg {
    if controller.auditing().iter().any(|scanning| scanning == key) {
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
        let first_found = worst >= Severity::High && !scan.new_advisories.is_empty();
        let pulse = first_found && landed(ui, controller, key, PULSE).is_some_and(|since| since < PULSE / 2);
        return pulsed(badge(worst, &format!("{} {}", scan.count(worst), worst.name())), pulse);
    }
    match scan.packages.iter().filter(|package| package.outdated()).count() {
        0 => seg("up to date", C::Green),
        outdated => seg(format!("↑ {outdated} outdated"), C::Gray),
    }
}

/// The apps, each with what its scan says - the selected one lit while the apps have the keys - and a note on how
/// they're counted.
fn app_rows(ui: &Ui, controller: &Controller, selected: usize, list_width: usize, focus: bool) -> Vec<Vec<Seg>> {
    let apps = controller.apps.iter().enumerate();
    let mut rows: Vec<Vec<Seg>> = apps
        .map(|(at, app)| {
            let state = state(ui, controller, &app.key);
            let icon = icons::app(ui.nerd, app, controller.framework(&app.key));
            let room = list_width.saturating_sub(state.text.chars().count() + 7).max(4);
            let marker = if at == selected { "▸" } else { " " };
            let mut row = vec![
                seg(format!(" {marker} {icon} "), C::Gray),
                seg(pad(&app.name, room), C::White),
                seg(" ", C::Gray),
            ];
            row.push(state);
            if at == selected && !focus {
                for part in row.iter_mut().filter(|part| part.bg.is_none()) {
                    (part.fg, part.bg) = (C::FocusText, Some(C::FocusBack));
                }
            }
            aim(row, Target::Item(at))
        })
        .collect();
    rows.push(Vec::new());
    let note = super::overlays::words(FOOTNOTE, list_width.saturating_sub(2));
    rows.extend(note.into_iter().map(|piece| vec![seg(format!(" {piece}"), C::DarkGray)]));
    rows
}

/// A package row's left edge: its severity's colour (none for an outdated one), or the accent on a changed row with
/// reduced motion, which doesn't light it up.
fn edge(package: &Package, accent: bool) -> Seg {
    let color = match package.severity {
        _ if accent => C::Cyan,
        Some(Severity::Critical | Severity::High) => C::Red,
        Some(Severity::Moderate) => C::Yellow,
        Some(Severity::Low | Severity::Info) => C::DarkGray,
        None => return seg(" ", C::Gray),
    };
    seg("▌", color)
}

/// `name` with what `/` typed lit in it.
fn highlighted(name: &str, filter: &str, color: C) -> Vec<Seg> {
    let (lower, wanted) = (name.to_lowercase(), filter.to_lowercase());
    let found = (!filter.is_empty() && lower.len() == name.len()).then(|| lower.find(&wanted)).flatten();
    let bounds = found.map(|at| (at, at + wanted.len()));
    match bounds.filter(|(at, end)| name.is_char_boundary(*at) && name.is_char_boundary(*end)) {
        Some((at, end)) => {
            vec![seg(&name[..at], color), on(name[at..end].to_string(), C::Black, C::Yellow), seg(&name[end..], color)]
        }
        None => vec![seg(name, color)],
    }
}

/// `segs` in exactly `cells`, a `…` where they're cut.
fn cut(segs: Vec<Seg>, cells: usize) -> Vec<Seg> {
    if width(&segs) <= cells {
        return fit(segs, cells);
    }
    let mut out = fit(segs, cells.saturating_sub(1));
    out.push(seg("…", C::DarkGray));
    out
}

/// `row` with its first `cells` cells in `fg` on `back` - a fill sweeping left to right.
fn swept(row: Vec<Seg>, cells: usize, fg: C, back: C) -> Vec<Seg> {
    let mut left = cells;
    let mut out = Vec::new();
    for part in row {
        let count = part.text.chars().count();
        if left == 0 {
            out.push(part);
        } else if count <= left {
            left -= count;
            out.push(Seg { fg, bg: Some(back), ..part });
        } else {
            let head: String = part.text.chars().take(left).collect();
            let tail: String = part.text.chars().skip(left).collect();
            out.push(Seg { text: head, fg, bg: Some(back), ..part.clone() });
            out.push(Seg { text: tail, ..part });
            left = 0;
        }
    }
    out
}

/// The picked row lit across (but its edge), as the selection bar is.
fn light(row: &mut [Seg]) {
    for part in row.iter_mut().skip(1).filter(|part| part.bg.is_none()) {
        part.bg = Some(C::FocusBack);
        if matches!(part.fg, C::Gray | C::DarkGray | C::White) {
            part.fg = C::FocusText;
        }
    }
}

/// How the table looks right now.
struct Look {
    /// Dimmed: its app is being scanned.
    dim: bool,
    /// How long ago its app's scan landed, while that moves.
    landed: Option<Duration>,
    /// Reduced motion: a changed row keeps an accent edge.
    edge: bool,
    spin: String,
    /// The packages whose fixes are being installed, and those whose fixes just failed (and how long ago).
    installing: Vec<String>,
    failed: Option<(Vec<String>, Duration)>,
    /// `e`: each vulnerable package's advisory on a line under it.
    expanded: bool,
}

/// The table: its column titles, then a package a row - an edge in its severity's colour, its name (what `/` typed
/// lit) with what it comes in through and `new` when the scan before didn't have it, its versions, its severity framed
/// (or `↑ outdated only`, the row dimmed; or a spinner while its fix installs) and whether the app references it itself
/// - and what the last scan fixed, as `resolved ✓`.
fn table_rows(
    list: &[&Package],
    scan: Option<&AppAudit>,
    filter: &str,
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
    let landing = look.landed.filter(|since| *since < LAND);
    for (at, package) in list.iter().enumerate() {
        let changed = new.contains(&package.name);
        let installing = look.installing.contains(&package.name);
        let failed = look.failed.as_ref().filter(|(names, _)| names.contains(&package.name)).map(|(_, since)| *since);
        let vulnerable = package.severity.is_some();
        let mut name = vec![seg(" ", C::Gray)];
        name.extend(highlighted(&package.name, filter, if vulnerable { C::White } else { C::DarkGray }));
        if let Some(first) = package.via.first() {
            let more = if package.via.len() > 1 { format!(" +{}", package.via.len() - 1) } else { String::new() };
            name.push(seg(format!(" via {first}{more}"), C::DarkGray));
        }
        // The name, cut with … where it doesn't fit, and a gap before the versions.
        let mut row = vec![edge(package, changed && look.edge)];
        row.extend(cut(name, name_cells.saturating_sub(2)));
        row.push(seg(" ", C::Gray));
        let latest = package.latest.clone().filter(|_| package.outdated()).unwrap_or_default();
        let latest_color = match () {
            _ if package.major_behind() => C::Yellow,
            _ if vulnerable => C::Gray,
            _ => C::DarkGray,
        };
        let current_color = if vulnerable { C::Gray } else { C::DarkGray };
        row.extend([
            seg(pad(&package.current, VERSION_CELLS), current_color),
            seg(pad(&latest, VERSION_CELLS), latest_color),
        ]);
        let mut severity = match package.severity {
            _ if installing => vec![seg(format!(" {} installing", look.spin), C::Yellow)],
            Some(severity) => vec![pill(severity)],
            None => vec![seg(" ↑ outdated only", C::DarkGray)],
        };
        // A finding the last scan added: its `new` chip after the severity, where it hides nothing.
        if changed && !installing {
            let blink_off = landing.is_some_and(|since| since >= BLINK && since < BLINK * 2);
            severity.push(seg(" ", C::Gray));
            severity.push(if blink_off { seg(NEW, C::Yellow) } else { on(NEW, C::Black, C::Yellow) });
        }
        row.extend(fit(severity, SEVERITY_CELLS));
        row.push(seg(pad(if package.direct { "direct" } else { "transitive" }, KIND_CELLS), C::DarkGray));
        let red = failed.is_some_and(|since| (since.as_millis() / (FAILED.as_millis() / 4)).is_multiple_of(2));
        for part in row.iter_mut().skip(1).filter(|part| part.bg.is_none()) {
            if red {
                (part.fg, part.bg) = (C::Black, Some(C::Red));
            } else if changed && landing.is_some() {
                part.bg = Some(C::DarkCyan);
                if matches!(part.fg, C::Gray | C::DarkGray | C::White) {
                    part.fg = C::FocusText;
                }
            } else if look.dim {
                part.fg = C::DarkGray;
            }
        }
        rows.push(aim(row, Target::Package(at)));
        if look.expanded
            && let Some(advisory) = package.advisories.first()
        {
            let what = if advisory.title.is_empty() { advisory.id.clone() } else { advisory.title.clone() };
            let fix = package.fix.as_ref().filter(|fix| !fix.version.is_empty());
            let fix = fix.map_or(String::new(), |fix| format!(" · fixed in {}", fix.version));
            let more = match package.advisories.len() {
                1 => String::new(),
                count => format!(" · +{} more", count - 1),
            };
            let line = vec![seg("    └ ", C::DarkGray), seg(format!("{what}{fix}{more}"), C::DarkGray)];
            rows.push(aim(line, Target::Package(at)));
        }
    }
    let resolved = scan.into_iter().flat_map(|scan| &scan.fixed).filter(|fixed| named(&fixed.name, filter));
    for fixed in resolved {
        let row = vec![
            seg(" ", C::Gray),
            seg(pad(&format!(" {}", fixed.name), name_cells - 1), C::Gray),
            seg(pad(&fixed.from, VERSION_CELLS), C::DarkGray),
            seg(pad(fixed.to.as_deref().unwrap_or(""), VERSION_CELLS), C::Gray),
            seg(pad(" resolved ✓", SEVERITY_CELLS), C::Green),
        ];
        rows.push(match look.landed.filter(|since| *since < SWEEP) {
            Some(since) => {
                let filled = (since.as_secs_f64() / SWEEP.as_secs_f64() * table_width as f64).round() as usize;
                swept(row, filled, C::Black, C::Green)
            }
            None => row,
        });
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
/// what `i` will run (lit up just after `y` copied it). What's longer than the pane wraps at its words.
fn details(
    controller: &Controller,
    app: &App,
    package: Option<&Package>,
    detail_width: usize,
    copied: bool,
) -> Vec<Vec<Seg>> {
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
            vec![seg(" ", C::Gray), pill(severity), seg(format!(" {} · {how}", advisory.id), C::DarkGray)]
        }
        (Some(severity), None) => vec![seg(" ", C::Gray), pill(severity), seg(format!(" {how}"), C::DarkGray)],
        (None, _) => vec![seg(" ↑ outdated only", C::Gray), seg(format!(" · {how}"), C::DarkGray)],
    });
    let title =
        |out: &mut Vec<Vec<Seg>>, text: &str| out.extend([Vec::new(), vec![seg(format!(" {text}"), C::DarkCyan)]]);
    let label = |out: &mut Vec<Vec<Seg>>, text: &str| out.push(vec![seg(format!(" {text}"), C::DarkGray)]);
    let value = |out: &mut Vec<Vec<Seg>>, text: &str, color: C| {
        out.extend(chunk(text, room).into_iter().map(|piece| vec![seg(format!(" {piece}"), color)]));
    };
    let wrapped = |out: &mut Vec<Vec<Seg>>, text: &str, color: C| {
        out.extend(super::overlays::words(text, room).into_iter().map(|piece| vec![seg(format!(" {piece}"), color)]));
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
            let tag = pill(advisory.severity);
            let indent = 1 + width(std::slice::from_ref(&tag));
            let pieces = super::overlays::words(&text, room.saturating_sub(indent).max(10));
            for (at, piece) in pieces.into_iter().enumerate() {
                let mut line =
                    if at == 0 { vec![seg(" ", C::Gray), tag.clone()] } else { vec![seg(" ".repeat(indent), C::Gray)] };
                line.push(seg(format!(" {piece}"), C::Gray));
                out.push(line);
            }
            if new.contains(&advisory.id) {
                let chip = on(NEW, C::Black, C::Yellow);
                match out.last_mut() {
                    Some(line) if width(line) + width(std::slice::from_ref(&chip)) <= room + 1 => line.push(chip),
                    _ => out.push(vec![seg(" ".repeat(indent + 1), C::Gray), chip]),
                }
            }
        }
    }
    match audit::command(app, package) {
        Some(command) => {
            title(&mut out, "WHAT `i` WILL RUN");
            for piece in chunk(&command, room) {
                let text = format!(" {piece}");
                out.push(vec![if copied { on(text, C::Black, C::Cyan) } else { seg(text, C::Cyan) }]);
            }
            wrapped(&mut out, &format!("in {} · y copies it", folder(controller, app)), C::DarkGray);
            if package.fix.as_ref().is_some_and(|fix| fix.major) || (package.fix.is_none() && package.major_behind()) {
                wrapped(&mut out, "a new major version: check what changed first", C::Yellow);
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

/// What the view draws: its rows but the selection bar, and on which row of the table it goes.
struct Layout {
    header: Vec<Seg>,
    keys: Vec<Vec<Seg>>,
    titles: Vec<Seg>,
    left: Vec<Vec<Seg>>,
    /// The table as it shows, cut to its room.
    table: Vec<Vec<Seg>>,
    details: Vec<Vec<Seg>>,
    /// The picked package's row in `table`, while the packages have the keys.
    picked: Option<usize>,
    beside: bool,
    list_width: usize,
    right: usize,
    table_width: usize,
    height: usize,
}

/// The whole view: the header, the keys (or the question before an install, or the filter being typed), the panes'
/// titles, then the apps beside the table and the picked package's details (beside the table when there's room,
/// under it when not), the selection bar gliding to the picked row.
pub fn rows(ui: &mut Ui, controller: &Controller, view_width: usize, rows: usize) -> Vec<Vec<Seg>> {
    let target = layout(ui, controller, view_width, rows).and_then(|drawn| drawn.picked);
    let bar = target.map(|line| ui.motion.bar(line, ui.now));
    let Some(mut drawn) = layout(ui, controller, view_width, rows) else { return Vec::new() };
    if let Some(row) = bar.and_then(|line| drawn.table.get_mut(line)) {
        light(row);
    }
    compose(drawn, view_width)
}

fn layout(ui: &Ui, controller: &Controller, view_width: usize, rows: usize) -> Option<Layout> {
    let Some(Overlay::Audit { app, index, packages, only, confirm, batch, expanded, filter, typing, copied, .. }) =
        &ui.overlay
    else {
        return None;
    };
    let app_at = controller.apps.len().checked_sub(1).map(|last| (*app).min(last))?;
    let the_app = &controller.apps[app_at];
    let scan = scan_of(controller, app_at);
    let list = shown(controller, app_at, *only, filter);
    let index = (*index).min(list.len().saturating_sub(1));
    let list_width = (view_width / 4).clamp(LIST_WIDTH.0, LIST_WIDTH.1).min(view_width.saturating_sub(30));
    let right = view_width.saturating_sub(list_width + 3);
    let beside = right >= BESIDE_FROM;
    let table_width = if beside { right.saturating_sub(DETAIL_WIDTH + 3) } else { right };
    let keys = if *batch {
        vec![batch_question(controller, app_at, view_width)]
    } else if *confirm {
        vec![question(controller, app_at, list.get(index).copied(), view_width)]
    } else {
        super::viewbar::keys(&hint(*packages, *only, *typing), view_width)
    };
    let height = rows.saturating_sub(2 + keys.len());
    let failed = controller.audit_install_failed().filter(|(key, ..)| *key == the_app.key);
    let look = Look {
        dim: moving(ui) && controller.auditing().contains(&the_app.key),
        landed: landed(ui, controller, &the_app.key, SWEEP.max(LAND)),
        edge: ui.reduced_motion,
        spin: spin(ui).to_string(),
        installing: match controller.audit_installing() {
            Some((key, _)) if key == the_app.key => controller.audit_installing_names().to_vec(),
            _ => Vec::new(),
        },
        failed: failed.and_then(|(.., names, at)| Some((names.to_vec(), within(ui, Some(at), FAILED)?))),
        expanded: *expanded,
    };
    let mut table = table_rows(&list, scan, filter, &look, table_width);
    table.truncate(arrived(ui, table.len()).max(1));
    if let Some(error) = scan.and_then(|scan| scan.error.as_ref()) {
        table.insert(0, vec![seg(format!("  ✗ the last scan failed: {error} - r tries again"), C::Red)]);
    }
    if list.is_empty() {
        table.extend(empty(controller, the_app, *only, filter));
    }
    let copied = within(ui, *copied, COPY_FLASH).is_some();
    let detail_width = if beside { DETAIL_WIDTH } else { right };
    let details = details(controller, the_app, list.get(index).copied(), detail_width, copied);
    let room = if beside {
        height
    } else {
        // The table takes the rows it needs (up to what leaves the details half), the details the rest.
        let most = height.saturating_sub(details.len().min(height / 2) + 2).max(3);
        table.len().clamp(3, most)
    };
    // The picked package's row, wherever the details lines put it.
    let mark = Some(Target::Package(index));
    let at = table.iter().position(|row| row.iter().any(|part| part.target == mark)).unwrap_or(index + 1);
    let table = window(&table, at, room);
    let mark = Some(Target::Package(index));
    let picked = if *packages { table.iter().position(|row| row.iter().any(|part| part.target == mark)) } else { None };
    let mut packages_title = vec![seg(format!(" PACKAGES · {}", the_app.name.to_uppercase()), C::DarkCyan)];
    packages_title.push(match (*typing, filter.is_empty()) {
        (false, true) => seg("   / filter", C::DarkGray),
        (typing, _) => seg(format!("   / {filter}{}", if typing { ui.cursor() } else { "" }), C::Yellow),
    });
    let titles = if beside {
        let mut line = fit(packages_title, table_width);
        line.extend([seg(" │ ", C::DarkGray), seg(" DETAILS", C::DarkCyan)]);
        line
    } else {
        packages_title
    };
    Some(Layout {
        header: header(ui, controller),
        keys,
        titles,
        left: app_rows(ui, controller, app_at, list_width, *packages),
        table,
        details,
        picked,
        beside,
        list_width,
        right,
        table_width,
        height,
    })
}

fn compose(drawn: Layout, view_width: usize) -> Vec<Vec<Seg>> {
    let Layout { header, keys, titles, left, table, details, beside, list_width, right, table_width, height, .. } =
        drawn;
    let right_rows: Vec<Vec<Seg>> = if beside {
        (0..height)
            .map(|row| {
                let mut line = fit(table.get(row).cloned().unwrap_or_default(), table_width);
                line.push(seg(" │ ", C::DarkGray));
                line.extend(fit(details.get(row).cloned().unwrap_or_default(), DETAIL_WIDTH));
                line
            })
            .collect()
    } else {
        let mut stacked = table;
        stacked.extend([vec![seg("─".repeat(right), C::DarkGray)], vec![seg(" DETAILS", C::DarkCyan)]]);
        stacked.extend(details);
        stacked
    };
    let mut pane_titles = fit(vec![seg(" APPS", C::DarkCyan)], list_width);
    pane_titles.push(seg(" │ ", C::DarkGray));
    pane_titles.extend(titles);
    let mut out = vec![fit(header, view_width)];
    out.extend(keys.into_iter().map(|row| fit(row, view_width)));
    out.push(fit(pane_titles, view_width));
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

    fn texts(row: &[Seg]) -> Vec<&str> {
        row.iter().map(|part| part.text.as_str()).collect()
    }

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

    #[test]
    fn the_filter_lights_its_match_and_a_fill_sweeps_from_the_left() {
        assert_eq!(texts(&highlighted("Newtonsoft.Json", "JSON", C::White)), ["Newtonsoft.", "Json", ""]);
        assert_eq!(texts(&highlighted("lodash", "", C::White)), ["lodash"]);
        let row = vec![seg("ab", C::Gray), seg("cdef", C::Gray)];
        let filled = swept(row, 3, C::Black, C::Green);
        assert_eq!(texts(&filled), ["ab", "c", "def"]);
        let lit: Vec<bool> = filled.iter().map(|part| part.bg == Some(C::Green)).collect();
        assert_eq!(lit, [true, true, false], "the first three cells");
    }
}
