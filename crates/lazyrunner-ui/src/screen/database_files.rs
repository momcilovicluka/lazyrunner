//! The database screen's copies, backups, exports and imports (#110), and its files section: `c` copies the
//! highlighted database to a new one (its name typed, `Shop_copy` until you type another), `b` backs it up, `e`
//! exports it, `Del` drops it (after a yes; never the one in use); `Tab` gives the files the keys, where `i` imports
//! the picked file as a new database and `Del` deletes it. A new database gets a connection string and an offer to
//! switch to it. The motion is worked out here each frame: a job's wash across its row, folding into a file icon as
//! the file slides into the section; a new database's ghost row filling as it's restored, glinting once it's there;
//! a row that goes dimming and closing up. With reduced motion everything steps.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use lazyrunner_core::output::Hue;
use lazyrunner_core::run::Controller;
use lazyrunner_core::run::controller::Doing;
use lazyrunner_core::run::controller::database_files::is_local;
use lazyrunner_core::run::controller::save_points::db_key;
use lazyrunner_core::run::database_files::{self as files, Chore, DbFile, Kind};
use lazyrunner_core::run::secrets::{self, Entry};

use super::db_rows::{self, FileRow, Files};
use super::icons::{self, Icon};
use super::migrations::{Confirm, Risk, SavePointYes};
use super::motion::ease;
use super::seg::{Seg, seg};
use super::theme::C;
use super::{Overlay, Ui};

/// A new file slides into the section this long; a row that goes dims and closes up this long; a new database's
/// row glints this long.
const SLIDE: Duration = Duration::from_millis(250);
pub(super) const COLLAPSE: Duration = Duration::from_millis(300);
pub(super) const GLINT: Duration = super::motion::SWEEP;
/// The files section's rows at most (its title aside), and its rows with no files.
const FILES_MOST: usize = 6;
const FILES_EMPTY: usize = 2;
/// Files newer than these are lit, then plain, then dim.
const FRESH: Duration = Duration::from_secs(24 * 60 * 60);
const RECENT: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// What a name typed makes: a copy of a database, or an import of a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Making {
    Copy { server: String, database: String },
    Import { path: String, name: String, server: String },
}

/// A new database's name being typed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Naming {
    pub making: Making,
    pub text: String,
}

/// A row that's going: what it said, where it was, since when.
#[derive(Clone, Debug)]
pub(super) struct Leaving {
    pub(super) text: String,
    pub(super) at: usize,
    pub(super) file: bool,
    pub(super) since: Instant,
}

/// The files' part of the database screen: whether the files have the keys, the picked one, a name being typed, the
/// new database to offer a switch to, and the motion's state.
#[derive(Default)]
pub struct View {
    pub on_files: bool,
    pub picked: usize,
    pub naming: Option<Naming>,
    offer: Option<(String, String)>,
    known: Option<Vec<String>>,
    arrived: HashMap<String, Instant>,
    pub(super) leaving: Vec<Leaving>,
    pub(super) glint: Option<(String, Instant)>,
    seen_done: Option<Instant>,
    /// When a copy's restore began (its time left counts from then), by job.
    pub(super) restoring: Option<(u64, Instant)>,
}

/// The highlighted connection string.
fn highlighted(ui: &Ui) -> Option<Entry> {
    let picker = ui.picker.as_ref()?;
    picker.entries().get(picker.index).cloned()
}

pub(super) fn still(ui: &Ui) -> bool {
    ui.reduced_motion || !ui.motion.enabled
}

pub(super) fn since(ui: &Ui, at: Instant) -> Duration {
    ui.now.saturating_duration_since(at)
}

/// As the screen opens: the files listed again and each server's databases measured.
pub fn opened(ui: &mut Ui, controller: &mut Controller) {
    ui.files_view = View { seen_done: controller.db_done.as_ref().map(|done| done.ended), ..View::default() };
    controller.refresh_files();
    let Some(picker) = ui.picker.as_ref() else { return };
    let mut servers: Vec<String> =
        picker.groups.iter().flat_map(|(_, entries)| entries.iter().map(|entry| entry.server.clone())).collect();
    servers.sort_by_key(|server| server.to_lowercase());
    servers.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    for server in servers {
        controller.measure(&server);
    }
}

/// Whether a new database's name is being typed (letters are text then).
pub fn typing(ui: &Ui) -> bool {
    ui.picker.is_some() && ui.files_view.naming.is_some()
}

