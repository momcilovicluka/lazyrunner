//! The spec tab of `H` (#100): each running .NET app's endpoints from its OpenAPI document, by tag - whether each was
//! called this session (how often, its p95) - and the endpoints its log has that the document doesn't. `Enter` on one
//! fills it in - its path, query and header parameters, its JSON body from the schema - and `Enter` again sends it to
//! the app: the request comes in like any other, the requests tab shows it. `y` copies it as curl, `r` reads the
//! documents again.

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use crossterm::event::KeyEvent;
use lazyrunner_core::output::Hue;
use lazyrunner_core::profile::AppKind;
use lazyrunner_core::run::Controller;
use lazyrunner_core::run::openapi::{self, Operation, Place, route_key};
use lazyrunner_core::run::requests::{Endpoint, endpoints};

use super::requests::{CopyAs, Tab, View, command_with};
use super::seg::{Seg, pad, seg};
use super::theme::C;
use super::{Overlay, Ui};

const PATH_CELLS: usize = 38;
const CALLED_CELLS: usize = 20;
const JSON: &str = "application/json";
const NOT_IN_SPEC: &str = "in the log, not in the document";
const HINT: &str =
    " Tab requests · j k choose · Enter fill it in and send · y copy as curl · r read the documents again · Esc back";
const FORM_HINT: &str = " ↑ ↓ (Tab) a field · type to change it · Enter sends it · Esc cancels";

type Fetched = Result<(String, Vec<Operation>), String>;

/// An app's document: being read, read (where from, its operations), or why it couldn't be.
pub enum Loaded {
    Fetching(Receiver<Fetched>),
    Ready { from: String, operations: Vec<Operation> },
    Failed(String),
}

pub type Specs = HashMap<String, Loaded>;

/// An endpoint being filled in: the app and port it goes to, the operation, its fields and the one being typed in.
#[derive(Clone, Debug)]
pub struct Form {
    app: String,
    port: u16,
    operation: Operation,
    fields: Vec<Field>,
    at: usize,
}

/// A parameter (none: the body), whether it's needed, and its value.
#[derive(Clone, Debug)]
struct Field {
    name: String,
    place: Option<Place>,
    required: bool,
    value: String,
}

/// A row of the tab.
enum Item<'a> {
    App(String),
    Tag(&'a str),
    Op { app: &'a str, port: u16, operation: &'a Operation, called: Option<(usize, f64)> },
    Logged(Endpoint),
}

/// The running .NET apps' documents not read yet start being read, each in the background.
fn fetch_missing(specs: &mut Specs, controller: &Controller) {
    for app in &controller.apps {
        let AppKind::Dotnet { swagger, .. } = &app.kind else { continue };
        let Some(port) = controller.port(&app.key).filter(|_| controller.is_running(&app.key)) else { continue };
        if specs.contains_key(&app.key) {
            continue;
        }
        let (sender, results) = channel();
        let swagger = swagger.clone();
        std::thread::spawn(move || {
            let _ = sender.send(openapi::fetch(port, swagger.as_deref()));
        });
        specs.insert(app.key.clone(), Loaded::Fetching(results));
    }
}

fn poll(specs: &mut Specs) {
    for loaded in specs.values_mut() {
        let Loaded::Fetching(results) = loaded else { continue };
        let next = match results.try_recv() {
            Ok(Ok((from, operations))) => Loaded::Ready { from, operations },
            Ok(Err(why)) => Loaded::Failed(why),
            Err(TryRecvError::Empty) => continue,
            Err(TryRecvError::Disconnected) => Loaded::Failed("reading it stopped".into()),
        };
        *loaded = next;
    }
}

/// Whether a document is being read while the tab shows (a frame comes when it's in).
pub fn animating(ui: &Ui) -> bool {
    matches!(&ui.overlay, Some(Overlay::Requests(view)) if view.tab == Tab::Spec)
        && ui.specs.values().any(|loaded| matches!(loaded, Loaded::Fetching(_)))
}

