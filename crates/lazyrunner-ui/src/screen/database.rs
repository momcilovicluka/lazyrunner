//! The database screen (`d`): the connection strings in the user secrets, each with whether its server answers -
//! checked as the screen opens, the dots breathing meanwhile - the apps that use it, its pending migrations and when
//! you last switched to it. `Enter` switches and restarts the APIs; `n` adds one (a copy), `/` filters, `y` copies a
//! row, `h` `l` another key, `Esc` goes back. Its rows come in one after another as it opens, and each answer flashes
//! as it arrives.

use std::collections::HashMap;
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use lazyrunner_core::run::Controller;
use lazyrunner_core::run::secrets::Entry;
use unicode_width::UnicodeWidthStr;

use super::Ui;
use super::mouse::Target;
use super::seg::{Seg, aim, focus_row, on, pad, seg, take_cells};
use super::theme::C;

/// Whether a server answered: being checked, answering (in how many ms), or not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reach {
    Checking,
    Up(u128),
    Down,
}

/// The servers' answers, by server, filled in by background checks.
pub type Reachability = Arc<Mutex<HashMap<String, Reach>>>;

/// How long a server has to answer, and SQL Server's port when the connection string names none.
const TIMEOUT: Duration = Duration::from_millis(1500);
const SQL_PORT: u16 = 1433;
/// The config's record of when each database was switched to, by its label.
pub const SWITCHED: &str = "databaseSwitched";
/// The columns' titles; between two columns at least `GAP` cells, at most `MOST_GAP` when the screen is wide; the
/// database and server never narrower than `LABEL_LEAST`; before them the active one's dot.
const TITLES: [&str; 5] = ["DATABASE @ SERVER", "ANSWERS", "APPS ON IT", "MIGRATIONS", "SWITCHED TO"];
const GAP: usize = 2;
const MOST_GAP: usize = 6;
const LABEL_LEAST: usize = 24;
const MARK_CELLS: usize = 3;
const CUT: &str = "…";
/// Each row comes in this long after the one above it, dim at first for this long; an answer flashes this long.
const ROW_STAGGER: Duration = Duration::from_millis(35);
const ROW_DIM: Duration = Duration::from_millis(120);
const ANSWER_FLASH: Duration = Duration::from_millis(500);

/// The screen's motion: when it opened, and when each server's answer came in.
#[derive(Debug, Default)]
pub struct Motion {
    opened: Option<Instant>,
    answered: HashMap<String, Instant>,
}

impl Motion {
    pub fn opened(now: Instant) -> Self {
        Self { opened: Some(now), answered: HashMap::new() }
    }
}

/// A server's host and port from its connection string: `host`, `host,port`, `tcp:host,port`, `host\instance`; none
/// for a local named instance or a pipe.
fn address(server: &str) -> Option<(String, u16)> {
    let server = server.trim().trim_start_matches("tcp:");
    if server.starts_with('(') || server.starts_with("np:") || server.is_empty() {
        return None;
    }
    let (host, port) = match server.split_once(',') {
        Some((host, port)) => (host, port.trim().parse().ok()?),
        None => (server, SQL_PORT),
    };
    let host = host.split('\\').next().filter(|host| !host.is_empty() && *host != ".").unwrap_or("localhost");
    Some((host.to_string(), port))
}

/// Checks each entry's server in the background: does it answer, and how fast.
pub fn check(entries: &[Entry]) -> Reachability {
    let reach: Reachability = Arc::default();
    for entry in entries {
        let server = entry.server.clone();
        let Some((host, port)) = address(&server) else { continue };
        let Ok(mut known) = reach.lock() else { continue };
        if known.contains_key(&server) {
            continue;
        }
        known.insert(server.clone(), Reach::Checking);
        drop(known);
        let shared = Arc::clone(&reach);
        std::thread::spawn(move || {
            let started = Instant::now();
            let address = (host.as_str(), port).to_socket_addrs().ok().and_then(|mut found| found.next());
            let up = address.is_some_and(|address| TcpStream::connect_timeout(&address, TIMEOUT).is_ok());
            let answer = if up { Reach::Up(started.elapsed().as_millis()) } else { Reach::Down };
            if let Ok(mut known) = shared.lock() {
                known.insert(server, answer);
            }
        });
    }
    reach
}