/// The files' keys, and the database keys that make files; whether the key was one of theirs.
pub fn key(ui: &mut Ui, controller: &mut Controller, name: &str, typed: Option<char>) -> bool {
    if ui.files_view.naming.is_some() {
        naming_key(ui, controller, name, typed);
        return true;
    }
    if name == "Tab" {
        cycle(ui, controller);
        return true;
    }
    if ui.files_view.on_files && files_key(ui, controller, name) {
        return true;
    }
    let Some(entry) = highlighted(ui) else { return false };
    match name {
        "c" => start_copy(ui, controller, &entry),
        "b" => {
            if let Err(why) = controller.backup_database(&entry.server, &entry.database) {
                controller.add_log(format!("✗ {why}"), Hue::Red);
            }
        }
        "e" => {
            if offer_install(ui, controller) {
                return true;
            }
            if let Err(why) = controller.export_database(&entry.server, &entry.database) {
                controller.add_log(format!("✗ {why}"), Hue::Red);
            }
        }
        "o" => open_folder(ui, controller, None),
        "i" if controller.db_files.is_empty() => controller.add_log(
            format!(
                "No .bak or .bacpac files yet - b backs up, e exports, or put one in {}",
                controller.files_folder().display()
            ),
            Hue::Gray,
        ),
        "i" => {
            (ui.save_view.on_rail, ui.files_view.on_files) = (false, true);
            controller.add_log("Pick the file to import, then i.", Hue::Gray);
        }
        "Delete" if !ui.save_view.on_rail => ask_drop(ui, controller, &entry),
        _ => return false,
    }
    true
}

/// `Tab`: the databases, then the save points' rail, the files and the pull's steps - each when there's something
/// in it.
fn cycle(ui: &mut Ui, controller: &mut Controller) {
    let Some(entry) = highlighted(ui) else { return };
    let has = [
        true,
        !controller.save_points_of(&entry.server, &entry.database).is_empty(),
        !controller.db_files.is_empty(),
        super::pull_view::can_focus(controller),
    ];
    let now = [ui.save_view.on_rail, ui.files_view.on_files, ui.pull_view.focused].iter().position(|on| *on);
    let now = now.map_or(0, |at| at + 1);
    let next = (1..=has.len()).map(|step| (now + step) % has.len()).find(|at| has[*at]).unwrap_or(0);
    if next == 0 && now == 0 {
        let said =
            format!("No save points of {} and no files yet - s makes a save point, b backs it up", entry.database);
        controller.add_log(said, Hue::Gray);
    }
    (ui.save_view.on_rail, ui.files_view.on_files, ui.pull_view.focused) = (next == 1, next == 2, next == 3);
}

/// The keys while the files have them; whether the key was one of theirs.
fn files_key(ui: &mut Ui, controller: &mut Controller, name: &str) -> bool {
    let count = controller.db_files.len();
    let picked = ui.files_view.picked.min(count.saturating_sub(1));
    match name {
        "j" | "DownArrow" => ui.files_view.picked = (picked + 1).min(count.saturating_sub(1)),
        "k" | "UpArrow" => ui.files_view.picked = picked.saturating_sub(1),
        "Escape" => ui.files_view.on_files = false,
        "i" | "Enter" => {
            if let Some(file) = controller.db_files.get(picked).cloned() {
                start_import(ui, controller, &file);
            }
        }
        "Delete" => {
            if let Some(file) = controller.db_files.get(picked).cloned() {
                ask_delete(ui, &file);
            }
        }
        "o" => {
            let file = controller.db_files.get(picked).cloned();
            open_folder(ui, controller, file.as_ref());
        }
        _ => return false,
    }
    true
}

/// `o`: the folder of the backups and exports in the system's file manager - a picked file's, with it selected. A
/// backup on another server's disk can't be shown here: its path is copied, and the log says it's on the server.
fn open_folder(ui: &mut Ui, controller: &mut Controller, file: Option<&DbFile>) {
    let Some(file) = file else {
        let folder = controller.files_folder();
        let opened = std::fs::create_dir_all(&folder).is_ok() && (ui.opener)(&folder.display().to_string());
        let said = if opened { "Opened" } else { "Couldn't open" };
        controller.add_log(format!("{said} {}", folder.display()), Hue::Gray);
        return;
    };
    match &file.server {
        Some(server) if !is_local(server) => {
            let copied = (ui.clipboard)(&file.path).is_ok();
            let how = if copied { "its path is copied" } else { "its path" };
            controller.add_log(
                format!("{} is on {server}, in its backup folder - {how}: {}", file.name, file.path),
                Hue::Gray,
            );
        }
        _ => {
            let shown = (ui.revealer)(std::path::Path::new(&file.path));
            let said = if shown { "Showed" } else { "Couldn't show" };
            controller.add_log(format!("{said} {} in its folder", file.name), Hue::Gray);
        }
    }
}

