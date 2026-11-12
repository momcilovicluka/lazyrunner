//! An app's CPU as a flame chart (#83), a drill-down of the Stack screen: `Alt+p` (or `p` on a Health card or a Map
//! box) samples a running .NET app with dotnet-trace, and its capture opens here - the call tree from the bottom up,
//! each frame as wide as its time on the CPU and coloured by its namespace, the split by namespace on top, the hottest
//! frames by self time beside it. `h j k l` move between frames, `Enter` zooms into one, `Backspace` out, `/` finds
//! frames, `[` `]` step through the app's captures and `c` compares one with the one before.

use lazyrunner_core::output::Hue;
use lazyrunner_core::run::Controller;
use std::time::{Duration, Instant};

use lazyrunner_core::run::profile::{HOT, Profile, namespace, short};

use super::icons::{self, Icon};
use super::mouse::Target;
use super::screens::Screen;
use super::seg::{Seg, aim, on, seg};
use super::stack::Tab;
use super::theme::C;
use super::{Overlay, Ui};

/// The namespaces' colours: the light ones with black text, the dark ones with white.
const LIGHT: [C; 4] = [C::Cyan, C::Green, C::Yellow, C::Magenta];
const DARK: [C; 4] = [C::Blue, C::DarkCyan, C::DarkGreen, C::DarkYellow];
/// The frames listed by self time.
const TOP: usize = 10;
/// Wider than this the top frames sit beside the chart, as wide as this.
const SIDE_FROM: usize = 120;
const SIDE: usize = 46;
/// A frame whose self time changed by this share of the whole is tinted when comparing.
const CHANGED: f64 = 0.05;
/// A row of the chart blooms in this long after the one under it; hot frames pulse once after.
const BLOOM_STEP: Duration = Duration::from_millis(40);
const PULSE_FROM: Duration = Duration::from_millis(500);
const PULSE: Duration = Duration::from_millis(200);
/// Zooming glides the frame to the full width this long; the breadcrumb's new step stays lit this long.
const GLIDE: Duration = Duration::from_millis(120);
const CRUMB: Duration = Duration::from_millis(400);
/// What `+` and `-` change a capture's length by.
const SECONDS_STEP: u64 = 5;

/// The fewest rows the chart gets.
const LEAST_CHART: usize = 4;

/// The chart: the app, which of its captures, the picked frame and the one zoomed into (nodes of the call tree), the
/// search, whether it compares with the capture before, the Stack tab it was opened from, and the chart's size as last
/// drawn (for the keys).
#[derive(Clone, Debug)]
pub struct View {
    pub app: String,
    pub capture: usize,
    pub picked: usize,
    pub zoom: usize,
    pub query: String,
    pub typing: bool,
    pub compare: bool,
    pub back: Option<Tab>,
    pub size: (usize, usize),
    /// When it opened (it blooms upward), when it last zoomed in and where that frame was (it glides to the full
    /// width), and when the breadcrumb last changed.
    pub opened: Instant,
    pub zoomed: Option<(Instant, f64, f64)>,
    pub crumb: Option<Instant>,
}

fn view(ui: &Ui) -> Option<&View> {
    match &ui.overlay {
        Some(Overlay::Flame(view)) => Some(view),
        _ => None,
    }
}

fn view_mut(ui: &mut Ui) -> Option<&mut View> {
    match &mut ui.overlay {
        Some(Overlay::Flame(view)) => Some(view),
        _ => None,
    }
}

/// App `app`'s newest capture as a chart; false when it has none.
pub fn open(ui: &mut Ui, controller: &Controller, app: &str) -> bool {
    let kept = controller.profiles_of(app).len();
    if kept == 0 {
        return false;
    }
    let back = (Screen::of(ui.overlay.as_ref()) == Some(Screen::Stack)).then_some(ui.stack_tab);
    let view = View {
        app: app.to_string(),
        capture: kept - 1,
        picked: 0,
        zoom: 0,
        query: String::new(),
        typing: false,
        compare: false,
        back: back.or(Some(Tab::Health)),
        size: (0, 0),
        opened: ui.now,
        zoomed: None,
        crumb: None,
    };
    ui.overlay = Some(Overlay::Flame(Box::new(view)));
    true
}