fn same(method: &str, route: &str, other_method: &str, other_route: &str) -> bool {
    method == other_method && route_key(route) == route_key(other_route)
}

/// The tab's rows: each app's documents by tag, then what its log has that the document doesn't.
fn items<'a>(specs: &'a Specs, controller: &'a Controller) -> Vec<Item<'a>> {
    let summed = endpoints(controller.requests.items.iter());
    let mut items = Vec::new();
    for app in &controller.apps {
        let Some(loaded) = specs.get(&app.key) else { continue };
        let port = controller.port(&app.key).unwrap_or(0);
        let operations = match loaded {
            Loaded::Fetching(_) => {
                items.push(Item::App(format!("{} · reading its OpenAPI document…", app.name)));
                continue;
            }
            Loaded::Failed(why) => {
                items.push(Item::App(format!("{} · {why}", app.name)));
                continue;
            }
            Loaded::Ready { from, operations } => {
                items.push(Item::App(format!("{} · {} endpoints · {from}", app.name, operations.len())));
                operations
            }
        };
        let mut tag = None;
        for operation in operations {
            if tag != Some(operation.tag.as_str()) {
                tag = Some(operation.tag.as_str());
                items.push(Item::Tag(&operation.tag));
            }
            let called = summed
                .iter()
                .find(|each| {
                    each.app == app.key && same(&each.method, &each.endpoint, &operation.method, &operation.path)
                })
                .map(|each| (each.count, each.p95));
            items.push(Item::Op { app: &app.key, port, operation, called });
        }
        let documented = |each: &Endpoint| {
            operations.iter().any(|operation| same(&each.method, &each.endpoint, &operation.method, &operation.path))
        };
        let logged: Vec<Endpoint> =
            summed.iter().filter(|each| each.app == app.key && !documented(each)).cloned().collect();
        if !logged.is_empty() {
            items.push(Item::Tag(NOT_IN_SPEC));
            items.extend(logged.into_iter().map(Item::Logged));
        }
    }
    items
}

/// The picked endpoint: its app, port and operation.
fn picked(specs: &Specs, controller: &Controller, index: usize) -> Option<(String, u16, Operation)> {
    let items = items(specs, controller);
    let mut ops = items.into_iter().filter_map(|item| match item {
        Item::Op { app, port, operation, .. } => Some((app.to_string(), port, operation.clone())),
        _ => None,
    });
    ops.nth(index)
}

fn method_color(method: &str) -> C {
    match method {
        "GET" => C::Green,
        "POST" => C::Cyan,
        "PUT" | "PATCH" => C::Yellow,
        "DELETE" => C::Red,
        _ => C::Gray,
    }
}

pub fn hint(view: &View) -> &'static str {
    if view.form.is_some() { FORM_HINT } else { HINT }
}

pub fn rows(
    view: &mut View,
    specs: &mut Specs,
    controller: &Controller,
    (width, height): (usize, usize),
    cursor: &str,
) -> Vec<Vec<Seg>> {
    fetch_missing(specs, controller);
    poll(specs);
    let items = items(specs, controller);
    let count = items.iter().filter(|item| matches!(item, Item::Op { .. })).count();
    view.spec_index = view.spec_index.min(count.saturating_sub(1));
    if items.is_empty() {
        let why = "   No .NET app runs and listens yet: start one, and its OpenAPI document's endpoints show here.";
        return vec![vec![seg(why, C::DarkGray)]];
    }
    let mut lines = Vec::new();
    let (mut at, mut picked_line) = (0, 0);
    for item in &items {
        match item {
            Item::App(said) => lines.push(vec![seg(format!(" {said}"), C::White)]),
            Item::Tag(tag) => lines.push(vec![seg(format!("   {tag}"), C::DarkCyan)]),
            Item::Op { operation, called, .. } => {
                let called_said = match called {
                    Some((count, p95)) => format!("{count}× · p95 {} ms", p95.round()),
                    None => "not called".to_string(),
                };
                let mut line = vec![
                    seg(format!("     {:<8}", operation.method), method_color(&operation.method)),
                    seg(pad(&operation.path, PATH_CELLS), C::White),
                    seg(pad(&called_said, CALLED_CELLS), if called.is_some() { C::Green } else { C::DarkGray }),
                    seg(operation.summary.clone(), C::Gray),
                ];
                if at == view.spec_index {
                    picked_line = lines.len();
                    for part in &mut line {
                        (part.fg, part.bg) = (C::FocusText, Some(C::FocusBack));
                    }
                }
                lines.push(line);
                at += 1;
            }
            Item::Logged(endpoint) => lines.push(vec![
                seg(format!("     {:<8}", endpoint.method), C::DarkGray),
                seg(pad(&endpoint.endpoint, PATH_CELLS), C::Yellow),
                seg(format!("{}× in the log", endpoint.count), C::DarkYellow),
            ]),
        }
    }
    let form = view.form.as_ref().map_or_else(Vec::new, |form| form_rows(form, width, cursor));
    let room = height.saturating_sub(form.len()).max(3);
    let first = picked_line.saturating_sub(room / 2).min(lines.len().saturating_sub(room));
    let mut out: Vec<Vec<Seg>> = lines.into_iter().skip(first).take(room).collect();
    out.extend(form);
    out
}

