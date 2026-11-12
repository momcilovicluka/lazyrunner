//! P (or `!` while no port is in the way): the port map (#81), full screen - every port the apps want and who really
//! holds it, read from the system. Its header counts the conflicts and the keys follow it, as in the other views; an
//! occupancy strip gives every port at a glance, a cell each in the table's order, and a status line says what's
//! wrong; then the table - port, app, the process holding it (PID and command line), its state (`●` free, `◐` its own
//! app, `×` something else, `∅` a range Windows reserved), for how long, who held it over the last minute (with room),
//! and what to do about it - the conflicts first. The ranges Windows reserved are listed under it.
//!
//! `!` stops what holds the picked port (after a yes) - the row dims and a spinner runs until the port is free - `Enter`
//! goes to its app's column, `w` opens it in the browser, `r` reads the ports again and `y` copies what frees the
//! reserved ranges. While the view is closed, a stopped app's port that something else took puts a chip in the panel's
//! header (`! 5001 taken`); a click opens the view on it.
//!
//! It moves (not with reduced motion): the strip fills left to right and the rows come in one after another as it
//! opens; a port that changes flashes its row's state and its cells; a new conflict slides in with a red edge that
//! pulses twice; one being asked about pulses until the answer; a freed port wipes out who held it while `● freed`
//! draws in, then flashes green. The header's chip lights up once as it appears.

use std::time::{Duration, Instant};

use lazyrunner_core::output::Hue;
use lazyrunner_core::run::Controller;
use lazyrunner_core::run::ports::{CELLS, Holder, Holding, Port, Ports, TAKEN_AFTER};

use super::checklist::WINNAT;
use super::columns::{fit, spinner};
use super::icons;
use super::motion::COPY_FLASH;
use super::mouse::Target;
use super::seg::{Seg, aim, on, pad, seg, take_cells, width};
use super::theme::C;
use super::{Overlay, Ui};

/// The table's columns: the port, the app, the state, how long (and the gaps); who holds it gets the rest, at least
/// `HOLDER_CELLS` - then, with room, what to do and the last minute.
const PORT_CELLS: usize = 5;
const NAME_CELLS: usize = 18;
const STATE_CELLS: usize = 17;
const FOR_CELLS: usize = 5;
const HOLDER_CELLS: usize = 20;
const ACTION_CELLS: usize = 28;
const FIXED_CELLS: usize = 2 + PORT_CELLS + 2 + NAME_CELLS + 2 + 2 + STATE_CELLS + 2 + FOR_CELLS;
/// A reserved range's hatched band.
const BAND_CELLS: usize = 10;
/// A change flashes this long; a new conflict slides in over `SLIDE` and pulses its edge twice in `PULSE`; a freed
/// port's wipe takes `WIPE` (who held it out, then `freed` in), then it flashes green, and says `freed` for `FREED`.
const FLASH: Duration = Duration::from_millis(150);
const SLIDE: Duration = Duration::from_millis(200);
const PULSE: Duration = Duration::from_millis(400);
const PULSE_STEP: u128 = 100;
const WIPE: Duration = Duration::from_millis(300);
const FREED: Duration = Duration::from_millis(1500);
/// As the view opens the strip's cells and the rows come in this far apart, all within `STAGGER_ALL`.
const STAGGER: Duration = Duration::from_millis(25);
const STAGGER_ALL: Duration = Duration::from_millis(250);
/// The header's chip lights up this long as it appears.
const POP: Duration = Duration::from_millis(300);
/// A port being freed spins at most this long: then it's shown as it is.
const STOPPING: Duration = Duration::from_secs(5);