fn start_copy(ui: &mut Ui, controller: &mut Controller, entry: &Entry) {
    if let Some(busy) = controller.database_busy() {
        controller.add_log(format!("lazyrunner is {busy} - copy once it's done"), Hue::Yellow);
        return;
    }
    let text = controller.copy_default(&entry.server, &entry.database);
    let making = Making::Copy { server: entry.server.clone(), database: entry.database.clone() };
    ui.files_view.naming = Some(Naming { making, text });
}

fn start_import(ui: &mut Ui, controller: &mut Controller, file: &DbFile) {
    if file.kind == Kind::Bacpac && offer_install(ui, controller) {
        return;
    }
    if let Some(busy) = controller.database_busy() {
        controller.add_log(format!("lazyrunner is {busy} - import once it's done"), Hue::Yellow);
        return;
    }
    let Some(server) = file.server.clone().or_else(|| highlighted(ui).map(|entry| entry.server)) else { return };
    let text = controller.import_default(&server, file);
    let making = Making::Import { path: file.path.clone(), name: file.name.clone(), server };
    ui.files_view.naming = Some(Naming { making, text });
}

fn naming_key(ui: &mut Ui, controller: &mut Controller, name: &str, typed: Option<char>) {
    let Some(naming) = ui.files_view.naming.as_mut() else { return };
    match name {
        "Escape" => ui.files_view.naming = None,
        "Backspace" => {
            naming.text.pop();
        }
        "Enter" => {
            let Some(Naming { making, text }) = ui.files_view.naming.take() else { return };
            let (server, result) = match &making {
                Making::Copy { server, database } => (server, controller.copy_database(server, database, &text)),
                Making::Import { path, server, .. } => (server, controller.import_file(path, server, &text)),
            };
            match result {
                Ok(_) => ui.files_view.offer = Some((server.clone(), text.trim().to_string())),
                Err(why) => controller.add_log(format!("✗ {why}"), Hue::Red),
            }
        }
        _ => {
            if let Some(c) = typed {
                naming.text.push(c);
            }
        }
    }
}

/// SqlPackage missing: a y/n box offering to install it (never without a yes); whether it's missing.
fn offer_install(ui: &mut Ui, controller: &mut Controller) -> bool {
    let Err(why) = controller.sqlpackage_ready() else { return false };
    controller.add_log(format!("✗ {why}"), Hue::Yellow);
    let lines = vec![
        "SqlPackage isn't installed - exports and .bacpac imports run through it.".to_string(),
        format!("Install it now? {}", lazyrunner_core::run::sqlpackage::INSTALL_TEXT),
    ];
    open_confirm(
        ui,
        Confirm { lines, jobs: Vec::new(), risk: Risk::Apply, save_point: Some(SavePointYes::InstallSqlPackage) },
    );
    true
}

/// `Del` on a database: a y/n box that names it and its size, or why it can't go.
fn ask_drop(ui: &mut Ui, controller: &mut Controller, entry: &Entry) {
    if let Err(why) = controller.can_drop(&entry.server, &entry.database) {
        controller.add_log(format!("✗ Can't drop {}: {why}", entry.label), Hue::Yellow);
        return;
    }
    let size = controller
        .size_of(&entry.server, &entry.database)
        .map_or_else(String::new, |bytes| format!(" ({})", files::size(bytes)));
    let mut lines = vec![
        format!("Drop {} @ {}{size}?", entry.database, entry.server),
        "Its data is gone for good - b backs it up first, if you may need it.".to_string(),
        "Everyone still connected to it is cut off (SINGLE_USER WITH ROLLBACK IMMEDIATE).".to_string(),
    ];
    let points = controller.save_points_of(&entry.server, &entry.database).len();
    if points > 0 {
        lines.push(format!("Its {points} save point{} go too.", if points == 1 { "" } else { "s" }));
    }
    lines.push(
        "Its connection string leaves secrets.json (the file before is kept as secrets.json.lazyrunner-backup).".into(),
    );
    let yes = SavePointYes::DropDatabase { server: entry.server.clone(), database: entry.database.clone() };
    open_confirm(ui, Confirm { lines, jobs: Vec::new(), risk: Risk::LosesData, save_point: Some(yes) });
}

