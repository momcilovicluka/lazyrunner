//! The code map (#89): the stack view's Code tab. It opens on the apps, linked when one's code uses another's and
//! dashed with how many projects two share; `Enter` goes into one - its projects, .NET and TypeScript, in columns by
//! what depends on what - then into a project (a .NET project's namespaces, a TypeScript project's folders, with what
//! they use outside it, dashed) and into a folder (its NgModules and components); `Backspace` back out, `a` all the
//! projects. A cycle is red, a link a rule doesn't allow yellow (the profile's `layers`, Nx's module boundaries), one
//! only Nx knows of blue; a link another way already implies is hidden (`r` shows them).
//!
//! `/` searches every level, and files, by name; `Enter` on one shows its graph - all it depends on and all that
//! depends on it - as `g` does for the picked box. `f` keeps only the picked box and its neighbours, with its own lines
//! (`+` `-` how far), on by itself for a big level; `z` zooms out (compact boxes, then just names with the picked
//! box's lines), `Z` back in; `t` hides the test projects; `c` steps through the cycles, `o` opens a box's file, `y`
//! copies its path. Under the map, the picked box: what it depends on and what uses it, by name, what a change to it
//! affects, the rules it breaks. A map bigger than the room has a minimap.
//!
//! It moves: a level zooms in (its boxes growing) or out (shrinking), its edges drawing in outwards from the picked
//! box a step at a time; focus dims what it's about to hide first.

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::time::{Duration, Instant};

use crossterm::event::KeyEvent;

use lazyrunner_core::output::Hue;
use lazyrunner_core::run::Controller;
use lazyrunner_core::run::build_map::{BuildRun, ProjectState};
use lazyrunner_core::run::code::{CodeMap, Kind, Link, STACK, Stack};

use super::code_draw::{self, Look, MINI};
use super::graph;
use super::mouse::Target;
use super::seg::{Seg, aim, seg};
use super::theme::C;
use super::{Overlay, Ui};

/// What `b` and `Alt+b` say when no .NET app has built yet.
pub(super) const NO_BUILD: &str =
    "No build to show yet: the build map shows a .NET app's build once it has run one (b builds the focused app).";
/// What `w` says when the build is over.
const NOTHING_BUILDING: &str = "Nothing is building now: w follows a build while it runs.";

/// A level with more boxes than this opens focused on one.
const FOCUS_ABOVE: usize = 24;
/// What focus hides shows dimmed this long first.
const FADE: Duration = Duration::from_millis(320);
/// A cycle's edges pulse after a change; everything has settled by its end.
pub(super) const PULSE: Duration = Duration::from_millis(800);
/// A box's edge kept this far from the view's side when it scrolls to the picked one.
const MARGIN: usize = 2;

/// Which way the last change of level went.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Zoom {
    Still,
    In,
    Out,
}

/// How far out the map is: boxes saying as much as fits, boxes with their names alone, or just the names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scale {
    Normal,
    Compact,
    Overview,
}

impl Scale {
    fn out(self) -> Self {
        match self {
            Self::Normal => Self::Compact,
            _ => Self::Overview,
        }
    }

    fn closer(self) -> Self {
        match self {
            Self::Overview => Self::Compact,
            _ => Self::Normal,
        }
    }
}

/// The Code tab: the level it's on (the apps' [`STACK`], none for the projects, else the part it's inside of), the
/// app whose projects alone show, the picked box, the box whose graph shows, how far focus reaches (none: all of the
/// level) and how far it did before, the scale, whether implied edges and test projects show, which cycle `c` is on,
/// the `/` search and its chosen match, and when and which way the view last changed.
#[derive(Clone, Debug)]
pub struct View {
    pub inside: Option<String>,
    pub app: Option<String>,
    pub picked: Option<String>,
    pub trace: Option<String>,
    pub focus: Option<usize>,
    pub before: Option<Option<usize>>,
    pub scale: Scale,
    pub all_edges: bool,
    pub hide_tests: bool,
    pub cycle: usize,
    pub typing: bool,
    pub query: String,
    pub choice: usize,
    pub changed: Instant,
    pub zoom: Zoom,
    /// Whether this level has been looked at: a map without apps opens on its projects, a big level focused.
    pub sized: bool,
    /// The app whose build shows over the projects (#82), by its key: `b`, `Alt+b`.
    pub build: Option<String>,
    /// `w`: the picked box follows what builds, and what was picked before, to go back to once the build is over.
    pub following: bool,
    pub before_follow: Option<Option<String>>,
    /// `B`: the build's projects alone, by their ids - kept up with the build.
    pub only: Option<BTreeSet<String>>,
}

/// The Code tab: on the apps, or on `app`'s projects when one is given.
pub fn open(ui: &mut Ui, controller: &mut Controller, app: Option<String>) {
    controller.want_code();
    let view = View {
        inside: if app.is_some() { None } else { Some(STACK.into()) },
        app,
        picked: None,
        trace: None,
        focus: None,
        before: None,
        scale: Scale::Normal,
        all_edges: false,
        hide_tests: false,
        cycle: 0,
        typing: false,
        query: String::new(),
        choice: 0,
        changed: ui.now,
        zoom: Zoom::In,
        sized: false,
        build: None,
        following: false,
        before_follow: None,
        only: None,
    };
    ui.stack_tab = super::stack::Tab::Code;
    ui.overlay = Some(Overlay::Code(Box::new(view)));
}