/// P, `!` with no port in the way, or a click on the header's chip (`port` its port): the view, on that port, else on
/// the first conflict. The ports and the ranges Windows reserved are read again.
pub fn open(ui: &mut Ui, controller: &mut Controller, port: Option<u16>) {
    controller.rescan_ports();
    let shown = shown(controller.port_map());
    let index = port.and_then(|port| shown.iter().position(|row| row.port == port)).unwrap_or(0);
    ui.overlay = Some(Overlay::Ports { index, confirm: false, copied: None, opened: ui.now, stopping: None });
}

/// The header chip's click: the view, on the first port taken.
pub fn open_taken(ui: &mut Ui, controller: &mut Controller) {
    let port = controller.port_map().taken(ui.now).first().map(|row| row.port);
    open(ui, controller, port);
}

/// The app whose port is picked in the view, by its key.
pub(super) fn picked_app(ui: &Ui, controller: &Controller) -> Option<String> {
    let Some(Overlay::Ports { index, .. }) = &ui.overlay else { return None };
    shown(controller.port_map()).get(*index).map(|row| row.key.clone())
}

/// The ports as the table lists them: the conflicts first, then the rest in the apps' order.
fn shown(map: &Ports) -> Vec<&Port> {
    let (mut first, rest): (Vec<&Port>, Vec<&Port>) = map.rows.iter().partition(|row| row.holding == Holding::Other);
    first.extend(rest);
    first
}

/// The header's chip while a stopped app's port has been taken (`! 5001 taken`, and how many more), and whether it's
/// lighting up as it appears.
pub fn chip(ui: &Ui, controller: &Controller) -> Option<(String, bool)> {
    if matches!(ui.overlay, Some(Overlay::Ports { .. })) {
        return None;
    }
    let taken = controller.port_map().taken(ui.now);
    let first = taken.first()?;
    let more = if taken.len() > 1 { format!(" +{}", taken.len() - 1) } else { String::new() };
    let popping = within(ui, Some(first.since + TAKEN_AFTER), POP).is_some();
    Some((format!("! {} taken{more}", first.port), popping))
}

/// What the view keeps: the picked row, whether `!` asks, when `y` copied, when it opened, the port being freed.
struct View {
    index: usize,
    confirm: bool,
    copied: Option<Instant>,
    opened: Instant,
    stopping: Option<(String, Instant)>,
}

fn view(ui: &Ui) -> Option<View> {
    let Some(Overlay::Ports { index, confirm, copied, opened, stopping }) = &ui.overlay else { return None };
    Some(View { index: *index, confirm: *confirm, copied: *copied, opened: *opened, stopping: stopping.clone() })
}

fn with(ui: &mut Ui, change: impl FnOnce(&mut usize, &mut bool, &mut Option<Instant>, &mut Option<(String, Instant)>)) {
    if let Some(Overlay::Ports { index, confirm, copied, stopping, .. }) = &mut ui.overlay {
        change(index, confirm, copied, stopping);
    }
}

/// j k move, `!` stops what holds the picked port (after a yes: y or Enter, n or Esc not), Enter goes to its app's
/// column, w opens it in the browser, r reads the ports again, y copies what frees the reserved ranges, Esc q P
/// close; false for a key it doesn't use.
pub fn key(ui: &mut Ui, controller: &mut Controller, name: &str) -> bool {
    let Some(state) = view(ui) else { return false };
    let rows = shown(controller.port_map());
    let count = rows.len();
    let picked = rows.get(state.index.min(count.saturating_sub(1))).map(|row| (*row).clone());
    if state.confirm {
        match name {
            "y" | "Enter" => {
                let now = ui.now;
                let key = picked.as_ref().map(|row| row.key.clone());
                let port = picked.as_ref().map(|row| row.port);
                with(ui, |_, confirm, _, stopping| {
                    *confirm = false;
                    *stopping = key.clone().map(|key| (key, now));
                });
                if let (Some(key), Some(port)) = (key, port) {
                    controller.free_port(&key, port);
                }
            }
            "n" | "Escape" | "q" => with(ui, |_, confirm, _, _| *confirm = false),
            _ => {}
        }
        return true;
    }
    match name {
        "j" | "DownArrow" | "k" | "UpArrow" => {
            let step = if matches!(name, "j" | "DownArrow") { 1 } else { -1 };
            with(ui, |index, _, _, _| *index = super::keys::wrapped(*index, step, count.max(1)));
        }
        "!" => match &picked {
            Some(row) if row.holding == Holding::Other => with(ui, |_, confirm, _, _| *confirm = true),
            Some(row) => {
                let text = format!("Nothing else holds port {} - ! stops what took an app's port", row.port);
                controller.add_log(text, Hue::DarkGray);
            }
            None => {}
        },
        "Enter" => {
            if let Some(at) = picked.and_then(|row| controller.apps.iter().position(|app| app.key == row.key)) {
                ui.focus = at;
                ui.overlay = None;
            }
        }
        "w" => {
            if let Some(row) = &picked {
                super::links::open_url(ui, controller, &format!("http://localhost:{}", row.port));
            }
        }
        "r" => controller.rescan_ports(),
        "y" => copy_fix(ui, controller),
        "Escape" | "q" | "P" => ui.overlay = None,
        _ => return false,
    }
    true
}

