//! The boxes drawn over the columns: the database picker, the command palette, the x menu and the settings, the
//! key reference (on the whole screen), the startup checklist and the changelog.

use lazyrunner_core::changelog;
use lazyrunner_core::run::Controller;
use lazyrunner_core::run::checks::Status;
use unicode_width::UnicodeWidthStr;

use super::actions::{self, Action};
use super::checklist::Checklist;
use super::columns::{hue_color, spinner};
use super::icons::{self, Icon};
use super::mouse::Target;
use super::seg::{Seg, aim, limit, on, pad, seg, width};
use super::theme::C;
use super::{Overlay, Ui};

pub const LOGO: [&str; 3] = [
    "█   ▄▀▄ ▀▀█ █ █ █▀▄ █ █ █▀▄ █▀▄ █▀▀ █▀▄",
    "█   █▀█ ▄▀  ▀█▀ █▀▄ █ █ █ █ █ █ █▀  █▀▄",
    "▀▀▀ ▀ ▀ ▀▀▀  ▀  ▀ ▀ ▀▀▀ ▀ ▀ ▀ ▀ ▀▀▀ ▀ ▀",
];
const SPARKLE: [char; 2] = ['✦', '✧'];
/// How fast the light sweeps across the checklist's logo.
const SWEEP_COLUMNS_PER_SECOND: f64 = 12.0;
pub const WHATS_NEW_MAX_LINES: usize = 12;
const VERSION: &str = lazyrunner_core::changelog::VERSION;

/// Rows for a centred box: a title, the lines (each cut to fit), a footer.
pub fn boxed(
    title: &str,
    lines: Vec<Vec<Seg>>,
    footer: &str,
    cells: usize,
    rows: usize,
    max_inner: usize,
) -> Vec<Vec<Seg>> {
    draining_box(title, lines, footer, cells, rows, max_inner, None)
}

/// A box whose bottom edge drains from the right as `drain` goes from 1 to 0: the time until it closes by itself.
fn draining_box(
    title: &str,
    lines: Vec<Vec<Seg>>,
    footer: &str,
    cells: usize,
    rows: usize,
    max_inner: usize,
    drain: Option<f64>,
) -> Vec<Vec<Seg>> {
    let inner = cells.saturating_sub(8).min(max_inner).max(20);
    let left = " ".repeat(cells.saturating_sub(inner + 4) / 2);
    let mut boxed = vec![vec![
        seg("┌─", C::Cyan),
        seg(format!(" {title} "), C::White),
        seg(format!("{}┐", "─".repeat(inner.saturating_sub(title.width() + 1))), C::Cyan),
    ]];
    for line in lines {
        let segs = limit(&line, inner); // never push the right border out
        let room = inner.saturating_sub(width(&segs));
        // An item takes its whole row, its padding too.
        let padding = segs
            .last()
            .and_then(|last| last.target)
            .map_or_else(|| seg(" ".repeat(room), C::Gray), |target| seg(" ".repeat(room), C::Gray).at(target));
        let mut row = vec![seg("│ ", C::Cyan)];
        row.extend(segs);
        row.push(padding);
        row.push(seg(" │", C::Cyan));
        boxed.push(row);
    }
    if !footer.is_empty() {
        boxed.push(vec![seg("│ ", C::Cyan), seg(pad(footer, inner), C::DarkGray), seg(" │", C::Cyan)]);
    }
    let edge = inner + 2;
    let lit = drain.map_or(edge, |drain| (drain.clamp(0.0, 1.0) * edge as f64).round() as usize);
    boxed.push(if lit == edge {
        vec![seg(format!("└{}┘", "─".repeat(edge)), C::Cyan)]
    } else {
        vec![seg(format!("└{}", "─".repeat(lit)), C::Cyan), seg(format!("{}┘", "─".repeat(edge - lit)), C::DarkGray)]
    });
    // Inside the box a click keeps it open (an item's click picks it); the margin around it is outside.
    let boxed: Vec<Vec<Seg>> = boxed
        .into_iter()
        .map(|row| {
            let mut framed = vec![seg(left.clone(), C::Gray)];
            framed.extend(aim(row, Target::Box));
            framed
        })
        .collect();
    let top = rows.saturating_sub(boxed.len()) / 2;
    (0..rows).map(|row| row.checked_sub(top).and_then(|index| boxed.get(index).cloned()).unwrap_or_default()).collect()
}

/// A selected item: the whole line in the highlight colours.
fn selected(text: &str, inner: usize) -> Vec<Seg> {
    vec![on(pad(text, inner), C::Black, C::Cyan)]
}

