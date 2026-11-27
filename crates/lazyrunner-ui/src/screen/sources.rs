//! The database screen's Sources section (#114), under the files (`Tab` to it): the team's sources and this
//! machine's own (`only mine`) in one list. A whole server opens (`Enter`) into its databases - name, size, when it
//! last changed, and the local database linked to it - as last listed, with how long ago; `g` lists again and `/`
//! filters. On a remote database `p` pulls, `e` only exports, `i` imports without switching, `l` links it to a local
//! database and `+` keeps it as a source of its own; on a source `,` sets its password, `E` edits it, `Del` removes it
//! (asked) and `a` adds one. Its motion: a server's databases unfold row by row, fresher ones ticking in when it's
//! listed again; a listing's dot breathes; a saved source's row arrives with one glint, where it went flashing beside
//! it. With reduced motion everything steps.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use lazyrunner_core::output::Hue;
use lazyrunner_core::profile::sources::{DatabaseSource, Reach};
use lazyrunner_core::run::Controller;
use lazyrunner_core::run::catalog::Listed;
use lazyrunner_core::run::database_files as files;
use unicode_width::UnicodeWidthStr;

use super::icons::{self, Icon};
use super::migrations::{Confirm, Risk, SavePointYes};
use super::pull_view::{Purpose, Want};
use super::seg::{Seg, focus_row, pad, seg, sweep};
use super::theme::C;
use super::{Overlay, Ui};

/// A database row unfolds this long after the one above it, dim at first this long; a saved source's row glints
/// this long and where it went flashes this long; a listing's dot breathes once in this long.
const ROW_STAGGER: Duration = Duration::from_millis(35);
const ROW_DIM: Duration = Duration::from_millis(120);
const GLINT: Duration = super::motion::SWEEP;
const FLASH: Duration = Duration::from_millis(900);
const BREATH: Duration = Duration::from_millis(1200);
/// The section's rows at most while it hasn't the keys (its title aside), and its name column at least.
const ROWS_MOST: usize = 6;
const NAME_LEAST: usize = 10;
/// A database's name and size columns.
const DATABASE_LEAST: usize = 14;
const SIZE_CELLS: usize = 9;
const CHANGED_CELLS: usize = 20;
/// The rows' indent: a source's, and a database's under it.
const INDENT: &str = " ";
const DATABASE_INDENT: &str = "     ";

/// The Sources section's state: whether it has the keys, the row picked, the servers open (and when each opened),
/// the filter, and its motion - a source just saved (and where it went), a local database just linked or updated,
/// each source's list as last seen (a fresher one ticks in).
#[derive(Debug, Default)]
pub struct View {
    pub focused: bool,
    pub picked: usize,
    pub open: HashMap<String, Instant>,
    pub filter: String,
    pub filtering: bool,
    pub arrived: Option<(String, String, Instant)>,
    pub linked: Option<(String, Instant)>,
    pub updated: Option<(String, Instant)>,
    seen: HashMap<String, String>,
    ticked: HashMap<String, Instant>,
}

/// A row of the section: a source, or one of a whole server's databases.
#[derive(Clone, Debug, PartialEq)]
pub enum Row {
    Source(DatabaseSource),
    Database { source: DatabaseSource, listed: Listed },
}

impl Row {
    fn source(&self) -> &DatabaseSource {
        match self {
            Self::Source(source) | Self::Database { source, .. } => source,
        }
    }

    /// The remote database it is, when it is one: a database source's own, or one under a server.
    pub fn remote(&self) -> Option<(String, Option<String>)> {
        match self {
            Self::Source(source) if !source.is_server() => Some((source.name.clone(), None)),
            Self::Database { source, listed } => Some((source.name.clone(), Some(listed.name.clone()))),
            Self::Source(_) => None,
        }
    }
}

fn still(ui: &Ui) -> bool {
    ui.reduced_motion || !ui.motion.enabled
}

fn since(ui: &Ui, at: Instant) -> Duration {
    ui.now.saturating_duration_since(at)
}

