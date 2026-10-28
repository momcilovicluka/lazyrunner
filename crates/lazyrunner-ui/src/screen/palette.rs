//! The command palette (`:`): every action and setting as a row of keycaps - grouped under headers while nothing is
//! typed, best matches first once something is, the typed letters lit in each label, the focused app's icon on what
//! acts on it - under a coloured selection bar that slides to the chosen row. Enter runs the action at once; the
//! palette shows its keycap pressed for a frame and wipes away (screen::motion).

use lazyrunner_core::run::Controller;
use unicode_width::UnicodeWidthStr;

use super::actions::{self, Action};
use super::icons::{self, Icon};
use super::motion::Closing;
use super::mouse::Target;
use super::overlays::boxed;
use super::seg::{Seg, aim, keycaps, seg, width};
use super::theme::C;
use super::{Overlay, Ui};

const INNER: usize = 84;
const FOOTER: &str = "type to search · ↑↓ choose · Enter run · Esc close";
/// A row's icon and the space after it.
const ICON_CELLS: usize = 4;
/// The least room a hint gets beside its label before it's left out.
const MIN_HINT: usize = 8;

/// A row of the list: a group's header (above the item at that index), or an item.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Row {
    Header(usize),
    Item(usize),
}

/// The open palette.
pub fn rows(ui: &mut Ui, controller: &Controller, cells: usize, rows: usize) -> Vec<Vec<Seg>> {
    let Some(Overlay::Palette { query, index }) = &ui.overlay else { return Vec::new() };
    let (query, wanted) = (query.clone(), *index);
    let found = actions::palette(ui, controller, &query);
    let index = wanted.min(found.len().saturating_sub(1));
    if let Some(Overlay::Palette { index: stored, .. }) = &mut ui.overlay {
        *stored = index;
    }
    let bar = ui.motion.bar(index, ui.now);
    draw(ui, controller, &query, &found, bar, None, cells, rows)
}

/// The palette Enter closed, while it shows the chosen keycap pressed and then wipes away.
pub fn closing(ui: &Ui, controller: &Controller, cells: usize, rows: usize) -> Option<Vec<Vec<Seg>>> {
    let (query, index, phase) = ui.motion.palette_closing(ui.now)?;
    let found = actions::palette(ui, controller, &query);
    let pressed = (phase == Closing::Pressed).then_some(index);
    let lines = draw(ui, controller, &query, &found, index, pressed, cells, rows);
    Some(match phase {
        Closing::Pressed => lines,
        Closing::Wiping(left) => super::reveal(lines, left),
    })
}

#[allow(clippy::too_many_arguments)]
fn draw(
    ui: &Ui,
    controller: &Controller,
    query: &str,
    found: &[Action],
    bar: usize,
    pressed: Option<usize>,
    cells: usize,
    rows: usize,
) -> Vec<Vec<Seg>> {
    let widest = actions::palette(ui, controller, "").iter().map(natural_width).max().unwrap_or(0);
    let inner = super::overlays::fit_inner(cells, INNER, widest);
    let count = if found.len() == 1 { "1 command".to_string() } else { format!("{} commands", found.len()) };
    let mut prompt = vec![seg("› ", C::Cyan), seg(query, C::White), seg(ui.cursor(), C::Cyan)];
    let gap = inner.saturating_sub(width(&prompt) + count.width());
    prompt.extend([seg(" ".repeat(gap), C::Gray), seg(count, C::DarkGray)]);
    let mut lines = vec![prompt, vec![seg("─".repeat(inner), C::DarkGray)]];
    let grouped = query.trim().is_empty();
    let mut list = Vec::new();
    for (at, action) in found.iter().enumerate() {
        if grouped && (at == 0 || found[at - 1].group != action.group) {
            list.push(Row::Header(at));
        }
        list.push(Row::Item(at));
    }
    // The bar's row in view, with its group's header when that fits.
    let room = rows.saturating_sub(8).max(1);
    let at = list.iter().position(|row| *row == Row::Item(bar)).unwrap_or(0);
    let mut first = (at + 1).saturating_sub(room).min(list.len().saturating_sub(room));
    if first == at && first > 0 && room > 1 && matches!(list[at - 1], Row::Header(_)) {
        first -= 1;
    }
    for row in list.iter().skip(first).take(room) {
        lines.push(match *row {
            Row::Header(at) => header(ui, found[at].group, inner),
            Row::Item(at) => {
                let line = item(ui, controller, &found[at], query, at == bar, pressed == Some(at), inner);
                aim(line, Target::Item(at))
            }
        });
    }
    if found.is_empty() {
        lines.push(vec![seg("  no matching command", C::DarkGray)]);
    }
    boxed("Commands", lines, FOOTER, cells, rows, inner)
}