/// A capture just done opens on its own while the columns or the Stack screen show.
pub fn open_fresh(ui: &mut Ui, controller: &Controller) {
    let Some((key, at)) = &controller.last_profiled else { return };
    if ui.profile_seen == Some(*at) {
        return;
    }
    ui.profile_seen = Some(*at);
    let here = ui.overlay.is_none() || Screen::of(ui.overlay.as_ref()) == Some(Screen::Stack);
    if here && ui.picker.is_none() {
        open(ui, controller, key);
    }
}

/// `Alt+p`, `p`: samples app `key`; an app that isn't running opens its last capture instead, when it has one.
pub fn start(ui: &mut Ui, controller: &mut Controller, key: &str) {
    if let Err(why) = controller.profile(key, controller.profile_seconds())
        && (controller.is_running(key) || !open(ui, controller, key))
    {
        controller.add_log(why, Hue::Yellow);
    }
}

fn profile_of<'a>(controller: &'a Controller, view: &View) -> Option<&'a Profile> {
    controller.profiles_of(&view.app).get(view.capture)
}

/// A frame drawn: its node, how deep (0 the zoomed frame, at the bottom), where it starts and how wide.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Bar {
    node: usize,
    depth: usize,
    x: usize,
    width: usize,
}

/// The frames from `zoom` up, in `cells` across and `room` deep: each as wide as its time, the busiest callee first.
fn layout(profile: &Profile, zoom: usize, (cells, room): (usize, usize)) -> Vec<Bar> {
    layout_from(profile, zoom, room, (0.0, cells as f64))
}

/// The same, the zoomed frame drawn at `(x, width)` (gliding to the full width).
fn layout_from(profile: &Profile, zoom: usize, room: usize, at: (f64, f64)) -> Vec<Bar> {
    let mut bars = Vec::new();
    place(profile, zoom, (0, room), at, &mut bars);
    bars
}

/// How far along `since` is in `length`, eased out; none once it's over or with motion off.
fn progress(ui: &Ui, since: Instant, length: Duration) -> Option<f64> {
    let elapsed = ui.now.saturating_duration_since(since);
    (super::stack::moving(ui) && elapsed < length).then(|| {
        let t = elapsed.as_secs_f64() / length.as_secs_f64();
        1.0 - (1.0 - t) * (1.0 - t)
    })
}

/// Whether the chart moves: blooming, a hot frame's pulse, a zoom's glide, the breadcrumb's lit step.
pub fn animating(ui: &Ui) -> bool {
    let Some(view) = view(ui) else { return false };
    let lit = |at: Option<Instant>, length: Duration| at.is_some_and(|at| progress(ui, at, length).is_some());
    lit(Some(view.opened), PULSE_FROM + PULSE) || lit(view.zoomed.map(|(at, ..)| at), GLIDE) || lit(view.crumb, CRUMB)
}

fn place(profile: &Profile, node: usize, (depth, room): (usize, usize), (x, width): (f64, f64), bars: &mut Vec<Bar>) {
    let (from, to) = (x.round() as usize, (x + width).round() as usize);
    if depth >= room || to <= from {
        return;
    }
    bars.push(Bar { node, depth, x: from, width: to - from });
    let total = profile.nodes[node].total.max(f64::EPSILON);
    let mut children = profile.nodes[node].children.clone();
    children.sort_by(|a, b| profile.nodes[*b].total.total_cmp(&profile.nodes[*a].total));
    let mut at = x;
    for child in children {
        let share = width * profile.nodes[child].total / total;
        place(profile, child, (depth + 1, room), (at, share), bars);
        at += share;
    }
}

/// A namespace's colour, the same in every capture.
fn colour(space: &str) -> (C, C) {
    let hash = space.bytes().fold(2_166_136_261u32, |hash, byte| (hash ^ u32::from(byte)).wrapping_mul(16_777_619));
    let at = hash as usize % (LIGHT.len() + DARK.len());
    if at < LIGHT.len() { (C::Black, LIGHT[at]) } else { (C::White, DARK[at - LIGHT.len()]) }
}

