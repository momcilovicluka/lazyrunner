//! The database screen (`d`): the connection strings in the user secrets, each with whether its server answers -
//! checked as the screen opens, the dots breathing meanwhile - the apps that use it, its pending migrations and when
//! you last switched to it. `Enter` switches and restarts the APIs; `n` adds one (a copy), `/` filters, `y` copies a
//! row, `h` `l` another key, `Esc` goes back.

use std::collections::HashMap;
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use lazyrunner_core::run::Controller;
use lazyrunner_core::run::secrets::Entry;

use super::Ui;
use super::mouse::Target;
use super::seg::{Seg, aim, on, pad, seg};
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
const LABEL_CELLS: usize = 34;
const ANSWER_CELLS: usize = 15;
const APPS_CELLS: usize = 26;
const PENDING_CELLS: usize = 12;

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

/// Whether a server is still being checked (its dot breathes).
pub fn animating(ui: &Ui) -> bool {
    ui.picker.is_some()
        && !ui.reduced_motion
        && ui
            .db_reach
            .as_ref()
            .and_then(|reach| reach.lock().ok().map(|known| known.values().any(|reach| *reach == Reach::Checking)))
            .unwrap_or(false)
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

/// How long ago an RFC 3339 time was.
fn ago(at: &str) -> Option<String> {
    let at = chrono::DateTime::parse_from_rfc3339(at).ok()?;
    let seconds = (chrono::Local::now().fixed_offset() - at).num_seconds().max(0) as f64;
    Some(format!("{} ago", lazyrunner_core::run::format_duration(seconds)))
}

/// An entry's row: active or not, its database and server, whether it answers, the apps on it, its pending
/// migrations, when it was last switched to.
fn row(ui: &Ui, controller: &Controller, entry: &Entry, picked: bool) -> Vec<Seg> {
    let marker = if entry.active { "●" } else { " " };
    let label = pad(&format!(" {marker} {}", entry.label), LABEL_CELLS);
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
    let pending = if pending > 0 {
        seg(pad(&format!("{pending} pending"), PENDING_CELLS), C::Yellow)
    } else {
        seg(pad("-", PENDING_CELLS), C::DarkGray)
    };
    let last = controller.config[SWITCHED][&entry.label].as_str().and_then(ago).unwrap_or_else(|| "-".to_string());
    let answer = answer(ui, &entry.server);
    let answer = seg(pad(&answer.text, ANSWER_CELLS), answer.fg);
    if picked {
        let text = format!("{label}{}{}{}{last}", answer.text, pad(&apps, APPS_CELLS), pending.text);
        return vec![on(text, C::FocusText, C::FocusBack)];
    }
    vec![
        seg(label, if entry.active { C::Green } else { C::Gray }),
        answer,
        seg(pad(&apps, APPS_CELLS), C::Gray),
        pending,
        seg(last, C::DarkGray),
    ]
}

pub fn rows(ui: &mut Ui, controller: &Controller, cells: usize, rows: usize) -> Vec<Vec<Seg>> {
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
    let title = format!(
        " {}{}{}{}{}",
        pad("  DATABASE @ SERVER", LABEL_CELLS - 1),
        pad("ANSWERS", ANSWER_CELLS),
        pad("APPS ON IT", APPS_CELLS),
        pad("MIGRATIONS", PENDING_CELLS),
        "SWITCHED TO"
    );
    lines.push(vec![seg(title, C::DarkCyan)]);
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
    for (at, entry) in entries.iter().enumerate().skip(top).take(room) {
        lines.push(aim(super::columns::fit(row(ui, controller, entry, at == index), cells), Target::Item(at)));
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

    #[test]
    fn a_servers_address_is_its_host_and_port_and_a_local_instance_has_none() {
        assert_eq!(address("dev-sql"), Some(("dev-sql".into(), 1433)));
        assert_eq!(address("tcp:dev-sql,14330"), Some(("dev-sql".into(), 14330)));
        assert_eq!(address(r"localhost\SQLEXPRESS"), Some(("localhost".into(), 1433)));
        assert_eq!(address("(localdb)\\MSSQLLocalDB"), None);
    }
}
