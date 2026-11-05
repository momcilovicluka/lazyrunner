//! The port map (#81): every port the apps want, who holds it now - nothing, its own app running here, something
//! else, or a range Windows reserved - and who held it over the last minute, a cell every two seconds. Kept from the
//! listening snapshots the controller takes anyway; nothing is sampled for it.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use super::net::{self, Listening, PortRange};

/// A port's history: a cell per `CELL`, the last `CELLS` kept (a minute).
pub const CELL: Duration = Duration::from_secs(2);
pub const CELLS: usize = 30;
/// Something else holding a stopped app's port for this long is worth saying (not a server that's just stopping).
pub const TAKEN_AFTER: Duration = Duration::from_secs(3);
/// The command lines kept for processes that held a port, at most.
const COMMANDS_KEPT: usize = 64;

/// Who holds a port.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Holding {
    Free,
    /// Its app, running in lazyrunner.
    Own,
    /// Something else: a process lazyrunner didn't start.
    Other,
    /// Nothing, but Windows reserved it: no app can listen on it.
    Reserved,
}

impl Holding {
    pub fn name(self) -> &'static str {
        match self {
            Self::Free => "free",
            Self::Own => "own",
            Self::Other => "other",
            Self::Reserved => "reserved",
        }
    }
}

/// The process holding a port: its PID, name and (when it could be read) command line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Holder {
    pub pid: u32,
    pub name: String,
    pub command: Option<String>,
}

/// An app's port, as the map shows it.
#[derive(Clone, Debug)]
pub struct Port {
    pub key: String,
    pub port: u16,
    pub holding: Holding,
    pub holder: Option<Holder>,
    /// Since when it's been as it is (held by this process, free, reserved).
    pub since: Instant,
    /// What it was before it last changed, and when; none until it does.
    pub changed: Option<(Holding, Instant)>,
    /// Who held it before it last changed.
    pub was: Option<Holder>,
    /// Who held it every `CELL`, oldest first.
    pub cells: VecDeque<Holding>,
    cell_at: Instant,
}

/// The apps' ports, and the ranges Windows reserved as last read.
#[derive(Clone, Debug, Default)]
pub struct Ports {
    pub rows: Vec<Port>,
    pub reserved: Vec<PortRange>,
    commands: HashMap<u32, String>,
}

impl Ports {
    /// A new snapshot: `wanted` is each app's key, port and whether it runs here, in the apps' order. A snapshot that
    /// couldn't list the ports changes nothing.
    pub fn update(&mut self, wanted: &[(String, u16, bool)], listening: &Listening, now: Instant) {
        if listening.problem.is_some() {
            return;
        }
        self.remember(&listening.commands);
        let mut old: HashMap<String, Port> = self.rows.drain(..).map(|row| (row.key.clone(), row)).collect();
        for (key, port, running) in wanted {
            let owner = listening.ports.get(port);
            let holding = match (owner, running) {
                (Some(_), true) => Holding::Own,
                (Some(_), false) => Holding::Other,
                (None, _) if net::reserved(*port, &self.reserved).is_some() => Holding::Reserved,
                (None, _) => Holding::Free,
            };
            let holder = owner.map(|owner| Holder {
                pid: owner.pid,
                name: owner.name.clone(),
                command: self.commands.get(&owner.pid).cloned(),
            });
            let mut row = match old.remove(key).filter(|row| row.port == *port) {
                Some(mut row) => {
                    let pid = |holder: &Option<Holder>| holder.as_ref().map(|holder| holder.pid);
                    if row.holding != holding {
                        row.changed = Some((row.holding, now));
                        row.was = row.holder.take();
                    }
                    if row.holding != holding || pid(&row.holder) != pid(&holder) {
                        row.since = now;
                    }
                    Port { holding, holder, ..row }
                }
                None => Port {
                    key: key.clone(),
                    port: *port,
                    holding,
                    holder,
                    since: now,
                    changed: None,
                    was: None,
                    cells: VecDeque::new(),
                    cell_at: now,
                },
            };
            if row.cells.is_empty() || now.saturating_duration_since(row.cell_at) >= CELL {
                row.cells.push_back(holding);
                row.cell_at = now;
                while row.cells.len() > CELLS {
                    row.cells.pop_front();
                }
            }
            self.rows.push(row);
        }
    }

