//! The database screen's servers next to secrets.json (`a`): a card with every server the connection strings name and,
//! for each, its databases compared with them - `●` in both, `+` on the server but in no connection string, `✗` in a
//! connection string but gone from the server, or every connection string of a server that doesn't answer (an old
//! name of this machine). `Enter` acts on one row (adds a `+`, takes a `✗` out, goes to a `●`); `+` adds every `+`,
//! `-` takes every `✗` out, `r` does both - secrets.json made to match the servers. Nothing on a server is touched.

use std::time::{Duration, Instant};

use lazyrunner_core::output::Hue;
use lazyrunner_core::run::Controller;
use lazyrunner_core::run::database_files as files;
use lazyrunner_core::run::secrets::{self, Entry};

use super::database::Reach;
use super::overlays::boxed_in;
use super::pull_view::Card;
use super::seg::{Seg, focus_row, seg};
use super::theme::C;
use super::{Overlay, Ui};

/// The card's text at most this wide; its rows at most this many at a time.
const INNER: usize = 76;
const ROWS_MOST: usize = 18;
/// A row a key just changed glints this long.
const GLINT: Duration = Duration::from_millis(400);

/// The card: the row picked, and when a key last changed what it lists.
#[derive(Debug)]
pub struct SyncCard {
    pub index: usize,
    pub changed: Option<Instant>,
}

/// A server's line in the card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Server {
    /// It answers: how many databases it has (SQL Server's own left out).
    Answers(usize),
    /// It doesn't answer: how many connection strings name it.
    Down(usize),
    /// Its databases aren't read yet.
    Reading,
}

/// One line of the card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Row {
    Server {
        server: String,
        state: Server,
    },
    /// On the server and in a connection string; `active` the one in use.
    Both {
        server: String,
        database: String,
        active: bool,
    },
    /// On the server, in no connection string.
    Missing {
        server: String,
        database: String,
    },
    /// In a connection string, not on the server (`down`: the server doesn't answer at all); `active` the one in use.
    Gone {
        server: String,
        database: String,
        down: bool,
        active: bool,
    },
}

impl Row {
    fn pickable(&self) -> bool {
        !matches!(self, Self::Server { .. })
    }
}

/// `a`: opens the card on the first row it can act on.
pub fn open(ui: &mut Ui, controller: &mut Controller) {
    let rows = rows_of(ui, controller);
    let index = rows.iter().position(|row| matches!(row, Row::Missing { .. } | Row::Gone { active: false, .. }));
    let index = index.or_else(|| rows.iter().position(Row::pickable)).unwrap_or(0);
    super::pull_view::open(ui, Card::Sync(Box::new(SyncCard { index, changed: None })));
}

/// Every connection string's server and database, the one in use marked, each pair once.
fn connections(ui: &Ui) -> Vec<Entry> {
    let Some(picker) = ui.picker.as_ref() else { return Vec::new() };
    let mut all: Vec<Entry> = picker.groups.iter().flat_map(|(_, entries)| entries.iter().cloned()).collect();
    all.sort_by_key(|entry| std::cmp::Reverse(entry.active));
    let mut seen: Vec<(String, String)> = Vec::new();
    all.retain(|entry| {
        let key = (entry.server.to_lowercase(), entry.database.to_lowercase());
        let new = !seen.contains(&key);
        seen.push(key);
        new
    });
    all
}

/// The card's lines: each server the connection strings name, then its databases compared with them.
pub fn rows_of(ui: &Ui, controller: &Controller) -> Vec<Row> {
    let all = connections(ui);
    let mut servers: Vec<String> = all.iter().map(|entry| entry.server.clone()).collect();
    servers.sort_by_key(|server| server.to_lowercase());
    servers.dedup_by(|one, other| one.eq_ignore_ascii_case(other));
    let mut rows = Vec::new();
    for server in servers {
        let named: Vec<&Entry> = all.iter().filter(|entry| entry.server.eq_ignore_ascii_case(&server)).collect();
        let down = super::database::reach_of(ui, &server) == Some(Reach::Down);
        let sizes = controller.sizes(&server).filter(|sizes| sizes.at.is_some() && sizes.error.is_none());
        if down {
            rows.push(Row::Server { server: server.clone(), state: Server::Down(named.len()) });
            for entry in &named {
                let (database, active) = (entry.database.clone(), entry.active);
                rows.push(Row::Gone { server: server.clone(), database, down: true, active });
            }
            continue;
        }
        let Some(sizes) = sizes else {
            rows.push(Row::Server { server, state: Server::Reading });
            continue;
        };
        let on: Vec<String> = sizes
            .databases
            .iter()
            .map(|(name, _)| name.clone())
            .filter(|name| !files::SYSTEM.iter().any(|system| system.eq_ignore_ascii_case(name)))
            .collect();
        rows.push(Row::Server { server: server.clone(), state: Server::Answers(on.len()) });
        let mut names: Vec<String> = on.clone();
        names.extend(named.iter().map(|entry| entry.database.clone()));
        names.sort_by_key(|name| name.to_lowercase());
        names.dedup_by(|one, other| one.eq_ignore_ascii_case(other));
        for database in names {
            let entry = named.iter().find(|entry| entry.database.eq_ignore_ascii_case(&database));
            let there = on.iter().any(|name| name.eq_ignore_ascii_case(&database));
            let server = server.clone();
            rows.push(match (entry, there) {
                (Some(entry), true) => Row::Both { server, database, active: entry.active },
                (Some(entry), false) => Row::Gone { server, database, down: false, active: entry.active },
                (None, _) => Row::Missing { server, database },
            });
        }
    }
    rows
}