/// y: what frees the ranges Windows reserved, to paste into an elevated terminal; the status line says it was copied.
fn copy_fix(ui: &mut Ui, controller: &mut Controller) {
    if controller.port_map().reserved.is_empty() {
        controller.add_log("Windows reserved no ports - there's nothing to free", Hue::DarkGray);
        return;
    }
    let what = "what frees the ports Windows reserved - paste it into an elevated terminal";
    super::copy::put(ui.clipboard, controller, WINNAT, what);
    let now = ui.now;
    with(ui, |_, _, copied, _| *copied = Some(now));
}

/// A click on a port picks it, a double click goes to its app's column.
pub fn click(ui: &mut Ui, controller: &mut Controller, target: Option<Target>, double: bool) -> bool {
    if view(ui).is_none() {
        return false;
    }
    let Some(Target::Item(at)) = target else { return false };
    with(ui, |index, confirm, _, _| (*index, *confirm) = (at, false));
    if double {
        key(ui, controller, "Enter");
    }
    true
}

/// Whether its motion runs: not with reduced motion, nor in the screen tests' frozen frames.
fn moving(ui: &Ui) -> bool {
    ui.motion.enabled && !ui.reduced_motion
}

/// How long ago `at` was, while it's within `within` and motion runs.
fn within(ui: &Ui, at: Option<Instant>, within: Duration) -> Option<Duration> {
    let since = ui.now.saturating_duration_since(at?);
    (moving(ui) && since < within).then_some(since)
}

/// The port being freed, while it still is (its row spins).
fn stopping<'a>(ui: &Ui, state: &'a View, row: &Port) -> Option<&'a str> {
    let (key, at) = state.stopping.as_ref()?;
    let still = *key == row.key && row.holding == Holding::Other && ui.now.saturating_duration_since(*at) < STOPPING;
    still.then_some(key.as_str())
}

/// Whether the view (or the header's chip) moves: opening, a port that changed, a question, a port being freed.
pub fn animating(ui: &Ui, controller: &Controller) -> bool {
    let map = controller.port_map();
    let popping = map.taken(ui.now).first().is_some_and(|row| within(ui, Some(row.since + TAKEN_AFTER), POP).is_some());
    let Some(state) = view(ui) else { return popping };
    let changed = map.rows.iter().any(|row| within(ui, row.changed.map(|(_, at)| at), FREED).is_some());
    let freeing = map.rows.iter().any(|row| stopping(ui, &state, row).is_some());
    within(ui, Some(state.opened), STAGGER_ALL).is_some()
        || within(ui, state.copied, COPY_FLASH).is_some()
        || changed
        || (moving(ui) && (state.confirm || freeing))
}