/// Whether frame `name` is one `/` looks for.
fn found(query: &str, name: &str) -> bool {
    !query.is_empty() && name.to_lowercase().contains(&query.to_lowercase())
}

/// How much more (or less) time each frame spent in itself than in the capture before, when comparing.
fn changes(controller: &Controller, view: &View) -> Option<std::collections::HashMap<String, f64>> {
    let kept = controller.profiles_of(&view.app);
    let now = kept.get(view.capture)?;
    let before = kept.get(view.capture.checked_sub(1)?).filter(|_| view.compare)?;
    Some(now.compare(before))
}

/// A frame's bar: its name (a hot one marked) in its namespace's colour - picked lit, a search's others dimmed, a
/// change tinted warm or cool.
fn bar_segs(
    ui: &Ui,
    profile: &Profile,
    bar: Bar,
    view: &View,
    changes: Option<&std::collections::HashMap<String, f64>>,
) -> Vec<Seg> {
    let name = profile.name(bar.node);
    let total = profile.total().max(f64::EPSILON);
    let hot = bar.node != 0 && profile.nodes[bar.node].own / total >= HOT;
    let label = if bar.node == 0 {
        format!("all {:.0} ms", profile.total())
    } else {
        let fire = if hot { format!("{} ", icons::icon(ui.nerd, Icon::Fire)) } else { String::new() };
        format!("{fire}{}", short(name))
    };
    let change = changes.and_then(|changes| changes.get(name)).copied().unwrap_or(0.0);
    let (fg, bg) = match () {
        _ if bar.node == view.picked => (C::FocusText, C::FocusBack),
        _ if !view.query.is_empty() && !found(&view.query, name) => (C::Gray, C::DarkGray),
        _ if change > total * CHANGED => (C::White, C::Red),
        _ if change < -total * CHANGED => (C::White, C::Blue),
        _ if hot && pulsing(ui, view) => (C::Black, C::White),
        _ if hot => (C::White, C::DarkRed),
        _ => colour(&namespace(name)),
    };
    let text: String = label.chars().take(bar.width.saturating_sub(1)).collect();
    vec![on(format!("{text:<width$}", width = bar.width.saturating_sub(1)), fg, bg), seg(" ", C::Gray)]
}

/// Whether the hot frames pulse now: once, as the chart has bloomed.
fn pulsing(ui: &Ui, view: &View) -> bool {
    let elapsed = ui.now.saturating_duration_since(view.opened);
    super::stack::moving(ui) && elapsed >= PULSE_FROM && elapsed < PULSE_FROM + PULSE
}

/// The chart's rows, the deepest on top, the zoomed frame at the bottom: blooming upward as it opens, the zoomed frame
/// gliding to the full width.
fn chart(ui: &Ui, controller: &Controller, view: &View, profile: &Profile, size: (usize, usize)) -> Vec<Vec<Seg>> {
    let (cells, room) = size;
    let full = (0.0, cells as f64);
    let at = match view.zoomed.and_then(|(since, x, width)| Some((progress(ui, since, GLIDE)?, x, width))) {
        Some((t, x, width)) => (x * (1.0 - t), width + (full.1 - width) * t),
        None => full,
    };
    let bloomed = if super::stack::moving(ui) {
        (ui.now.saturating_duration_since(view.opened + super::motion::REVEAL).as_millis() / BLOOM_STEP.as_millis())
            as usize
            + 1
    } else {
        usize::MAX
    };
    let mut bars = layout_from(profile, view.zoom, room, at);
    bars.retain(|bar| bar.depth < bloomed);
    let changes = changes(controller, view);
    let depth = bars.iter().map(|bar| bar.depth + 1).max().unwrap_or(0);
    (0..depth)
        .rev()
        .map(|row| {
            let mut line = Vec::new();
            let mut at = 0;
            let mut level: Vec<Bar> = bars.iter().copied().filter(|bar| bar.depth == row).collect();
            level.sort_by_key(|bar| bar.x);
            for bar in level {
                if bar.x > at {
                    line.push(seg(" ".repeat(bar.x - at), C::Gray));
                }
                line.extend(aim(bar_segs(ui, profile, bar, view, changes.as_ref()), Target::Item(bar.node)));
                at = bar.x + bar.width;
            }
            line
        })
        .collect()
}