/// The form under the list: the request it makes, then each field - the one being typed in lit, with the cursor.
fn form_rows(form: &Form, width: usize, cursor: &str) -> Vec<Vec<Seg>> {
    let mut rows = vec![
        vec![seg("─".repeat(width), C::DarkGray)],
        vec![
            seg(format!(" {} ", form.operation.method), method_color(&form.operation.method)),
            seg(form.operation.path.clone(), C::White),
            seg(format!("  {}", form.operation.summary), C::DarkGray),
        ],
    ];
    for (at, field) in form.fields.iter().enumerate() {
        let place = match field.place {
            Some(Place::Path) => "path",
            Some(Place::Query) => "query",
            Some(Place::Header) => "header",
            None => "body",
        };
        let needed = if field.required { "*" } else { " " };
        let typing = if at == form.at { cursor } else { "" };
        let label = pad(&format!("   {}{needed} ({place})", field.name), 30);
        let color = if at == form.at { C::Cyan } else { C::Gray };
        rows.push(vec![seg(label, C::DarkGray), seg(format!("{}{typing}", field.value), color)]);
    }
    rows
}

pub fn key(ui: &mut Ui, controller: &mut Controller, name: &str, key: &KeyEvent) -> bool {
    let Some(Overlay::Requests(view)) = &mut ui.overlay else { return false };
    if let Some(form) = view.form.as_mut() {
        let count = form.fields.len().max(1);
        match name {
            "Escape" => view.form = None,
            "Enter" => {
                if let Some(form) = view.form.take() {
                    send(ui, controller, form);
                }
            }
            "Tab" | "DownArrow" => form.at = super::keys::wrapped(form.at, 1, count),
            "Shift+Tab" | "UpArrow" => form.at = super::keys::wrapped(form.at, -1, count),
            "Backspace" => {
                if let Some(field) = form.fields.get_mut(form.at) {
                    field.value.pop();
                }
            }
            _ => {
                if let Some(field) = form.fields.get_mut(form.at) {
                    field.value.extend(super::keys::typed(key));
                }
            }
        }
        return true;
    }
    let count = items(&ui.specs, controller).iter().filter(|item| matches!(item, Item::Op { .. })).count();
    let Some(Overlay::Requests(view)) = &mut ui.overlay else { return false };
    match name {
        "Escape" | "q" | "H" => ui.overlay = None,
        "Tab" => (view.tab, view.index, view.follow) = (Tab::Requests, 0, true),
        "j" | "DownArrow" => view.spec_index = super::keys::wrapped(view.spec_index, 1, count.max(1)),
        "k" | "UpArrow" => view.spec_index = super::keys::wrapped(view.spec_index, -1, count.max(1)),
        "g" | "Home" => view.spec_index = 0,
        "G" | "End" => view.spec_index = count.saturating_sub(1),
        "r" => ui.specs.clear(),
        "Enter" | "Space" => {
            let picked = picked(&ui.specs, controller, view.spec_index);
            if let (Some((app, port, operation)), Some(Overlay::Requests(view))) = (picked, &mut ui.overlay) {
                view.form = Some(fill_in(app, port, operation));
            }
        }
        "y" => copy(ui, controller),
        _ => return false,
    }
    true
}