/// The `+` rows: on a server, in no connection string.
fn missing(rows: &[Row]) -> Vec<(String, String)> {
    rows.iter()
        .filter_map(|row| match row {
            Row::Missing { server, database } => Some((server.clone(), database.clone())),
            _ => None,
        })
        .collect()
}

/// The `✗` rows not in use.
fn gone(rows: &[Row]) -> Vec<(String, String)> {
    rows.iter()
        .filter_map(|row| match row {
            Row::Gone { server, database, active: false, .. } => Some((server.clone(), database.clone())),
            _ => None,
        })
        .collect()
}

/// Adds a connection string for each database: a copy of one to its server with the database's name, not in use.
fn add(ui: &mut Ui, controller: &mut Controller, wanted: &[(String, String)]) {
    let Some(picker) = ui.picker.as_ref() else { return };
    let path = picker.path.clone();
    let current: Vec<Entry> = picker.groups.get(picker.group).map(|(_, entries)| entries.clone()).unwrap_or_default();
    let every = connections(ui);
    let mut added = 0;
    for (server, database) in wanted {
        let template =
            current.iter().chain(every.iter()).find(|entry| entry.server.eq_ignore_ascii_case(server)).cloned();
        let Some(template) = template else { continue };
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let at = secrets::connections(&text)
            .into_iter()
            .find(|entry| entry.key == template.key && entry.server.eq_ignore_ascii_case(server))
            .unwrap_or(template);
        match secrets::add(&path, &at, database) {
            Ok(()) => added += 1,
            Err(error) => controller.add_log(format!("✗ {database} @ {server}: {error}"), Hue::Red),
        }
    }
    if added > 0 {
        let what = if added == 1 { "a connection string".to_string() } else { format!("{added} connection strings") };
        controller.add_log(format!("Added {what} to secrets.json (not in use) - Enter switches to one"), Hue::Cyan);
    }
    super::database_files::reload(ui, None);
}

/// Takes the connection strings out of secrets.json, a server at a time.
fn take_out(ui: &mut Ui, controller: &mut Controller, wanted: &[(String, String)]) {
    let mut servers: Vec<String> = wanted.iter().map(|(server, _)| server.clone()).collect();
    servers.dedup_by(|one, other| one.eq_ignore_ascii_case(other));
    for server in servers {
        let databases: Vec<String> = wanted
            .iter()
            .filter(|(of, _)| of.eq_ignore_ascii_case(&server))
            .map(|(_, database)| database.clone())
            .collect();
        super::database_files::forget(ui, controller, &server, &databases);
    }
}

/// `Enter` on a `●`: the card closes on its row in the list.
fn go_to(ui: &mut Ui, server: &str, database: &str) {
    ui.overlay = None;
    let Some(picker) = ui.picker.as_mut() else { return };
    let named =
        |entry: &Entry| entry.server.eq_ignore_ascii_case(server) && entry.database.eq_ignore_ascii_case(database);
    let Some(group) = picker.groups.iter().position(|(_, entries)| entries.iter().any(named)) else { return };
    picker.group = group;
    picker.filter.clear();
    if let Some(at) = picker.entries().iter().position(named) {
        picker.index = at;
    }
}

/// The next row a key can act on from `at`, a row at a time in `step`'s direction, going round.
fn step_to(rows: &[Row], at: usize, step: isize) -> usize {
    let count = rows.len().max(1) as isize;
    let mut next = at as isize;
    for _ in 0..count {
        next = (next + step).rem_euclid(count);
        if rows.get(next as usize).is_some_and(Row::pickable) {
            return next as usize;
        }
    }
    at
}

