//! Bookmarks in the marks box (#84): `b` keeps the picked mark as one, asking for its name - the input slides up from
//! the bottom of the box while the picked mark holds its row, and the mark's edge dissolves from the marks' magenta to
//! the bookmarks' accent, a step a frame, in place. A bookmark from an earlier run shows its kept lines on `Enter`; `e`
//! exports them all to Markdown. While any has its line in a column the panel shows a chip with how many. None of it
//! moves with reduced motion: it steps.

use std::time::{Duration, Instant};

use jiff::Timestamp;
use lazyrunner_core::output::Hue;
use lazyrunner_core::run::Controller;
use lazyrunner_core::run::bookmarks::Bookmark;
use lazyrunner_core::run::session::ago;

use super::icons::{self, Icon};
use super::motion::{Spot, ease};
use super::mouse::Target;
use super::overlays::boxed;
use super::seg::{Seg, ellipsis, on, pad, seg};
use super::theme::C;
use super::{Overlay, Ui};

/// The bookmarks' colour, on the rail and in the column; marks stay magenta.
pub const ACCENT: C = C::Cyan;
/// A kept mark's edge on its way from the marks' colour to the bookmarks', a step each.
const DISSOLVE: [C; 3] = [C::Magenta, C::Blue, ACCENT];
/// The naming input: how long it takes to slide up, and its rows.
const SLIDE: Duration = Duration::from_millis(150);
const NAMING_ROWS: usize = 3;
/// The name a bookmark gets when none is typed: its line, cut to this.
const NAME_FROM_LINE: usize = 40;
const NAME_CELLS: usize = 18;
const APP_CELLS: usize = 10;
const INNER: usize = 84;

/// The edge of app `app`'s bookmarked line `number`: the accent, or a step of the dissolve while it was just kept.
pub fn edge(ui: &Ui, app: usize, number: u64) -> C {
    match ui.motion.lit(&Spot::Kept { app, line: number }, ui.now) {
        Some(left) => DISSOLVE[(((1.0 - left) * DISSOLVE.len() as f64) as usize).min(DISSOLVE.len() - 1)],
        None => ACCENT,
    }
}

/// The panel's chip while bookmarks have their lines in the columns: how many; a click opens the list.
pub fn chip(ui: &Ui, controller: &Controller) -> Option<Seg> {
    let count = controller.bookmarks_attached();
    (count > 0).then(|| seg(format!("   {} {count}", icons::icon(ui.nerd, Icon::Bookmark)), ACCENT).at(Target::Marks))
}

/// `b` on a mark: the naming input opens.
pub fn name(ui: &mut Ui) {
    if let Some(Overlay::Marks { naming, .. }) = &mut ui.overlay {
        *naming = Some((String::new(), ui.now));
    }
}

/// Enter in the naming input: app `app`'s marked line `number` kept as a bookmark named `typed` (none typed: after
/// its line), its edge dissolving into the accent; the new bookmark's id.
pub fn keep(ui: &mut Ui, controller: &mut Controller, (app, number): (usize, u64), typed: &str) -> Option<String> {
    let line = controller.states.get(app)?.output.numbered(number, number).next()?.text.trim().to_string();
    let name = match typed.trim() {
        "" => ellipsis(&line, NAME_FROM_LINE).trim_end().to_string(),
        typed => typed.to_string(),
    };
    let Some(id) = controller.keep_bookmark(app, number, &name) else {
        controller.add_log("That line has left the column's buffer: nothing to keep.", Hue::DarkGray);
        return None;
    };
    ui.motion.flash(Spot::Kept { app, line: number }, ui.now);
    controller.add_log(format!("▸ Kept as a bookmark: {name} - it finds its line again after a restart"), Hue::Cyan);
    Some(id)
}