fn key_of(name: &str) -> String {
    name.to_lowercase()
}

/// The rows shown: each source, and an open server's databases, as the filter has them.
pub fn rows_of(ui: &Ui, controller: &Controller) -> Vec<Row> {
    let filter = ui.sources_view.filter.to_lowercase();
    let matches = |text: &str| filter.is_empty() || text.to_lowercase().contains(&filter);
    let mut rows = Vec::new();
    for source in controller.sources() {
        let listed: Vec<Listed> = match ui.sources_view.open.contains_key(&key_of(&source.name)) {
            true => controller.catalog(&source.name).map(|catalog| catalog.databases.clone()).unwrap_or_default(),
            false => Vec::new(),
        };
        let shown: Vec<Listed> = listed.into_iter().filter(|listed| matches(&listed.name)).collect();
        if !matches(&source.name) && !matches(&source.label()) && shown.is_empty() {
            continue;
        }
        rows.push(Row::Source(source.clone()));
        rows.extend(shown.into_iter().map(|listed| Row::Database { source: source.clone(), listed }));
    }
    rows
}

/// The row picked, when the section has rows.
fn picked(ui: &Ui, controller: &Controller) -> Option<Row> {
    let rows = rows_of(ui, controller);
    rows.get(ui.sources_view.picked.min(rows.len().saturating_sub(1))).cloned()
}

/// Opens a source in the section (it gets the keys, the source picked): its databases listed when none are yet.
pub fn open_source(ui: &mut Ui, controller: &mut Controller, name: &str) {
    focus(ui);
    ui.sources_view.open.insert(key_of(name), ui.now);
    if let Some(at) = rows_of(ui, controller)
        .iter()
        .position(|row| matches!(row, Row::Source(source) if source.name.eq_ignore_ascii_case(name)))
    {
        ui.sources_view.picked = at;
    }
    if controller.catalog(name).is_none() && !controller.is_listing(name) {
        super::pull_view::proceed(ui, controller, Want::new(Purpose::List, name, None));
    }
}

/// The section gets the keys (the others' sections lose them).
pub fn focus(ui: &mut Ui) {
    (ui.save_view.on_rail, ui.files_view.on_files, ui.pull_view.focused) = (false, false, false);
    ui.sources_view.focused = true;
}

/// Whether a filter is being typed.
pub fn typing(ui: &Ui) -> bool {
    ui.picker.is_some() && ui.sources_view.focused && ui.sources_view.filtering
}

/// The section's keys while it has them; whether the key was one of them.
pub fn key(ui: &mut Ui, controller: &mut Controller, name: &str, typed: Option<char>) -> bool {
    if !ui.sources_view.focused {
        return false;
    }
    if ui.sources_view.filtering {
        let view = &mut ui.sources_view;
        match name {
            "Escape" => (view.filter, view.filtering, view.picked) = (String::new(), false, 0),
            "Enter" => view.filtering = false,
            "Backspace" => {
                view.filter.pop();
            }
            _ => {
                if let Some(c) = typed {
                    view.filter.push(c);
                    view.picked = 0;
                }
            }
        }
        return true;
    }
    let count = rows_of(ui, controller).len();
    let row = picked(ui, controller);
    match name {
        "j" | "DownArrow" => ui.sources_view.picked = (ui.sources_view.picked + 1).min(count.saturating_sub(1)),
        "k" | "UpArrow" => ui.sources_view.picked = ui.sources_view.picked.saturating_sub(1),
        "Escape" if !ui.sources_view.filter.is_empty() => ui.sources_view.filter.clear(),
        "Escape" => ui.sources_view.focused = false,
        "/" => ui.sources_view.filtering = true,
        "a" => super::source_form::open_new(ui, controller),
        _ => return row.is_some_and(|row| row_key(ui, controller, name, &row)),
    }
    true
}

