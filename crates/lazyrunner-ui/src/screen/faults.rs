//! `f` on the stack map (K): faults on the picked app's calls (#105) - to another app (or a host you type), delayed,
//! failed with a status, dropped, or answered with a canned response from a file; for a share of the calls, on
//! routes starting with a path. `Enter` sets one (a .NET caller starts again once to load lazyrunner's hook), `Del`
//! takes the target's away, `Esc` goes back to the map; `F` there takes them all away.

use crossterm::event::KeyEvent;
use lazyrunner_core::output::Hue;
use lazyrunner_core::run::Controller;
use lazyrunner_core::run::faults::Kind;

use super::icons::{self, Icon};
use super::overlays::boxed;
use super::seg::{Seg, pad, seg};
use super::theme::C;
use super::{Overlay, Ui};

const INNER: usize = 92;
const LABEL_CELLS: usize = 9;
const VALUE_CELLS: usize = 44;
const KINDS: [&str; 4] = ["delay", "fail", "drop", "mock"];
/// What each kind's value starts at: ms to wait, the status to answer with, nothing, a mock's status.
const VALUES: [&str; 4] = ["800", "500", "", "200"];
const SHARE_STEP: u8 = 10;
const FOOTER: &str =
    "↑ ↓ a field · ← → change it · type into it · Enter sets the fault · Del takes this target's away · Esc the map";
const HOST: &str = "a host you type…";