/// `e`: the bookmarks as Markdown in the state folder, its path copied.
pub fn export(ui: &Ui, controller: &mut Controller) {
    if controller.bookmarks().is_empty() {
        controller.add_log("No bookmarks to export - b in ' keeps a mark as one.", Hue::DarkGray);
        return;
    }
    match controller.export_bookmarks() {
        Ok(file) => {
            let shown = file.display().to_string();
            controller.add_log(format!("▸ The bookmarks as Markdown: {shown}"), Hue::Cyan);
            super::copy::put(ui.clipboard, controller, &shown, "the bookmarks file's path");
        }
        Err(error) => controller.add_log(format!("✗ The bookmarks weren't exported: {error}"), Hue::Red),
    }
}

/// A bookmark in the `y` timeline: when its line came (else when it was kept), its app, its name, note and line.
pub fn timeline_line(ui: &Ui, controller: &Controller, bookmark: &Bookmark) -> String {
    let app = app_name(controller, &bookmark.app);
    let line = controller.bookmark_line(&bookmark.id).and_then(|(app, number)| {
        controller.states[app].output.numbered(number, number).next().map(|line| super::marks::clock(ui, line.born))
    });
    let time = line.unwrap_or_else(|| {
        lazyrunner_core::time::as_written(&bookmark.kept)
            .map_or_else(|| "--:--:--".into(), |at| at.strftime(lazyrunner_core::time::DAY_CLOCK).to_string())
    });
    let note = if bookmark.note.is_empty() { String::new() } else { format!("{} - ", bookmark.note) };
    format!("{time}  {app}  [{}] {note}{}", bookmark.name, bookmark.line.trim())
}

fn app_name(controller: &Controller, key: &str) -> String {
    controller.app(key).map_or_else(|| key.to_string(), |app| app.name.clone())
}

/// Where it stands: how long ago its line came this run, else from an earlier run - or, after 30 days without its
/// line, for how long.
fn standing(ui: &Ui, controller: &Controller, bookmark: &Bookmark) -> (String, bool) {
    if let Some((app, number)) = controller.bookmark_line(&bookmark.id) {
        let line = controller.states[app].output.numbered(number, number).next();
        let age = line.map(|line| ui.now.saturating_duration_since(line.born).as_secs_f64());
        return (age.map_or_else(String::new, |age| format!("{} ago", lazyrunner_core::run::format_age(age))), true);
    }
    match bookmark.stale_days(Timestamp::now()) {
        Some(days) => (format!("not found for {days} days"), false),
        None => ("from an earlier run".into(), false),
    }
}

/// A bookmark's row in the marks box: its edge (the icon with Nerd Font icons), name, app, where it stands, its note
/// and its line - the one under the selection bar lit, the picked one's note being typed yellow; a stale one dim.
pub fn row(
    ui: &Ui,
    controller: &Controller,
    bookmark: &Bookmark,
    (picked, on_bar): (bool, bool),
    (inner, writing): (usize, Option<&str>),
) -> Vec<Seg> {
    let (status, attached) = standing(ui, controller, bookmark);
    let stale = !attached && bookmark.stale_days(Timestamp::now()).is_some();
    let edge_color = match controller.bookmark_line(&bookmark.id) {
        Some((app, number)) => edge(ui, app, number),
        None if stale => C::DarkGray,
        None => ACCENT,
    };
    let lead = format!(
        " {} {} {}  ",
        pad(&bookmark.name, NAME_CELLS),
        pad(&app_name(controller, &bookmark.app), APP_CELLS),
        status
    );
    if picked && let Some(typing) = writing {
        let typed = format!("{} {lead}note: {typing}{}", icons::icon(ui.nerd, Icon::Bookmark), ui.cursor());
        return vec![on(pad(&typed, inner), C::Black, C::Yellow)];
    }
    let dim = |color: C| if stale { C::DarkGray } else { color };
    let noted = if bookmark.note.is_empty() { String::new() } else { format!("{} · ", bookmark.note) };
    let row = vec![
        seg(format!(" {}", icons::icon(ui.nerd, Icon::Bookmark)), edge_color),
        seg(format!(" {}", pad(&bookmark.name, NAME_CELLS)), dim(ACCENT)),
        seg(format!(" {}", pad(&app_name(controller, &bookmark.app), APP_CELLS)), dim(C::Magenta)),
        seg(format!(" {status}  "), C::DarkGray),
        seg(noted, dim(C::Yellow)),
        seg(bookmark.line.trim().to_string(), if attached { C::White } else { dim(C::Gray) }),
    ];
    if on_bar { super::overlays::on_bar(ui, row, (2, 2), inner) } else { row }
}

