//! The database screen's rows as a copy, backup, export, import or drop draws them (#110): a job's wash across its
//! database's row, saying how far it is and how long is left, folding into a file icon once it's done; a new
//! database's ghost row under its source, filling as it's restored; a dropped one closing up; and each database's size.
//! Worked out each frame from the controller's job; with reduced motion everything steps.

use std::time::{Duration, Instant};

use lazyrunner_core::run::Controller;
use lazyrunner_core::run::controller::save_points::db_key;
use lazyrunner_core::run::controller::{DbWork, Doing};
use lazyrunner_core::run::database_files::{self as files, Chore, Progress};
use lazyrunner_core::run::secrets::Entry;

use super::Ui;
use super::database_files::{COLLAPSE, GLINT, since, still};
use super::db_rows::{self, Ghost, Wash};
use super::icons::{self, Icon};
use super::motion::ease;
use super::seg::{Seg, seg};
use super::theme::C;

/// A share glides to each new one this long; a finished wash folds into its file icon this long; a ghost row slides
/// open this long.
const GLIDE: Duration = Duration::from_millis(140);
pub(super) const FOLD: Duration = Duration::from_millis(350);
const OPEN: Duration = Duration::from_millis(150);
/// The shimmer runs along a wash's fill once every this long.
const SHIMMER: Duration = Duration::from_millis(1400);
/// A size's beads in the databases' size column, and that column's number.
const SIZE_BEADS: usize = 10;
const SIZE_CELLS: usize = 7;

/// How far a share has come, gliding from the one before to the newest as the job reports it.
fn glide(share: Option<(f64, f64, Instant)>, now: Instant, still: bool) -> f64 {
    match share {
        None => 0.0,
        Some((_, newest, _)) if still => newest,
        Some((before, newest, at)) => {
            before + (newest - before) * ease(now.saturating_duration_since(at).as_secs_f64() / GLIDE.as_secs_f64())
        }
    }
}

/// `about 20s left`, from the rate so far.
fn left(share: f64, elapsed: Duration) -> Option<String> {
    let left = files::eta(share, elapsed)?;
    Some(format!("about {} left", lazyrunner_core::run::format_duration(left.as_secs_f64().round())))
}

/// What a job says on its row: `backing up · 64% · 1.5 GB of 2.4 GB · about 20s left`.
fn status(verb: &str, progress: &Progress, share: Option<f64>, elapsed: Duration) -> String {
    let mut parts = vec![verb.to_string()];
    match share {
        Some(share) => parts.push(format!("{}%", (share * 100.0).round())),
        None => {
            if let Some(step) = &progress.step {
                parts.push(format!("{step}…"));
            }
        }
    }
    match (progress.tables, progress.bytes, progress.total) {
        (Some((at, of)), ..) if at > 0 => {
            parts.push(format!("table {at} of {of}"));
            if let Some(table) = &progress.table {
                parts.push(table.clone());
            }
        }
        (_, Some(bytes), Some(total)) => parts.push(format!("{} of {}", files::size(bytes), files::size(total))),
        (_, Some(bytes), None) if bytes > 0 => parts.push(format!("{} so far", files::size(bytes))),
        _ => {}
    }
    if let Some(left) = share.and_then(|share| left(share, elapsed)) {
        parts.push(left);
    }
    parts.join(" · ")
}

/// The job on `entry`'s database, when there is one.
fn work_on<'a>(controller: &'a Controller, entry: &Entry) -> Option<(&'a DbWork, &'a Chore)> {
    let work = controller.db_work.as_ref()?;
    let Doing::Database(chore) = &work.doing else { return None };
    let same = work.server.eq_ignore_ascii_case(&entry.server) && work.database.eq_ignore_ascii_case(&entry.database);
    same.then_some((work, chore))
}

/// A database's row as a job on it draws it: washed while it's backed up, exported or copied, folding into a file
/// icon once that's done, dim while it's dropped; glinting once it's made. As it is otherwise.
pub fn job_row(
    ui: &Ui,
    controller: &Controller,
    entry: &Entry,
    line: Vec<Seg>,
    cells: usize,
    label_cells: usize,
    picked: bool,
) -> Vec<Seg> {
    let still = still(ui);
    let mark = if entry.active { " ● " } else { "   " }.to_string();
    let icon = icons::icon(ui.nerd, Icon::Zip).to_string();
    let washing = |share: f64, status: String, fold: Option<f64>| {
        let shimmer = (!still).then(|| (ui.seconds() % SHIMMER.as_secs_f64()) / SHIMMER.as_secs_f64());
        let wash = Wash {
            mark: mark.clone(),
            label: entry.label.clone(),
            label_cells,
            status,
            share,
            shimmer,
            fold,
            icon: icon.clone(),
            picked,
        };
        db_rows::wash(&wash, cells)
    };
    if let Some((work, chore)) = work_on(controller, entry) {
        let elapsed = since(ui, work.started);
        return match chore {
            Chore::Drop => super::seg::dim(line).into_iter().chain([seg("  dropping…", C::Yellow)]).collect(),
            Chore::Copy { .. } if work.progress.restoring => line,
            Chore::Import { .. } => line,
            chore => {
                let share = glide(work.share, ui.now, still);
                let real = work.share.map(|(_, newest, _)| newest);
                let verb = if matches!(chore, Chore::Copy { .. }) { "copying · backing up" } else { chore.verb() };
                washing(share, status(verb, &work.progress, real, elapsed), None)
            }
        };
    }
    if let Some(done) = controller.db_done.as_ref().filter(|done| done.ok && !still)
        && let Doing::Database(chore @ (Chore::Backup | Chore::Export)) = &done.doing
        && done.server.eq_ignore_ascii_case(&entry.server)
        && done.database.eq_ignore_ascii_case(&entry.database)
        && since(ui, done.ended) < FOLD
    {
        let fold = ease(since(ui, done.ended).as_secs_f64() / FOLD.as_secs_f64());
        let file = done.made.as_ref().and_then(|made| made.file.as_deref()).unwrap_or_default();
        let name = file.rsplit(['\\', '/']).next().unwrap_or_default();
        return washing(
            1.0,
            format!("{} · {name}", if *chore == Chore::Backup { "backed up" } else { "exported" }),
            Some(fold),
        );
    }
    match ui.files_view.glint.as_ref().filter(|_| !still) {
        Some((key, at)) if *key == db_key(&entry.server, &entry.database) && since(ui, *at) < GLINT => {
            super::seg::sweep(line, since(ui, *at).as_secs_f64() / GLINT.as_secs_f64(), cells, C::Black, C::Cyan)
        }
        _ => line,
    }
}

