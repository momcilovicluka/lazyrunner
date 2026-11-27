//! Linking a local database to its remote twin (#114): `L` on a local database (or `l` on a remote one in the Sources
//! section, the other way round) opens a card of the other side's databases, the closest first - the same name, the
//! same without a pull's suffix, then the most alike - the suggested one lit with its matching part underlined.
//! `Enter` links it, `j` `k` pick another; `L` on a linked database unlinks it. Once linked, its row's `↔` draws
//! itself in (`database`).

use std::time::{Duration, Instant};

use lazyrunner_core::output::Hue;
use lazyrunner_core::run::Controller;
use lazyrunner_core::run::controller::save_points::db_key;
use lazyrunner_core::run::db_links::{self, Remote, common_start};

use super::icons::{self, Icon};
use super::overlays::boxed_in;
use super::pull_view::Card;
use super::seg::{Seg, focus_row, seg};
use super::theme::C;
use super::{Overlay, Ui};

/// The card's text at most this wide; its choices at most this many.
const INNER: usize = 60;
const CHOICES_MOST: usize = 10;
/// The suggestion's light pulses once over this long as the card opens.
const LIT: Duration = Duration::from_millis(600);

/// A pair the card may link: a local database (its server and name) and a remote one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pair {
    pub local: (String, String),
    pub remote: Remote,
}

/// The card: what's being linked (a local database, or a remote one), its choices, the one picked, when it opened.
#[derive(Debug)]
pub struct LinkCard {
    pub from_local: bool,
    pub title: String,
    pub choices: Vec<Pair>,
    pub index: usize,
    pub opened: Instant,
}

/// `L` on a local database: unlinks a linked one, else opens the card of remote databases, the closest first.
pub fn open_for_local(ui: &mut Ui, controller: &mut Controller, server: &str, database: &str) {
    if controller.link_of(server, database).is_some() {
        if let Err(why) = controller.unlink_database(server, database) {
            controller.add_log(format!("✗ {why}"), Hue::Red);
        }
        return;
    }
    let remotes = controller.suggest_twins(database);
    if remotes.is_empty() {
        controller.add_log(
            "No remote databases to link to yet - a source names one, or a whole server lists its own (Sources, Enter)",
            Hue::Gray,
        );
        return;
    }
    let local = (server.to_string(), database.to_string());
    let choices = remotes.into_iter().map(|remote| Pair { local: local.clone(), remote }).collect();
    let card = LinkCard { from_local: true, title: format!("Link {database} to"), choices, index: 0, opened: ui.now };
    super::pull_view::open(ui, Card::Link(Box::new(card)));
}

/// `l` on a remote database: the card of local databases, the closest to it first.
pub fn open_for_remote(ui: &mut Ui, controller: &mut Controller, source: &str, database: &str) {
    let Some(picker) = ui.picker.as_ref() else { return };
    let mut locals: Vec<(String, String)> = picker
        .groups
        .iter()
        .flat_map(|(_, entries)| entries.iter().map(|entry| (entry.server.clone(), entry.database.clone())))
        .collect();
    locals
        .sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()).then(a.1.to_lowercase().cmp(&b.1.to_lowercase())));
    locals.dedup_by(|a, b| a.0.eq_ignore_ascii_case(&b.0) && a.1.eq_ignore_ascii_case(&b.1));
    locals.sort_by_key(|(_, local)| db_links::rank(local, database));
    if locals.is_empty() {
        controller.add_log("No local databases to link to - n adds a connection string", Hue::Gray);
        return;
    }
    let remote = Remote { source: source.to_string(), database: database.to_string() };
    let choices = locals.into_iter().map(|local| Pair { local, remote: remote.clone() }).collect();
    let card = LinkCard {
        from_local: false,
        title: format!("Link {source}/{database} to"),
        choices,
        index: 0,
        opened: ui.now,
    };
    super::pull_view::open(ui, Card::Link(Box::new(card)));
}