pub fn picker(ui: &mut Ui, cells: usize, rows: usize) -> Vec<Vec<Seg>> {
    let cursor = ui.cursor();
    let Some(picker) = ui.picker.as_mut() else { return Vec::new() };
    let entries = picker.entries();
    let (key, group_size) =
        picker.groups.get(picker.group).map_or((String::new(), 0), |(key, entries)| (key.clone(), entries.len()));
    let inner = cells.saturating_sub(8).clamp(20, 96);
    let visible = entries.len().min(rows.saturating_sub(8)).max(1);
    if picker.index < picker.top {
        picker.top = picker.index;
    }
    if picker.index >= picker.top + visible {
        picker.top = picker.index + 1 - visible;
    }
    let mut lines =
        vec![vec![seg(format!("secrets.json · {group_size} connection strings · active one marked ●"), C::DarkGray)]];
    if picker.filtering || !picker.filter.is_empty() {
        let cursor = if picker.filtering { cursor } else { " " };
        lines.push(vec![
            seg("/ ", C::Cyan),
            seg(picker.filter.clone(), C::White),
            seg(cursor, C::Cyan),
            seg(format!("  {} shown", entries.len()), C::DarkGray),
        ]);
    }
    if entries.is_empty() {
        lines.push(vec![seg("  nothing matches", C::DarkGray)]);
    }
    for (index, entry) in entries.iter().enumerate().skip(picker.top).take(visible) {
        let marker = if entry.active { '●' } else { ' ' };
        let text = format!(" {marker} {}{}", entry.label, if entry.active { "   (active)" } else { "" });
        let line = if index == picker.index {
            selected(&text, inner)
        } else {
            vec![seg(pad(&text, inner), if entry.active { C::Green } else { C::Gray })]
        };
        lines.push(aim(line, Target::Item(index)));
    }
    let group_info = if picker.groups.len() > 1 {
        format!("  ({}/{}, h l other keys)", picker.group + 1, picker.groups.len())
    } else {
        String::new()
    };
    if let Some(name) = &picker.new_name {
        lines.push(vec![
            seg("+ new database (copy of the selected one): ", C::Cyan),
            seg(name.clone(), C::White),
            seg(cursor, C::Cyan),
        ]);
        return boxed(
            &format!("Database · {key}"),
            lines,
            "type the database name · Enter adds it (inactive) · Esc cancels",
            cells,
            rows,
            96,
        );
    }
    let more = if entries.len() > visible {
        format!("  ({}-{} of {})", picker.top + 1, picker.top + visible, entries.len())
    } else {
        String::new()
    };
    let footer = if picker.filtering {
        "type to filter · Enter keeps it · Esc clears it".to_string()
    } else {
        format!("j k choose · Enter switch + restart the APIs · n new · y copy row · / filter · Esc close{more}")
    };
    boxed(&format!("Database · {key}{group_info}"), lines, &footer, cells, rows, 96)
}

/// The items of the x menu or the , settings.
pub fn menu_items(ui: &Ui, controller: &Controller) -> Vec<Action> {
    match ui.overlay {
        Some(Overlay::Menu { .. }) => actions::menu(ui, controller),
        Some(Overlay::Settings { .. }) => actions::settings(controller),
        _ => Vec::new(),
    }
}

/// The x menu (actions for the focused app) and the , settings: an item's key runs it, j k + Enter too.
pub fn menu(ui: &mut Ui, controller: &Controller, cells: usize, rows: usize) -> Vec<Vec<Seg>> {
    let items = menu_items(ui, controller);
    const INNER: usize = 64;
    let index = match &mut ui.overlay {
        Some(Overlay::Menu { index } | Overlay::Settings { index }) => {
            *index = (*index).min(items.len().saturating_sub(1));
            *index
        }
        _ => 0,
    };
    let lines: Vec<Vec<Seg>> = items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let key = pad(&format!(" {}", item.key), 8);
            let hint = if item.hint.is_empty() { String::new() } else { format!("  {}", item.hint) };
            let flipped = matches!(&item.run, actions::Run::Setting { toggle, .. }
                if ui.motion.lit(&super::motion::Spot::Setting(*toggle), ui.now).is_some());
            if flipped {
                let line = pad(&format!("{key}{}{hint}", item.label), INNER);
                return aim(vec![on(line, C::Black, C::Green)], Target::Item(i));
            }
            if i == index {
                return aim(selected(&format!("{key}{}{hint}", item.label), INNER), Target::Item(i));
            }
            let label_color = if item.label.starts_with("[x]") { C::Green } else { C::White };
            aim(vec![seg(key, C::Cyan), seg(item.label.clone(), label_color), seg(hint, C::DarkGray)], Target::Item(i))
        })
        .collect();
    if matches!(ui.overlay, Some(Overlay::Menu { .. })) {
        let name = controller.apps.get(ui.focus).map_or(String::new(), |app| app.name.clone());
        boxed(&name, lines, "press a key, or j k + Enter · Esc / x close", cells, rows, INNER)
    } else {
        boxed("Settings (saved)", lines, "press a letter to toggle, or j k + Enter · Esc / , close", cells, rows, INNER)
    }
}