/// The box: whose calls, the target and kind picked, the value, mock file, share, path and host typed, and the field.
#[derive(Clone, Debug)]
pub struct View {
    pub caller: usize,
    target: usize,
    kind: usize,
    value: String,
    file: String,
    share: u8,
    path: String,
    host: String,
    field: Field,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Field {
    To,
    Fault,
    Value,
    File,
    Share,
    Path,
}

/// A target's label, and where its calls go (`host:port` and the app) - none for a host to type.
type Choice = (String, Option<(String, String)>);

/// The targets a caller's calls can go to: the apps it calls first, then the others with a port, then a host.
fn targets(controller: &Controller, caller: usize) -> Vec<Choice> {
    let Some(from) = controller.apps.get(caller).map(|app| app.key.clone()) else { return Vec::new() };
    let called: Vec<String> =
        controller.stack_edges().into_iter().filter(|edge| edge.from == from).map(|edge| edge.to).collect();
    let mut apps: Vec<&lazyrunner_core::profile::App> =
        controller.apps.iter().filter(|app| app.key != from && controller.port(&app.key).is_some()).collect();
    apps.sort_by_key(|app| !called.contains(&app.key));
    let mut list: Vec<Choice> = apps
        .into_iter()
        .filter_map(|app| {
            let target = format!("localhost:{}", controller.port(&app.key)?);
            Some((format!("{} ({target})", app.name), Some((target, app.key.clone()))))
        })
        .collect();
    list.push((HOST.to_string(), None));
    list
}

pub fn open(ui: &mut Ui, caller: usize) {
    let view = View {
        caller,
        target: 0,
        kind: 1,
        value: VALUES[1].to_string(),
        file: String::new(),
        share: 100,
        path: String::new(),
        host: String::new(),
        field: Field::To,
    };
    ui.overlay = Some(Overlay::Faults(Box::new(view)));
}

/// The fields shown for the kind picked: a drop has no value, only a mock a file.
fn fields(view: &View) -> Vec<Field> {
    let mut fields = vec![Field::To, Field::Fault];
    if KINDS[view.kind] != "drop" {
        fields.push(Field::Value);
    }
    if KINDS[view.kind] == "mock" {
        fields.push(Field::File);
    }
    fields.extend([Field::Share, Field::Path]);
    fields
}

pub fn key(ui: &mut Ui, controller: &mut Controller, name: &str, key: &KeyEvent) -> bool {
    let Some(Overlay::Faults(view)) = &mut ui.overlay else { return false };
    let choices = targets(controller, view.caller);
    let on_host = choices.get(view.target).is_some_and(|(_, target)| target.is_none());
    let shown = fields(view);
    let at = shown.iter().position(|field| *field == view.field).unwrap_or(0);
    match name {
        "Escape" => {
            let caller = view.caller;
            ui.overlay = Some(Overlay::Stack { index: caller, opened: ui.now });
        }
        "UpArrow" | "Shift+Tab" => view.field = shown[super::keys::wrapped(at, -1, shown.len())],
        "DownArrow" | "Tab" => view.field = shown[super::keys::wrapped(at, 1, shown.len())],
        "LeftArrow" | "RightArrow" => {
            let by = if name == "RightArrow" { 1 } else { -1 };
            match view.field {
                Field::To => view.target = super::keys::wrapped(view.target, by, choices.len().max(1)),
                Field::Fault => {
                    view.kind = super::keys::wrapped(view.kind, by, KINDS.len());
                    view.value = VALUES[view.kind].to_string();
                }
                Field::Share if by > 0 => view.share = (view.share + SHARE_STEP).min(100),
                Field::Share => view.share = view.share.saturating_sub(SHARE_STEP).max(SHARE_STEP),
                _ => {}
            }
        }
        "Enter" => {
            let view = view.as_ref().clone();
            set(controller, &view, &choices);
        }
        "Delete" => {
            let caller = controller.apps.get(view.caller).map(|app| app.key.clone());
            let target = match choices.get(view.target) {
                Some((_, Some((_, app)))) => Some(app.clone()),
                _ => Some(view.host.clone()).filter(|host| !host.is_empty()),
            };
            controller.clear_faults(caller.as_deref(), target.as_deref());
        }
        "Backspace" => {
            if let Some(text) = text_of(view, on_host) {
                text.pop();
            }
        }
        _ => {
            if let Some(text) = text_of(view, on_host) {
                text.extend(super::keys::typed(key));
            }
        }
    }
    true
}

/// The text the picked field types into, if it's one.
fn text_of(view: &mut View, on_host: bool) -> Option<&mut String> {
    match view.field {
        Field::To if on_host => Some(&mut view.host),
        Field::Value => Some(&mut view.value),
        Field::File => Some(&mut view.file),
        Field::Path => Some(&mut view.path),
        _ => None,
    }
}

/// Enter: the fault as the box has it.
fn set(controller: &mut Controller, view: &View, choices: &[Choice]) {
    let Some(caller) = controller.apps.get(view.caller).map(|app| app.key.clone()) else { return };
    let target = match choices.get(view.target) {
        Some((_, Some((target, app)))) => (target.clone(), Some(app.clone())),
        _ if view.host.trim().is_empty() => {
            controller.add_log("Type the host its calls go to (api.example.com, or host:port).", Hue::Yellow);
            return;
        }
        _ => (view.host.trim().to_string(), None),
    };
    let number = |text: &str, fallback: u64| text.trim().parse::<u64>().unwrap_or(fallback);
    let status = |fallback: u16| u16::try_from(number(&view.value, u64::from(fallback))).unwrap_or(fallback);
    let kind = match KINDS[view.kind] {
        "delay" => Kind::Delay(number(&view.value, 800)),
        "drop" => Kind::Drop,
        "mock" => {
            let path = controller.root.join(view.file.trim());
            match std::fs::read_to_string(&path) {
                Ok(body) if !view.file.trim().is_empty() => {
                    Kind::Mock { status: status(200), body, file: view.file.trim().into() }
                }
                _ => {
                    let said = format!("A mock answers with a file's content: {} doesn't read.", path.display());
                    controller.add_log(said, Hue::Yellow);
                    return;
                }
            }
        }
        _ => Kind::Fail(status(500)),
    };
    let path = view.path.trim();
    let path = if path.is_empty() || path.starts_with('/') { path.to_string() } else { format!("/{path}") };
    controller.set_fault(&caller, target, &path, kind, view.share);
}

pub fn rows(ui: &Ui, controller: &Controller, cells: usize, rows: usize) -> Vec<Vec<Seg>> {
    let Some(Overlay::Faults(view)) = &ui.overlay else { return Vec::new() };
    let Some(caller) = controller.apps.get(view.caller) else { return Vec::new() };
    let inner = cells.saturating_sub(8).clamp(20, INNER);
    let choices = targets(controller, view.caller);
    let on_host = choices.get(view.target).is_some_and(|(_, target)| target.is_none());
    let cursor = ui.cursor();
    let mut lines = Vec::new();
    for field in fields(view) {
        let picked = field == view.field;
        let typing = if picked { cursor } else { "" };
        let (label, value, note) = match field {
            Field::To => {
                let name = choices.get(view.target).map_or(String::new(), |(label, _)| label.clone());
                let host = if view.host.is_empty() { HOST } else { &view.host };
                let value = if on_host { format!("◀ {host}{typing} ▶") } else { format!("◀ {name} ▶") };
                ("TO", value, "← → another app, or a host")
            }
            Field::Fault => ("FAULT", format!("◀ {} ▶", KINDS[view.kind]), "delay · fail · drop · mock"),
            Field::Value if KINDS[view.kind] == "delay" => {
                ("MS", format!("{}{typing}", view.value), "how long each call waits")
            }
            Field::Value => ("STATUS", format!("{}{typing}", view.value), "what it answers with"),
            Field::File => ("FILE", format!("{}{typing}", view.file), "the body, from the repo"),
            Field::Share => ("SHARE", format!("◀ {}% ▶", view.share), "of the calls"),
            Field::Path => ("PATH", format!("{}{typing}", view.path), "only routes starting with it (none: all)"),
        };
        let marker = if picked { "▸" } else { " " };
        lines.push(vec![
            seg(format!(" {marker} {}", pad(label, LABEL_CELLS)), if picked { C::White } else { C::DarkGray }),
            seg(pad(&value, VALUE_CELLS), if picked { C::Cyan } else { C::Gray }),
            seg(note, C::DarkGray),
        ]);
    }
    let set: Vec<_> = controller.faults().iter().filter(|fault| fault.caller == caller.key).collect();
    lines.push(Vec::new());
    lines.push(vec![seg(format!(" ON {}'S CALLS", caller.name.to_uppercase()), C::DarkCyan)]);
    if set.is_empty() {
        lines.push(vec![seg("   none yet", C::DarkGray)]);
    }
    for fault in set {
        let to_app = fault.to_app.as_deref().and_then(|key| controller.app(key));
        let to = to_app.map_or(fault.target.clone(), |app| app.name.clone());
        let hits = controller.fault_hits(fault.id);
        let answered = if hits > 0 { format!("   {hits} answered") } else { String::new() };
        lines.push(vec![
            seg(format!("   → {}", pad(&to, 24)), C::White),
            seg(pad(&fault.said(), 30), C::Yellow),
            seg(answered, C::DarkGray),
        ]);
    }
    let title = icons::titled(ui.nerd, Icon::Alert, &format!("Faults on {}'s calls", caller.name));
    boxed(&title, lines, FOOTER, cells, rows, inner)
}