/// The time in itself by namespace, as one bar across.
fn split(profile: &Profile, cells: usize) -> Vec<Seg> {
    let total = profile.total().max(f64::EPSILON);
    let mut line = Vec::new();
    let mut used = 0;
    for (space, ms) in profile.namespaces() {
        let width = ((ms / total) * cells as f64).round() as usize;
        if width == 0 || used + width > cells {
            continue;
        }
        let label = format!("{space} {:.0}%", ms / total * 100.0);
        let (fg, bg) = colour(&space);
        line.push(on(format!("{:<width$}", label.chars().take(width).collect::<String>()), fg, bg));
        used += width;
    }
    line
}

/// Where the chart is zoomed: from all to the zoomed frame, the newest step lit a moment.
fn breadcrumb(ui: &Ui, profile: &Profile, view: &View) -> Vec<Seg> {
    let mut path = vec![view.zoom];
    while let Some(parent) = profile.nodes[*path.last().unwrap_or(&0)].parent {
        path.push(parent);
    }
    path.reverse();
    let lit = view.crumb.is_some_and(|at| progress(ui, at, CRUMB).is_some());
    let mut line = vec![seg(" ", C::Gray)];
    for (at, node) in path.iter().enumerate() {
        let last = at + 1 == path.len();
        let name = if *node == 0 { "all".to_string() } else { short(profile.name(*node)) };
        let color = match () {
            _ if last && lit => C::Yellow,
            _ if last => C::White,
            _ => C::DarkGray,
        };
        if at > 0 {
            line.push(seg(" › ", C::DarkGray));
        }
        line.push(seg(name, color));
    }
    line
}

/// The frames by self time, with how each changed since the capture before when comparing.
fn top(ui: &Ui, controller: &Controller, view: &View, profile: &Profile) -> Vec<Vec<Seg>> {
    let total = profile.total().max(f64::EPSILON);
    let changes = changes(controller, view);
    let mut lines = vec![vec![seg(" TOP FRAMES BY SELF TIME", C::DarkCyan)]];
    for hot in profile.top(TOP).into_iter().filter(|hot| hot.own > 0.0) {
        let share = hot.own / total;
        let fire = if share >= HOT { icons::icon(ui.nerd, Icon::Fire) } else { " " };
        let mut line = vec![seg(format!(" {fire} {:>3.0}% {:>7.0} ms  ", share * 100.0, hot.own), C::Gray)];
        if let Some(change) = changes.as_ref().map(|changes| changes.get(&hot.name).copied().unwrap_or(0.0)) {
            line.push(match change {
                _ if change >= 1.0 => seg(format!("▲ +{change:.0} ms  "), C::Red),
                _ if change <= -1.0 => seg(format!("▼ −{:.0} ms  ", -change), C::Green),
                _ => seg("           ", C::Gray),
            });
        }
        line.push(seg(short(&hot.name), C::White));
        lines.push(line);
    }
    lines
}

/// The picked frame in words: its name, its namespace, its time in all and in itself.
fn picked_line(profile: &Profile, view: &View) -> Vec<Seg> {
    let node = &profile.nodes[view.picked];
    let total = profile.total().max(f64::EPSILON);
    let name = profile.name(view.picked);
    if view.picked == 0 {
        let threads = format!(" all  {:.0} ms on the CPU, every thread's - k goes up into it", node.total);
        return vec![seg(threads, C::Gray)];
    }
    vec![
        seg(format!(" {}", short(name)), C::White),
        seg(format!("  {}", namespace(name)), C::DarkGray),
        seg(
            format!(
                "  {:.0} ms in all ({:.0}%) · {:.0} ms in itself",
                node.total,
                node.total / total * 100.0,
                node.own
            ),
            C::Gray,
        ),
    ]
}