/// A holding's mark, words and colour.
fn look(holding: Holding) -> (&'static str, &'static str, C) {
    match holding {
        Holding::Free => ("●", "free", C::Green),
        Holding::Own => ("◐", "its app", C::Blue),
        Holding::Other => ("×", "held by other", C::Red),
        Holding::Reserved => ("∅", "reserved", C::Yellow),
    }
}

/// A history cell: who held the port then.
fn cell(holding: Holding) -> (&'static str, C) {
    match holding {
        Holding::Free => ("·", C::DarkGray),
        Holding::Own => ("▆", C::Blue),
        Holding::Other => ("▆", C::Red),
        Holding::Reserved => ("░", C::Yellow),
    }
}

/// How long, in a few cells: 12s, 3m, 2h, 4d.
fn age(took: Duration) -> String {
    let secs = took.as_secs();
    match secs {
        0..60 => format!("{secs}s"),
        60..3600 => format!("{}m", secs / 60),
        3600..86400 => format!("{}h", secs / 3600),
        _ => format!("{}d", secs / 86400),
    }
}

/// The time of day `at` was: 09:39:58.
fn clock(ui: &Ui, at: Instant) -> String {
    let ago = chrono::TimeDelta::from_std(ui.now.saturating_duration_since(at)).unwrap_or_default();
    (chrono::Local::now() - ago).format("%H:%M:%S").to_string()
}

/// `segs` lit in `color`: a flash.
fn flashed(segs: Vec<Seg>, color: C) -> Vec<Seg> {
    segs.into_iter().map(|part| Seg { fg: C::Black, bg: Some(color), ..part }).collect()
}

/// The first `share` of `segs`' cells.
fn share(segs: Vec<Seg>, share: f64) -> Vec<Seg> {
    let cells = width(&segs);
    fit(segs, (cells as f64 * share.clamp(0.0, 1.0)).ceil() as usize)
}

/// Who holds a port: the process, its PID and command line; a range Windows reserved; or nothing.
fn holder(controller: &Controller, row: &Port, holding: Holding, holder: Option<&Holder>) -> Vec<Seg> {
    match (holding, holder) {
        (Holding::Other | Holding::Own, Some(holder)) => {
            let command = holder.command.as_ref().map_or(String::new(), |command| format!(" · {command}"));
            vec![seg(format!("{} (PID {})", holder.name, holder.pid), C::White), seg(command, C::DarkGray)]
        }
        (Holding::Reserved, _) => {
            let range = lazyrunner_core::run::net::reserved(row.port, &controller.port_map().reserved);
            let text = range.map_or("Windows".into(), |(first, last)| format!("Windows · {first}-{last}"));
            vec![seg(text, C::DarkGray)]
        }
        (Holding::Other, None) => vec![seg("something else", C::White)],
        _ => vec![seg("—", C::DarkGray)],
    }
}

/// A row's state as it is or moving: a change flashes in its new colour; a freed port wipes out what held it while
/// `freed` draws in, then flashes green; one being freed spins.
fn state(ui: &Ui, row: &Port, freeing: bool) -> Vec<Seg> {
    if freeing {
        return vec![seg(format!("{} stopping it", spinner(ui)), C::DarkGray)];
    }
    let (mark, words, color) = look(row.holding);
    let freed = row.holding == Holding::Free
        && row.changed.is_some_and(|(was, at)| was == Holding::Other && ui.now.saturating_duration_since(at) < FREED);
    let now_said = vec![seg(format!("{mark} {}", if freed { "freed" } else { words }), color)];
    let Some((was, since)) = row.changed.and_then(|(was, at)| Some((was, within(ui, Some(at), WIPE + FLASH)?))) else {
        return now_said;
    };
    let half = WIPE / 2;
    if freed && since < WIPE + FLASH {
        if since < half {
            let (mark, words, color) = look(was);
            return share(vec![seg(format!("{mark} {words}"), color)], 1.0 - since.as_secs_f64() / half.as_secs_f64());
        }
        if since < WIPE {
            return share(now_said, (since - half).as_secs_f64() / half.as_secs_f64());
        }
        return flashed(now_said, C::Green);
    }
    if since < FLASH { flashed(now_said, color) } else { now_said }
}