fn reach_of(ui: &Ui, server: &str) -> Option<Reach> {
    ui.db_reach.as_ref()?.lock().ok()?.get(server).copied()
}

/// Whether a server is still being checked (its dot breathes), rows still come in, or an answer flashes.
pub fn animating(ui: &Ui) -> bool {
    let checking = ui
        .db_reach
        .as_ref()
        .and_then(|reach| reach.lock().ok().map(|known| known.values().any(|reach| *reach == Reach::Checking)))
        .unwrap_or(false);
    let since = |at: &Instant| ui.now.saturating_duration_since(*at);
    let rows = ui.picker.as_ref().map_or(0, |picker| picker.entries().len()) as u32;
    let coming = ui.db_motion.opened.is_some_and(|at| since(&at) < ROW_STAGGER * rows + ROW_DIM);
    let flashing = ui.db_motion.answered.values().any(|at| since(at) < ANSWER_FLASH);
    ui.picker.is_some() && !ui.reduced_motion && (checking || coming || flashing)
}

/// Notes when each server's answer came in (it flashes then).
fn note_answers(ui: &mut Ui) {
    let Some(known) = ui.db_reach.as_ref().and_then(|reach| reach.lock().ok().map(|known| known.clone())) else {
        return;
    };
    for (server, reach) in known {
        if reach != Reach::Checking && !ui.db_motion.answered.contains_key(&server) {
            ui.db_motion.answered.insert(server, ui.now);
        }
    }
}

/// The answer cell: answering in so many ms, not answering, being checked (breathing), or a local instance.
fn answer(ui: &Ui, server: &str) -> Seg {
    match reach_of(ui, server) {
        Some(Reach::Up(ms)) => seg(format!("● {ms} ms"), C::Green),
        Some(Reach::Down) => seg("✗ no answer", C::Red),
        Some(Reach::Checking) => {
            let lit = ui.reduced_motion || ui.tick() % 4 < 2;
            seg("◌ checking", if lit { C::Yellow } else { C::DarkYellow })
        }
        None => seg("– local", C::DarkGray),
    }
}

/// How long ago an RFC 3339 time was, as the checklist says it: `12 min ago`, `2 h ago`, `3 d ago`.
fn ago(at: &str) -> Option<String> {
    let at = at.parse::<jiff::Timestamp>().ok()?;
    let seconds = jiff::Timestamp::now().duration_since(at).as_secs().max(0) as u64;
    Some(lazyrunner_core::run::session::ago(Duration::from_secs(seconds)))
}

/// An entry's columns: its database and server, whether it answers, the apps on it, its pending migrations, when it
/// was last switched to.
fn parts(ui: &Ui, controller: &Controller, entry: &Entry) -> [Seg; 5] {
    let apps = if entry.active {
        let users = controller.database_users();
        let names: Vec<&str> =
            users.iter().filter_map(|key| controller.app(key)).map(|app| app.name.as_str()).collect();
        if names.is_empty() { "-".to_string() } else { names.join(", ") }
    } else {
        "-".to_string()
    };
    let contexts = controller.contexts.as_deref().unwrap_or_default();
    let pending: usize =
        contexts.iter().filter(|context| context.db == entry.database).map(|context| context.pending().len()).sum();
    let pending = if pending > 0 { seg(format!("{pending} pending"), C::Yellow) } else { seg("-", C::DarkGray) };
    let last = controller.config[SWITCHED][&entry.label].as_str().and_then(ago).unwrap_or_else(|| "-".to_string());
    let label = seg(entry.label.clone(), if entry.active { C::Green } else { C::Gray });
    [label, answer(ui, &entry.server), seg(apps, C::Gray), pending, seg(last, C::DarkGray)]
}