fn group_icon(group: &str) -> Icon {
    match group {
        "Run" => Icon::Run,
        "Tools" => Icon::Tools,
        "View" => Icon::View,
        "Help" => Icon::Help,
        _ => Icon::Settings,
    }
}

/// " ▸ Run ─────────"
fn header(ui: &Ui, group: &str, inner: usize) -> Vec<Seg> {
    let title = format!(" {} {group} ", icons::icon(ui.nerd, group_icon(group)));
    let rule = "─".repeat(inner.saturating_sub(title.width()));
    vec![seg(title, C::DarkCyan), seg(rule, C::DarkGray)]
}

/// An action: its icon (the focused app's on what acts on it), its label with the typed letters lit, its hint when
/// there's room, and its keys as keycaps on the right - all on the bar's colours when it's the chosen one.
/// A command's row with nothing cut: its icon, label, hint and keycap.
fn natural_width(action: &Action) -> usize {
    let hint = if action.hint.is_empty() { 0 } else { 2 + action.hint.width() };
    ICON_CELLS + action.label.width() + hint + 2 + width(&keycaps(&action.key, C::White, C::DarkGray))
}

fn item(
    ui: &Ui,
    controller: &Controller,
    action: &Action,
    query: &str,
    on_bar: bool,
    pressed: bool,
    inner: usize,
) -> Vec<Seg> {
    let (fg, bg) = if on_bar { (C::FocusText, Some(C::FocusBack)) } else { (C::White, None) };
    let paint = |text: String, color: C| Seg { text, fg: color, bg, style: None, target: None, underline: false };
    let icon = match controller.apps.get(ui.focus).filter(|_| action.focused) {
        Some(app) => icons::app(ui.nerd, app, controller.framework(&app.key)).to_string(),
        None => icons::icon(ui.nerd, group_icon(action.group)).to_string(),
    };
    // A keycap stands out on the bar, and shows pressed (the bar's colours: sunk into it) for a frame after Enter.
    let (cap_fg, cap_bg) = match (pressed, on_bar) {
        (true, _) => (C::FocusText, C::FocusBack),
        (false, true) => (C::FocusBack, C::FocusText),
        (false, false) => (C::White, C::DarkGray), // dark, so a column of them doesn't glare
    };
    let mut caps = keycaps(&action.key, cap_fg, cap_bg);
    for gap in caps.iter_mut().filter(|cap| cap.bg.is_none()) {
        gap.bg = bg;
    }
    let caps_cells = width(&caps);
    let mut segs = vec![paint(format!(" {icon}  "), if on_bar { C::FocusText } else { C::Cyan })];
    let room = inner.saturating_sub(ICON_CELLS + caps_cells + 2);
    let label = if action.hint.is_empty() { action.label.clone() } else { format!("{}  ", action.label) };
    if on_bar && label.width() + action.hint.width() > room {
        segs.push(paint(ui.scrolling_parts(&[&label, &action.hint], room), fg));
    } else {
        let label: Vec<char> = action.label.chars().collect();
        let shown = label.len().min(room);
        let lit = actions::fuzzy_positions(query, &action.label);
        let mut at = 0;
        while at < shown {
            let matched = lit.contains(&at);
            let end = (at..shown).find(|i| lit.contains(i) != matched).unwrap_or(shown);
            let color = match (matched, on_bar) {
                (true, false) => C::Yellow,
                _ => fg,
            };
            let mut run = paint(label[at..end].iter().collect(), color);
            run.underline = matched;
            segs.push(run);
            at = end;
        }
        let hint_room = room.saturating_sub(shown + 2);
        if !action.hint.is_empty() && hint_room >= MIN_HINT {
            let hint: String = if action.hint.chars().count() > hint_room {
                format!("{}…", action.hint.chars().take(hint_room - 1).collect::<String>())
            } else {
                action.hint.clone()
            };
            segs.push(paint(format!("  {hint}"), if on_bar { C::FocusText } else { C::DarkGray }));
        }
    }
    let filled = width(&segs);
    segs.push(paint(" ".repeat(inner.saturating_sub(filled + caps_cells + 1)), fg));
    segs.extend(caps);
    segs.push(paint(" ".into(), fg));
    segs
}
