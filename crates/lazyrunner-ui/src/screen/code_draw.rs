//! The code map's drawing (#89): each part a box saying as much as fits (or, zoomed out, a box with its name alone,
//! or its name alone), a part outside the level dashed and dim with where it is, an edge's color what it is - a
//! cycle's red, a broken rule's yellow with a `✗`, the picked box's cyan and on top, one only Nx knows of blue, two
//! apps sharing projects dashed with how many - and the motion: a level's boxes growing or shrinking, its edges
//! drawing in outwards from the picked box, a cycle pulsing.

use std::collections::HashMap;
use std::time::Duration;

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use lazyrunner_core::run::build_map::{BuildRun, ProjectState};
use lazyrunner_core::run::code::{CodeMap, Kind, Link, Part, STACK, Stack, TOP_FOLDER};
use lazyrunner_core::run::history::Kind as Timed;
use lazyrunner_core::run::{Controller, format_duration};

use super::Ui;
use super::code::{PULSE, Scale, Shown, View, Zoom, distances, moving, progress, since};
use super::graph::{self, Map, Motion, Rect};
use super::icons::{self, Icon};
use super::mouse::Target;
use super::seg::{Seg, on, seg};
use super::theme::C;

/// An edge draws in this long, the next step out from the picked box this much later, at most this many steps.
const REVEAL: Duration = Duration::from_millis(120);
const OUTWARD: Duration = Duration::from_millis(90);
const OUTWARD_MOST: usize = 6;
/// A level's boxes grow (or shrink) this long, from this many cells wider on the way out.
pub(super) const ZOOM: Duration = Duration::from_millis(240);
const ZOOM_PAD: f64 = 3.0;
const PULSE_STEP: u128 = 200;
/// The most a box says.
const DETAIL_MOST: usize = 3;
/// The longest name a box shows when the map doesn't fit, compact, and in the overview; the picked box's is whole.
const LONGEST: usize = 32;
const COMPACT_NAME: usize = 24;
const OVERVIEW_NAME: usize = 20;
/// The least of a name's start kept before its end when it's cut.
const HEAD_LEAST: usize = 4;
/// The minimap's size, its frame included.
pub(super) const MINI: (usize, usize) = (22, 8);

/// How the boxes look: how much they say, how long a name may be, and bare (a name, no border).
#[derive(Clone, Copy, Debug)]
pub(super) struct Look {
    pub detail: usize,
    pub short: Option<usize>,
    pub bare: bool,
}

impl Look {
    /// The least a scale shows: what h j k l step through.
    pub(super) fn of(scale: Scale) -> Self {
        match scale {
            Scale::Normal => Self { detail: 0, short: None, bare: false },
            Scale::Compact => Self { detail: 0, short: Some(COMPACT_NAME), bare: false },
            Scale::Overview => Self { detail: 0, short: Some(OVERVIEW_NAME), bare: true },
        }
    }
}

pub(super) fn icon(nerd: bool, part: &Part) -> &'static str {
    match (part.kind, part.stack) {
        (Kind::Namespace | Kind::Folder, _) => icons::icon(nerd, Icon::Folder),
        (Kind::Module, _) => icons::icon(nerd, Icon::Package),
        (_, Stack::Dotnet) => icons::icon(nerd, Icon::Dotnet),
        (_, Stack::Node) => icons::icon(nerd, Icon::Angular),
    }
}

/// Where a part is: an app, a project, or in the part it's inside of - a folder with its project (a project's
/// top folder, the project).
pub(super) fn place(map: &CodeMap, at: usize) -> String {
    match map.parts[at].inside.as_deref() {
        Some(STACK) => "an app".into(),
        None => "a project".into(),
        Some(id) => {
            let Some(inside) = map.find(id) else { return String::new() };
            let folder = &map.parts[inside];
            let project = folder.inside.as_deref().and_then(|id| map.find(id)).map(|at| map.parts[at].name.as_str());
            match project {
                Some(project) if folder.kind == Kind::Folder && folder.name == TOP_FOLDER => format!("in {project}"),
                Some(project) if folder.kind == Kind::Folder => format!("in {project} › {}", folder.name),
                _ => format!("in {}", folder.name),
            }
        }
    }
}

/// The start of a name, `cells` wide at most.
fn head(name: &str, cells: usize) -> String {
    let mut cut = String::new();
    for grapheme in name.graphemes(true) {
        if cut.width() + grapheme.width() > cells {
            break;
        }
        cut.push_str(grapheme);
    }
    cut
}