/// What to do about a port: stop what took it (and since when), go to its own app, where the next start lands, the
/// fix for a reserved range.
fn action(ui: &Ui, row: &Port, asking: bool, freeing: bool) -> Vec<Seg> {
    match row.holding {
        _ if freeing => vec![seg("waiting for it to free up", C::DarkGray)],
        Holding::Other if asking => vec![seg("y stops it · n doesn't", C::Yellow)],
        Holding::Other => {
            vec![seg("! stop it", C::White), seg(format!(" · since {}", clock(ui, row.since)), C::DarkGray)]
        }
        Holding::Own => vec![seg("Enter its column · w opens", C::DarkGray)],
        Holding::Free => vec![seg("its next start lands here", C::DarkGray)],
        Holding::Reserved => vec![seg("y copies what frees it", C::DarkGray)],
    }
}

/// The row's left edge: red while something else holds the port (pulsing twice as it takes it, and while `!` asks
/// about it), else the picked row's in its state's colour.
fn edge(ui: &Ui, row: &Port, picked: bool, asking: bool) -> Seg {
    let blink = |since: Duration| (since.as_millis() / PULSE_STEP) % 2 == 1;
    if row.holding == Holding::Other {
        let pulsing = row.changed.and_then(|(_, at)| within(ui, Some(at), PULSE));
        let off =
            pulsing.is_some_and(blink) || (asking && moving(ui) && blink(ui.now.saturating_duration_since(ui.started)));
        return seg(if off { " " } else { "▌" }, C::Red);
    }
    if picked { seg("▌", look(row.holding).2) } else { seg(" ", C::Gray) }
}

/// The last minute, a cell every two seconds, oldest first; the newest flashes as the port changes.
fn history(ui: &Ui, row: &Port) -> Vec<Seg> {
    let mut out = vec![seg(" ".repeat(CELLS.saturating_sub(row.cells.len())), C::Gray)];
    let flash = row.changed.and_then(|(_, at)| within(ui, Some(at), FLASH)).is_some();
    let last = row.cells.len().saturating_sub(1);
    for (at, holding) in row.cells.iter().enumerate() {
        let (text, color) = cell(*holding);
        let part = if flash && at == last { on(text, C::Black, color) } else { seg(text, color) };
        match out.last_mut() {
            Some(previous) if at > 0 && previous.fg == part.fg && previous.bg == part.bg => {
                previous.text.push_str(text)
            }
            _ => out.push(part),
        }
    }
    out
}

/// How the table's columns fit `cells`: who holds a port gets the rest; what to do, then the last minute, with room.
struct Columns {
    holder: usize,
    action: bool,
    history: bool,
}

fn columns(cells: usize) -> Columns {
    let action = cells >= FIXED_CELLS + HOLDER_CELLS + 2 + ACTION_CELLS;
    let history = action && cells >= FIXED_CELLS + HOLDER_CELLS + 2 + ACTION_CELLS + 2 + CELLS;
    let used = FIXED_CELLS + if action { 2 + ACTION_CELLS } else { 0 } + if history { 2 + CELLS } else { 0 };
    Columns { holder: cells.saturating_sub(used).max(8), action, history }
}

/// The table's titles.
fn titles(columns: &Columns) -> Vec<Seg> {
    let mut text = format!(
        "  {:>PORT_CELLS$}  {}  {}  {}  {:>FOR_CELLS$}",
        "port",
        pad("app", NAME_CELLS),
        pad("holder", columns.holder),
        pad("state", STATE_CELLS),
        "for"
    );
    if columns.history {
        text.push_str(&format!("  {:>CELLS$}", "the last minute →"));
    }
    if columns.action {
        text.push_str(&format!("  {}", pad("action", ACTION_CELLS)));
    }
    vec![seg(text, C::DarkGray)]
}