/// Each column's width and the gap between them for `cells`: each as wide as the most it holds (its title at least);
/// the database and server get what the others leave, down to [`LABEL_LEAST`]; what's still left widens the gaps.
fn widths(all: &[[Seg; 5]], cells: usize) -> ([usize; 5], usize) {
    let most = |at: usize| all.iter().map(|parts| parts[at].text.width()).chain([TITLES[at].width()]).max();
    let mut widths: [usize; 5] = std::array::from_fn(|at| most(at).unwrap_or_default());
    let others = MARK_CELLS + widths[1..].iter().sum::<usize>() + GAP * (TITLES.len() - 1);
    widths[0] = widths[0].min(cells.saturating_sub(others)).max(LABEL_LEAST);
    let left = cells.saturating_sub(others + widths[0]);
    (widths, GAP + (left / (TITLES.len() - 1)).min(MOST_GAP - GAP))
}

/// `text` in `cells`: padded, or cut with `…` when it's longer.
fn cell(text: &str, cells: usize) -> String {
    if text.width() <= cells {
        return pad(text, cells);
    }
    format!("{}{CUT}", take_cells(text, cells.saturating_sub(CUT.width())))
}

/// An entry's row in the columns' widths: the active one's dot, then its columns (a fresh answer flashing); the one
/// under the selection bar lit across `cells`, keeping its colours.
fn row(
    entry: &Entry,
    parts: [Seg; 5],
    (widths, gap): ([usize; 5], usize),
    picked: Option<usize>,
    flash: bool,
) -> Vec<Seg> {
    let mark = if entry.active { " ● " } else { "   " };
    let mut out = vec![seg(mark, C::Green)];
    for (at, part) in parts.into_iter().enumerate() {
        let after = if at + 1 == TITLES.len() { 0 } else { gap };
        let text = pad(&cell(&part.text, widths[at]), widths[at] + after);
        out.push(if at == 1 && flash { on(text, C::Black, part.fg) } else { Seg { text, ..part } });
    }
    match picked {
        Some(cells) => focus_row(out, cells),
        None => out,
    }
}

/// Whether `entry`'s server answered a moment ago (its answer flashes).
fn flashing(ui: &Ui, entry: &Entry) -> bool {
    let since = ui.db_motion.answered.get(&entry.server).map(|at| ui.now.saturating_duration_since(*at));
    !ui.reduced_motion && since.is_some_and(|since| since < ANSWER_FLASH)
}