/// `Del` on a file: a y/n box that names it, its size and where it is.
fn ask_delete(ui: &mut Ui, file: &DbFile) {
    let size = file.bytes.map_or_else(String::new, |bytes| format!(" ({})", files::size(bytes)));
    let mut lines = vec![format!("Delete {}{size}?", file.name), format!("It's {}", file.path)];
    if let Some(server) = &file.server {
        lines.push(format!("In {server}'s backup folder: the server deletes it (xp_delete_file)."));
    }
    lines.push("It can't be undone.".into());
    let yes = SavePointYes::DeleteFile(file.path.clone());
    open_confirm(ui, Confirm { lines, jobs: Vec::new(), risk: Risk::LosesData, save_point: Some(yes) });
}

fn open_confirm(ui: &mut Ui, confirm: Confirm) {
    ui.behind = ui.overlay.take();
    ui.overlay = Some(Overlay::Confirm(Box::new(confirm)));
}

/// A yes in the y/n box for a database or a file.
pub fn yes(ui: &mut Ui, controller: &mut Controller, yes: SavePointYes) {
    let result = match yes {
        SavePointYes::DropDatabase { server, database } => controller.drop_database(&server, &database).map(|_| ()),
        SavePointYes::DeleteFile(path) => {
            if let Some(at) = controller.db_files.iter().position(|file| file.path == path) {
                let text = controller.db_files[at].name.clone();
                ui.files_view.leaving.push(Leaving { text, at, file: true, since: ui.now });
            }
            controller.delete_file(&path).map(|_| ())
        }
        SavePointYes::InstallSqlPackage => {
            controller.install_sqlpackage();
            Ok(())
        }
        SavePointYes::DropPulled(copies) => controller.drop_pulled(&copies).map(|_| ()),
        SavePointYes::Switch { server, database } => {
            switch_to(ui, controller, &server, &database);
            Ok(())
        }
        other => {
            super::save_points::yes(controller, other);
            Ok(())
        }
    };
    if let Err(why) = result {
        controller.add_log(format!("✗ {why}"), Hue::Red);
    }
}

/// Switches to `database` on `server` as `Enter` does, the screen staying open on it.
fn switch_to(ui: &mut Ui, controller: &mut Controller, server: &str, database: &str) {
    let Some(picker) = ui.picker.as_ref() else { return };
    let text = std::fs::read_to_string(&picker.path).unwrap_or_default();
    let entry = secrets::connections(&text)
        .into_iter()
        .find(|entry| entry.server.eq_ignore_ascii_case(server) && entry.database.eq_ignore_ascii_case(database));
    match entry {
        Some(entry) => controller.switch_database(&entry),
        None => controller.add_log(format!("No connection string to {database} @ {server} to switch to"), Hue::Yellow),
    }
    reload(ui, Some((server, database)));
}

/// The picker's connection strings read again, the highlight on `keep` (else where it was).
pub(super) fn reload(ui: &mut Ui, keep: Option<(&str, &str)>) {
    let Some(picker) = ui.picker.as_mut() else { return };
    let text = std::fs::read_to_string(&picker.path).unwrap_or_default();
    let was = picker.entries().get(picker.index).map(|entry| (entry.server.clone(), entry.database.clone()));
    picker.groups = secrets::groups(&text);
    picker.group = picker.group.min(picker.groups.len().saturating_sub(1));
    let wanted = keep.map(|(server, database)| (server.to_string(), database.to_string())).or(was);
    let entries = picker.entries();
    let found = wanted.and_then(|(server, database)| {
        entries.iter().position(|entry| {
            entry.server.eq_ignore_ascii_case(&server) && entry.database.eq_ignore_ascii_case(&database)
        })
    });
    picker.index = found.unwrap_or(picker.index).min(entries.len().saturating_sub(1));
}