/// While the Code tab is open, the code map is kept fresh.
pub fn keep_fresh(ui: &Ui, controller: &mut Controller) {
    if matches!(ui.overlay, Some(Overlay::Code(_))) {
        controller.want_code();
    }
}

pub(super) fn view(ui: &Ui) -> Option<&View> {
    match &ui.overlay {
        Some(Overlay::Code(view)) => Some(view),
        _ => None,
    }
}

pub(super) fn view_mut(ui: &mut Ui) -> Option<&mut View> {
    match &mut ui.overlay {
        Some(Overlay::Code(view)) => Some(view),
        _ => None,
    }
}

pub(super) fn moving(ui: &Ui) -> bool {
    ui.motion.enabled && !ui.reduced_motion
}

pub(super) fn since(ui: &Ui, view: &View) -> Duration {
    ui.now.saturating_duration_since(view.changed)
}

/// How far along a change is that takes `long`: 1 when it's done or nothing moves.
pub(super) fn progress(ui: &Ui, view: &View, long: Duration) -> f64 {
    if moving(ui) { (since(ui, view).as_secs_f64() / long.as_secs_f64()).min(1.0) } else { 1.0 }
}

/// What the tab shows: the level's parts (those focus or the graph keeps, and while focus fades in, those it's about
/// to hide - `dim`), their links (and how many implied ones are hidden), the projects apps share, the picked one, the
/// one whose graph it is, the level's cycles.
pub(super) struct Shown {
    pub parts: Vec<usize>,
    pub links: Vec<Link>,
    pub picked: Option<usize>,
    pub trace: Option<usize>,
    pub cycles: Vec<Vec<usize>>,
    pub dim: BTreeSet<usize>,
    pub hidden: usize,
    pub shares: Vec<(usize, usize, usize)>,
}

/// How many links (either way) each part is from `from`.
pub(super) fn distances(links: &[Link], from: usize) -> HashMap<usize, usize> {
    let mut far = HashMap::from([(from, 0)]);
    let mut queue = VecDeque::from([from]);
    while let Some(at) = queue.pop_front() {
        for link in links {
            let next = match at {
                _ if link.from == at => link.to,
                _ if link.to == at => link.from,
                _ => continue,
            };
            if !far.contains_key(&next) {
                far.insert(next, far[&at] + 1);
                queue.push_back(next);
            }
        }
    }
    far
}

/// A part's graph: it, all it depends on, and all that depends on it.
fn traced(links: &[Link], center: usize) -> BTreeSet<usize> {
    let mut kept = BTreeSet::from([center]);
    for forward in [true, false] {
        let mut queue = VecDeque::from([center]);
        let mut seen = BTreeSet::from([center]);
        while let Some(at) = queue.pop_front() {
            for link in links {
                let (here, there) = if forward { (link.from, link.to) } else { (link.to, link.from) };
                if here == at && seen.insert(there) {
                    kept.insert(there);
                    queue.push_back(there);
                }
            }
        }
    }
    kept
}

/// Whether `links` lead from `from` to `to` another way than straight.
fn around(links: &[Link], from: usize, to: usize) -> bool {
    let next = |at: usize| links.iter().filter(move |link| link.from == at).map(|link| link.to);
    let mut queue: VecDeque<usize> = next(from).filter(|&first| first != to).collect();
    let mut seen = BTreeSet::new();
    while let Some(at) = queue.pop_front() {
        if at == to {
            return true;
        }
        if at != from && seen.insert(at) {
            queue.extend(next(at));
        }
    }
    false
}

/// The links no longer way implies (a cycle's kept) - taken out one at a time, so what's left still reaches all it
/// did - and how many are hidden.
fn reduced(links: Vec<Link>, cycles: &[Vec<usize>]) -> (Vec<Link>, usize) {
    let looped = |link: &Link| cycles.iter().any(|cycle| cycle.contains(&link.from) && cycle.contains(&link.to));
    let mut kept = links;
    let mut at = 0;
    let mut hidden = 0;
    while at < kept.len() {
        let link = kept[at];
        if !looped(&link) && around(&kept, link.from, link.to) {
            kept.remove(at);
            hidden += 1;
        } else {
            at += 1;
        }
    }
    (kept, hidden)
}

/// On the apps' level, the projects each two apps share - when neither's code uses the other's.
fn shares(map: &CodeMap, parts: &[usize], links: &[Link]) -> Vec<(usize, usize, usize)> {
    let mut shared = Vec::new();
    for (index, &one) in parts.iter().enumerate() {
        for &other in &parts[index + 1..] {
            let linked =
                links.iter().any(|link| (link.from, link.to) == (one, other) || (link.from, link.to) == (other, one));
            let (Some(a), Some(b)) = (map.parts[one].app.as_deref(), map.parts[other].app.as_deref()) else { continue };
            let count = map.shared_projects(a, b);
            if !linked && count > 0 {
                shared.push((one, other, count));
            }
        }
    }
    shared
}