pub fn rows(ui: &mut Ui, controller: &Controller, cells: usize, rows: usize) -> Vec<Vec<Seg>> {
    note_answers(ui);
    let bar = ui.picker.as_ref().map(|picker| picker.index).map_or(0, |index| ui.motion.bar(index, ui.now));
    let cursor = ui.cursor().to_string();
    let Some(picker) = ui.picker.as_ref() else { return Vec::new() };
    let entries = picker.entries();
    let (key, size) = picker.groups.get(picker.group).map_or((String::new(), 0), |(key, all)| (key.clone(), all.len()));
    let groups = if picker.groups.len() > 1 {
        format!(" · key {}/{} (h l)", picker.group + 1, picker.groups.len())
    } else {
        String::new()
    };
    let mut lines = vec![vec![
        seg(format!(" {key}"), C::White),
        seg(format!("{groups} · secrets.json · {size} connection strings · the active one ●"), C::DarkGray),
    ]];
    let hint = if picker.filtering {
        " type to filter · Enter keeps it · Esc clears it".to_string()
    } else if picker.new_name.is_some() {
        " type the database name · Enter adds it (inactive) · Esc cancels".to_string()
    } else {
        " j k choose · Enter switch + restart the APIs · n new (a copy) · / filter · y copy row · Esc back".to_string()
    };
    lines.extend(super::viewbar::keys(&hint, cells));
    if picker.filtering || !picker.filter.is_empty() {
        lines.push(vec![seg(
            format!(" / {}{}", picker.filter, if picker.filtering { cursor.as_str() } else { "" }),
            C::Cyan,
        )]);
    }
    lines.push(Vec::new());
    let all: Vec<[Seg; 5]> = entries.iter().map(|entry| parts(ui, controller, entry)).collect();
    let (widths, gap) = widths(&all, cells);
    let titles: String = TITLES
        .iter()
        .enumerate()
        .map(|(at, title)| if at + 1 == TITLES.len() { title.to_string() } else { pad(title, widths[at] + gap) })
        .collect();
    lines.push(vec![seg(format!("{}{titles}", " ".repeat(MARK_CELLS)), C::DarkCyan)]);
    if entries.is_empty() {
        lines.push(vec![seg("   nothing matches", C::DarkGray)]);
    }
    let room = rows.saturating_sub(lines.len() + 2).max(1);
    let (index, mut top) = (picker.index, picker.top);
    if index < top {
        top = index;
    }
    if index >= top + room {
        top = index + 1 - room;
    }
    let opened = ui.db_motion.opened.filter(|_| !ui.reduced_motion).map(|at| ui.now.saturating_duration_since(at));
    for (shown, ((at, entry), parts)) in entries.iter().enumerate().zip(all).skip(top).take(room).enumerate() {
        let comes = ROW_STAGGER * shown as u32;
        let picked = (at == bar).then_some(cells);
        let mut line = super::columns::fit(row(entry, parts, (widths, gap), picked, flashing(ui, entry)), cells);
        match opened {
            Some(since) if since < comes => line = Vec::new(),
            Some(since) if since < comes + ROW_DIM && at != index => {
                line.iter_mut().for_each(|part| part.fg = C::DarkGray);
            }
            _ => {}
        }
        lines.push(aim(line, Target::Item(at)));
    }
    if let Some(name) = picker.new_name.clone() {
        lines.push(Vec::new());
        lines.push(vec![seg(format!(" + new database (a copy of the picked one): {name}{cursor}"), C::Cyan)]);
    }
    if let Some(picker) = ui.picker.as_mut() {
        picker.top = top;
    }
    lines.truncate(rows);
    lines.resize(rows, Vec::new());
    lines.into_iter().map(|line| aim(line, Target::Box)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(label: &str) -> [Seg; 5] {
        [
            seg(label, C::Gray),
            seg("● 1 ms", C::Green),
            seg("WebAPI, Admin", C::Gray),
            seg("-", C::Gray),
            seg("-", C::Gray),
        ]
    }

    #[test]
    fn the_names_get_the_width_the_screen_has_and_the_gaps_what_is_left() {
        let long = "shop-orders-data-uat-copy-110320 @ BUILD-MACHINE-01";
        let all = [entry("shop @ here"), entry(long)];
        let (wide, gap) = widths(&all, 200);
        assert_eq!((wide[0], gap), (long.width(), MOST_GAP), "the longest name whole, the gaps at their widest");
        let (narrow, gap) = widths(&all, 80);
        assert_eq!((narrow[0], gap), (80 - MARK_CELLS - narrow[1..].iter().sum::<usize>() - GAP * 4, GAP));
        assert_eq!(widths(&all, 40).0[0], LABEL_LEAST, "never narrower than that");
        assert_eq!(cell(long, 10), "shop-orde…");
        assert_eq!(cell("so", 4), "so  ");
    }

    #[test]
    fn a_servers_address_is_its_host_and_port_and_a_local_instance_has_none() {
        assert_eq!(address("dev-sql"), Some(("dev-sql".into(), 1433)));
        assert_eq!(address("tcp:dev-sql,14330"), Some(("dev-sql".into(), 14330)));
        assert_eq!(address(r"localhost\SQLEXPRESS"), Some(("localhost".into(), 1433)));
        assert_eq!(address("(localdb)\\MSSQLLocalDB"), None);
    }
}