/// A port's row: its edge, port, app, holder, state, for how long, and with room its last minute and what to do. A
/// new conflict slides in; one being freed is dimmed; the picked one's command line scrolls when it doesn't fit.
fn port_row(ui: &Ui, controller: &Controller, state: &View, row: &Port, picked: bool, columns: &Columns) -> Vec<Seg> {
    let app = controller.app(&row.key);
    let name = app.map_or(row.key.as_str(), |app| app.name.as_str());
    let icon = app.map_or("", |app| icons::app(ui.nerd, app, controller.framework(&row.key)));
    let asking = picked && state.confirm && row.holding == Holding::Other;
    let freeing = stopping(ui, state, row).is_some();
    let mut line = vec![edge(ui, row, picked, asking), seg(" ", C::Gray)];
    line.push(seg(format!("{:>PORT_CELLS$}", row.port), C::White));
    line.push(seg("  ", C::Gray));
    line.push(seg(pad(&take_cells(&format!("{icon} {name}"), NAME_CELLS), NAME_CELLS), C::Gray));
    line.push(seg("  ", C::Gray));
    let held = holder(controller, row, row.holding, row.holder.as_ref());
    // The picked row's command line scrolls when it doesn't fit; its process stays.
    let held = if picked { ui.scrolling_row(held, 1, columns.holder) } else { held };
    line.extend(fit(held, columns.holder));
    line.push(seg("  ", C::Gray));
    line.extend(fit(self::state(ui, row, freeing), STATE_CELLS));
    line.push(seg("  ", C::Gray));
    line.push(seg(format!("{:>FOR_CELLS$}", age(ui.now.saturating_duration_since(row.since))), C::DarkGray));
    if columns.history {
        line.push(seg("  ", C::Gray));
        line.extend(history(ui, row));
    }
    if columns.action {
        line.push(seg("  ", C::Gray));
        line.extend(fit(action(ui, row, asking, freeing), ACTION_CELLS));
    }
    if freeing {
        for part in line.iter_mut().skip(1) {
            part.fg = C::DarkGray;
        }
    }
    let slid = row
        .changed
        .filter(|_| row.holding == Holding::Other)
        .and_then(|(_, at)| within(ui, Some(at), SLIDE))
        .map(|since| since.as_secs_f64() / SLIDE.as_secs_f64());
    match slid {
        Some(progress) => share(line, progress),
        None => line,
    }
}

/// The row lit across (but its edge), as the selection bar is.
fn light(row: &mut [Seg]) {
    for part in row.iter_mut().skip(1).filter(|part| part.bg.is_none()) {
        part.bg = Some(C::FocusBack);
        if matches!(part.fg, C::Gray | C::DarkGray | C::White) {
            part.fg = C::FocusText;
        }
    }
}

/// How many of `count` things have come in since the view opened.
fn arrived(ui: &Ui, opened: Instant, count: usize) -> usize {
    if !moving(ui) {
        return count;
    }
    let step = STAGGER.min(STAGGER_ALL / count.max(1) as u32).max(Duration::from_millis(1));
    let since = ui.now.saturating_duration_since(opened);
    ((since.as_millis() / step.as_millis()) as usize + 1).min(count)
}

/// The conflicts, as the header says them.
fn conflicts(others: usize) -> String {
    if others == 1 { "1 conflict".into() } else { format!("{others} conflicts") }
}

/// The first row: the view, the project, its conflicts, what it does.
fn header(controller: &Controller) -> Vec<Seg> {
    let (wanted, others, _) = controller.port_map().summary();
    vec![
        seg(format!("  {}", controller.project.name), C::White),
        if others > 0 { seg(format!("  {}", conflicts(others)), C::Red) } else { seg("  no conflicts", C::Green) },
        seg(format!("  {wanted} tracked · who really holds each port"), C::DarkGray),
    ]
}