/// A name `most` cells at most: its start, a `…` and as many of its last dotted (or slashed) pieces as fit - so two
/// names ending alike stay apart; else just those pieces after the `…`, else its start and a `…`.
pub(super) fn short(name: &str, most: usize) -> String {
    if name.width() <= most {
        return name.to_string();
    }
    let pieces: Vec<&str> = name.split_inclusive(['.', '/']).collect();
    let rests: Vec<String> = (1..pieces.len()).map(|start| pieces[start..].concat()).collect();
    let headed = rests.iter().find(|rest| most.saturating_sub(rest.width() + 1) >= HEAD_LEAST);
    if let Some(rest) = headed {
        return format!("{}…{rest}", head(name, most - rest.width() - 1));
    }
    if let Some(rest) = rests.iter().find(|rest| rest.width() < most) {
        return format!("…{rest}");
    }
    format!("{}…", head(name, most.saturating_sub(1)))
}

/// What a part has, for its box: an app's projects, a .NET project's packages, a TypeScript one's modules.
fn has(part: &Part) -> String {
    let count = |count: usize, what: &str| format!(" · {count} {what}{}", if count == 1 { "" } else { "s" });
    match part.stack {
        _ if part.inside.as_deref() == Some(STACK) => count(part.packages, "project"),
        Stack::Dotnet if part.packages > 0 => count(part.packages, "package"),
        Stack::Node if part.modules > 0 && part.inside.is_none() => count(part.modules, "module"),
        _ => String::new(),
    }
}

/// The build over the projects (#82): the build, its critical path (its projects' places in it), the spinner's frame.
pub(super) struct Layer<'a> {
    pub run: &'a BuildRun,
    pub critical: Vec<usize>,
    pub spinner: &'a str,
}

/// A time as the build map says it: tenths under ten seconds.
fn took(seconds: f64) -> String {
    if seconds < 10.0 { format!("{seconds:.1}s") } else { format_duration(seconds) }
}

/// A project's state in the build: its mark, what the box says, its colour.
fn build_state(layer: &Layer, at: usize) -> (&'static str, String, C) {
    match layer.run.state(at) {
        ProjectState::Done => {
            let own = layer.run.own_seconds(at).map_or(String::new(), took);
            ("✓", format!("✓ {own}"), C::Green)
        }
        ProjectState::Building => ("◌", format!("{} building", layer.spinner), C::Yellow),
        ProjectState::Queued => ("·", "· queued".into(), C::DarkGray),
        ProjectState::Failed => ("✗", "✗ failed".into(), C::Red),
        ProjectState::Skipped => ("–", "– not built".into(), C::DarkGray),
    }
}

/// The header's say on the build: how far a running one is, how long a done one took and its critical path.
pub(super) fn build_summary(ui: &Ui, run: &BuildRun) -> Vec<Seg> {
    let mut segs = summary_of(ui, run);
    segs.push(seg(" · b hides it", C::DarkGray));
    segs
}

fn summary_of(ui: &Ui, run: &BuildRun) -> Vec<Seg> {
    let hammer = icons::icon(ui.nerd, Icon::Hammer);
    let (total, done) = (run.projects.len(), run.done());
    let critical =
        run.critical_seconds().map_or(String::new(), |seconds| format!(" · critical path {}", took(seconds)));
    match (run.seconds, run.ok) {
        (None, _) => vec![seg(format!(" {hammer} building {} {done}/{total}{critical}", run.app), C::Yellow)],
        (Some(seconds), Some(true)) => {
            let of = if critical.is_empty() { String::new() } else { format!(" of {}", took(seconds)) };
            vec![seg(format!(" {hammer} {} built in {}{critical}{of}", run.app, took(seconds)), C::Magenta)]
        }
        (Some(seconds), _) => {
            vec![seg(
                format!(" {hammer} {} build failed after {} ({done}/{total} built)", run.app, took(seconds)),
                C::Red,
            )]
        }
    }
}

/// Under the map, the picked project in the build: its own time against its usual, or how it failed.
pub(super) fn build_details(controller: &Controller, layer: &Layer, part: &Part, width: usize) -> Vec<Vec<Seg>> {
    let Some(at) = layer.run.at_path(&part.path) else {
        let text = format!(" {} isn't in {}'s build", part.name, layer.run.app);
        return vec![super::columns::fit(vec![seg(text, C::DarkGray)], width)];
    };
    let (_, said, color) = build_state(layer, at);
    let key = format!("{}/{}", layer.run.key, layer.run.projects[at].name);
    let usual = controller.durations.usual(Timed::Project, &key);
    let mut text = format!(" ⚒ in this build: {said}");
    if let (Some(own), Some(usual)) = (layer.run.own_seconds(at), usual) {
        let change = if usual > 0.0 { (own - usual) / usual } else { 0.0 };
        let mark = match () {
            _ if change >= 0.15 => " ▲",
            _ if change <= -0.15 => " ▼",
            _ => "",
        };
        text.push_str(&format!(" · usual {}{mark}", took(usual)));
    }
    if layer.critical.contains(&at) {
        text.push_str(" · on the critical path");
    }
    vec![super::columns::fit(vec![seg(text, color)], width)]
}