    /// The command lines a snapshot read, kept for the processes still holding a port.
    fn remember(&mut self, read: &HashMap<u32, String>) {
        self.commands.extend(read.iter().map(|(pid, command)| (*pid, command.clone())));
        if self.commands.len() > COMMANDS_KEPT {
            let held: Vec<u32> =
                self.rows.iter().filter_map(|row| row.holder.as_ref().map(|holder| holder.pid)).collect();
            self.commands.retain(|pid, _| held.contains(pid) || read.contains_key(pid));
        }
    }

    /// The PIDs whose command line is known: a snapshot doesn't read those again.
    pub fn known(&self) -> Vec<u32> {
        self.commands.keys().copied().collect()
    }

    /// The ports of stopped apps that something else has held for a while: worth knowing before the next start.
    pub fn taken(&self, now: Instant) -> Vec<&Port> {
        self.rows
            .iter()
            .filter(|row| row.holding == Holding::Other && now.saturating_duration_since(row.since) >= TAKEN_AFTER)
            .collect()
    }

    /// How many ports are wanted, held by something else, and reserved.
    pub fn summary(&self) -> (usize, usize, usize) {
        let count = |holding| self.rows.iter().filter(|row| row.holding == holding).count();
        (self.rows.len(), count(Holding::Other), count(Holding::Reserved))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run::net::Owner;

    fn listening(held: &[(u16, u32)]) -> Listening {
        let mut listening = Listening::default();
        for (port, pid) in held {
            listening.ports.insert(*port, Owner { pid: *pid, name: "node.exe".into() });
        }
        listening
    }

    fn wanted(running: bool) -> Vec<(String, u16, bool)> {
        vec![("orders".into(), 47201, running), ("shop".into(), 47202, false), ("billing".into(), 49710, false)]
    }

    #[test]
    fn each_port_says_who_holds_it_and_since_when() {
        let mut ports = Ports { reserved: vec![(49700, 49799)], ..Ports::default() };
        let start = Instant::now();
        let mut held = listening(&[(47201, 10), (47202, 20)]);
        held.commands.insert(20, "node vite --port 47202".into());
        ports.update(&wanted(true), &held, start);
        let holdings: Vec<Holding> = ports.rows.iter().map(|row| row.holding).collect();
        assert_eq!(holdings, [Holding::Own, Holding::Other, Holding::Reserved]);
        let shop = &ports.rows[1];
        assert_eq!(shop.holder.as_ref().and_then(|holder| holder.command.as_deref()), Some("node vite --port 47202"));
        assert_eq!(ports.summary(), (3, 1, 1));
        assert!(ports.taken(start).is_empty(), "not yet: it may be a server stopping");
        assert_eq!(ports.taken(start + TAKEN_AFTER).len(), 1);
        assert_eq!(ports.known(), [20], "its command line isn't read again");

        let later = start + Duration::from_secs(1);
        ports.update(&wanted(true), &listening(&[(47201, 10)]), later);
        let shop = &ports.rows[1];
        assert_eq!((shop.holding, shop.since), (Holding::Free, later));
        assert_eq!(shop.changed, Some((Holding::Other, later)));
        assert_eq!(shop.was.as_ref().map(|holder| holder.pid), Some(20), "the wipe says who it was");
        assert_eq!(shop.cells.len(), 1, "a cell every two seconds, not every snapshot");
        ports.update(&wanted(true), &listening(&[(47201, 10)]), start + CELL);
        assert_eq!(ports.rows[1].cells, [Holding::Other, Holding::Free]);
        assert_eq!(ports.rows[0].since, start, "held by the same process all along");
    }

    #[test]
    fn the_history_keeps_a_minute_and_a_failed_snapshot_changes_nothing() {
        let mut ports = Ports::default();
        let start = Instant::now();
        for step in 0..40u32 {
            ports.update(&wanted(false), &listening(&[]), start + CELL * step);
        }
        assert_eq!(ports.rows[0].cells.len(), CELLS);
        let broken = Listening { problem: Some("denied".into()), ..Listening::default() };
        ports.update(&wanted(false), &broken, start + CELL * 41);
        assert_eq!(ports.rows[0].cells.len(), CELLS);
        assert_eq!(ports.rows[0].holding, Holding::Free);
    }
}