/// The keys on a row; whether the key was one of them.
fn row_key(ui: &mut Ui, controller: &mut Controller, name: &str, row: &Row) -> bool {
    let source = row.source().clone();
    let remote = row.remote();
    let on_remote = |purpose: Purpose, ui: &mut Ui, controller: &mut Controller| match &remote {
        Some((source, database)) => {
            super::pull_view::proceed(ui, controller, Want::new(purpose, source, database.as_deref()));
        }
        None => controller
            .add_log(format!("{} is a whole server - Enter opens it, then pick a database", source.name), Hue::Gray),
    };
    match name {
        "Enter" if source.is_server() && matches!(row, Row::Source(_)) => toggle(ui, controller, &source.name),
        "Enter" => controller.add_log(
            format!(
                "{} - p pulls it · e exports it · i imports it · l links it",
                match row {
                    Row::Database { listed, .. } => format!("{}/{}", source.name, listed.name),
                    Row::Source(_) => source.label(),
                }
            ),
            Hue::Gray,
        ),
        "g" => super::pull_view::proceed(ui, controller, Want::new(Purpose::List, &source.name, None)),
        "p" => on_remote(Purpose::Pull, ui, controller),
        "e" => on_remote(Purpose::Export, ui, controller),
        "i" => on_remote(Purpose::Import, ui, controller),
        "l" | "L" => match remote {
            Some((source, database)) => {
                let database = database
                    .or_else(|| controller.source(&source).and_then(|found| found.database().map(String::from)));
                super::link_card::open_for_remote(ui, controller, &source, &database.unwrap_or_default());
            }
            None => on_remote(Purpose::Pull, ui, controller),
        },
        "+" => match row {
            Row::Database { listed, .. } => match controller.keep_as_source(&source.name, &listed.name) {
                Ok(said) => {
                    let kept = controller.sources().last().map(|found| found.name.clone()).unwrap_or_default();
                    let whose = if source.mine {
                        "only mine"
                    } else {
                        said.rsplit('(').next().unwrap_or_default().trim_end_matches(')')
                    };
                    arrived(ui, controller, &kept, whose);
                }
                Err(why) => controller.add_log(format!("✗ {why}"), Hue::Red),
            },
            Row::Source(_) => controller.add_log("+ keeps a database under a server as a source of its own", Hue::Gray),
        },
        "," => super::pull_view::proceed(ui, controller, Want::new(Purpose::Password, &source.name, None)),
        "E" => super::source_form::open_edit(ui, controller, &source.name),
        "Delete" => ask_remove(ui, &source),
        _ => return false,
    }
    true
}

/// `Enter` on a server: open (its databases listed when none are yet) or close.
fn toggle(ui: &mut Ui, controller: &mut Controller, name: &str) {
    if ui.sources_view.open.remove(&key_of(name)).is_some() {
        return;
    }
    open_source(ui, controller, name);
}

/// A source saved: its row picked, arriving with a glint, where it went flashing beside it.
pub fn arrived(ui: &mut Ui, controller: &Controller, name: &str, whose: &str) {
    focus(ui);
    ui.sources_view.filter.clear();
    if let Some(at) = rows_of(ui, controller)
        .iter()
        .position(|row| matches!(row, Row::Source(source) if source.name.eq_ignore_ascii_case(name)))
    {
        ui.sources_view.picked = at;
    }
    ui.sources_view.arrived = Some((key_of(name), whose.to_string(), ui.now));
}

fn ask_remove(ui: &mut Ui, source: &DatabaseSource) {
    let whose = if source.mine { "this machine's sources" } else { "the profile (the team's list)" };
    let lines = vec![
        format!("Remove {} ({}) from {whose}?", source.name, source.label()),
        "Nothing on its server is touched; a pulled copy and its link stay.".to_string(),
    ];
    let yes = SavePointYes::RemoveSource(source.name.clone());
    ui.behind = ui.overlay.take();
    ui.overlay =
        Some(Overlay::Confirm(Box::new(Confirm { lines, jobs: Vec::new(), risk: Risk::Apply, save_point: Some(yes) })));
}