/// What sets a box apart: picked, in a cycle, about to be hidden by focus, outside the level.
#[derive(Clone, Copy)]
struct Marks {
    picked: bool,
    cyclic: bool,
    dim: bool,
    outside: bool,
}

/// A bare box: its name in its color, the picked one lit.
fn bare(part: &Part, text: String, marks: Marks) -> graph::Node {
    let color = match () {
        _ if marks.dim || marks.outside || part.kind == Kind::Tests => C::DarkGray,
        _ if marks.cyclic => C::Red,
        _ => C::Gray,
    };
    let name = if marks.picked { on(text, C::FocusText, C::FocusBack) } else { seg(text, color) };
    graph::Node { lines: vec![vec![name]], pad: 0, border: color, dashed: false, target: None, bare: true }
}

/// A part's box, saying as much as `look` lets it: 0 its name; 1 what it is (and where, outside the level); 2 its
/// files and what it has; 3 how many it uses and are using it.
fn node(ui: &Ui, map: &CodeMap, at: usize, marks: Marks, look: Look, layer: Option<&Layer>) -> graph::Node {
    let part = &map.parts[at];
    let name = match look.short {
        Some(most) if !marks.picked => short(&part.name, most),
        _ => part.name.clone(),
    };
    // The build over the projects: each of its projects' state; a project the build hasn't, dim.
    let built = layer.filter(|_| part.inside.is_none()).map(|layer| (layer, layer.run.at_path(&part.path)));
    let marks = match built {
        Some((_, None)) => Marks { dim: true, ..marks },
        _ => marks,
    };
    let state = built.and_then(|(layer, place)| Some((layer, place?))).map(|(layer, place)| {
        let (mark, said, color) = build_state(layer, place);
        (mark, said, color, layer.critical.contains(&place))
    });
    let text = match (&state, look.bare) {
        (Some((mark, ..)), true) => format!("{mark} {} {name}", icon(ui.nerd, part)),
        _ => format!("{} {name}", icon(ui.nerd, part)),
    };
    if look.bare {
        return bare(part, text, marks);
    }
    let quiet = marks.dim || marks.outside;
    let color = match () {
        _ if quiet => C::DarkGray,
        _ if marks.picked => C::White,
        _ => C::Gray,
    };
    let border = match part.kind {
        _ if marks.dim => C::DarkGray,
        _ if marks.picked => C::Cyan,
        _ if marks.cyclic => C::Red,
        _ if marks.outside => C::DarkGray,
        Kind::App => C::DarkGreen,
        Kind::Module => C::DarkMagenta,
        Kind::Library => C::Gray,
        Kind::Tests | Kind::Namespace | Kind::Folder | Kind::Component => C::DarkGray,
    };
    let mut lines = vec![vec![seg(text, color)]];
    if look.detail >= 1 {
        let what = match (&part.framework, part.stack) {
            _ if marks.outside => format!(" · {}", place(map, at)),
            (Some(framework), _) => format!(" · {framework}"),
            _ if part.inside.as_deref() != Some(STACK) => String::new(),
            (None, Stack::Dotnet) => " · .NET".into(),
            (None, Stack::Node) => " · npm".into(),
        };
        lines.push(vec![seg(format!("{}{what}", part.kind.name()), C::DarkGray)]);
    }
    if look.detail >= 2 {
        lines.push(vec![seg(format!("{} files{}", part.files, has(part)), C::DarkGray)]);
    }
    if look.detail >= 3 {
        let (uses, used) = (map.reach(at, true, false).len(), map.reach(at, false, false).len());
        lines.push(vec![seg(format!("uses {uses} · used by {used}"), C::DarkGray)]);
    }
    let mut border = border;
    if let Some((_, said, color, critical)) = state {
        lines.insert(1, vec![seg(said, color)]);
        border = match () {
            _ if marks.picked => border,
            _ if color == C::Red || color == C::Yellow => color,
            _ if critical => C::Magenta,
            _ => border,
        };
    }
    let pad = if look.detail == 0 { 1 } else { 2 };
    let dashed = part.kind == Kind::Tests || marks.outside;
    graph::Node { lines, pad, border, dashed, target: None, bare: false }
}