/// The naming input under the list, `inner` wide: a rule, the name being typed, and what comes along. It slides up
/// from the box's bottom edge - its rows are kept from the start, so nothing above it moves.
pub fn naming_rows(ui: &Ui, (typed, opened): (&str, Instant), note: Option<&str>, inner: usize) -> Vec<Vec<Seg>> {
    let along = match note {
        Some(note) => format!("   its note comes along: {note}"),
        None => "   it keeps its line and the lines either side; after a restart it finds its line again".into(),
    };
    let panel = [
        vec![seg(format!(" {}", "┄".repeat(inner.saturating_sub(2))), C::DarkGray)],
        vec![
            on(format!(" {} ", icons::icon(ui.nerd, Icon::Bookmark)), C::Black, ACCENT),
            on(pad(&format!(" name: {typed}{}", ui.cursor()), inner.saturating_sub(3)), C::Black, ACCENT),
        ],
        vec![seg(along, C::DarkGray)],
    ];
    let moving = ui.motion.enabled && !ui.reduced_motion;
    let through = ui.now.saturating_duration_since(opened).as_secs_f64() / SLIDE.as_secs_f64();
    let shown = if moving { (ease(through) * NAMING_ROWS as f64).ceil() as usize } else { NAMING_ROWS };
    let shown = shown.min(NAMING_ROWS);
    let mut rows: Vec<Vec<Seg>> = (shown..NAMING_ROWS).map(|_| Vec::new()).collect();
    rows.extend(panel.into_iter().take(shown));
    rows
}

/// Whether the naming input is still sliding up.
pub fn animating(ui: &Ui) -> bool {
    match &ui.overlay {
        Some(Overlay::Marks { naming: Some((_, opened)), .. }) => {
            !ui.reduced_motion && ui.motion.enabled && ui.now.saturating_duration_since(*opened) < SLIDE
        }
        _ => false,
    }
}

/// `Enter` on a bookmark from an earlier run: the lines it kept, in a box - its own on the accent edge.
pub fn kept_rows(ui: &Ui, controller: &Controller, bookmark: &Bookmark, cells: usize, rows: usize) -> Vec<Vec<Seg>> {
    let inner = cells.saturating_sub(8).clamp(20, INNER);
    let kept = bookmark.kept.parse::<Timestamp>().ok().map(|at| {
        let age = Timestamp::now().duration_since(at).as_secs().max(0);
        ago(Duration::from_secs(u64::try_from(age).unwrap_or(0)))
    });
    let mut about = vec![app_name(controller, &bookmark.app), format!("kept {}", kept.unwrap_or_default())];
    if !bookmark.branch.is_empty() {
        about.push(format!("on {}", bookmark.branch));
    }
    about.push(standing(ui, controller, bookmark).0);
    let mut lines = vec![vec![seg(format!(" {}", about.join(" · ")), C::DarkGray)]];
    if !bookmark.note.is_empty() {
        lines.push(vec![seg(format!(" note: {}", bookmark.note), C::Yellow)]);
    }
    lines.push(Vec::new());
    let near = |text: &String| vec![seg("  ┆ ", C::DarkGray), seg(text.clone(), C::DarkGray)];
    lines.extend(bookmark.before.iter().map(near));
    lines.push(vec![seg("  ▌ ", ACCENT), seg(bookmark.line.clone(), C::White)]);
    lines.extend(bookmark.after.iter().map(near));
    let title = icons::titled(ui.nerd, Icon::Bookmark, &bookmark.name);
    boxed(&title, lines, "Esc back to the list · d deletes it · e exports them all", cells, rows, inner)
}