/// The yes to removing a source: it's gone from its list; then, when its password is kept and no other source signs
/// in with it, an offer to forget that too.
pub fn remove(ui: &mut Ui, controller: &mut Controller, name: &str) {
    let found = controller.source(name);
    if let Err(why) = controller.remove_source(name) {
        controller.add_log(format!("✗ {why}"), Hue::Red);
        return;
    }
    ui.sources_view.picked = ui.sources_view.picked.saturating_sub(1);
    let Some(found) = found.filter(|found| found.needs_password() && !controller.password_shared(found)) else {
        return;
    };
    if controller.kept_password(&found).ok().flatten().is_none() {
        return;
    }
    let lines = vec![
        format!("Forget {}'s password in {} too?", found.name, controller.credential_store()),
        "n keeps it, for when the source is added again.".to_string(),
    ];
    let yes = SavePointYes::ForgetPassword(Box::new(found));
    ui.overlay =
        Some(Overlay::Confirm(Box::new(Confirm { lines, jobs: Vec::new(), risk: Risk::Apply, save_point: Some(yes) })));
}

/// The section's hint while it has the keys.
pub fn hint(ui: &Ui, controller: &Controller) -> Option<String> {
    if !ui.sources_view.focused {
        return None;
    }
    if ui.sources_view.filtering {
        return Some(" type to filter the sources · Enter keeps it · Esc clears it".into());
    }
    let hint = match picked(ui, controller) {
        Some(Row::Source(source)) if source.is_server() => {
            " Enter open · g list again · , password · E edit · Del remove · a add · / filter · Tab databases · Esc back"
        }
        Some(Row::Source(_)) => {
            " p pull · e export · i import · l link · , password · E edit · Del remove · a add · / filter · Esc back"
        }
        Some(Row::Database { .. }) => {
            " p pull · e export only · i import · l link · + keep as a source · g list again · / filter · Esc back"
        }
        None => " a add a source - paste its connection string, or fill it in · Tab databases · Esc back",
    };
    Some(hint.to_string())
}

/// The rows the section takes out of `left`: its title and a row for each (one when there are none), all of them
/// while it has the keys.
pub fn height(ui: &Ui, controller: &Controller, left: usize) -> usize {
    if left < 2 {
        return 0;
    }
    let count = rows_of(ui, controller).len().max(1)
        + usize::from(ui.sources_view.filtering || !ui.sources_view.filter.is_empty());
    let most = if ui.sources_view.focused { left - 1 } else { ROWS_MOST };
    1 + count.min(most).min(left - 1)
}

/// Whether anything of it moves: a listing's breathing dot, rows unfolding or ticking in, a glint, a flash.
pub fn animating(ui: &Ui, controller: &Controller) -> bool {
    if ui.picker.is_none() || still(ui) {
        return false;
    }
    let view = &ui.sources_view;
    let recent = |at: &Instant, long: Duration| since(ui, *at) < long;
    let unfolding = view.open.values().chain(view.ticked.values()).any(|at| recent(at, ROW_DIM + ROW_STAGGER * 40));
    !controller.listing.is_empty()
        || unfolding
        || view.arrived.as_ref().is_some_and(|(_, _, at)| recent(at, FLASH.max(GLINT)))
        || view.linked.as_ref().is_some_and(|(_, at)| recent(at, super::database::LINK_DRAW))
        || view.updated.as_ref().is_some_and(|(_, at)| recent(at, GLINT))
}

/// A whole server's list: being listed (its dot breathing), how many and how long ago, or not yet.
fn listed_cell(ui: &Ui, controller: &Controller, source: &DatabaseSource) -> Seg {
    if controller.is_listing(&source.name) {
        let phase = since(ui, ui.started).as_millis() % BREATH.as_millis();
        let lit = still(ui) || phase < BREATH.as_millis() / 2;
        return seg("◌ listing…", if lit { C::Yellow } else { C::DarkYellow });
    }
    match controller.catalog(&source.name) {
        Some(catalog) => {
            let age = ago(&catalog.at).map(|age| format!(" · listed {age}")).unwrap_or_default();
            let count = catalog.databases.len();
            seg(format!("{count} database{}{age}", if count == 1 { "" } else { "s" }), C::DarkGray)
        }
        None => seg("Enter lists its databases", C::DarkGray),
    }
}