/// The level's parts and links: an app's projects alone when it's on one - the build's alone for `B` - the test
/// projects hidden when they are.
fn level(map: &CodeMap, view: &View) -> (Vec<usize>, Vec<Link>) {
    let (mut parts, links) = map.level(view.inside.as_deref());
    if let Some(app) = view.app.as_deref().filter(|_| view.inside.is_none() && view.only.is_none()) {
        let mine = map.app_projects(app);
        parts.retain(|at| mine.contains(at));
    }
    if view.hide_tests {
        parts.retain(|&at| map.parts[at].kind != Kind::Tests);
    }
    if let Some(only) = view.only.as_ref().filter(|_| view.inside.is_none()) {
        parts.retain(|&at| only.contains(&map.parts[at].id));
    }
    let links = links.into_iter().filter(|link| parts.contains(&link.from) && parts.contains(&link.to)).collect();
    (parts, links)
}

/// What focus keeps, and what it's about to hide.
type Focused = (Option<BTreeSet<usize>>, BTreeSet<usize>);

/// What focus keeps (none: everything), and while it fades in, what it's about to hide.
fn focused(ui: &Ui, view: &View, picked: Option<usize>, (parts, links): (&[usize], &[Link])) -> Focused {
    let within = |depth: usize, from: usize| {
        distances(links, from).into_iter().filter(|&(_, far)| far <= depth).map(|(at, _)| at).collect::<BTreeSet<_>>()
    };
    let kept = |focus: Option<usize>| focus.zip(picked).map(|(depth, from)| within(depth, from));
    let now = kept(view.focus);
    let fading = moving(ui) && since(ui, view) < FADE;
    let then = view.before.filter(|_| fading).map(kept);
    let wider = match (&then, &now) {
        (Some(None), Some(_)) => Some(None),
        (Some(Some(then)), Some(now)) if then.is_superset(now) && then != now => Some(Some(then.clone())),
        _ => None,
    };
    match (wider, now) {
        (Some(then), Some(now)) => {
            let all = then.unwrap_or_else(|| parts.iter().copied().collect());
            let dim = all.difference(&now).copied().collect();
            (Some(all), dim)
        }
        (_, now) => (now, BTreeSet::new()),
    }
}

/// The parts of the level - all, the picked one's neighbourhood (focus) or one part's graph - and their links.
pub(super) fn shown(ui: &Ui, map: &CodeMap, view: &View) -> Shown {
    let (mut parts, mut links) = level(map, view);
    let trace = view.trace.as_deref().and_then(|id| map.find(id)).filter(|at| parts.contains(at));
    let class = |at: usize| matches!(map.parts[at].kind, Kind::Module | Kind::Component);
    if let Some(center) = trace.filter(|&center| class(center)) {
        // A module's graph goes through the modules it imports and that import it, in any folder or project.
        links = map.links.iter().filter(|link| class(link.from) && class(link.to)).copied().collect();
        parts = traced(&links, center).into_iter().collect();
    }
    if let Some(center) = trace {
        let keep = traced(&links, center);
        parts.retain(|at| keep.contains(at));
        links.retain(|link| keep.contains(&link.from) && keep.contains(&link.to));
    }
    let cycles = map.cycles(&parts, &links);
    let picked = view.picked.as_deref().and_then(|id| map.find(id)).filter(|at| parts.contains(at));
    let picked = picked.or(trace).or_else(|| first(map, &parts));
    let focusing = view.focus.is_some() && trace.is_none();
    let (keep, dim) =
        if trace.is_none() { focused(ui, view, picked, (&parts, &links)) } else { (None, BTreeSet::new()) };
    if let Some(keep) = keep {
        parts.retain(|at| keep.contains(at));
        links.retain(|link| keep.contains(&link.from) && keep.contains(&link.to));
    }
    let mut hidden = 0;
    if let (true, Some(center)) = (focusing, picked) {
        let far = distances(&links, center);
        let step = |link: &Link| far.get(&link.from).zip(far.get(&link.to)).is_some_and(|(a, b)| a.abs_diff(*b) == 1);
        links.retain(|link| dim.contains(&link.from) || dim.contains(&link.to) || step(link));
    } else if !view.all_edges {
        (links, hidden) = reduced(links, &cycles);
    }
    let shares = if view.inside.as_deref() == Some(STACK) { shares(map, &parts, &links) } else { Vec::new() };
    Shown { parts, links, picked, trace, cycles, dim, hidden, shares }
}

/// The box to start on: an app's, else the first.
fn first(map: &CodeMap, parts: &[usize]) -> Option<usize> {
    parts.iter().copied().find(|&at| map.parts[at].kind == Kind::App).or_else(|| parts.first().copied())
}

/// Takes a level's first look: a map without apps opens on its projects, a big level focused. Keeps up with the build.
fn settle(ui: &mut Ui, controller: &Controller) {
    let Some(map) = controller.code_map() else { return };
    let run = view(ui).and_then(|view| view.build.as_deref()).and_then(|key| controller.build_run(key));
    let Some(view) = view_mut(ui) else { return };
    if let Some(run) = run {
        keep_up(map, run, view);
    }
    if view.sized {
        return;
    }
    view.sized = true;
    if view.inside.as_deref() == Some(STACK) && map.level(Some(STACK)).0.is_empty() {
        view.inside = None;
    }
    if view.focus.is_none() && view.trace.is_none() && level(map, view).0.len() > FOCUS_ABOVE {
        view.focus = Some(1);
    }
}