fn header(controller: &Controller, view: &View, profile: &Profile) -> Vec<Seg> {
    let mut segs = super::stack::tabs(view.back.unwrap_or(Tab::Health));
    let name = controller.app(&view.app).map_or(view.app.as_str(), |app| app.name.as_str());
    let kept = controller.profiles_of(&view.app).len();
    let at = profile.at.get(11..19).unwrap_or(&profile.at);
    segs.push(seg(format!("  › Profile {name}"), C::White));
    segs.push(seg(
        format!(
            " · capture {} of {kept} · {at} · {} s · {:.0} ms on the CPU",
            view.capture + 1,
            profile.seconds,
            profile.total()
        ),
        C::DarkGray,
    ));
    if view.compare {
        segs.push(seg(" · against the one before: ▲ warmer, ▼ cooler", C::Yellow));
    }
    segs
}

pub fn rows(ui: &mut Ui, controller: &Controller, cells: usize, rows: usize) -> Vec<Vec<Seg>> {
    let Some(current) = view(ui).cloned() else { return Vec::new() };
    let Some(profile) = profile_of(controller, &current) else { return Vec::new() };
    let hint = " h j k l move · Enter zoom in · Backspace out · / find · [ ] captures · c compare · o its source · \
                y copy the top · p profile again · + - its length · Esc back";
    let mut lines = vec![super::columns::fit(header(controller, &current, profile), cells)];
    lines.extend(super::viewbar::keys(hint, cells));
    if current.typing || !current.query.is_empty() {
        lines.push(vec![seg(format!(" /{}{}", current.query, if current.typing { ui.cursor() } else { "" }), C::Cyan)]);
    }
    lines.push(split(profile, cells.saturating_sub(1)));
    lines.push(breadcrumb(ui, profile, &current));
    lines.push(Vec::new());
    let side = cells >= SIDE_FROM;
    let chart_cells = if side { cells.saturating_sub(SIDE + 2) } else { cells.saturating_sub(1) };
    let below = if side { 2 } else { TOP + 3 };
    let room = rows.saturating_sub(lines.len() + below).max(LEAST_CHART);
    let drawn = chart(ui, controller, &current, profile, (chart_cells, room));
    let top_lines = top(ui, controller, &current, profile);
    let height = drawn.len().max(if side { top_lines.len() } else { 0 });
    for row in 0..height {
        let at = (row + drawn.len()).checked_sub(height);
        let mut line = super::columns::fit(at.and_then(|at| drawn.get(at)).cloned().unwrap_or_default(), chart_cells);
        if side {
            line.push(seg(" │", C::DarkGray));
            line.extend(top_lines.get(row).cloned().unwrap_or_default());
        }
        lines.push(line);
    }
    lines.push(picked_line(profile, &current));
    if !side {
        lines.push(Vec::new());
        lines.extend(top_lines);
    }
    if let Some(view) = view_mut(ui) {
        view.size = (chart_cells, room);
    }
    lines.truncate(rows);
    lines.resize(rows, Vec::new());
    lines.into_iter().map(|line| aim(line, Target::Box)).collect()
}

/// The frame next to the picked one: a callee (`k`), the caller (`j`), the one beside it (`h` `l`).
fn step(profile: &Profile, view: &View, name: &str) -> usize {
    let bars = layout(profile, view.zoom, view.size);
    let Some(here) = bars.iter().find(|bar| bar.node == view.picked).copied() else { return view.zoom };
    let parent = profile.nodes[view.picked].parent;
    match name {
        "k" | "UpArrow" => {
            let callees = bars
                .iter()
                .filter(|bar| bar.depth == here.depth + 1 && profile.nodes[bar.node].parent == Some(here.node));
            callees.max_by_key(|bar| bar.width).map_or(here.node, |bar| bar.node)
        }
        "j" | "DownArrow" if view.picked != view.zoom => parent.unwrap_or(view.zoom),
        "h" | "LeftArrow" | "l" | "RightArrow" => {
            let mut level: Vec<Bar> = bars.iter().copied().filter(|bar| bar.depth == here.depth).collect();
            level.sort_by_key(|bar| bar.x);
            let at = level.iter().position(|bar| bar.node == here.node).unwrap_or(0);
            let next = if matches!(name, "h" | "LeftArrow") { at.checked_sub(1) } else { Some(at + 1) };
            next.and_then(|next| level.get(next)).map_or(here.node, |bar| bar.node)
        }
        _ => view.picked,
    }
}