/// How long ago an RFC 3339 time was: `3 min ago`.
fn ago(at: &str) -> Option<String> {
    let at = at.parse::<jiff::Timestamp>().ok()?;
    let seconds = jiff::Timestamp::now().duration_since(at).as_secs().max(0) as u64;
    Some(lazyrunner_core::run::session::ago(Duration::from_secs(seconds)))
}

/// Where a source is and how it logs in - never a password.
fn place(source: &DatabaseSource) -> String {
    match &source.reach {
        Reach::Server { server, database, user, auth } => {
            let login = user.clone().unwrap_or_else(|| auth.name().to_string());
            match database {
                Some(database) => format!("{server} · {database} · {login}"),
                None => format!("{server} · {login}"),
            }
        }
        Reach::Env(name) => format!("${name}"),
    }
}

/// The local database linked to a remote one, as its row says it: `↔ Shop_qa_20261120 · pulled 12 d ago`.
fn linked_local(controller: &Controller, source: &str, remote: &str) -> Option<String> {
    let link = controller
        .db_links
        .iter()
        .find(|link| link.source.eq_ignore_ascii_case(source) && link.remote.eq_ignore_ascii_case(remote))?;
    let age = link.pulled.as_deref().and_then(ago).map(|age| format!(" · pulled {age}")).unwrap_or_default();
    Some(format!("↔ {}{age}", link.database))
}

/// A source's row.
fn source_row(ui: &Ui, controller: &Controller, source: &DatabaseSource, name_cells: usize) -> Vec<Seg> {
    let open = ui.sources_view.open.contains_key(&key_of(&source.name));
    let fold = match (source.is_server(), open) {
        (true, true) => "▾ ",
        (true, false) => "▸ ",
        (false, _) => "  ",
    };
    let icon = icons::icon(ui.nerd, if source.is_server() { Icon::Server } else { Icon::Database });
    let icon_cells = [Icon::Server, Icon::Database].map(|icon| icons::icon(ui.nerd, icon).width()).into_iter().max();
    let mut line = vec![
        seg(format!("{INDENT}{fold}"), C::DarkGray),
        seg(format!("{} ", pad(icon, icon_cells.unwrap_or_default())), C::Cyan),
        seg(pad(&source.name, name_cells), C::White),
        seg(format!("  {}", place(source)), C::Gray),
    ];
    let right = match source.database() {
        _ if source.is_server() => listed_cell(ui, controller, source),
        Some(database) => linked_local(controller, &source.name, database)
            .map_or_else(|| seg("", C::Gray), |text| seg(text, C::DarkCyan)),
        None => seg("", C::Gray),
    };
    if !right.text.is_empty() {
        line.push(seg("   ", C::Gray));
        line.push(right);
    }
    let flash = ui
        .sources_view
        .arrived
        .as_ref()
        .filter(|(name, _, at)| *name == key_of(&source.name) && !still(ui) && since(ui, *at) < FLASH);
    if let Some((_, whose, _)) = flash {
        line.push(super::seg::on(format!(" {whose} "), C::Black, C::Cyan));
    } else if source.mine {
        line.push(seg("   only mine", C::DarkGray));
    }
    line
}

/// A database's row under its server.
fn database_row(controller: &Controller, source: &DatabaseSource, listed: &Listed, name_cells: usize) -> Vec<Seg> {
    let size = listed.bytes.map_or_else(|| "-".to_string(), files::size);
    let changed =
        listed.changed.as_deref().and_then(ago).map_or_else(|| "-".to_string(), |age| format!("changed {age}"));
    let mut line = vec![
        seg(DATABASE_INDENT, C::Gray),
        seg(pad(&listed.name, name_cells), C::Gray),
        seg(format!("  {:>width$}", size, width = SIZE_CELLS), C::Gray),
        seg(format!("   {}", pad(&changed, CHANGED_CELLS)), C::DarkGray),
    ];
    if let Some(local) = linked_local(controller, &source.name, &listed.name) {
        line.push(seg(local, C::DarkCyan));
    }
    line
}