/// What `i` offers: npm ci or npm install (key, command, what it does).
const INSTALLS: [(&str, &str, &str); 2] = [
    ("c", "npm ci", "a clean install of exactly what package-lock.json says"),
    ("n", "npm install", "adds and updates packages to match package.json (the lock file may change)"),
];

/// i: npm ci or npm install, for the npm app i installs.
pub fn install(ui: &mut Ui, controller: &Controller, cells: usize, rows: usize) -> Vec<Vec<Seg>> {
    const INNER: usize = 80;
    let index = match &mut ui.overlay {
        Some(Overlay::Install { index }) => {
            *index = (*index).min(INSTALLS.len() - 1);
            *index
        }
        _ => 0,
    };
    let lines: Vec<Vec<Seg>> = INSTALLS
        .iter()
        .enumerate()
        .map(|(i, (key, command, hint))| {
            let key = pad(&format!(" {key}"), 5);
            if i == index {
                return aim(selected(&format!("{key}{command}  {hint}"), INNER), Target::Item(i));
            }
            aim(
                vec![seg(key, C::Cyan), seg(*command, C::White), seg(format!("  {hint}"), C::DarkGray)],
                Target::Item(i),
            )
        })
        .collect();
    let focused = controller.apps.get(ui.focus).map(|app| app.key.as_str());
    let app = controller
        .npm_ci_key(focused)
        .and_then(|key| controller.app(&key))
        .map_or(String::new(), |app| app.name.clone());
    boxed(&format!("Install {app}'s packages"), lines, "c or n, or j k + Enter · Esc / i close", cells, rows, INNER)
}

/// y: what to copy from the focused app's output, each with how much there is of it now.
pub fn copy(ui: &mut Ui, controller: &Controller, cells: usize, rows: usize) -> Vec<Vec<Seg>> {
    use super::copy::{PICKS, Pick, gather};
    const INNER: usize = 72;
    let index = match &mut ui.overlay {
        Some(Overlay::Copy { index }) => {
            *index = (*index).min(PICKS.len() - 1);
            *index
        }
        _ => 0,
    };
    let lines: Vec<Vec<Seg>> = PICKS
        .iter()
        .enumerate()
        .map(|(i, (pick, key, label))| {
            let (lines, entries) = gather(ui, controller, *pick);
            let count = lines.iter().filter(|line| !line.is_empty()).count();
            let plural =
                |count: usize, one: &str, many: &str| format!("{count} {}", if count == 1 { one } else { many });
            let size = match pick {
                _ if lines.is_empty() => "none".to_string(),
                Pick::LastError => format!("with its stack trace, {}", plural(count, "line", "lines")),
                Pick::Errors | Pick::Warnings | Pick::Both => {
                    format!("{entries} distinct {}", if entries == 1 { "entry" } else { "entries" })
                }
                Pick::Search | Pick::Output => plural(count, "line", "lines"),
            };
            let key = pad(&format!(" {key}"), 5);
            let label = pad(label, 22);
            if i == index {
                return aim(selected(&format!("{key}{label}{size}"), INNER), Target::Item(i));
            }
            let color = if lines.is_empty() { C::DarkGray } else { C::White };
            aim(vec![seg(key, C::Cyan), seg(label, color), seg(size, C::DarkGray)], Target::Item(i))
        })
        .collect();
    let app = controller.apps.get(ui.focus).map_or(String::new(), |app| app.name.clone());
    boxed(
        &format!("Copy from {app}"),
        lines,
        "press a key (y y: the last error), or j k + Enter · Esc close",
        cells,
        rows,
        INNER,
    )
}

/// The digits that focus a column, for the key reference: "1 2 3", or "1-9" for many columns.
fn column_digits(count: usize) -> String {
    let count = count.clamp(1, 9);
    if count <= 4 { (1..=count).map(|n| n.to_string()).collect::<Vec<_>>().join(" ") } else { format!("1-{count}") }
}