/// `o`: the picked frame's file in the editor - where the profile says it is (Node's), else its type's file, found by
/// its name (`Quote.cs` for `Shop.Pricing.Quote.Total`).
fn open_source(ui: &mut Ui, controller: &mut Controller, profile: &Profile, node: usize) {
    if let Some((file, line)) = profile.source(node) {
        super::links::open_file(ui, controller, std::path::Path::new(file), Some(*line), None);
        return;
    }
    let name = profile.name(node).to_string();
    let parts: Vec<String> = short(&name).split('.').map(str::to_string).collect();
    let Some(kind) = parts.first().filter(|_| parts.len() > 1) else {
        controller.add_log("Pick a method's frame to open its source.", Hue::DarkGray);
        return;
    };
    let kind = kind.split('`').next().unwrap_or(kind);
    match lazyrunner_core::links::resolve(&controller.root, &format!("{kind}.cs")) {
        Some(path) => super::links::open_file(ui, controller, &path, None, None),
        None => controller.add_log(
            format!("No {kind}.cs in the repo - it's a library's frame, or its file has another name."),
            Hue::DarkGray,
        ),
    }
}

/// The keys while `/` takes typed text.
fn typing_key(view: &mut View, name: &str, key: &crossterm::event::KeyEvent) {
    match name {
        "Escape" => {
            view.query.clear();
            view.typing = false;
        }
        "Enter" => view.typing = false,
        "Backspace" => {
            view.query.pop();
        }
        _ => view.query.extend(super::keys::typed(key)),
    }
}

/// Whether the chart takes typed text.
pub fn typing(ui: &Ui) -> bool {
    view(ui).is_some_and(|view| view.typing)
}

/// The chart's keys; false for one it doesn't use.
pub fn key(ui: &mut Ui, controller: &mut Controller, name: &str, key: &crossterm::event::KeyEvent) -> bool {
    let Some(current) = view(ui).cloned() else { return false };
    if current.typing {
        if let Some(view) = view_mut(ui) {
            typing_key(view, name, key);
        }
        return true;
    }
    let Some(profile) = profile_of(controller, &current).cloned() else {
        ui.overlay = None;
        return true;
    };
    let kept = controller.profiles_of(&current.app).len();
    let mut next = current.clone();
    match name {
        "Escape" | "q" if !current.query.is_empty() => next.query.clear(),
        "Escape" | "q" => {
            match current.back {
                Some(tab) => {
                    ui.stack_tab = tab;
                    super::stack::reopen(ui, controller);
                }
                None => ui.overlay = None,
            }
            return true;
        }
        "h" | "j" | "k" | "l" | "LeftArrow" | "DownArrow" | "UpArrow" | "RightArrow" => {
            next.picked = step(&profile, &current, name);
        }
        "Enter" => {
            let from = layout(&profile, current.zoom, current.size).into_iter().find(|bar| bar.node == current.picked);
            next.zoomed = from.map(|bar| (ui.now, bar.x as f64, bar.width as f64));
            (next.zoom, next.crumb) = (current.picked, Some(ui.now));
        }
        "Backspace" => {
            next.zoom = profile.nodes[current.zoom].parent.unwrap_or(0);
            (next.zoomed, next.crumb) = (None, Some(ui.now));
        }
        "g" => (next.zoom, next.picked, next.crumb) = (0, 0, Some(ui.now)),
        "/" => (next.typing, next.query) = (true, String::new()),
        "[" | "]" => {
            let capture = if name == "[" { current.capture.checked_sub(1) } else { Some(current.capture + 1) };
            let Some(capture) = capture.filter(|&capture| capture < kept) else {
                controller.add_log(
                    format!("{kept} capture{} of this app kept.", if kept == 1 { "" } else { "s" }),
                    Hue::DarkGray,
                );
                return true;
            };
            (next.capture, next.zoom, next.picked) = (capture, 0, 0);
        }
        "c" if current.capture == 0 => {
            controller.add_log("No capture before this one to compare with: p profiles it again.", Hue::DarkGray);
        }
        "c" => next.compare = !current.compare,
        "y" => {
            let text: Vec<String> = profile
                .top(TOP)
                .iter()
                .map(|hot| format!("{:>5.1}%  {:>8.0} ms  {}", hot.own / profile.total() * 100.0, hot.own, hot.name))
                .collect();
            super::copy::put(ui.clipboard, controller, &text.join("\n"), "the top frames");
        }
        "o" => open_source(ui, controller, &profile, current.picked),
        "+" | "=" => controller.set_profile_seconds(controller.profile_seconds() + SECONDS_STEP),
        "-" => controller.set_profile_seconds(controller.profile_seconds().saturating_sub(SECONDS_STEP)),
        "p" => {
            start(ui, controller, &current.app);
            return true;
        }
        _ => return false,
    }
    if let Some(view) = view_mut(ui) {
        *view = next;
    }
    true
}