/// Each frame: a job just done is noticed - a new database's row read in and glinting (and the switch offered), a
/// dropped one closing up - and new files slide in.
pub fn poll(ui: &mut Ui, controller: &Controller) {
    if ui.picker.is_none() {
        return;
    }
    note_files(ui, controller);
    if let Some(work) = controller.db_work.as_ref()
        && work.progress.restoring
        && ui.files_view.restoring.is_none_or(|(job, _)| job != work.job)
    {
        ui.files_view.restoring = Some((work.job, ui.now));
    }
    let Some(done) = controller.db_done.as_ref() else { return };
    if ui.files_view.seen_done == Some(done.ended) {
        return;
    }
    ui.files_view.seen_done = Some(done.ended);
    let Doing::Database(chore) = &done.doing else { return };
    if !done.ok {
        return;
    }
    match chore {
        Chore::Drop => {
            let entries = ui.picker.as_ref().map(|picker| picker.entries()).unwrap_or_default();
            let gone = entries.iter().position(|entry| {
                entry.server.eq_ignore_ascii_case(&done.server) && entry.database.eq_ignore_ascii_case(&done.database)
            });
            if let Some(at) = gone {
                let text = entries[at].label.clone();
                ui.files_view.leaving.push(Leaving { text, at, file: false, since: ui.now });
            }
            reload(ui, None);
        }
        Chore::Copy { to } | Chore::Import { to, .. } => {
            reload(ui, None);
            ui.files_view.glint = Some((db_key(&done.server, to), ui.now));
            let offered = ui.files_view.offer.take();
            let ours = offered.is_some_and(|(server, database)| {
                server.eq_ignore_ascii_case(&done.server) && database.eq_ignore_ascii_case(to)
            });
            if ours && ui.overlay.is_none() {
                let lines = vec![
                    format!("Switch to {to} @ {}?", done.server),
                    "It's made, and its connection string is in secrets.json - the APIs restart on it.".to_string(),
                ];
                let yes = SavePointYes::Switch { server: done.server.clone(), database: to.clone() };
                open_confirm(ui, Confirm { lines, jobs: Vec::new(), risk: Risk::Apply, save_point: Some(yes) });
            }
        }
        Chore::Backup | Chore::Export => {}
    }
}

/// Files that weren't there last frame slide in.
fn note_files(ui: &mut Ui, controller: &Controller) {
    let paths: Vec<String> = controller.db_files.iter().map(|file| file.path.clone()).collect();
    if let Some(known) = ui.files_view.known.as_ref() {
        for path in paths.iter().filter(|path| !known.contains(path)) {
            ui.files_view.arrived.insert(path.clone(), ui.now);
        }
    }
    ui.files_view.known = Some(paths);
    let now = ui.now;
    ui.files_view.arrived.retain(|_, at| now.saturating_duration_since(*at) < SLIDE);
    ui.files_view.leaving.retain(|gone| now.saturating_duration_since(gone.since) < COLLAPSE);
}

/// The rows the files section takes, out of `left`.
pub fn height(controller: &Controller, left: usize) -> usize {
    let files = controller.db_files.len();
    let wanted = if files == 0 { FILES_EMPTY } else { 2 + files.min(FILES_MOST) };
    wanted.min(left / 2).max(FILES_EMPTY.min(left))
}