/// The key reference: every action by group, lowercase and uppercase twins on one line, then navigation, in as many
/// columns as the height needs.
pub fn help(ui: &Ui, controller: &Controller, cells: usize, rows: usize) -> Vec<Vec<Seg>> {
    let entry =
        |keys: &str, label: &str| vec![seg(format!("  {keys:<12}"), C::Yellow), seg(label.to_string(), C::Gray)];
    let all = actions::actions(ui, controller);
    let mut groups: Vec<&str> = Vec::new();
    for action in &all {
        if !groups.contains(&action.group) {
            groups.push(action.group);
        }
    }
    let mut entries: Vec<Vec<Seg>> = Vec::new();
    for group in groups {
        entries.push(vec![seg(group, C::Cyan)]);
        for action in all.iter().filter(|action| action.group == group) {
            let key = action.key.as_str();
            // A focused-app key and its uppercase twin for every app share a line (u U); other keys stand alone (d D).
            let is_upper = key.len() == 1 && key.chars().all(|c| c.is_ascii_uppercase());
            if is_upper && all.iter().any(|other| other.focused && other.key == key.to_lowercase()) {
                continue;
            }
            if key == "-" {
                continue;
            }
            let twin = all
                .iter()
                .find(|other| action.focused && other.key != key && other.key == key.to_uppercase() && key.len() == 1);
            if let Some(twin) = twin {
                entries.push(entry(
                    &format!("{key} {}", twin.key),
                    &format!("{} · focused / all", action.label.replace(" the focused app", "")),
                ));
            } else if key == "+" {
                entries.push(entry("+ -", "Widen / narrow the focused column"));
            } else {
                entries.push(entry(key, &action.label));
            }
        }
    }
    entries.push(vec![seg("Navigate", C::Cyan)]);
    let digits = column_digits(controller.apps.len());
    for (keys, label) in [
        ("h l Tab", "focus another column"),
        (digits.as_str(), "jump to a column"),
        ("j k ↑ ↓", "scroll a line"),
        ("^u ^d ^b ^f", "half page / page (PgUp PgDn)"),
        ("gg G < >", "oldest / follow new output"),
        ("n N Esc", "search: older / newer / clear"),
    ] {
        entries.push(entry(keys, label));
    }
    let columns = entries.len().div_ceil(rows.saturating_sub(4).max(1)).max(2);
    let per_column = entries.len().div_ceil(columns);
    let widths: Vec<usize> = (0..columns)
        .map(|c| entries.iter().skip(c * per_column).take(per_column).map(|entry| width(entry)).max().unwrap_or(0) + 4)
        .collect();
    let lines: Vec<Vec<Seg>> = (0..per_column)
        .map(|i| {
            let mut line = Vec::new();
            for (c, column_width) in widths.iter().enumerate() {
                let Some(entry) = entries.get(c * per_column + i) else { break };
                line.extend(entry.clone());
                if c + 1 < columns {
                    line.push(seg(" ".repeat(column_width.saturating_sub(width(entry)).max(1)), C::Gray));
                }
            }
            line
        })
        .collect();
    let inner = widths.iter().sum::<usize>().saturating_sub(4);
    let footer = "any key closes · : searches every command · lowercase = focused app, uppercase = all";
    boxed(&format!("Keys · lazyrunner v{VERSION}"), lines, footer, cells, rows, inner)
}

fn icon(status: Status, tick: u64, animated: bool, nerd: bool) -> String {
    match status {
        Status::Pending => "·".into(),
        Status::Running => {
            if animated {
                spinner(tick).to_string()
            } else {
                "●".into()
            }
        }
        Status::Ok => icons::icon(nerd, Icon::Ok).into(),
        Status::Warn => icons::icon(nerd, Icon::Warn).into(),
        Status::Fail => icons::icon(nerd, Icon::Fail).into(),
        Status::Skip => icons::icon(nerd, Icon::Skip).into(),
    }
}

fn status_color(status: Status) -> C {
    match status {
        Status::Pending | Status::Skip => C::DarkGray,
        Status::Running | Status::Warn => C::Yellow,
        Status::Ok => C::Green,
        Status::Fail => C::Red,
    }
}