/// The counts, and the occupancy strip: a cell per port in the table's order, filling left to right as the view
/// opens, a cell flashing as its port changes.
fn overview(ui: &Ui, map: &Ports, opened: Instant) -> Vec<Vec<Seg>> {
    let (wanted, others, reserved) = map.summary();
    let counts = vec![
        seg(" ports", C::White),
        seg(format!(" · {wanted} tracked · "), C::DarkGray),
        seg(conflicts(others), if others > 0 { C::Red } else { C::DarkGray }),
        seg(" · ", C::DarkGray),
        seg(format!("{reserved} reserved"), if reserved > 0 { C::Yellow } else { C::DarkGray }),
        seg(" · read from the system, never guessed from the profile", C::DarkGray),
    ];
    let rows = shown(map);
    let mut strip = vec![seg(" occupancy ", C::DarkGray)];
    for row in rows.iter().take(arrived(ui, opened, rows.len())) {
        let color = look(row.holding).2;
        let text = if row.holding == Holding::Reserved { "░░" } else { "██" };
        let flash = row.changed.and_then(|(_, at)| within(ui, Some(at), FLASH)).is_some();
        strip.push(if flash { seg(text, C::White) } else { seg(text, color) });
        strip.push(seg(" ", C::Gray));
    }
    strip.push(seg(" a cell per port, in the table's order", C::DarkGray));
    vec![counts, strip]
}

/// The ranges Windows reserved, hatched, the apps' ports in them marked - those first - in `room` rows at most.
fn bands(controller: &Controller, room: usize) -> Vec<Vec<Seg>> {
    let map = controller.port_map();
    if map.reserved.is_empty() || room < 3 {
        return Vec::new();
    }
    let inside = |(first, last): (u16, u16)| -> Vec<&Port> {
        map.rows.iter().filter(|row| (first..=last).contains(&row.port)).collect()
    };
    let mut ranges = map.reserved.clone();
    ranges.sort_by_key(|range| (inside(*range).is_empty(), range.0));
    let mut out = vec![
        Vec::new(),
        vec![
            seg(" RESERVED BY WINDOWS", C::DarkCyan),
            seg("   no app can listen in these - y copies what frees them", C::DarkGray),
        ],
    ];
    let shown = room.saturating_sub(out.len() + 1).max(1);
    for range in ranges.iter().take(shown) {
        let mut line = vec![seg(format!(" {}", "░".repeat(BAND_CELLS)), C::Yellow)];
        line.push(seg(format!(" {:>5}-{:<5}", range.0, range.1), C::Yellow));
        for row in inside(*range) {
            let name = controller.app(&row.key).map_or(row.key.as_str(), |app| app.name.as_str());
            line.push(seg(format!("  ∅ {} {name}", row.port), C::Yellow));
        }
        out.push(line);
    }
    if ranges.len() > shown {
        out.push(vec![seg(format!(" +{} more ranges", ranges.len() - shown), C::DarkGray)]);
    }
    out
}

/// The keys, under the header as in the other views - or, while `!` asks, the question: the keys first.
fn keys(state: &View, picked: Option<&Port>, cells: usize) -> Vec<Vec<Seg>> {
    let Some(row) = picked.filter(|row| state.confirm && row.holding == Holding::Other) else {
        let keys = " j k move · ! stops the holder · Enter its column · w opens it · r reads again · y copies the fix · \
                    Esc closes";
        return super::viewbar::keys(keys, cells);
    };
    let who = row
        .holder
        .as_ref()
        .map_or("what holds it".to_string(), |holder| format!("{} (PID {})", holder.name, holder.pid));
    let text = format!(" y stops {who}, which holds {} · n doesn't ", row.port);
    vec![vec![on(pad(&text, cells), C::Black, C::Yellow)]]
}