/// A new database's ghost row: after its source's row (`entry`, a copy) or after the last row (`None`, an import).
pub fn ghost_after(
    ui: &Ui,
    controller: &Controller,
    entry: Option<&Entry>,
    cells: usize,
    label_cells: usize,
) -> Option<Vec<Seg>> {
    let still = still(ui);
    let work = controller.db_work.as_ref()?;
    let Doing::Database(chore) = &work.doing else { return None };
    let label = match (chore, entry) {
        (Chore::Copy { to }, Some(entry))
            if work.server.eq_ignore_ascii_case(&entry.server)
                && work.database.eq_ignore_ascii_case(&entry.database) =>
        {
            format!("{to} @ {}", work.server)
        }
        (Chore::Import { to, .. }, None) => format!("{to} @ {}", work.server),
        _ => return None,
    };
    let restoring = work.progress.restoring;
    let share = if restoring { glide(work.share, ui.now, still) } else { 0.0 };
    let real = work.share.map(|(_, newest, _)| newest).filter(|_| restoring && work.progress.tables.is_none());
    let from = ui.files_view.restoring.filter(|(job, _)| *job == work.job).map_or(work.started, |(_, at)| at);
    let text = if restoring {
        let verb = if matches!(chore, Chore::Import { file, .. } if file.to_lowercase().ends_with(".bacpac")) {
            "importing"
        } else {
            "restoring"
        };
        status(verb, &work.progress, real, since(ui, from))
    } else {
        "waiting for the backup…".to_string()
    };
    let share = match work.progress.tables {
        Some((at, of)) if of > 0 && restoring => {
            glide(work.share, ui.now, still).max((at.saturating_sub(1)) as f64 / of as f64)
        }
        _ => share,
    };
    let opened = if still { 1.0 } else { ease(since(ui, work.started).as_secs_f64() / OPEN.as_secs_f64()) };
    Some(db_rows::ghost(&Ghost { label, label_cells, status: text, share, opened }, cells))
}

/// The row going at row `at` (a dropped database's), closing up.
pub fn leaving_at(ui: &Ui, at: usize) -> Option<Vec<Seg>> {
    if still(ui) {
        return None;
    }
    let gone = ui.files_view.leaving.iter().find(|gone| !gone.file && gone.at == at)?;
    let share = since(ui, gone.since).as_secs_f64() / COLLAPSE.as_secs_f64();
    (share < 1.0).then(|| db_rows::collapse(vec![seg(format!("   {}", gone.text), C::Gray)], share))
}

/// A database's size cell: `2.4 GB ▰▰▰▰▰▰▰▰▰▰`, its beads a share of the biggest database's (`most`).
pub fn size_cell(controller: &Controller, entry: &Entry, most: u64) -> Seg {
    let Some(bytes) = controller.size_of(&entry.server, &entry.database) else {
        let reading = controller.sizes(&entry.server).is_some_and(|sizes| sizes.reading.is_some());
        return seg(if reading { "…" } else { "-" }, C::DarkGray);
    };
    let (beads, _) = super::seg::gauge(bytes as f64 / most.max(1) as f64, SIZE_BEADS);
    let beads = if beads.is_empty() { super::seg::gauge(1.0, 1).0 } else { beads };
    seg(format!("{:>SIZE_CELLS$} {beads}", files::size(bytes)), C::Cyan)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_job_says_how_far_it_has_come_what_of_and_how_long_is_left() {
        let gb = 1024 * 1024 * 1024;
        let backup = Progress { bytes: Some(gb * 3 / 2), total: Some(gb * 12 / 5), ..Progress::default() };
        assert_eq!(
            status("backing up", &backup, Some(0.64), Duration::from_secs(36)),
            "backing up · 64% · 1.5 GB of 2.4 GB · about 20s left"
        );
        let export = Progress { tables: Some((112, 214)), table: Some("OrderLines".into()), ..Progress::default() };
        assert_eq!(
            status("exporting", &export, Some(0.52), Duration::from_millis(500)),
            "exporting · 52% · table 112 of 214 · OrderLines"
        );
        let early = Progress { step: Some("reading its schema".into()), tables: Some((0, 214)), ..Progress::default() };
        assert_eq!(status("exporting", &early, None, Duration::from_secs(3)), "exporting · reading its schema…");
        let start = Instant::now();
        let reported = Some((0.2, 0.6, start));
        assert!((glide(reported, start, false) - 0.2).abs() < 1e-9);
        assert!((glide(reported, start + GLIDE, false) - 0.6).abs() < 1e-9);
        assert_eq!(glide(reported, start, true), 0.6, "reduced motion: it steps");
        assert_eq!(glide(None, start, false), 0.0);
    }
}