/// The build's projects for `B`, and while following, the picked box on a project compiling now - kept while it
/// compiles, then the one that could start first - and once the build is over, back on the box picked before.
fn keep_up(map: &CodeMap, run: &BuildRun, view: &mut View) {
    let in_build = |at: usize| run.at_path(&map.parts[at].path);
    if view.only.is_some() {
        let projects = (0..map.parts.len()).filter(|&at| map.parts[at].inside.is_none() && in_build(at).is_some());
        view.only = Some(projects.map(|at| map.parts[at].id.clone()).collect());
    }
    if !view.following {
        return;
    }
    if !run.running() {
        view.following = false;
        if let Some(before) = view.before_follow.take() {
            view.picked = before;
        }
        return;
    }
    let picked = view.picked.as_deref().and_then(|id| map.find(id));
    if picked.and_then(in_build).is_some_and(|project| run.state(project) == ProjectState::Building) {
        return;
    }
    let parts = level(map, view).0;
    let next =
        run.building().into_iter().find_map(|project| parts.iter().copied().find(|&at| in_build(at) == Some(project)));
    if let Some(at) = next {
        view.picked = Some(map.parts[at].id.clone());
    }
}

/// Starts following the build: onto the projects, remembering the picked box.
fn start_following(ui: &mut Ui) {
    let now = ui.now;
    let Some(view) = view_mut(ui) else { return };
    if view.inside.is_some() || view.trace.is_some() {
        (view.inside, view.trace, view.sized) = (None, None, false);
        moved(view, now, Zoom::Out);
    }
    (view.following, view.before_follow) = (true, Some(view.picked.clone()));
}

/// A change the map moves for: `zoom` its way, edges drawing in again.
fn moved(view: &mut View, now: Instant, zoom: Zoom) {
    (view.changed, view.zoom, view.before) = (now, zoom, None);
}

/// Goes to another level: `inside` it, `app`'s projects alone, `picked` picked; zooming in or out.
fn go(ui: &mut Ui, (inside, app): (Option<String>, Option<String>), picked: Option<String>, zoom: Zoom) {
    let now = ui.now;
    if let Some(view) = view_mut(ui) {
        (view.inside, view.app, view.picked, view.focus, view.trace) = (inside, app, picked, None, None);
        (view.sized, view.query) = (false, String::new());
        moved(view, now, zoom);
    }
}

/// The Code tab on app `key`'s projects with its build over them (#82): the running build, followed, else its last.
pub fn open_build(ui: &mut Ui, controller: &mut Controller, key: String) {
    let showing = view(ui).is_some_and(|view| view.build.as_deref() == Some(key.as_str()));
    if showing {
        return;
    }
    if view(ui).is_none() {
        open(ui, controller, Some(key.clone()));
    } else {
        go(ui, (None, Some(key.clone())), None, Zoom::In);
    }
    let running = controller.build_run(&key).is_some_and(BuildRun::running);
    if let Some(view) = view_mut(ui) {
        view.build = Some(key);
    }
    if running {
        start_following(ui);
    }
}

/// Shows part `at`'s graph on its level: all it depends on and all that depends on it.
pub(super) fn show_graph(ui: &mut Ui, map: &CodeMap, at: usize) {
    let part = &map.parts[at];
    go(ui, (part.inside.clone(), None), Some(part.id.clone()), Zoom::In);
    if let Some(view) = view_mut(ui) {
        view.trace = Some(part.id.clone());
        view.hide_tests &= part.kind != Kind::Tests;
    }
}

/// The graph of `trace` (none: the whole level again).
fn retrace(ui: &mut Ui, trace: Option<String>) {
    let now = ui.now;
    if let Some(view) = view_mut(ui) {
        let zoom = if trace.is_some() { Zoom::In } else { Zoom::Out };
        (view.trace, view.focus) = (trace, None);
        moved(view, now, zoom);
    }
}

/// Focus to `focus`: what it hides dims first.
fn refocus(ui: &mut Ui, focus: Option<usize>) {
    let now = ui.now;
    if let Some(view) = view_mut(ui) {
        let before = view.focus;
        (view.focus, view.trace) = (focus, None);
        moved(view, now, Zoom::Still);
        view.before = Some(before);
    }
}

/// Changes the view with `change`, the map moving `zoom`'s way.
fn change(ui: &mut Ui, zoom: Zoom, change: impl FnOnce(&mut View)) {
    let now = ui.now;
    if let Some(view) = view_mut(ui) {
        change(view);
        moved(view, now, zoom);
    }
}

/// Whether the Code tab is taking typed text (`/`).
pub fn typing(ui: &Ui) -> bool {
    view(ui).is_some_and(|view| view.typing)
}

/// The keys of the Code tab; false for one it doesn't use.
pub fn key(ui: &mut Ui, controller: &mut Controller, name: &str, key: &KeyEvent) -> bool {
    let Some(current) = view(ui).cloned() else { return false };
    if current.typing {
        super::code_search::key(ui, controller, name, key);
        return true;
    }
    match name {
        "Escape" if current.trace.is_some() => retrace(ui, None),
        "Escape" | "q" | "K" => ui.overlay = None,
        "Tab" => super::health::open(ui, controller),
        "/" => change(ui, Zoom::Still, |view| (view.typing, view.query, view.choice) = (true, String::new(), 0)),
        _ => return map_key(ui, controller, name, &current),
    }
    true
}