/// The card's keys.
pub fn key(ui: &mut Ui, controller: &mut Controller, name: &str) {
    let rows = rows_of(ui, controller);
    let now = ui.now;
    let Some(Overlay::Pull(card)) = ui.overlay.as_mut() else { return };
    let Card::Sync(sync) = card.as_mut() else { return };
    let picked = rows.get(sync.index).cloned();
    match name {
        "j" | "DownArrow" => sync.index = step_to(&rows, sync.index, 1),
        "k" | "UpArrow" => sync.index = step_to(&rows, sync.index, -1),
        "Escape" | "q" | "a" => ui.overlay = None,
        "+" => {
            sync.changed = Some(now);
            add(ui, controller, &missing(&rows));
        }
        "-" | "Delete" => {
            sync.changed = Some(now);
            take_out(ui, controller, &gone(&rows));
        }
        "r" => {
            sync.changed = Some(now);
            take_out(ui, controller, &gone(&rows));
            add(ui, controller, &missing(&rows));
        }
        "Enter" => match picked {
            Some(Row::Missing { server, database }) => {
                sync.changed = Some(now);
                add(ui, controller, &[(server, database)]);
            }
            Some(Row::Gone { server, database, active: false, .. }) => {
                sync.changed = Some(now);
                take_out(ui, controller, &[(server, database)]);
            }
            Some(Row::Gone { database, active: true, .. }) => controller.add_log(
                format!("{database} is the one in use - switch to another database before taking it out"),
                Hue::Yellow,
            ),
            Some(Row::Both { server, database, .. }) => go_to(ui, &server, &database),
            _ => {}
        },
        _ => {}
    }
    let after = rows_of(ui, controller);
    if let Some(Overlay::Pull(card)) = ui.overlay.as_mut()
        && let Card::Sync(sync) = card.as_mut()
        && !after.get(sync.index).is_some_and(Row::pickable)
    {
        let back = sync.index.min(after.len().saturating_sub(1));
        sync.index = if after.get(back).is_some_and(Row::pickable) { back } else { step_to(&after, back, -1) };
    }
}

/// Whether the card moves: a row glinting after a key changed what it lists.
pub fn animating(ui: &Ui, sync: &SyncCard) -> bool {
    !ui.reduced_motion && sync.changed.is_some_and(|at| ui.now.saturating_duration_since(at) < GLINT)
}

/// A row's mark (two cells, then its sign), text and colour.
fn line(row: &Row, picked: bool) -> Vec<Seg> {
    let mark = if picked { " ▸ " } else { "   " };
    match row {
        Row::Server { server, state: Server::Answers(count) } => {
            vec![seg(format!(" ● {server}"), C::White), seg(format!(" · {count} databases"), C::DarkGray)]
        }
        Row::Server { server, state: Server::Down(count) } => vec![
            seg(format!(" ✗ {server}"), C::Red),
            seg(format!(" doesn't answer · {count} connection strings name it"), C::DarkGray),
        ],
        Row::Server { server, state: Server::Reading } => {
            vec![seg(format!(" ◌ {server}"), C::Gray), seg(" · its databases aren't read yet", C::DarkGray)]
        }
        Row::Both { database, active, .. } => {
            let mut line = vec![seg(format!("{mark}● {database}"), C::Green)];
            if *active {
                line.push(seg("  in use", C::DarkGreen));
            }
            line
        }
        Row::Missing { database, .. } => {
            vec![seg(format!("{mark}+ {database}"), C::Cyan), seg("  not in secrets.json", C::DarkCyan)]
        }
        Row::Gone { database, down, active, .. } => {
            let why = match (down, active) {
                (_, true) => "  in use - switch first",
                (true, false) => "",
                (false, false) => "  gone from the server",
            };
            vec![seg(format!("{mark}✗ {database}"), C::Red), seg(why, C::DarkRed)]
        }
    }
}

/// The card, centred over the database screen.
pub fn rows(ui: &Ui, controller: &Controller, sync: &SyncCard, width: usize, rows: usize) -> Vec<Vec<Seg>> {
    let all = rows_of(ui, controller);
    let index = sync.index.min(all.len().saturating_sub(1));
    let top = (index + 1).saturating_sub(ROWS_MOST);
    let mut lines: Vec<Vec<Seg>> = all
        .iter()
        .enumerate()
        .skip(top)
        .take(ROWS_MOST)
        .map(|(at, row)| {
            let picked = at == index && row.pickable();
            if picked { focus_row(line(row, true), INNER) } else { line(row, false) }
        })
        .collect();
    let since = sync.changed.filter(|_| !ui.reduced_motion).map(|at| ui.now.saturating_duration_since(at));
    if let Some(since) = since.filter(|since| *since < GLINT)
        && let Some(line) = lines.get_mut(index.saturating_sub(top))
    {
        let through = since.as_secs_f64() / GLINT.as_secs_f64();
        *line = super::seg::sweep(line.clone(), through, INNER, C::Black, C::Cyan);
    }
    let more = all.len().saturating_sub(top + ROWS_MOST);
    if more > 0 {
        lines.push(vec![seg(format!("   … {more} more (j k)"), C::DarkGray)]);
    }
    let (adds, outs) = (missing(&all).len(), gone(&all).len());
    lines.push(Vec::new());
    lines.push(vec![
        seg(format!(" {adds} to add · {outs} to take out"), C::Gray),
        seg(" · nothing on a server is touched", C::DarkGray),
    ]);
    lines.push(vec![seg(" Enter this row · + add all · - take all out · r both · j k pick · Esc close", C::DarkGray)]);
    boxed_in(C::Cyan, " Servers and secrets.json", lines, "", width, rows, INNER)
}