/// The startup checklist: the logo with a light sweeping across it, the checks, and what's new after an update.
/// A small terminal loses the logo first; with news to show, the checks that passed fold into one line; then the
/// news is cut.
pub fn checklist(ui: &Ui, cells: usize, rows: usize) -> Vec<Vec<Seg>> {
    let Some(Overlay::Checklist(list)) = &ui.overlay else { return Vec::new() };
    let list: &Checklist = list;
    let tick = ui.tick();
    let animated = !ui.reduced_motion;
    let room = rows.saturating_sub(3);
    let news = changelog::wrap(&list.news, cells.saturating_sub(8).clamp(20, 84).saturating_sub(3));
    let passed = list.items.iter().filter(|item| matches!(item.status, Status::Ok | Status::Skip)).count();
    let finished = list.items.iter().all(|item| !matches!(item.status, Status::Pending | Status::Running));
    let fold =
        !news.is_empty() && finished && passed > 1 && list.items.len() + news.len().min(WHATS_NEW_MAX_LINES) + 4 > room;
    let mut checks: Vec<Vec<Seg>> = list
        .items
        .iter()
        .filter(|item| !fold || !matches!(item.status, Status::Ok | Status::Skip))
        .map(|item| {
            let detail_color = if item.status == Status::Ok { C::Gray } else { status_color(item.status) };
            vec![
                seg(format!(" {} ", icon(item.status, tick, animated, ui.nerd)), status_color(item.status)),
                seg(format!("{:<19}", item.name), C::White),
                seg(item.detail.clone(), detail_color),
            ]
        })
        .collect();
    if fold {
        checks.insert(
            0,
            vec![
                seg(format!(" {} ", icons::icon(ui.nerd, Icon::Ok)), C::Green),
                seg(format!("{passed} checks passed"), C::Gray),
            ],
        );
    }
    let news_room = WHATS_NEW_MAX_LINES.min(room.saturating_sub(checks.len() + 3));
    let mut lines: Vec<Vec<Seg>> = Vec::new();
    let news_rows = if news.is_empty() { 1 } else { news.len().min(WHATS_NEW_MAX_LINES) + 3 };
    if checks.len() + news_rows + LOGO.len() + 2 <= room {
        lines.push(Vec::new());
        // The light's column: left to right, then a pause.
        let cycle = LOGO[0].chars().count() as f64 + 15.0;
        let sweep = if animated { (ui.seconds() * SWEEP_COLUMNS_PER_SECOND) % cycle - 6.0 } else { -99.0 };
        for row in LOGO {
            let mut segs = vec![seg("   ", C::Gray)];
            for (i, c) in row.chars().enumerate() {
                let distance = (i as f64 - sweep).abs();
                segs.push(seg(
                    c.to_string(),
                    if distance <= 1.0 {
                        C::White
                    } else if distance <= 3.0 {
                        C::Cyan
                    } else {
                        C::DarkCyan
                    },
                ));
            }
            lines.push(segs);
        }
    }
    lines.push(Vec::new());
    lines.extend(checks);
    lines.push(Vec::new());
    if !news.is_empty() && news_room >= 1 {
        lines.push(vec![seg(" ✦ What's new", C::Cyan)]);
        let shown = if news.len() > news_room { news_room.saturating_sub(1) } else { news.len() };
        for line in news.iter().take(shown) {
            lines.push(vec![seg(format!("   {}", line.text), hue_color(line.hue))]);
        }
        if news.len() > shown {
            lines.push(vec![seg(
                format!("   … {} more lines - , then c shows the changelog", news.len() - shown),
                C::DarkGray,
            )]);
        }
        lines.push(Vec::new());
    }
    let sparkle = if animated { SPARKLE[(tick / 3) as usize % 2] } else { SPARKLE[0] };
    let title = format!("{sparkle} lazyrunner v{VERSION} · startup checklist");
    draining_box(&title, lines, &list.footer, cells, rows, 84, list.draining(ui.now))
}

pub fn changelog(ui: &mut Ui, cells: usize, rows: usize) -> Vec<Vec<Seg>> {
    let Some(Overlay::Changelog { lines, top }) = &mut ui.overlay else { return Vec::new() };
    let wrapped = changelog::wrap(lines, cells.saturating_sub(8).clamp(20, 90));
    let visible = rows.saturating_sub(4).max(1);
    *top = (*top).min(wrapped.len().saturating_sub(visible));
    let shown: Vec<Vec<Seg>> =
        wrapped.iter().skip(*top).take(visible).map(|line| vec![seg(line.text.clone(), hue_color(line.hue))]).collect();
    let more = if wrapped.len() > visible {
        format!("{}-{} of {} · j k scroll · ", *top + 1, *top + shown.len(), wrapped.len())
    } else {
        String::new()
    };
    boxed(&format!("lazyrunner v{VERSION} · changelog"), shown, &format!("{more}Esc closes"), cells, rows, 90)
}