/// The files section, `rows` rows.
pub fn rows(ui: &mut Ui, controller: &Controller, cells: usize, rows: usize) -> Vec<Vec<Seg>> {
    let still = still(ui);
    let count = controller.db_files.len();
    ui.files_view.picked = ui.files_view.picked.min(count.saturating_sub(1));
    if count == 0 {
        ui.files_view.on_files = false;
    }
    let most = controller.db_files.iter().filter_map(|file| file.bytes).max().unwrap_or(0);
    let now = jiff::Timestamp::now();
    let icon = icons::icon(ui.nerd, Icon::Zip).to_string();
    let importing = controller.db_work.as_ref().and_then(|work| match &work.doing {
        Doing::Database(Chore::Import { file, to }) => Some((file.clone(), to.clone())),
        _ => None,
    });
    let mut shown: Vec<FileRow> = controller
        .db_files
        .iter()
        .enumerate()
        .map(|(at, file)| {
            let age = file.age(now);
            let (color, bar) = match age {
                Some(age) if age < FRESH => (C::White, C::Cyan),
                Some(age) if age < RECENT => (C::Gray, C::DarkCyan),
                _ => (C::DarkGray, C::DarkGray),
            };
            let slide = match ui.files_view.arrived.get(&file.path) {
                Some(at) if !still => ease(since(ui, *at).as_secs_f64() / SLIDE.as_secs_f64()),
                _ => 1.0,
            };
            let doing =
                importing.as_ref().filter(|(name, _)| *name == file.name).map(|(_, to)| format!("importing as {to}…"));
            FileRow {
                icon: icon.clone(),
                name: file.name.clone(),
                size: file.bytes.map_or_else(|| "-".into(), files::size),
                share: file.bytes.map_or(0.0, |bytes| bytes as f64 / most.max(1) as f64),
                age: age.map_or_else(|| "-".into(), lazyrunner_core::run::session::ago),
                color,
                bar,
                on: file.server.as_ref().map(|server| format!("in {server}'s backup folder")),
                doing,
                picked: at == ui.files_view.picked,
                slide,
                leaving: None,
            }
        })
        .collect();
    for gone in ui.files_view.leaving.iter().filter(|gone| gone.file && !still) {
        let share = since(ui, gone.since).as_secs_f64() / COLLAPSE.as_secs_f64();
        if share < 1.0 && !controller.db_files.iter().any(|file| file.name == gone.text) {
            let row = FileRow {
                icon: icon.clone(),
                name: gone.text.clone(),
                size: String::new(),
                share: 0.0,
                age: String::new(),
                color: C::DarkGray,
                bar: C::DarkGray,
                on: None,
                doing: None,
                picked: false,
                slide: 1.0,
                leaving: Some(share),
            };
            shown.insert(gone.at.min(shown.len()), row);
        }
    }
    let note = files_note(ui, controller);
    let section = Files {
        folder: controller.files_folder().display().to_string(),
        note,
        rows: shown,
        focused: ui.files_view.on_files,
    };
    db_rows::files(&section, cells, rows)
}

/// The line under the files' title: what to do with them, or that there are none yet.
fn files_note(ui: &Ui, controller: &Controller) -> Option<(String, C)> {
    if controller.db_files.is_empty() {
        return Some((
            "none yet - b backs a database up to the server's backup folder, e exports a .bacpac here".into(),
            C::DarkGray,
        ));
    }
    if ui.files_view.on_files {
        return None;
    }
    Some((
        "Tab picks a file · i imports it as a new database · Del deletes it · o opens the folder".into(),
        C::DarkGray,
    ))
}

/// The prompt line while a new database's name is typed.
pub fn prompt(ui: &Ui) -> Option<Vec<Seg>> {
    let naming = ui.files_view.naming.as_ref()?;
    let what = match &naming.making {
        Making::Copy { server, database } => format!("copy {database} @ {server} as"),
        Making::Import { name, server, .. } => format!("import {name} into {server} as"),
    };
    Some(vec![seg(format!(" + {what}: {}{}", naming.text, ui.cursor()), C::Cyan)])
}

/// Whether anything of the files moves: a job's wash or ghost row, a fold, a file sliding in, a row going, a glint,
/// a name being typed (its cursor).
pub fn animating(ui: &Ui, controller: &Controller) -> bool {
    if ui.picker.is_none() {
        return false;
    }
    let view = &ui.files_view;
    if view.naming.is_some() {
        return true;
    }
    if still(ui) {
        return false;
    }
    let working = controller.db_work.as_ref().is_some_and(|work| matches!(work.doing, Doing::Database(_)));
    let folding = controller.db_done.as_ref().is_some_and(|done| {
        matches!(done.doing, Doing::Database(_)) && since(ui, done.ended) < super::database_jobs::FOLD
    });
    let sliding = view.arrived.values().any(|at| since(ui, *at) < SLIDE);
    let leaving = view.leaving.iter().any(|gone| since(ui, gone.since) < COLLAPSE);
    let glinting = view.glint.as_ref().is_some_and(|(_, at)| since(ui, *at) < GLINT);
    working || folding || sliding || leaving || glinting
}

/// The screen's keys while the files have them, or a name is typed.
pub fn hint(ui: &Ui) -> Option<&'static str> {
    match &ui.files_view.naming {
        Some(Naming { making: Making::Copy { .. }, .. }) => {
            Some(" type the new database's name · Enter copies it · Esc cancels")
        }
        Some(Naming { making: Making::Import { .. }, .. }) => {
            Some(" type the new database's name · Enter imports it · Esc cancels")
        }
        None => ui.files_view.on_files.then_some(
            " j k pick a file · i import it as a new database · Del delete it · o show it in its folder · Tab \
                        databases · Esc back",
        ),
    }
}
