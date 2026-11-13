//! The boxes drawn over the columns: the database picker, the command palette, the x menu and the settings, the
//! key reference (on the whole screen), the startup checklist and the changelog.

use lazyrunner_core::changelog;
use lazyrunner_core::run::Controller;
use std::time::Duration;

use lazyrunner_core::run::checks::{Group, Kind, Status};
use unicode_width::UnicodeWidthStr;

use super::actions::{self, Action};
use super::checklist::{Checklist, LAND, SETTLE};
use super::columns::{fit, hue_color, spinner};
use super::icons::{self, Icon};
use super::motion::{FLASH, Spot};
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
/// The startup doctor's widest text, and from how wide the picked check's details go beside the list.
const DOCTOR_INNER: usize = 150;
const TWO_PANES_FROM: usize = 110;
/// A check's `FIX` tag, and the space before it.
const FIX_CELLS: usize = 6;
/// A check's row ends with how long it took and a bar of its share: this many cells.
const TOOK_CELLS: usize = 10;
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
/// How wide a box's text is in a terminal `cells` wide, `max_inner` at most.
pub fn box_inner(cells: usize, max_inner: usize) -> usize {
    cells.saturating_sub(8).min(max_inner).max(20)
}

fn draining_box(
    title: &str,
    lines: Vec<Vec<Seg>>,
    footer: &str,
    cells: usize,
    rows: usize,
    max_inner: usize,
    drain: Option<f64>,
) -> Vec<Vec<Seg>> {
    let inner = box_inner(cells, max_inner);
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

/// A setting's label as a switch: whether it's on, and the rest.
fn switch(label: &str) -> Option<(bool, &str)> {
    label.strip_prefix("[x] ").map(|rest| (true, rest)).or_else(|| label.strip_prefix("[ ] ").map(|rest| (false, rest)))
}

/// A selected item in parts (key, label, hint): when it doesn't fit, only its tail scrolls (`Ui::scrolling_parts`).
fn selected_parts(ui: &Ui, parts: &[&str], inner: usize) -> Vec<Seg> {
    vec![on(ui.scrolling_parts(parts, inner), C::Black, C::Cyan)]
}

/// A box's inner width: room for its widest row, at least `usual` (so boxes keep their size), never more than the
/// terminal has.
pub fn fit_inner(cells: usize, usual: usize, widest: usize) -> usize {
    cells.saturating_sub(8).min(usual.max(widest)).max(20)
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
    let widest = items
        .iter()
        .map(|item| 8 + item.label.width() + if item.hint.is_empty() { 0 } else { 2 + item.hint.width() })
        .max()
        .unwrap_or(0);
    let inner = fit_inner(cells, INNER, widest);
    let index = match &mut ui.overlay {
        Some(Overlay::Menu { index } | Overlay::Settings { index }) => {
            *index = (*index).min(items.len().saturating_sub(1));
            *index
        }
        _ => 0,
    };
    let row = |i: usize, item: &Action| {
        let key = pad(&format!(" {}", item.key), 8);
        let hint = if item.hint.is_empty() { String::new() } else { format!("  {}", item.hint) };
        // A setting's [x] / [ ] is a switch: a green ● on, a dim ○ off.
        let switch = switch(&item.label);
        let label =
            switch.map_or_else(|| item.label.clone(), |(on, rest)| format!("{} {rest}", if on { "●" } else { "○" }));
        let flipped = matches!(&item.run, actions::Run::Setting { toggle, .. }
            if ui.motion.lit(&super::motion::Spot::Setting(*toggle), ui.now).is_some());
        if flipped {
            let line = pad(&format!("{key}{label}{hint}"), inner);
            return aim(vec![on(line, C::Black, C::Green)], Target::Item(i));
        }
        if i == index {
            let label = if item.hint.is_empty() { label } else { format!("{label}  ") };
            return aim(selected_parts(ui, &[&key, &label, &item.hint], inner), Target::Item(i));
        }
        let mut segs = vec![seg(key, C::Cyan)];
        match switch {
            Some((true, rest)) => segs.extend([seg("● ", C::Green), seg(rest.to_string(), C::White)]),
            Some((false, rest)) => segs.extend([seg("○ ", C::DarkGray), seg(rest.to_string(), C::Gray)]),
            None => segs.push(seg(item.label.clone(), C::White)),
        }
        segs.push(seg(hint, C::DarkGray));
        aim(segs, Target::Item(i))
    };
    // The settings by group, a title over each, when there's room for them.
    let group = |item: &Action| match &item.run {
        actions::Run::Setting { toggle, .. } => actions::setting_group(*toggle),
        _ => "More",
    };
    let settings = matches!(ui.overlay, Some(Overlay::Settings { .. }));
    let groups = items.iter().map(group).collect::<std::collections::BTreeSet<_>>().len();
    let titled = settings && items.len() + groups + 3 <= rows;
    let mut lines: Vec<Vec<Seg>> = Vec::new();
    for (i, item) in items.iter().enumerate() {
        if titled && (i == 0 || group(item) != group(&items[i - 1])) {
            lines.push(vec![seg(format!(" {}", group(item).to_uppercase()), C::DarkCyan)]);
        }
        lines.push(row(i, item));
    }
    if matches!(ui.overlay, Some(Overlay::Menu { .. })) {
        let name = controller.apps.get(ui.focus).map_or(String::new(), |app| app.name.clone());
        boxed(&name, lines, "press a key, or j k + Enter · Esc / x close", cells, rows, inner)
    } else {
        boxed(
            &icons::titled(ui.nerd, Icon::Settings, "Settings (saved)"),
            lines,
            "press a letter to toggle, or j k + Enter · Esc / , close",
            cells,
            rows,
            inner,
        )
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
    let widest = INSTALLS.iter().map(|(_, command, hint)| 7 + command.width() + hint.width()).max().unwrap_or(0);
    let inner = fit_inner(cells, INNER, widest);
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
                return aim(selected_parts(ui, &[&key, &format!("{command}  "), hint], inner), Target::Item(i));
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
    boxed(
        &icons::titled(ui.nerd, Icon::Package, &format!("Install {app}'s packages")),
        lines,
        "c or n, or j k + Enter · Esc / i close",
        cells,
        rows,
        inner,
    )
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
                return aim(selected_parts(ui, &[&key, &label, &size], INNER), Target::Item(i));
            }
            let color = if lines.is_empty() { C::DarkGray } else { C::White };
            aim(vec![seg(key, C::Cyan), seg(label, color), seg(size, C::DarkGray)], Target::Item(i))
        })
        .collect();
    let app = controller.apps.get(ui.focus).map_or(String::new(), |app| app.name.clone());
    boxed(
        &icons::titled(ui.nerd, Icon::Clipboard, &format!("Copy from {app}")),
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
/// The key reference's keys: `/` filters it (`Enter` keeps the filter, `Esc` clears it), any other key closes it.
pub fn help_key(ui: &mut Ui, name: &str, key: &crossterm::event::KeyEvent) {
    if ui.help_typing {
        match name {
            "Escape" => (ui.help_filter, ui.help_typing) = (String::new(), false),
            "Enter" => ui.help_typing = false,
            "Backspace" => {
                ui.help_filter.pop();
            }
            _ => ui.help_filter.extend(super::keys::typed(key)),
        }
        return;
    }
    if name == "/" {
        (ui.help_typing, ui.help_filter) = (true, String::new());
        return;
    }
    ui.help_filter.clear();
    ui.overlay = None;
}

/// The groups' colours for their keys.
const GROUP_COLORS: [C; 6] = [C::Yellow, C::Green, C::Magenta, C::Cyan, C::Blue, C::DarkYellow];
/// The keys used most this session, listed at most.
const MOST_USED: usize = 6;

pub fn help(ui: &Ui, controller: &Controller, cells: usize, rows: usize) -> Vec<Vec<Seg>> {
    let color = std::cell::Cell::new(C::Yellow);
    let entry =
        |keys: &str, label: &str| vec![seg(format!("  {keys:<12}"), color.get()), seg(label.to_string(), C::Gray)];
    let all = actions::actions(ui, controller);
    let mut groups: Vec<&str> = Vec::new();
    for action in &all {
        if !groups.contains(&action.group) {
            groups.push(action.group);
        }
    }
    let mut entries: Vec<Vec<Seg>> = Vec::new();
    let mut used: Vec<(&String, &usize)> = ui.key_counts.iter().collect();
    used.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
    if !used.is_empty() {
        entries.push(vec![seg("You use most", C::White)]);
        for (key, count) in used.into_iter().take(MOST_USED) {
            let label = all.iter().find(|action| &action.key == key).map_or("", |action| action.label.as_str());
            entries.push(entry(key, &format!("{label} ×{count}")));
        }
    }
    for (place, group) in groups.into_iter().enumerate() {
        color.set(GROUP_COLORS[place % GROUP_COLORS.len()]);
        entries.push(vec![seg(group, C::Cyan)]);
        for action in all.iter().filter(|action| action.group == group) {
            let key = action.key.as_str();
            // A focused-app key and its uppercase twin for every app - in the same group, the same verb - share a line
            // (u U); other keys stand alone (d D, m M, o O).
            let is_upper = key.len() == 1 && key.chars().all(|c| c.is_ascii_uppercase());
            let verb = |label: &str| label.split_whitespace().next().unwrap_or_default().to_string();
            let twinned = |other: &Action| other.group == action.group && verb(&other.label) == verb(&action.label);
            if is_upper && all.iter().any(|other| twinned(other) && other.focused && other.key == key.to_lowercase()) {
                continue;
            }
            if key == "-" {
                continue;
            }
            let twin = all.iter().find(|other| {
                twinned(other)
                    && action.focused
                    && other.key != key
                    && other.key == key.to_uppercase()
                    && key.len() == 1
            });
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
    color.set(C::Yellow);
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
    if !ui.help_filter.is_empty() {
        let wanted = ui.help_filter.to_lowercase();
        let text = |entry: &Vec<Seg>| entry.iter().map(|part| part.text.to_lowercase()).collect::<String>();
        entries.retain(|entry| entry.len() > 1 && text(entry).contains(&wanted));
        if entries.is_empty() {
            entries.push(vec![seg(format!("No key matches '{}'.", ui.help_filter), C::DarkGray)]);
        }
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
    let typing = format!("/{}{} · Enter keeps it · Esc clears it", ui.help_filter, ui.cursor());
    let footer = if ui.help_typing {
        typing.as_str()
    } else {
        "/ filters · any key closes · : searches every command · lowercase = focused app, uppercase = all apps or a tool"
    };
    boxed(
        &icons::titled(ui.nerd, Icon::Keyboard, &format!("Keys · lazyrunner v{VERSION}")),
        lines,
        footer,
        cells,
        rows,
        inner,
    )
}

fn icon(status: Status, spin: Option<&str>, nerd: bool) -> String {
    match status {
        Status::Pending => "·".into(),
        Status::Running => spin.unwrap_or("●").into(),
        Status::Ok => icons::icon(nerd, Icon::Ok).into(),
        Status::Warn => icons::icon(nerd, Icon::Warn).into(),
        Status::Fail => icons::icon(nerd, Icon::Fail).into(),
        Status::Skip => icons::icon(nerd, Icon::Skip).into(),
    }
}

/// A check's name in its column, after its icon with Nerd Font icons - a command app's its language's, and its
/// language's chip without them.
fn check_name(nerd: bool, name: &str, language: Option<&str>) -> String {
    let icon = language.map(icons::language_icon).or_else(|| icons::check(name));
    match (icon.filter(|_| nerd), language) {
        (Some(icon), _) => format!("{} {name:<17}", icons::icon(true, icon)),
        (None, Some(language)) if !nerd => format!("{} {name:<16}", icons::language_chip(language)),
        (None, _) if nerd => format!("  {name:<17}"),
        (None, _) => format!("{name:<19}"),
    }
}

/// A kept session's age: within the hour green, within the day the accent, older amber.
fn age_color(freshness: lazyrunner_core::run::session::Freshness) -> C {
    use lazyrunner_core::run::session::Freshness;
    match freshness {
        Freshness::Recent => C::Green,
        Freshness::Today => C::Cyan,
        Freshness::Older => C::Yellow,
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

/// The startup doctor (#37, #77): a header - the project, a bar of the checks done, what they found, how long they
/// took and the slowest - then the checks under their groups' headers, every group open unless the terminal is too
/// short for them all, beside the picked check's details (under it on a narrower terminal); then what's new after an
/// update, a legend, the countdown to the start, and the keys. It moves too (not with reduced motion): the selection
/// bar glides a row at a time and flashes going round the end, the counts tick up a check at a time and pulse as they
/// change, and each check's mark lands bright as it finishes.
pub fn checklist(ui: &mut Ui, cells: usize, rows: usize) -> Vec<Vec<Seg>> {
    let animated = !ui.reduced_motion;
    let inner = box_inner(cells, DOCTOR_INNER);
    let wide = inner >= TWO_PANES_FROM;
    let left_width = if wide { inner * 3 / 5 } else { inner };
    let right_width = inner.saturating_sub(left_width + 3);
    let Some(list) = doctor(ui) else { return Vec::new() };
    let news = changelog::wrap(&list.news, left_width.saturating_sub(3));
    let news_rows = if news.is_empty() { 0 } else { news.len().min(WHATS_NEW_MAX_LINES) + 2 };
    let fold = 7 + groups(list).len() + list.items.len() + news_rows > rows.saturating_sub(4);
    let (mut left, picked) = checks_pane(ui, list, left_width, wide, fold);
    let bar = ui.motion.bar(picked, ui.now);
    let bright = ui.motion.lit(&Spot::Wrapped, ui.now).is_some();
    if let Some(row) = left.get_mut(bar) {
        light(row, left_width, bright);
    }
    let Some(list) = doctor(ui) else { return Vec::new() };
    let copied = list.flashed.is_some_and(|at| ui.now.saturating_duration_since(at) < super::motion::COPY_FLASH);
    if animated && copied {
        for part in left.iter_mut().flatten() {
            (part.fg, part.bg) = (C::Black, Some(C::Cyan)); // c just copied them
        }
    }
    if !news.is_empty() {
        left.extend([Vec::new(), vec![seg(" ✦ What's new", C::Cyan)]]);
        for line in news.iter().take(WHATS_NEW_MAX_LINES) {
            left.push(vec![seg(format!("   {}", line.text), hue_color(line.hue))]);
        }
        if news.len() > WHATS_NEW_MAX_LINES {
            let more = format!("   … {} more lines - , then c shows the changelog", news.len() - WHATS_NEW_MAX_LINES);
            left.push(vec![seg(more, C::DarkGray)]);
        }
    }
    left.extend([Vec::new(), legend(ui)]);
    let right = if wide { details(ui, list, list.selected, right_width, false) } else { Vec::new() };
    let mut lines = header(ui, list, inner);
    lines.push(Vec::new());
    for at in 0..left.len().max(right.len()) {
        let mut row = fit(left.get(at).cloned().unwrap_or_default(), left_width);
        if wide {
            row.push(seg(" │ ", C::DarkGray));
            row.extend(fit(right.get(at).cloned().unwrap_or_default(), right_width));
        }
        lines.push(row);
    }
    lines.extend([countdown(ui, list, inner), vec![seg(keys_line(ui), C::DarkGray)]]);
    let sparkle = if animated { SPARKLE[(ui.tick() / 3) as usize % 2] } else { SPARKLE[0] };
    let title = format!("{sparkle} lazyrunner v{VERSION} · startup doctor");
    draining_box(&title, lines, &list.footer, cells, rows, DOCTOR_INNER, list.draining(ui.now))
}

/// The startup checklist, while it's open.
fn doctor(ui: &Ui) -> Option<&Checklist> {
    match &ui.overlay {
        Some(Overlay::Checklist(list)) => Some(list.as_ref()),
        _ => None,
    }
}

/// The checks under their groups' headers (a group that passed folds to one row when `fold`), with the picked
/// check's details under its row when they aren't beside the list; and the line of the picked check.
fn checks_pane(ui: &Ui, list: &Checklist, width: usize, wide: bool, fold: bool) -> (Vec<Vec<Seg>>, usize) {
    let slowest = list.rows.iter().filter_map(|row| row.took).max().unwrap_or_default();
    let (mut left, mut picked) = (Vec::new(), 0);
    for (group, members) in &groups(list) {
        let passed = members
            .iter()
            .all(|&index| list.is_ignored(index) || matches!(list.items[index].status, Status::Ok | Status::Skip));
        if fold && passed && !members.contains(&list.selected) {
            left.push(group_header(ui, list, *group, members, width, true));
            continue;
        }
        left.push(group_header(ui, list, *group, members, width, false));
        for &index in members {
            if index == list.selected {
                picked = left.len();
            }
            left.push(check_row(ui, list, index, width, slowest));
            if !wide && index == list.selected && list.is_holding() && list.is_problem(index) {
                for row in details(ui, list, index, width.saturating_sub(4), true) {
                    let mut indented = vec![seg("    ", C::Gray)];
                    indented.extend(row);
                    left.push(indented);
                }
            }
        }
    }
    (left, picked)
}

/// The selection bar across a row of the checks: in the focus colours, or bright as it goes round the end of the
/// list. What has colours of its own (a `FIX` tag, a mark landing) keeps them.
fn light(row: &mut Vec<Seg>, width: usize, bright: bool) {
    *row = fit(std::mem::take(row), width);
    for part in row.iter_mut().filter(|part| part.bg.is_none()) {
        if bright {
            (part.fg, part.bg) = (C::Black, Some(C::Cyan));
            continue;
        }
        part.bg = Some(C::FocusBack);
        if matches!(part.fg, C::Gray | C::DarkGray | C::White) {
            part.fg = C::FocusText;
        }
    }
}

/// Whether the doctor moves: not with reduced motion, nor in the screen tests' frozen frames.
fn moving(ui: &Ui) -> bool {
    ui.motion.enabled && !ui.reduced_motion
}

/// The countdown to the start (none with reduced motion: the footer has it too): its hourglass, the seconds left,
/// and a bar across the box that drains with it.
fn countdown(ui: &Ui, list: &Checklist, inner: usize) -> Vec<Seg> {
    let Some((left, seconds)) = list.counting_down(ui.now).filter(|_| moving(ui)) else { return Vec::new() };
    let text = format!(" {} starting in {seconds} s  ", icons::icon(ui.nerd, Icon::Hourglass));
    let cells = inner.saturating_sub(text.width());
    let bar = share_bar(left, cells).trim_end().to_string();
    let rest = cells.saturating_sub(bar.width());
    vec![seg(text, C::Cyan), seg(bar, C::Cyan), seg("─".repeat(rest), C::DarkGray)]
}

/// The doctor's keys, under the checks.
fn keys_line(ui: &Ui) -> String {
    let clipboard = icons::icon(ui.nerd, Icon::Clipboard);
    format!(
        " j k move · n N problems · f fix · a fix all safe · i ignore · r retry · c {clipboard} report · Enter \
         continues"
    )
}

/// The checks by group, in the order they're drawn.
fn groups(list: &Checklist) -> Vec<(Group, Vec<usize>)> {
    let mut groups: Vec<(Group, Vec<usize>)> = Vec::new();
    for index in list.order() {
        let group = list.rows[index].kind.group();
        match groups.last_mut() {
            Some((last, members)) if *last == group => members.push(index),
            _ => groups.push((group, vec![index])),
        }
    }
    groups
}

/// The header: the project, a bar of the checks done, what they found, how long they took and the slowest.
fn header(ui: &Ui, list: &Checklist, inner: usize) -> Vec<Vec<Seg>> {
    let total = list.items.len();
    let tally = list.tally(moving(ui).then_some(ui.now));
    let done = tally.done;
    let mut first = vec![on(" DOCTOR ", C::Black, C::Cyan)];
    if !list.project.is_empty() {
        first.push(seg(format!(" {}", list.project), C::White));
    }
    first.push(seg(" startup checks", C::DarkGray));
    let cells = (inner / 4).clamp(10, 30);
    let filled = (done * cells).checked_div(total).unwrap_or(0);
    let mut second = vec![
        seg("▰".repeat(filled), C::Green),
        seg("▱".repeat(cells - filled), C::DarkGray),
        seg(format!("  {done} / {total} checks   "), C::Gray),
    ];
    let kinds =
        [(Status::Ok, C::Green), (Status::Warn, C::Yellow), (Status::Fail, C::Red), (Status::Skip, C::DarkGray)];
    for (at, (status, color)) in kinds.into_iter().enumerate() {
        let text = format!("{} {}", mark(ui, status), tally.counts[at]);
        let bumped = tally.bumped[at].filter(|_| moving(ui));
        let pulsing = bumped.is_some_and(|bumped| ui.now.saturating_duration_since(bumped) < FLASH);
        second.extend([if pulsing { on(text, C::Black, color) } else { seg(text, color) }, seg("   ", C::Gray)]);
    }
    if let Some(took) = list.elapsed(ui.now) {
        second.push(seg(format!("checked in {}", seconds(took)), C::DarkGray));
    }
    let slowest = list
        .order()
        .into_iter()
        .filter_map(|index| Some((index, list.rows[index].took?)))
        .max_by_key(|(_, took)| *took);
    if let Some((index, took)) = slowest {
        second.push(seg(format!(" · slowest {} {}", list.items[index].name, seconds(took)), C::DarkGray));
    }
    vec![first, second]
}

/// A group's header: its icon (with Nerd Font icons) and name, a rule, its worst result and its time; folded, how
/// many passed.
fn group_header(ui: &Ui, list: &Checklist, group: Group, members: &[usize], width: usize, folded: bool) -> Vec<Seg> {
    let icon = if ui.nerd { format!("{} ", icons::icon(true, group_icon(group))) } else { String::new() };
    let worst = members.iter().map(|&index| shown(list, index)).max_by_key(|status| severity(*status));
    let worst = worst.unwrap_or(Status::Ok);
    let took: Duration = members.iter().filter_map(|&index| list.rows[index].took).sum();
    let passed = if folded { format!(" · {} passed", members.len()) } else { String::new() };
    let name = format!(" {icon}{}{passed} ", group.name().to_uppercase());
    let result = format!(" {} {}", mark(ui, worst), seconds(took));
    let rule = width.saturating_sub(name.width() + result.width());
    vec![seg(name, C::DarkCyan), seg("─".repeat(rule), C::DarkGray), seg(result, status_color(worst))]
}

/// A check's result as drawn: running while a fix runs, skipped once `i` ignored it.
fn shown(list: &Checklist, index: usize) -> Status {
    match list.rows[index].fixing {
        Some(_) => Status::Running,
        None if list.is_ignored(index) => Status::Skip,
        None => list.items[index].status,
    }
}

/// How much a result needs a look, for a group's worst.
fn severity(status: Status) -> u8 {
    match status {
        Status::Fail => 5,
        Status::Warn => 4,
        Status::Running => 3,
        Status::Pending => 2,
        Status::Ok => 1,
        Status::Skip => 0,
    }
}

/// A check's row: an edge in red where it holds the start and amber where it warns, its mark, name and detail (dim
/// when skipped or ignored), how long it took with a bar of its share of the slowest, and a `FIX` tag when it has
/// one. A running fix spins its mark; once it worked the edge turns green and the mark flashes once, and when it
/// didn't the mark pulses red twice. A check that just finished lands with its mark bright for a moment.
fn check_row(ui: &Ui, list: &Checklist, index: usize, width: usize, slowest: Duration) -> Vec<Seg> {
    let (item, row) = (&list.items[index], &list.rows[index]);
    let animated = !ui.reduced_motion;
    let ignored = list.is_ignored(index);
    let settling = row
        .settled
        .map(|(at, worked)| (ui.now.saturating_duration_since(at), worked))
        .filter(|(since, _)| animated && *since < SETTLE * 2);
    let status = shown(list, index);
    let landing = moving(ui)
        && status != Status::Running
        && row.done_at.is_some_and(|at| ui.now.saturating_duration_since(at) < LAND);
    let edge = match (item.status, settling) {
        _ if ignored => seg(" ", C::Gray),
        (_, Some((since, true))) if since < SETTLE => seg("▌", if since < SETTLE / 2 { C::Yellow } else { C::Green }),
        (Status::Fail, _) => seg("▌", C::Red),
        (Status::Warn, _) => seg("▌", C::Yellow),
        _ => seg(" ", C::Gray),
    };
    let mut mark = seg(format!(" {} ", icon(status, animated.then(|| spinner(ui)), ui.nerd)), status_color(status));
    match settling {
        Some((since, true)) if since < FLASH => mark = on(mark.text, C::Black, C::Green),
        Some((since, false)) if (since.as_millis() / (SETTLE.as_millis() / 2)).is_multiple_of(2) => {
            mark = on(mark.text, C::Black, C::Red);
        }
        None if landing => mark = on(mark.text, C::Black, status_color(status)),
        _ => {}
    }
    let dim = status == Status::Skip;
    let detail = if ignored { format!("{} · ignored for this run", item.detail) } else { item.detail.clone() };
    let detail_color = match (item.status, &row.kind) {
        _ if dim => C::DarkGray,
        (Status::Ok, Kind::Session(freshness)) => age_color(*freshness),
        (Status::Ok, _) => C::Gray,
        (other, _) => status_color(other),
    };
    let segs = vec![
        edge,
        mark,
        seg(check_name(ui.nerd, &item.name, row.language), if dim { C::DarkGray } else { C::White }),
        seg(detail, detail_color),
    ];
    let running = row.started.filter(|_| status == Status::Running && row.took.is_none());
    let took = row.took.or_else(|| running.map(|started| ui.now.saturating_duration_since(started)));
    let share = row
        .took
        .filter(|_| !slowest.is_zero())
        .map(|took| (took.as_secs_f64() / slowest.as_secs_f64(), took == slowest));
    let mut out = fit(with_took(ui, segs, 3, width.saturating_sub(FIX_CELLS), took, share), width - FIX_CELLS);
    out.push(seg(" ", C::Gray));
    let tag = if item.status == Status::Fail { C::Red } else { C::Yellow };
    out.push(match list.fix_of(index) {
        Some(_) => on(" FIX ", C::Black, tag).at(Target::Fix(index)),
        None => seg("     ", C::Gray),
    });
    aim(out, Target::Item(index))
}

/// The picked check's details: its result, what it found, what its problem holds up, the keys that fix it, the
/// other problems, and `a` when there are safe fixes. `inline` (under its row, on a narrow terminal): only what its
/// problem holds up and what fixes it - the row says the rest.
fn details(ui: &Ui, list: &Checklist, index: usize, width: usize, inline: bool) -> Vec<Vec<Seg>> {
    let (Some(item), Some(row)) = (list.items.get(index), list.rows.get(index)) else { return Vec::new() };
    let ignored = list.is_ignored(index);
    let (word, color) = match item.status {
        _ if ignored => ("ignored for this run", C::DarkGray),
        Status::Ok => ("passed", C::Green),
        Status::Warn => ("warning", C::Yellow),
        Status::Fail => ("blocked", C::Red),
        Status::Skip => ("skipped", C::DarkGray),
        Status::Running => ("checking…", C::Yellow),
        Status::Pending => ("waiting to be checked", C::DarkGray),
    };
    let took = row.took.map_or(String::new(), |took| format!(" · {}", seconds(took)));
    let mut out = vec![
        vec![seg(item.name.clone(), C::White)],
        vec![seg(format!("{} {word}{took}", mark(ui, item.status)), color)],
    ];
    let room = width.saturating_sub(2).max(8);
    let section =
        |out: &mut Vec<Vec<Seg>>, title: &str| out.extend([Vec::new(), vec![seg(title.to_string(), C::DarkCyan)]]);
    let wrapped = |out: &mut Vec<Vec<Seg>>, text: &str, color: C| {
        out.extend(words(text, room).into_iter().map(|piece| vec![seg(format!("  {piece}"), color)]));
    };
    if inline {
        out.clear();
    } else if !item.detail.is_empty() {
        section(&mut out, "DETAILS");
        wrapped(&mut out, &item.detail, if item.status == Status::Ok { C::Gray } else { color });
    }
    let problem = matches!(item.status, Status::Warn | Status::Fail);
    if problem && !ignored {
        section(&mut out, "WHAT IT BLOCKS");
        wrapped(&mut out, list.blocks(index), C::Gray);
    }
    let mut fixes: Vec<(&str, String)> = Vec::new();
    if let Some(fix) = list.fix_of(index) {
        fixes.push(("f", fix.label().trim_start_matches("f ").to_string()));
    }
    if problem {
        fixes.push(("r", "check it again".into()));
    }
    if problem && row.kind == Kind::Database {
        fixes.push(("d", "choose another database".into()));
    }
    if problem {
        fixes.push(("i", if ignored { "stop ignoring it" } else { "ignore it for this run" }.into()));
    }
    if !fixes.is_empty() {
        section(&mut out, "FIX");
        for (key, what) in fixes {
            out.push(vec![seg(format!("  {key:<6}"), C::Cyan), seg(what, C::Gray)]);
        }
    }
    let others: Vec<usize> =
        list.order().into_iter().filter(|&other| other != index && list.is_problem(other)).collect();
    if !others.is_empty() && !inline {
        section(&mut out, "ALSO TO LOOK AT");
        for other in others {
            let other = &list.items[other];
            let text = format!("{} · {}", other.name, other.detail);
            out.push(vec![
                seg(format!("  {} ", mark(ui, other.status)), status_color(other.status)),
                seg(text, C::Gray),
            ]);
        }
    }
    let safe = list.safe_fixes();
    if safe > 0 && !inline {
        out.extend([Vec::new(), vec![seg("  a     ", C::Cyan), seg(format!("fix all safe ({safe})"), C::Gray)]]);
    }
    out
}

/// `text` wrapped at its spaces into lines of `cells` at most (a word longer than that is cut).
pub(super) fn words(text: &str, cells: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if !line.is_empty() && line.width() + 1 + word.width() > cells {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
        while line.width() > cells {
            let rest = line.split_off(line.char_indices().nth(cells).map_or(line.len(), |(at, _)| at));
            lines.push(std::mem::replace(&mut line, rest));
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// What the marks mean.
fn legend(ui: &Ui) -> Vec<Seg> {
    let mut legend = vec![seg(" ", C::Gray)];
    for (status, what, color) in [
        (Status::Ok, "passed", C::Green),
        (Status::Warn, "warning", C::Yellow),
        (Status::Fail, "blocked", C::Red),
        (Status::Skip, "skipped", C::DarkGray),
    ] {
        legend.push(seg(format!("{} {what}   ", mark(ui, status)), color));
    }
    legend
}

/// A result's mark: the status icon, plain or Nerd Font.
fn mark(ui: &Ui, status: Status) -> String {
    icon(status, None, ui.nerd)
}

/// A time in seconds, one decimal.
fn seconds(took: Duration) -> String {
    format!("{:.1} s", took.as_secs_f64())
}

/// A row, its parts after the first `keep` scrolling sideways when they don't fit, with how long it took at its end
/// and a bar of that as a share of the slowest - bright for the slowest.
fn with_took(
    ui: &Ui,
    segs: Vec<Seg>,
    keep: usize,
    inner: usize,
    took: Option<Duration>,
    share: Option<(f64, bool)>,
) -> Vec<Seg> {
    let room = inner.saturating_sub(TOOK_CELLS);
    let mut row = fit(ui.scrolling_row(segs, keep, room), room);
    let Some(took) = took else { return row };
    let seconds = took.as_secs_f64();
    let text = if seconds < 10.0 { format!("{seconds:.1} s") } else { format!("{seconds:.0} s") };
    row.push(seg(format!("{text:>7} "), C::DarkGray));
    if let Some((share, slowest)) = share {
        row.push(seg(share_bar(share, 2), if slowest { C::Cyan } else { C::DarkGray }));
    }
    row
}

/// `share` (0 to 1) of `cells` cells, in eighths.
fn share_bar(share: f64, cells: usize) -> String {
    const EIGHTHS: [char; 8] = [' ', '▏', '▎', '▍', '▌', '▋', '▊', '▉'];
    let eighths = (share.clamp(0.0, 1.0) * (cells * 8) as f64).round() as usize;
    let mut bar = "█".repeat(eighths / 8);
    if !eighths.is_multiple_of(8) && eighths / 8 < cells {
        bar.push(EIGHTHS[eighths % 8]);
    }
    format!("{bar:<cells$}")
}

/// A group's icon, for its header with Nerd Font icons.
fn group_icon(group: Group) -> Icon {
    match group {
        Group::Environment => Icon::Terminal,
        Group::Project => Icon::Folder,
        Group::Database => Icon::Database,
        Group::Packages => Icon::Package,
        Group::Update => Icon::Update,
    }
}

/// The cells the changelog's rail of versions takes before each line.
const RAIL_CELLS: usize = 2;

pub fn changelog(ui: &mut Ui, cells: usize, rows: usize) -> Vec<Vec<Seg>> {
    let Some(Overlay::Changelog { lines, top, seen }) = &mut ui.overlay else { return Vec::new() };
    let seen = seen.as_deref().and_then(changelog::version);
    let wrapped = changelog::wrap(lines, cells.saturating_sub(8).clamp(20, 90) - RAIL_CELLS);
    let visible = rows.saturating_sub(4).max(1);
    *top = (*top).min(wrapped.len().saturating_sub(visible));
    // A rail down the side: a dot per version, what's newer than the version it was last opened at lit.
    let mut newer = seen.is_some();
    let marked: Vec<Vec<Seg>> = wrapped
        .iter()
        .map(|line| {
            let heading = changelog::version(&line.text);
            if let (Some(version), Some(seen)) = (heading, seen) {
                newer = version > seen;
            }
            let (mark, color) = match (heading.is_some(), newer) {
                (true, true) => ("● ", C::Yellow),
                (true, false) => ("● ", C::DarkCyan),
                (false, true) => ("┃ ", C::Yellow),
                (false, false) => ("│ ", C::DarkGray),
            };
            vec![seg(mark, color), seg(line.text.clone(), hue_color(line.hue))]
        })
        .collect();
    let shown: Vec<Vec<Seg>> = marked.into_iter().skip(*top).take(visible).collect();
    let more = if wrapped.len() > visible {
        format!("{}-{} of {} · j k scroll · ", *top + 1, *top + shown.len(), wrapped.len())
    } else {
        String::new()
    };
    boxed(
        &icons::titled(ui.nerd, Icon::Changelog, &format!("lazyrunner v{VERSION} · changelog")),
        shown,
        &format!("{more}{}Esc closes", if seen.is_some() { "┃ new since you last looked · " } else { "" }),
        cells,
        rows,
        90,
    )
}