/// A form for `operation`: its parameters with their examples, then its body.
fn fill_in(app: String, port: u16, operation: Operation) -> Form {
    let params = operation.params.iter().map(|param| Field {
        name: param.name.clone(),
        place: Some(param.place),
        required: param.required,
        value: param.example.clone(),
    });
    let body =
        operation.body.clone().map(|body| Field { name: "body".into(), place: None, required: false, value: body });
    Form { app, port, fields: params.chain(body).collect(), operation, at: 0 }
}

/// Names and their values.
type Pairs = Vec<(String, String)>;

/// The values, headers and body a form sends.
fn parts(form: &Form) -> (Pairs, Pairs, Option<String>) {
    let of = |place: Place| {
        form.fields
            .iter()
            .filter(|field| field.place == Some(place))
            .map(|field| (field.name.clone(), field.value.clone()))
            .collect::<Vec<_>>()
    };
    let mut values = of(Place::Path);
    values.extend(of(Place::Query));
    let body = form.fields.iter().find(|field| field.place.is_none()).map(|field| field.value.clone());
    (values, of(Place::Header), body.filter(|body| !body.trim().is_empty()))
}

/// Sends the form's request in the background: how it went goes to the log, the request itself into the requests tab.
fn send(ui: &mut Ui, controller: &mut Controller, form: Form) {
    if let Some(field) = form.fields.iter().find(|field| field.required && field.value.trim().is_empty()) {
        controller.add_log(format!("{} needs a value before it's sent.", field.name), Hue::Yellow);
        if let Some(Overlay::Requests(view)) = &mut ui.overlay {
            view.form = Some(form);
        }
        return;
    }
    let (values, headers, body) = parts(&form);
    let url = openapi::url(form.port, &form.operation, &values);
    let method = form.operation.method.clone();
    let shown = url.trim_start_matches(&format!("http://localhost:{}", form.port)).to_string();
    let (sender, results) = channel();
    std::thread::spawn(move || {
        let outcome = match openapi::send(&method, &url, &headers, body.as_deref()) {
            Ok((status, ms)) => {
                let hue = if status >= 400 { Hue::Yellow } else { Hue::Cyan };
                (hue, format!("↗ {method} {shown} sent → {status} in {} ms", ms.round()))
            }
            Err(why) => (Hue::Red, format!("✗ {method} {shown} wasn't sent: {why}")),
        };
        let _ = sender.send(outcome);
    });
    ui.replays.push(results);
    let name = controller.app(&form.app).map_or(form.app.clone(), |app| app.name.clone());
    controller.add_log(format!("Sending {} {} to {name}…", form.operation.method, form.operation.path), Hue::DarkGray);
    if let Some(Overlay::Requests(view)) = &mut ui.overlay {
        (view.tab, view.index, view.follow) = (Tab::Requests, 0, true);
    }
}

/// y: the picked endpoint, with its examples, as curl.
fn copy(ui: &mut Ui, controller: &mut Controller) {
    let Some(Overlay::Requests(view)) = &ui.overlay else { return };
    let Some((app, port, operation)) = picked(&ui.specs, controller, view.spec_index) else { return };
    let form = fill_in(app, port, operation);
    let (values, _, body) = parts(&form);
    let url = openapi::url(port, &form.operation, &values);
    let content_type = if body.is_some() { JSON } else { "" };
    let text = command_with(CopyAs::Curl, &form.operation.method, &url, content_type, body.as_deref().unwrap_or(""));
    let what = format!("curl for {} {}", form.operation.method, form.operation.path);
    super::copy::put(ui.clipboard, controller, &text, &what);
}