/// A click picks a frame; a double click zooms into it.
pub fn click(ui: &mut Ui, target: Option<Target>, double: bool) -> bool {
    let Some(view) = view_mut(ui) else { return false };
    let Some(Target::Item(node)) = target else { return false };
    view.picked = node;
    if double {
        view.zoom = node;
    }
    true
}

/// The column's title while its app is being profiled: how long is left, as a bar filling.
pub fn title(ui: &mut Ui, controller: &Controller, key: &str) -> Option<Vec<Seg>> {
    let job = controller.profiling.as_ref().filter(|job| job.key == key)?;
    let elapsed = ui.now.saturating_duration_since(job.started).as_secs_f64();
    let seconds = job.seconds as f64;
    let animated = !ui.reduced_motion;
    let bar = super::columns::bar(ui, (elapsed / seconds).min(1.0), &format!("profile:{key}"), animated);
    let left = (seconds - elapsed).max(0.0);
    Some(vec![seg(format!("◉ profiling {bar}{left:.0} s"), C::Magenta)])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_are_as_wide_as_their_time_and_a_namespace_keeps_its_colour() {
        let text = r#"{ "shared": { "frames": [ { "name": "Thread (1)" }, { "name": "A!Shop.Main()" },
            { "name": "A!Shop.Pricing.Quote.Total()" }, { "name": "A!Shop.Pricing.Rules.Tax()" }, { "name": "CPU_TIME" } ] },
            "profiles": [ { "type": "evented", "events": [
              { "type": "O", "frame": 0, "at": 0 }, { "type": "O", "frame": 1, "at": 0 }, { "type": "O", "frame": 2, "at": 0 },
              { "type": "O", "frame": 4, "at": 0 }, { "type": "C", "frame": 4, "at": 75 }, { "type": "C", "frame": 2, "at": 75 },
              { "type": "O", "frame": 3, "at": 75 }, { "type": "O", "frame": 4, "at": 75 }, { "type": "C", "frame": 4, "at": 100 },
              { "type": "C", "frame": 3, "at": 100 }, { "type": "C", "frame": 1, "at": 100 } ] } ] }"#;
        let profile =
            lazyrunner_core::run::profile::from_speedscope(text, "orders", "2026-11-12T12:00:00+01:00", 10).unwrap();
        let bars = layout(&profile, 0, (40, 10));
        let width = |name: &str| bars.iter().find(|bar| profile.name(bar.node).contains(name)).map(|bar| bar.width);
        assert_eq!(
            (width("all"), width("Main"), width("Total"), width("Tax")),
            (Some(40), Some(40), Some(30), Some(10))
        );
        assert_eq!(colour("Shop.Pricing"), colour("Shop.Pricing"));
    }
}
