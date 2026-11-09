//! The code map's search (#89): `/` finds any part on any level - an app, project, namespace, folder, module or
//! component - or a source file, by its name; the best first (the name itself, then one it starts, then one it holds;
//! parts before files, this level's first). `Enter` goes to the one chosen and shows its graph: all it depends on and
//! all that depends on it.

use crossterm::event::KeyEvent;

use lazyrunner_core::run::Controller;
use lazyrunner_core::run::code::{CodeMap, Kind};

use super::Ui;
use super::code::{View, show_graph, view_mut};
use super::code_draw::{icon, place};
use super::seg::{Seg, seg};
use super::theme::C;

/// The matches listed at once, and kept at most.
const LISTED: usize = 8;
const KEPT: usize = 200;

/// A match: a part, or a source file (by its place in the map's files).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Found {
    Part(usize),
    File(usize),
}

/// What's found by the query, best first.
pub(super) fn matches(map: &CodeMap, view: &View) -> Vec<Found> {
    let query = view.query.to_lowercase();
    if query.is_empty() {
        return Vec::new();
    }
    let rank = |name: &str| {
        let name = name.to_lowercase();
        match () {
            _ if name == query => Some(0),
            _ if name.starts_with(&query) => Some(1),
            _ if name.contains(&query) => Some(2),
            _ => None,
        }
    };
    let elsewhere = |at: usize| map.parts[at].inside != view.inside;
    let mut found: Vec<((usize, usize, bool, usize), Found)> = Vec::new();
    for (at, part) in map.parts.iter().enumerate() {
        if let Some(rank) = rank(&part.name) {
            found.push(((rank, 0, elsewhere(at), part.name.len()), Found::Part(at)));
        }
    }
    for (at, file) in map.files.iter().enumerate() {
        let name = file.path.file_name().map(|name| name.to_string_lossy()).unwrap_or_default();
        if let Some(rank) = rank(&name) {
            found.push(((rank, 1, elsewhere(file.part), name.len()), Found::File(at)));
        }
    }
    found.sort_by_key(|(key, _)| *key);
    found.truncate(KEPT);
    found.into_iter().map(|(_, found)| found).collect()
}

/// The part a match goes to: a file's module or component, else its namespace or folder.
pub(super) fn target(map: &CodeMap, found: Found) -> usize {
    match found {
        Found::Part(at) => at,
        Found::File(at) => {
            let file = &map.files[at];
            let declared = map
                .parts
                .iter()
                .position(|part| matches!(part.kind, Kind::Module | Kind::Component) && part.path == file.path);
            declared.unwrap_or(file.part)
        }
    }
}

/// The keys while searching: typing, `↑` `↓` choose, `Enter` goes to the chosen one, `Esc` stops.
pub(super) fn key(ui: &mut Ui, controller: &Controller, name: &str, key: &KeyEvent) {
    let count = match (controller.code_map(), super::code::view(ui)) {
        (Some(map), Some(view)) => matches(map, view).len(),
        _ => 0,
    };
    let Some(view) = view_mut(ui) else { return };
    match name {
        "Escape" => (view.typing, view.query, view.choice) = (false, String::new(), 0),
        "UpArrow" => view.choice = view.choice.saturating_sub(1),
        "DownArrow" => view.choice = (view.choice + 1).min(count.saturating_sub(1)),
        "Backspace" => {
            view.query.pop();
            view.choice = 0;
        }
        "Enter" => {
            view.typing = false;
            let current = view.clone();
            let Some(map) = controller.code_map() else { return };
            let found = matches(map, &current);
            if let Some(&chosen) = found.get(current.choice.min(found.len().saturating_sub(1))) {
                show_graph(ui, map, target(map, chosen));
            }
        }
        _ => {
            view.query.extend(super::keys::typed(key));
            view.choice = 0;
        }
    }
}

/// A match's line: its name and what and where it is.
fn line(ui: &Ui, map: &CodeMap, found: Found, chosen: bool) -> Vec<Seg> {
    let (name, what) = match found {
        Found::Part(at) => {
            let part = &map.parts[at];
            (format!("{} {}", icon(ui.nerd, part), part.name), format!("{} · {}", part.kind.name(), place(map, at)))
        }
        Found::File(at) => {
            let file = &map.files[at].path;
            let name = file.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
            (name, format!("file · in {}", map.parts[target(map, found)].name))
        }
    };
    let mut line = vec![
        seg(if chosen { " ▸ " } else { "   " }, C::Cyan),
        seg(name, if chosen { C::White } else { C::Gray }),
        seg(format!("   {what}"), C::DarkGray),
    ];
    if chosen {
        for part in &mut line {
            part.bg = Some(C::FocusBack);
        }
    }
    line
}

/// Under the header while searching: the matches (the chosen one lit), and the keys.
pub(super) fn rows(ui: &Ui, map: &CodeMap, view: &View, width: usize) -> Vec<Vec<Seg>> {
    let found = matches(map, view);
    if view.query.is_empty() {
        let text = "  Type a name: an app, project, namespace, folder, module, component or file.";
        return vec![vec![seg(text, C::DarkGray)]];
    }
    if found.is_empty() {
        return vec![vec![seg("  Nothing by that name.", C::DarkGray)]];
    }
    let chosen = view.choice.min(found.len() - 1);
    let start = chosen.saturating_sub(LISTED - 1);
    let mut lines: Vec<Vec<Seg>> = found
        .iter()
        .enumerate()
        .skip(start)
        .take(LISTED)
        .map(|(at, &found)| super::columns::fit(line(ui, map, found, at == chosen), width))
        .collect();
    let keys = format!("  {} found · ↑ ↓ choose · Enter its graph · Esc stop", found.len());
    lines.push(vec![seg(keys, C::DarkGray)]);
    lines
}