/// How many steps out from the picked box each part of the map is (along links either way); unreached ones one more
/// than the farthest.
fn steps(shown: &Shown) -> HashMap<usize, usize> {
    let far = shown.picked.map(|picked| distances(&shown.links, picked)).unwrap_or_default();
    let most = far.values().copied().max().map_or(0, |most| most + 1);
    shown.parts.iter().map(|&at| (at, far.get(&at).copied().unwrap_or(most))).collect()
}

/// An edge's color: a cycle's red, a broken rule's yellow, the picked box's cyan, an Nx-only link's blue.
fn edge_color(map: &CodeMap, shown: &Shown, link: &Link) -> C {
    let looped = shown.cycles.iter().any(|cycle| cycle.contains(&link.from) && cycle.contains(&link.to));
    let broken = map.breaks.iter().any(|broken| broken.from == link.from && broken.to == link.to);
    let touches = Some(link.from) == shown.picked || Some(link.to) == shown.picked;
    match () {
        _ if shown.dim.contains(&link.from) || shown.dim.contains(&link.to) => C::DarkGray,
        _ if looped => C::Red,
        _ if broken => C::Yellow,
        _ if touches => C::Cyan,
        _ if link.implicit => C::Blue,
        _ => C::DarkGray,
    }
}

/// The map's edges and their motion: the links, and on the apps' level the projects two apps share.
fn edges(
    ui: &Ui,
    map: &CodeMap,
    shown: &Shown,
    view: &View,
    look: Look,
    layer: Option<&Layer>,
) -> (Vec<graph::Edge>, Vec<Motion>) {
    // The critical path's links: a project and the reference on it it waited for.
    let critical = |from: usize, to: usize| {
        layer.is_some_and(|layer| {
            let place = |at: usize| layer.run.at_path(&map.parts[at].path);
            let (Some(from), Some(to)) = (place(from), place(to)) else { return false };
            layer.critical.windows(2).any(|pair| (pair[0], pair[1]) == (to, from) || (pair[0], pair[1]) == (from, to))
        })
    };
    let index = |at: usize| shown.parts.iter().position(|&part| part == at);
    let far = steps(shown);
    let elapsed = if moving(ui) { since(ui, view) } else { PULSE };
    let pulse = moving(ui) && elapsed < PULSE && (elapsed.as_millis() / PULSE_STEP).is_multiple_of(2);
    let reveal = |from: usize, to: usize| {
        let step = far[&from].min(far[&to]).min(OUTWARD_MOST) as u32;
        elapsed.saturating_sub(OUTWARD * step).as_secs_f64() / REVEAL.as_secs_f64()
    };
    let touches = |from: usize, to: usize| Some(from) == shown.picked || Some(to) == shown.picked;
    let mut edges = Vec::new();
    let mut motions = Vec::new();
    for link in &shown.links {
        let (Some(from), Some(to)) = (index(link.from), index(link.to)) else { continue };
        let on_path = critical(link.from, link.to);
        let color = if on_path { C::Magenta } else { edge_color(map, shown, link) };
        let broken = map.breaks.iter().any(|broken| broken.from == link.from && broken.to == link.to);
        let label = if broken { "✗".to_string() } else { String::new() };
        let top = touches(link.from, link.to) || on_path;
        let dashed = link.lazy || link.implicit;
        edges.push(graph::Edge { from, to, color, dashed, label, quiet: look.bare && !top, top });
        let flash = (color == C::Red && pulse).then_some(C::White);
        motions.push(Motion { shown: reveal(link.from, link.to), dot: None, color: flash });
    }
    for &(one, other, count) in &shown.shares {
        let (Some(from), Some(to)) = (index(one), index(other)) else { continue };
        let top = touches(one, other);
        let color = if top { C::Cyan } else { C::DarkGray };
        let label = format!("{count} shared");
        edges.push(graph::Edge { from, to, color, dashed: true, label, quiet: look.bare && !top, top });
        motions.push(Motion { shown: reveal(one, other), ..Motion::default() });
    }
    (edges, motions)
}