/// Notes a source whose list is fresher than the one last shown: its rows tick in.
fn note_lists(ui: &mut Ui, controller: &Controller) {
    for (name, catalog) in &controller.catalogs {
        let key = key_of(name);
        let fresher = ui.sources_view.seen.get(&key).is_some_and(|seen| *seen != catalog.at);
        if fresher {
            ui.sources_view.ticked.insert(key.clone(), ui.now);
        }
        ui.sources_view.seen.insert(key, catalog.at.clone());
    }
}

/// The section, `rows` rows.
pub fn rows(ui: &mut Ui, controller: &Controller, cells: usize, rows: usize) -> Vec<Vec<Seg>> {
    if rows == 0 {
        return Vec::new();
    }
    note_lists(ui, controller);
    let all = rows_of(ui, controller);
    let view = &ui.sources_view;
    let title_color = if view.focused { C::Cyan } else { C::DarkCyan };
    let count = controller.sources().len();
    let mut lines = vec![vec![
        seg(" SOURCES", title_color),
        seg(
            match count {
                0 => "   none yet".to_string(),
                _ if view.focused => format!("   {count} · the team's and only mine"),
                _ => format!("   {count} · Tab here: Enter open · p pull · e export · l link · a add"),
            },
            C::DarkGray,
        ),
    ]];
    if view.filtering || !view.filter.is_empty() {
        let cursor = if view.filtering { ui.cursor().to_string() } else { String::new() };
        lines.push(vec![seg(format!(" / {}{cursor}", view.filter), C::Cyan)]);
    }
    if all.is_empty() {
        let said = if count == 0 {
            "   Tab here, then a adds one: a whole server or one database"
        } else {
            "   nothing matches"
        };
        lines.push(vec![seg(said, C::DarkGray)]);
        lines.truncate(rows);
        return lines;
    }
    let name_cells = all
        .iter()
        .filter_map(|row| match row {
            Row::Source(source) => Some(source.name.width()),
            Row::Database { .. } => None,
        })
        .max()
        .unwrap_or_default()
        .max(NAME_LEAST);
    let database_cells = all
        .iter()
        .filter_map(|row| match row {
            Row::Database { listed, .. } => Some(listed.name.width()),
            Row::Source(_) => None,
        })
        .max()
        .unwrap_or_default()
        .max(DATABASE_LEAST);
    let room = rows.saturating_sub(lines.len()).max(1);
    let picked = view.picked.min(all.len() - 1);
    let top = (picked + 1).saturating_sub(room);
    let mut under = 0usize;
    let mut last_source = String::new();
    for (at, row) in all.iter().enumerate().skip(top).take(room) {
        let mut line = match row {
            Row::Source(source) => {
                (under, last_source) = (0, key_of(&source.name));
                source_row(ui, controller, source, name_cells)
            }
            Row::Database { source, listed } => {
                under += 1;
                database_row(controller, source, listed, database_cells)
            }
        };
        line = super::columns::fit(line, cells);
        if let Row::Database { .. } = row {
            let started = view.open.get(&last_source).into_iter().chain(view.ticked.get(&last_source)).max().copied();
            if let Some(since_open) = started.filter(|_| !still(ui)).map(|at| since(ui, at)) {
                let comes = ROW_STAGGER * under as u32;
                if since_open < comes {
                    line = Vec::new();
                } else if since_open < comes + ROW_DIM {
                    line.iter_mut().for_each(|part| part.fg = C::DarkGray);
                }
            }
        }
        if let (Row::Source(source), Some((name, _, at))) = (row, &view.arrived)
            && *name == key_of(&source.name)
            && !still(ui)
            && since(ui, *at) < GLINT
        {
            line = sweep(line, since(ui, *at).as_secs_f64() / GLINT.as_secs_f64(), cells, C::Black, C::Cyan);
        }
        if view.focused && at == picked {
            line = focus_row(line, cells);
        }
        lines.push(line);
    }
    lines.truncate(rows);
    lines
}