/// `Enter`: into the picked box - an app's projects, a project's namespaces or folders, a folder's modules; what to
/// log when it has none.
fn enter(ui: &mut Ui, map: &CodeMap, at: usize) -> Option<String> {
    let part = &map.parts[at];
    if part.inside.as_deref() == Some(STACK) {
        let mine = part.app.as_ref().map(|app| map.app_projects(app)).unwrap_or_default();
        let picked = first(map, &mine).map(|at| map.parts[at].id.clone());
        go(ui, (None, part.app.clone()), picked, Zoom::In);
        return None;
    }
    let inner = map.level(Some(&part.id)).0;
    let own: Vec<usize> =
        inner.into_iter().filter(|&inner| map.parts[inner].inside.as_ref() == Some(&part.id)).collect();
    if own.is_empty() {
        return Some(format!("{} has no parts of its own to show", part.name));
    }
    let app = view(ui).and_then(|view| view.app.clone());
    go(ui, (Some(part.id.clone()), app), first(map, &own).map(|at| map.parts[at].id.clone()), Zoom::In);
    None
}

/// `Backspace`: out of a part's graph, else to the level the current one is in - a part's parent, the projects, the
/// apps.
fn back(ui: &mut Ui, map: &CodeMap, current: &View) {
    if current.trace.is_some() {
        retrace(ui, None);
        return;
    }
    match current.inside.as_deref() {
        Some(STACK) => {}
        Some(id) => {
            let parent = map.find(id).and_then(|at| map.parent(at)).map(str::to_string);
            go(ui, (parent, current.app.clone()), Some(id.to_string()), Zoom::Out);
        }
        None if !map.level(Some(STACK)).0.is_empty() => {
            let app = current.app.as_ref().map(|app| format!("app:{app}"));
            go(ui, (Some(STACK.into()), None), app.or(current.picked.clone()), Zoom::Out);
        }
        None if current.app.is_some() => go(ui, (None, None), current.picked.clone(), Zoom::Out),
        None => {}
    }
}

/// The keys that move about the map.
fn map_key(ui: &mut Ui, controller: &mut Controller, name: &str, current: &View) -> bool {
    match name {
        "b" => build_key(ui, controller, current),
        "w" => follow_key(ui, controller, current),
        "B" => only_key(ui, controller, current),
        _ => return moving_key(ui, controller, name, current),
    }
    true
}

/// The keys moving about the map by hand stop following the build.
const BY_HAND: [&str; 13] =
    ["h", "j", "k", "l", "LeftArrow", "DownArrow", "UpArrow", "RightArrow", "Enter", "Backspace", "a", "g", "c"];

/// The keys that move about the map.
fn moving_key(ui: &mut Ui, controller: &mut Controller, name: &str, current: &View) -> bool {
    if current.following && BY_HAND.contains(&name) {
        stop_following(ui);
    }
    let Some(map) = controller.code_map() else {
        return matches!(name, "Enter" | "Backspace" | "f" | "c" | "a" | "g" | "r" | "t" | "z" | "Z");
    };
    let shown = shown(ui, map, current);
    let picked = shown.picked;
    let id = |at: usize| map.parts[at].id.clone();
    match name {
        "h" | "j" | "k" | "l" | "LeftArrow" | "DownArrow" | "UpArrow" | "RightArrow" => {
            let drawn = code_draw::drawn(ui, map, &shown, current, Look::of(current.scale), false, None);
            let at = picked.and_then(|at| shown.parts.iter().position(|&part| part == at)).unwrap_or(0);
            let next = graph::step(&drawn.layout, at, name);
            let next = shown.parts.get(next).map(|&at| id(at));
            if let Some(view) = view_mut(ui) {
                view.picked = next;
            }
        }
        "Enter" => {
            if let Some(text) = picked.and_then(|at| enter(ui, map, at)) {
                controller.add_log(text, Hue::DarkGray);
            }
        }
        "Backspace" => back(ui, map, current),
        "a" if current.inside.is_some() || current.app.is_some() => {
            go(ui, (None, None), current.picked.clone(), Zoom::Out);
        }
        "g" => retrace(ui, if current.trace.is_some() { None } else { picked.map(id) }),
        "r" => change(ui, Zoom::Still, |view| view.all_edges = !view.all_edges),
        "t" => change(ui, Zoom::Still, |view| view.hide_tests = !view.hide_tests),
        "z" => change(ui, Zoom::Out, |view| view.scale = view.scale.out()),
        "Z" => change(ui, Zoom::In, |view| view.scale = view.scale.closer()),
        "f" => refocus(ui, if current.focus.is_some() { None } else { Some(1) }),
        "+" | "=" | "-" => {
            if let Some(depth) = current.focus {
                refocus(ui, Some(if name == "-" { depth.saturating_sub(1).max(1) } else { depth + 1 }));
            }
        }
        "c" if shown.cycles.is_empty() => controller.add_log("No cycles here.", Hue::DarkGray),
        "c" => {
            let at = current.cycle % shown.cycles.len();
            let target = shown.cycles[at].first().map(|&part| id(part));
            change(ui, Zoom::Still, |view| {
                (view.cycle, view.focus, view.trace, view.picked) = (at + 1, None, None, target)
            });
        }
        "o" => {
            let Some(path) = picked.map(|at| map.parts[at].path.display().to_string()) else { return true };
            if !(ui.opener)(&path) {
                controller.add_log(format!("✗ Couldn't open {path}"), Hue::Red);
            }
        }
        "y" => {
            let Some(path) = picked.map(|at| map.parts[at].path.display().to_string()) else { return true };
            super::copy::put(ui.clipboard, controller, &path, "its path");
        }
        _ => return matches!(name, "a"),
    }
    true
}