/// The map's boxes and lines as `look` has them; `zooming` while they grow or shrink as the level changes.
pub(super) fn drawn(
    ui: &Ui,
    map: &CodeMap,
    shown: &Shown,
    view: &View,
    look: Look,
    zooming: bool,
    layer: Option<&Layer>,
) -> Map {
    let cyclic = |at: usize| shown.cycles.iter().any(|cycle| cycle.contains(&at));
    let inner = view.inside.as_deref().filter(|inside| *inside != STACK);
    let outside = |at: usize| inner.is_some_and(|inside| map.parts[at].inside.as_deref() != Some(inside));
    let grown = progress(ui, view, ZOOM);
    let detail = match view.zoom {
        Zoom::In if zooming => look.detail.min((grown * (look.detail + 1) as f64) as usize),
        _ => look.detail,
    };
    let extra = match view.zoom {
        Zoom::Out if zooming => ((1.0 - grown) * ZOOM_PAD).round() as usize,
        _ => 0,
    };
    let nodes: Vec<graph::Node> = shown
        .parts
        .iter()
        .enumerate()
        .map(|(index, &at)| {
            let marks = Marks {
                picked: Some(at) == shown.picked,
                cyclic: cyclic(at),
                dim: shown.dim.contains(&at),
                outside: outside(at),
            };
            let mut node = node(ui, map, at, marks, Look { detail, ..look }, layer);
            if !look.bare {
                node.pad = match view.zoom {
                    Zoom::In if zooming => (node.pad as f64 * grown).round() as usize,
                    _ => node.pad + extra,
                };
            }
            node.target = Some(Target::Item(index));
            node
        })
        .collect();
    let (edges, motions) = edges(ui, map, shown, view, look, layer);
    Map::draw(&nodes, &edges, &motions)
}

/// The map as its scale has it - at the normal one with as much in its boxes as fits the room, names cut when nothing
/// does - and whether it fits.
pub(super) fn fitting(
    ui: &Ui,
    map: &CodeMap,
    shown: &Shown,
    view: &View,
    (room, rows): (usize, usize),
    layer: Option<&Layer>,
) -> (Map, bool) {
    let fits = |drawn: &Map| drawn.layout.width <= room && drawn.layout.height < rows;
    let look = match view.scale {
        Scale::Normal => {
            let full = |detail: usize| Look { detail, short: None, bare: false };
            let found =
                (0..=DETAIL_MOST).rev().map(full).find(|&look| fits(&drawn(ui, map, shown, view, look, false, layer)));
            found.unwrap_or(Look { detail: 0, short: Some(LONGEST), bare: false })
        }
        scale => Look::of(scale),
    };
    let settled = drawn(ui, map, shown, view, look, false, layer);
    let fit = fits(&settled);
    let zooming = moving(ui) && view.zoom != Zoom::Still && since(ui, view) < ZOOM;
    (if zooming { drawn(ui, map, shown, view, look, true, layer) } else { settled }, fit)
}

/// The minimap beside the map's first rows, the map's rows `room` wide beside it and the full width under it.
pub(super) fn with_minimap(
    drawn: &Map,
    shown: &Shown,
    (from, top): (usize, usize),
    (room, rows): (usize, usize),
) -> Vec<Vec<Seg>> {
    let narrow = room.saturating_sub(MINI.0 + 1);
    let colors = shown.parts.iter().map(|&at| match () {
        _ if Some(at) == shown.picked => C::Cyan,
        _ if shown.cycles.iter().any(|cycle| cycle.contains(&at)) => C::Red,
        _ => C::Gray,
    });
    let boxes: Vec<(Rect, C)> = drawn.layout.boxes.iter().copied().zip(colors).collect();
    let view = Rect { x: from, y: top, w: narrow, h: rows };
    let mini = super::minimap::rows(&boxes, (drawn.layout.width, drawn.layout.height), view, MINI);
    let (cut, full) = (drawn.rows(from, narrow), drawn.rows(from, room));
    (0..rows)
        .map(|row| match mini.get(row) {
            Some(mini) => {
                let mut line = super::columns::fit(cut.get(top + row).cloned().unwrap_or_default(), narrow);
                line.push(seg(" ", C::Gray));
                line.extend(mini.iter().cloned());
                line
            }
            None => full.get(top + row).cloned().unwrap_or_default(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_name_keeps_its_start_and_its_last_pieces() {
        assert_eq!(short("Orders.Api", 20), "Orders.Api");
        let long = "BMS.Services.ProcurementIntegrationService.Tests";
        assert_eq!(short(long, 40), "BMS.…ProcurementIntegrationService.Tests");
        assert_eq!(short("SettingsModule.Controllers", 24), "SettingsModu…Controllers", "apart from FocusModule's");
        assert_eq!(short("@focus/form-generator", 16), "…form-generator");
        assert_eq!(short("AVeryLongNameWithoutDots", 10), "AVeryLong…");
    }
}