/// The line under the strip: what `y` copied, the first conflict, or that all is well.
fn status(ui: &Ui, controller: &Controller, state: &View) -> Vec<Seg> {
    let map = controller.port_map();
    let mut line = vec![seg(format!(" {}  ", clock(ui, ui.now)), C::DarkGray)];
    if within(ui, state.copied, COPY_FLASH).is_some() {
        line.push(on(format!(" copied {WINNAT} - paste it into an elevated terminal "), C::Black, C::Cyan));
        return line;
    }
    if let Some(row) = map.rows.iter().find(|row| row.holding == Holding::Other) {
        let name = controller.app(&row.key).map_or(row.key.as_str(), |app| app.name.as_str());
        let who = row.holder.as_ref().map_or("something else", |holder| holder.name.as_str());
        let text =
            format!("▲ {} conflict since {} · {who} holds {name}'s port - ! stops it", row.port, clock(ui, row.since));
        line.push(seg(text, C::Red));
        return line;
    }
    let (_, _, reserved) = map.summary();
    line.push(if reserved > 0 {
        seg(format!("∅ {reserved} port(s) in a range Windows reserved - y copies what frees it"), C::Yellow)
    } else {
        seg("✓ every port is free or held by its own app", C::Green)
    });
    line
}

/// `line` across the whole view, in it (a click there keeps it open) unless it's a row's.
fn across(line: Vec<Seg>, cells: usize) -> Vec<Seg> {
    let target = line.iter().find_map(|part| part.target).unwrap_or(Target::Box);
    aim(fit(line, cells), target)
}

/// The view: its header and the keys (or the question before `!`), the counts, the occupancy strip and the status
/// line, the table (the picked row lit, the bar gliding to it, the rows coming in as it opens), the reserved ranges.
pub fn rows(ui: &mut Ui, controller: &Controller, cells: usize, rows: usize) -> Vec<Vec<Seg>> {
    let Some(state) = view(ui) else { return Vec::new() };
    let map = controller.port_map();
    let list = shown(map);
    let index = state.index.min(list.len().saturating_sub(1));
    let bar = ui.motion.bar(index, ui.now);
    let columns = columns(cells);
    let mut lines = vec![header(controller)];
    lines.extend(keys(&state, list.get(index).copied(), cells));
    lines.push(Vec::new());
    lines.extend(overview(ui, map, state.opened));
    lines.extend([status(ui, controller, &state), Vec::new(), titles(&columns)]);
    if list.is_empty() {
        lines.push(vec![seg("  no app has a port in this checkout", C::Gray)]);
    }
    for (at, row) in list.iter().enumerate().take(arrived(ui, state.opened, list.len())) {
        let mut line = aim(port_row(ui, controller, &state, row, at == index, &columns), Target::Item(at));
        if at == bar {
            light(&mut line);
        }
        lines.push(line);
    }
    lines.extend(bands(controller, rows.saturating_sub(lines.len())));
    lines.resize(rows, Vec::new());
    lines.into_iter().map(|line| across(line, cells)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_age_fits_in_a_few_cells() {
        assert_eq!(age(Duration::from_secs(12)), "12s");
        assert_eq!(age(Duration::from_secs(200)), "3m");
        assert_eq!(age(Duration::from_secs(7300)), "2h");
        assert_eq!(age(Duration::from_secs(90000)), "1d");
    }

    #[test]
    fn the_columns_make_room_for_the_action_then_the_last_minute() {
        let narrow = columns(110);
        assert!(narrow.action && !narrow.history, "at 110 cells the action fits, the last minute doesn't");
        let wide = columns(150);
        assert!(wide.action && wide.history);
        assert_eq!(FIXED_CELLS + wide.holder + 2 + ACTION_CELLS + 2 + CELLS, 150);
    }
}