fn stop_following(ui: &mut Ui) {
    if let Some(view) = view_mut(ui) {
        (view.following, view.before_follow) = (false, None);
    }
}

/// The build `w` and `B` are about: the one showing, else the app's on, the running build's or the last one's.
fn build_for(controller: &mut Controller, current: &View) -> Option<String> {
    let key = current.build.clone().or_else(|| controller.build_run_key(current.app.as_deref()));
    if key.is_none() {
        controller.add_log(NO_BUILD, Hue::DarkGray);
    }
    key
}

/// `w`: the picked box follows what builds - the project compiling now, the next once it's done - and goes back to
/// where it was once the build is over; or stops following.
fn follow_key(ui: &mut Ui, controller: &mut Controller, current: &View) {
    if current.following {
        stop_following(ui);
        return;
    }
    let Some(key) = build_for(controller, current) else { return };
    if !controller.build_run(&key).is_some_and(BuildRun::running) {
        controller.add_log(NOTHING_BUILDING, Hue::DarkGray);
        return;
    }
    open_build(ui, controller, key);
    start_following(ui);
}

/// `B`: the build's projects alone - the ones built, building and still to build, with their links - or everything
/// again.
fn only_key(ui: &mut Ui, controller: &mut Controller, current: &View) {
    if current.only.is_some() {
        change(ui, Zoom::In, |view| view.only = None);
        return;
    }
    let Some(key) = build_for(controller, current) else { return };
    open_build(ui, controller, key);
    change(ui, Zoom::Out, |view| {
        (view.inside, view.trace, view.focus) = (None, None, None);
        view.only = Some(BTreeSet::new());
    });
}

/// `b`: the build over the projects - the app's on, the running build's or the last one's - or off again.
fn build_key(ui: &mut Ui, controller: &mut Controller, current: &View) {
    if current.build.is_some() {
        change(ui, Zoom::Still, |view| (view.build, view.only, view.following) = (None, None, false));
        return;
    }
    match controller.build_run_key(current.app.as_deref()) {
        Some(key) => open_build(ui, controller, key),
        None => controller.add_log(NO_BUILD, Hue::DarkGray),
    }
}

/// A click picks a box; a double click goes inside it.
pub fn click(ui: &mut Ui, controller: &mut Controller, target: Option<Target>, double: bool) -> bool {
    let Some(current) = view(ui).cloned() else { return false };
    let Some(Target::Item(index)) = target else { return false };
    let Some(map) = controller.code_map() else { return true };
    let shown = shown(ui, map, &current);
    let id = shown.parts.get(index).map(|&at| map.parts[at].id.clone());
    if let Some(view) = view_mut(ui) {
        (view.picked, view.following) = (id.clone(), false);
    }
    if double {
        let picked = View { picked: id, ..current };
        map_key(ui, controller, "Enter", &picked);
    }
    true
}

/// Whether the tab moves: a level zooming, focus fading, its edges drawing in, a cycle's pulse.
pub fn animating(ui: &Ui, controller: &Controller) -> bool {
    let Some(view) = view(ui) else { return false };
    let building = view.build.as_deref().and_then(|key| controller.build_run(key)).is_some_and(|run| run.running());
    moving(ui) && (controller.reading_code() || since(ui, view) < PULSE || building)
}

/// What the level is, for the header: the apps, an app's projects, all of them, or the parts of one.
fn summary(map: &CodeMap, view: &View, parts: &[usize]) -> String {
    let plural = |count: usize, what: &str| format!("{count} {what}{}", if count == 1 { "" } else { "s" });
    match view.inside.as_deref() {
        Some(STACK) => plural(parts.len(), "app"),
        Some(id) => {
            let Some(at) = map.find(id) else { return String::new() };
            let inner = parts.iter().filter(|&&part| map.parts[part].inside.as_deref() == Some(id)).count();
            let what = match (map.parts[at].kind, map.parts[at].stack) {
                (Kind::Folder, _) => "modules and components",
                (_, Stack::Dotnet) => "namespaces",
                (_, Stack::Node) => "folders",
            };
            format!("{} · {inner} {what}", map.parts[at].name)
        }
        None => {
            let count = |stack| parts.iter().filter(|&&part| map.parts[part].stack == stack).count();
            let projects = format!("{} .NET · {} TypeScript projects", count(Stack::Dotnet), count(Stack::Node));
            let Some(app) = view.app.as_deref() else { return projects };
            let named = map.find(&format!("app:{app}")).map_or(app, |at| map.parts[at].name.as_str());
            format!("{named}: {projects}")
        }
    }
}