/// The card's keys.
pub fn key(ui: &mut Ui, controller: &mut Controller, name: &str) {
    let Some(Overlay::Pull(card)) = ui.overlay.as_mut() else { return };
    let Card::Link(link) = card.as_mut() else { return };
    let count = link.choices.len().max(1);
    match name {
        "j" | "DownArrow" => link.index = (link.index + 1) % count,
        "k" | "UpArrow" => link.index = (link.index + count - 1) % count,
        "Escape" | "q" => ui.overlay = None,
        "Enter" | "l" | "L" => {
            let Some(pair) = link.choices.get(link.index).cloned() else { return };
            ui.overlay = None;
            let (server, database) = (&pair.local.0, &pair.local.1);
            let pulled = controller.link_of(server, database).and_then(|link| link.pulled.clone());
            match controller.link_database((server, database), (&pair.remote.source, &pair.remote.database), pulled) {
                Ok(_) => ui.sources_view.linked = Some((db_key(server, database), ui.now)),
                Err(why) => controller.add_log(format!("✗ {why}"), Hue::Red),
            }
        }
        _ => {}
    }
}

/// Whether the card moves: its suggestion's light as it opens.
pub fn animating(ui: &Ui, link: &LinkCard) -> bool {
    !ui.reduced_motion && ui.motion.enabled && ui.now.saturating_duration_since(link.opened) < LIT
}

/// A choice's row: the other side's name, its part that matches underlined; the suggestion (the first) lit.
fn choice_row(link: &LinkCard, at: usize, pair: &Pair, lit: bool) -> Vec<Seg> {
    let (name, other) = match link.from_local {
        true => (format!("{}/{}", pair.remote.source, pair.remote.database), &pair.local.1),
        false => (format!("{} @ {}", pair.local.1, pair.local.0), &pair.remote.database),
    };
    let skip = if link.from_local { pair.remote.source.chars().count() + 1 } else { 0 };
    let matched = if at == 0 { common_start(&name.chars().skip(skip).collect::<String>(), other) } else { 0 };
    let chars: Vec<char> = name.chars().collect();
    let head: String = chars[..skip.min(chars.len())].iter().collect();
    let part: String = chars.iter().skip(skip).take(matched).collect();
    let rest: String = chars.iter().skip(skip + matched).collect();
    let color = if at == link.index { C::White } else { C::Gray };
    let mark = if at == link.index { " ▸ " } else { "   " };
    let mut line = vec![seg(mark, color), seg(head, color)];
    if !part.is_empty() {
        let mut underlined = seg(part, if lit { C::Cyan } else { color });
        underlined.underline = true;
        line.push(underlined);
    }
    line.push(seg(rest, color));
    if at == 0 {
        line.push(seg("   suggested", C::DarkCyan));
    }
    if at == link.index { focus_row(line, INNER) } else { line }
}

/// The card, centred over the database screen.
pub fn rows(ui: &Ui, link: &LinkCard, width: usize, rows: usize) -> Vec<Vec<Seg>> {
    let lit = !ui.reduced_motion && ui.motion.enabled && ui.now.saturating_duration_since(link.opened) < LIT;
    let top = (link.index + 1).saturating_sub(CHOICES_MOST);
    let mut lines: Vec<Vec<Seg>> = link
        .choices
        .iter()
        .enumerate()
        .skip(top)
        .take(CHOICES_MOST)
        .map(|(at, pair)| choice_row(link, at, pair, lit))
        .collect();
    let more = link.choices.len().saturating_sub(CHOICES_MOST);
    if more > 0 {
        lines.push(vec![seg(format!("   … {more} more (j k)"), C::DarkGray)]);
    }
    lines.push(Vec::new());
    lines.push(vec![seg(" Enter link · j k pick another · Esc cancel", C::DarkGray)]);
    boxed_in(C::Cyan, &icons::titled(ui.nerd, Icon::Link, &link.title), lines, "", width, rows, INNER)
}