/// The header: the tabs, the level, then what the map has and how it's shown.
fn header(ui: &Ui, controller: &Controller, view: &View, shown: Option<&Shown>, width: usize) -> Vec<Seg> {
    let mut segs = super::stack::tabs(super::stack::Tab::Code);
    segs.push(seg("   ", C::Gray));
    let (Some(map), Some(shown)) = (controller.code_map(), shown) else {
        segs.push(seg("reading the code…", C::Yellow));
        return super::columns::fit(segs, width);
    };
    let (parts, links) = level(map, view);
    let cycles = map.cycles(&parts, &links).len();
    let broken = map.breaks.iter().filter(|broken| parts.contains(&broken.from) && parts.contains(&broken.to)).count();
    segs.push(seg(summary(map, view, &parts), C::DarkGray));
    let plural = |count: usize| if count == 1 { "" } else { "s" };
    if let Some(at) = shown.trace {
        let (uses, used) = (shown.parts.len().saturating_sub(1), map.parts[at].name.as_str());
        segs.push(seg(format!(" · the graph of {used} ({uses} linked)"), C::Cyan));
    }
    if cycles > 0 {
        segs.push(seg(format!(" · {cycles} cycle{}", plural(cycles)), C::Red));
    }
    if broken > 0 {
        segs.push(seg(format!(" · {broken} rule break{}", plural(broken)), C::Yellow));
    }
    if let Some(depth) = view.focus.filter(|_| shown.trace.is_none()) {
        segs.push(seg(format!(" · focus {depth}"), C::Cyan));
    }
    if view.typing {
        segs.push(seg(format!("   /{}{}", view.query, ui.cursor()), C::Cyan));
    }
    super::columns::fit(segs, width)
}

/// Under the keys, how the map is shown: the edges hidden as implied, the test projects hidden, its scale, Nx asked.
fn notes(controller: &Controller, view: &View, shown: Option<&Shown>, width: usize) -> Vec<Vec<Seg>> {
    let mut notes = Vec::new();
    let hidden = shown.map_or(0, |shown| shown.hidden);
    if hidden > 0 {
        notes.push(format!("{hidden} implied edge{} hidden (r)", if hidden == 1 { "" } else { "s" }));
    }
    if view.hide_tests {
        notes.push("tests hidden (t)".into());
    }
    if view.only.is_some() {
        notes.push("only the build's projects (B)".into());
    }
    match view.scale {
        Scale::Normal => {}
        Scale::Compact => notes.push("compact (Z)".into()),
        Scale::Overview => notes.push("overview (Z)".into()),
    }
    if controller.asking_nx() {
        notes.push("asking Nx…".into());
    }
    if notes.is_empty() {
        return vec![Vec::new()];
    }
    super::seg::wrap(&[seg(format!(" {}", notes.join(" · ")), C::DarkGray)], width, 1, NOTE_ROWS)
}

/// The rows the notes under the keys take at most, and each of the picked box's lines under the map (#90: they wrap
/// instead of scrolling), the rows after a line's first indented.
const NOTE_ROWS: usize = 2;
const DETAIL_ROWS: usize = 3;
const DETAIL_INDENT: usize = 3;

/// The rules the picked part breaks or is broken by, and what only Nx knows it's linked to.
fn rules(map: &CodeMap, at: usize) -> Vec<Seg> {
    let mut segs = Vec::new();
    for broken in map.breaks.iter().filter(|broken| broken.from == at || broken.to == at) {
        let (from, to) = (&map.parts[broken.from].name, &map.parts[broken.to].name);
        segs.push(seg(format!(" ✗ {from} → {to}: {}", broken.why), C::Yellow));
    }
    let implicit: Vec<&str> = map
        .links
        .iter()
        .filter(|link| link.implicit && (link.from == at || link.to == at))
        .map(|link| map.parts[if link.from == at { link.to } else { link.from }].name.as_str())
        .collect();
    if !implicit.is_empty() {
        segs.push(seg(format!(" · linked by Nx alone to {}", implicit.join(", ")), C::Blue));
    }
    segs
}

/// What the picked part uses and what uses it, by name - and for an app, the apps it shares projects with.
fn names(map: &CodeMap, shown: &Shown, at: usize) -> Vec<Seg> {
    let named =
        |parts: Vec<usize>| parts.iter().map(|&part| map.parts[part].name.as_str()).collect::<Vec<_>>().join(", ");
    let (uses, users) = (map.reach(at, true, false), map.reach(at, false, false));
    let mut segs = Vec::new();
    if !uses.is_empty() {
        segs.push(seg(format!(" → uses {}", named(uses)), C::Gray));
    }
    if !users.is_empty() {
        segs.push(seg(format!("   ← used by {}", named(users)), C::Gray));
    }
    for &(one, other, count) in shown.shares.iter().filter(|share| share.0 == at || share.1 == at) {
        let with = &map.parts[if one == at { other } else { one }].name;
        segs.push(seg(format!("   shares {count} projects with {with}"), C::Gray));
    }
    segs
}

/// Under the map, the picked box: what it depends on and what uses it (how many, and by name), what a change to it
/// affects, its path and tags, the rules it breaks.
fn details(controller: &Controller, map: &CodeMap, shown: &Shown, width: usize) -> Vec<Vec<Seg>> {
    let Some(at) = shown.picked else { return Vec::new() };
    let part = &map.parts[at];
    let (uses, uses_all) = (map.reach(at, true, false).len(), map.reach(at, true, true).len());
    let users_all = map.reach(at, false, true);
    let tests = users_all.iter().filter(|&&user| map.parts[user].kind == Kind::Tests).count();
    let used = map.reach(at, false, false).len();
    let what = match part.inside.as_deref() {
        Some(STACK) => "app",
        None => "project",
        Some(_) => "part",
    };
    let plural = |count: usize| if count == 1 { "" } else { "s" };
    let tested = match part.inside {
        None => format!(", {tests} test project{}", plural(tests)),
        Some(_) => String::new(),
    };
    let affects = format!("a change here affects {} {what}{}{tested}", users_all.len(), plural(users_all.len()));
    let path = part.path.strip_prefix(&controller.root).unwrap_or(&part.path).display().to_string();
    let counts = format!(
        "   {} · depends on {uses} ({uses_all} in all) · used by {used} ({} in all)",
        part.kind.name(),
        users_all.len()
    );
    let first = vec![seg(format!(" {}", part.name), C::White), seg(counts, C::Gray)];
    let mut second = vec![
        seg(format!(" {affects}"), if users_all.is_empty() { C::DarkGray } else { C::Yellow }),
        seg(format!(" · {path}"), C::DarkGray),
    ];
    if !part.tags.is_empty() {
        second.push(seg(format!(" · tags {}", part.tags.join(", ")), C::DarkGray));
    }
    let mut lines = super::seg::wrap(&first, width, DETAIL_INDENT, DETAIL_ROWS);
    lines.extend(super::seg::wrap(&second, width, DETAIL_INDENT, DETAIL_ROWS));
    for extra in [names(map, shown, at), rules(map, at)] {
        if !extra.is_empty() {
            lines.extend(super::seg::wrap(&extra, width, DETAIL_INDENT, DETAIL_ROWS));
        }
    }
    lines
}

pub fn rows(ui: &mut Ui, controller: &Controller, width: usize, rows: usize) -> Vec<Vec<Seg>> {
    settle(ui, controller);
    let Some(current) = view(ui).cloned() else { return Vec::new() };
    let map = controller.code_map();
    let shown = map.map(|map| shown(ui, map, &current));
    let hint = " h j k l choose · Enter inside · Backspace out · / search · g its graph · f focus (+ - how far) · z Z \
                zoom · r implied edges · t tests · a all projects · c cycles · b the build (w follow it · B it alone) · o \
                open · y copy path · Tab health · Esc back";
    let mut lines = vec![header(ui, controller, &current, shown.as_ref(), width)];
    lines.extend(super::viewbar::keys(hint, width));
    if let Some(run) = current.build.as_deref().and_then(|key| controller.build_run(key)) {
        lines.push(super::columns::fit(code_draw::build_summary(ui, run, &current), width));
    }
    lines.extend(notes(controller, &current, shown.as_ref(), width));
    let (Some(map), Some(shown)) = (map, shown) else {
        lines.resize(rows, Vec::new());
        return lines.into_iter().map(|line| aim(line, Target::Box)).collect();
    };
    if current.typing {
        lines.extend(super::code_search::rows(ui, map, &current, width));
        lines.push(Vec::new());
    }
    let run = current.build.as_deref().and_then(|key| controller.build_run(key));
    let layer =
        run.map(|run| code_draw::Layer { run, critical: run.critical_path(), spinner: super::columns::spinner(ui) });
    let mut below = details(controller, map, &shown, width);
    if let (Some(layer), Some(at)) = (&layer, shown.picked) {
        below.splice(0..0, code_draw::build_details(controller, layer, &map.parts[at], width));
    }
    let room = width.saturating_sub(2);
    let map_rows = rows.saturating_sub(lines.len() + below.len() + 1).max(3);
    let (drawn, fits) = code_draw::fitting(ui, map, &shown, &current, (room, map_rows), layer.as_ref());
    let mini = !fits && room > MINI.0 * 2 && map_rows >= MINI.1;
    let seen = if mini { room.saturating_sub(MINI.0 + 1) } else { room };
    let picked = shown.picked.and_then(|at| shown.parts.iter().position(|&part| part == at));
    let box_at = picked.and_then(|at| drawn.layout.boxes.get(at)).copied().unwrap_or_default();
    let from = (box_at.x + box_at.w + MARGIN).saturating_sub(seen);
    let top = (box_at.y + box_at.h + 1).saturating_sub(map_rows);
    if shown.parts.is_empty() {
        lines.push(vec![seg("  No projects found under the root or in the npm apps' folders.", C::DarkGray)]);
    }
    let map_lines = if mini {
        code_draw::with_minimap(&drawn, &shown, (from, top), (room, map_rows))
    } else {
        drawn.rows(from, room).into_iter().skip(top).take(map_rows).collect()
    };
    for row in map_lines {
        let mut line = vec![seg("  ", C::Gray)];
        line.extend(row);
        lines.push(line);
    }
    lines.resize(rows.saturating_sub(below.len()), Vec::new());
    lines.extend(below);
    lines.truncate(rows);
    lines.into_iter().map(|line| aim(line, Target::Box)).collect()
}
