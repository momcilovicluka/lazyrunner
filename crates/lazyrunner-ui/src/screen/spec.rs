//! The spec tab of `H` (#100): each running .NET app's endpoints from its OpenAPI document, by tag - whether each was
//! called this session (how often, its p95) - and the endpoints its log has that the document doesn't. `Enter` on one
//! fills it in - its path, query and header parameters, its JSON body from the schema - and `Enter` again sends it to
//! the app: the request comes in like any other, its answer shows under the list (`l` opens its request in the
//! requests tab), and the values are remembered for the next time. `y` copies it as curl, `Y` as a .http file's
//! request, `r` reads the documents again.

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::{Duration, Instant};

use crossterm::event::KeyEvent;
use lazyrunner_core::output::Hue;
use lazyrunner_core::profile::AppKind;
use lazyrunner_core::run::Controller;
use lazyrunner_core::run::openapi::{self, Operation, Place, route_key};
use lazyrunner_core::run::requests::{Endpoint, endpoints};

use super::columns::fit;
use super::requests::{CopyAs, Tab, View, command_with};
use super::seg::{Seg, on, pad, seg};
use super::theme::C;
use super::{Overlay, Ui};

const PATH_CELLS: usize = 38;
const CALLED_CELLS: usize = 20;
const JSON: &str = "application/json";
const NOT_IN_SPEC: &str = "in the log, not in the document";
const HINT: &str = " Tab requests · j k choose · Enter fill it in and send · l its request · y copy as curl · Y as .http · r read again \
     · Esc back";
const FORM_HINT: &str = " ↑ ↓ (Tab) a field · type to change it · Enter sends it · Esc cancels";

/// A method pill's cells, the answer's body lines shown, and the `↗` that rises from the form as it's sent: how long
/// and how many rows.
const METHOD_CELLS: usize = 10;
const ANSWER_LINES: usize = 8;
const FLY: Duration = Duration::from_millis(450);
const FLY_ROWS: usize = 6;

type Fetched = Result<(String, Vec<Operation>), String>;

/// What the tab remembers this session: each endpoint's values as last sent, and the last send.
#[derive(Default)]
pub struct Memory {
    values: HashMap<String, Vec<String>>,
    response: Option<Response>,
}

/// A send: its app, method and path (and as shown, its values in), when, and what came back.
pub struct Response {
    app: String,
    method: String,
    path: String,
    shown: String,
    at: Instant,
    got: Got,
}

enum Got {
    Waiting(Receiver<Result<openapi::Sent, String>>),
    Done(Result<openapi::Sent, String>),
}

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

/// Whether a document is being read or an answer awaited while the tab shows (a frame comes when it's in).
pub fn animating(ui: &Ui) -> bool {
    let waiting = ui.spec_memory.response.as_ref().is_some_and(|response| {
        matches!(response.got, Got::Waiting(_)) || ui.now.saturating_duration_since(response.at) < FLY
    });
    matches!(&ui.overlay, Some(Overlay::Requests(view)) if view.tab == Tab::Spec)
        && (waiting || ui.specs.values().any(|loaded| matches!(loaded, Loaded::Fetching(_))))
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

/// A method as a pill, padded to line the paths up.
fn pill(method: &str) -> Vec<Seg> {
    let gap = METHOD_CELLS.saturating_sub(method.chars().count() + 2);
    vec![on(format!(" {method} "), C::Black, method_color(method)), seg(" ".repeat(gap), C::Gray)]
}

/// How the tab draws: the cursor, the spinner's frame, now, and whether motion is reduced.
pub type Look<'a> = (&'a str, &'a str, Instant, bool);

pub fn rows(
    view: &mut View,
    specs: &mut Specs,
    memory: &mut Memory,
    controller: &Controller,
    (width, height): (usize, usize),
    (cursor, spin, now, still): Look,
) -> Vec<Vec<Seg>> {
    fetch_missing(specs, controller);
    poll(specs);
    poll_answer(memory);
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
            Item::Op { app, operation, called, .. } => {
                let called_said = match called {
                    Some((count, p95)) => format!("{count}× · p95 {} ms", p95.round()),
                    None => "not called".to_string(),
                };
                let remembered = memory.values.contains_key(&memory_key(app, operation));
                let mut line = vec![seg("     ", C::Gray)];
                line.extend(pill(&operation.method));
                line.extend([
                    seg(pad(&operation.path, PATH_CELLS), C::White),
                    seg(pad(&called_said, CALLED_CELLS), if called.is_some() { C::Green } else { C::DarkGray }),
                    seg(if remembered { "◆ " } else { "  " }, C::DarkCyan),
                    seg(operation.summary.clone(), C::Gray),
                ]);
                if at == view.spec_index {
                    picked_line = lines.len();
                    for part in line.iter_mut().filter(|part| part.bg.is_none()) {
                        (part.fg, part.bg) = (C::FocusText, Some(C::FocusBack));
                    }
                }
                lines.push(line);
                at += 1;
            }
            Item::Logged(endpoint) => lines.push(vec![
                seg(format!("      {:<9}", endpoint.method), C::DarkGray),
                seg(pad(&endpoint.endpoint, PATH_CELLS), C::Yellow),
                seg(format!("{}× in the log", endpoint.count), C::DarkYellow),
            ]),
        }
    }
    let below = match view.form.as_ref() {
        Some(form) => form_rows(form, width, cursor),
        None => memory.response.as_ref().map_or_else(Vec::new, |response| answer_rows(response, width, spin)),
    };
    let room = height.saturating_sub(below.len()).max(3);
    let first = picked_line.saturating_sub(room / 2).min(lines.len().saturating_sub(room));
    let mut out: Vec<Vec<Seg>> = lines.into_iter().skip(first).take(room).collect();
    let pane = out.len();
    out.extend(below);
    if let Some(since) = memory.response.as_ref().map(|response| now.saturating_duration_since(response.at))
        && since < FLY
        && !still
    {
        let risen = (since.as_secs_f64() / FLY.as_secs_f64() * FLY_ROWS as f64) as usize;
        let row = (pane + 1).saturating_sub(risen).min(out.len().saturating_sub(1));
        let line = fit(std::mem::take(&mut out[row]), width.saturating_sub(3));
        out[row] = [line, vec![seg(" ↗", C::Cyan)]].concat();
    }
    out
}

/// The form under the list: the request it makes, then each field - the one being typed in lit, with the cursor.
fn form_rows(form: &Form, width: usize, cursor: &str) -> Vec<Vec<Seg>> {
    let mut head = pill(&form.operation.method);
    head.insert(0, seg(" ", C::Gray));
    head.extend([
        seg(form.operation.path.clone(), C::White),
        seg(format!("  {}", form.operation.summary), C::DarkGray),
    ]);
    let mut rows = vec![vec![seg("─".repeat(width), C::DarkGray)], head];
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

fn status_color(status: u16) -> C {
    match status {
        0..300 => C::Green,
        300..400 => C::Cyan,
        400..500 => C::Yellow,
        _ => C::Red,
    }
}

/// The last send's answer under the list: its status as a pill, how long it took, and its body (JSON laid out).
fn answer_rows(response: &Response, width: usize, spin: &str) -> Vec<Vec<Seg>> {
    let said = format!("{} {}", response.method, response.shown);
    let mut rows = vec![vec![seg("─".repeat(width), C::DarkGray)]];
    match &response.got {
        Got::Waiting(_) => rows.push(vec![seg(format!(" {spin} sending {said}…"), C::Cyan)]),
        Got::Done(Err(why)) => rows.push(vec![seg(format!(" ✗ {said} wasn't sent: {why}"), C::Red)]),
        Got::Done(Ok(sent)) => {
            rows.push(vec![
                seg(" ← ", C::DarkGray),
                on(format!(" {} ", sent.status), C::Black, status_color(sent.status)),
                seg(format!(" {} ms · {said}", sent.ms.round()), C::White),
                seg("   l its details in the requests tab", C::DarkGray),
            ]);
            let body = serde_json::from_str::<serde_json::Value>(&sent.body)
                .ok()
                .and_then(|value| serde_json::to_string_pretty(&value).ok())
                .unwrap_or_else(|| sent.body.clone());
            let room = width.saturating_sub(4).max(10);
            let lines: Vec<&str> = body.lines().collect();
            for line in lines.iter().take(ANSWER_LINES) {
                rows.push(vec![seg(format!("   {}", super::requests::cut(line, room)), C::Gray)]);
            }
            if lines.len() > ANSWER_LINES {
                rows.push(vec![seg(format!("   … {} more lines", lines.len() - ANSWER_LINES), C::DarkGray)]);
            }
            if body.trim().is_empty() {
                rows.push(vec![seg("   (no body)", C::DarkGray)]);
            }
        }
    }
    rows
}

/// The answer, once it's in.
fn poll_answer(memory: &mut Memory) {
    let Some(response) = memory.response.as_mut() else { return };
    let Got::Waiting(answered) = &response.got else { return };
    response.got = match answered.try_recv() {
        Ok(sent) => Got::Done(sent),
        Err(TryRecvError::Empty) => return,
        Err(TryRecvError::Disconnected) => Got::Done(Err("sending it stopped".into())),
    };
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
            if let Some((app, port, operation)) = picked {
                let form = fill_in(app, port, operation, &ui.spec_memory);
                if let Some(Overlay::Requests(view)) = &mut ui.overlay {
                    view.form = Some(form);
                }
            }
        }
        "y" => copy(ui, controller, CopyAs::Curl),
        "Y" => copy(ui, controller, CopyAs::Http),
        "l" => open_answered(ui, controller),
        _ => return false,
    }
    true
}

/// Where an endpoint's values are remembered: its app, method and path.
fn memory_key(app: &str, operation: &Operation) -> String {
    format!("{app} {} {}", operation.method, operation.path)
}

/// A form for `operation`: its parameters with the values last sent (else their examples), then its body.
fn fill_in(app: String, port: u16, operation: Operation, memory: &Memory) -> Form {
    let params = operation.params.iter().map(|param| Field {
        name: param.name.clone(),
        place: Some(param.place),
        required: param.required,
        value: param.example.clone(),
    });
    let body =
        operation.body.clone().map(|body| Field { name: "body".into(), place: None, required: false, value: body });
    let mut fields: Vec<Field> = params.chain(body).collect();
    if let Some(values) = memory.values.get(&memory_key(&app, &operation)).filter(|values| values.len() == fields.len())
    {
        for (field, value) in fields.iter_mut().zip(values) {
            field.value.clone_from(value);
        }
    }
    Form { app, port, fields, operation, at: 0 }
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

/// Sends the form's request in the background: how it went goes to the log and the answer under the list, the request
/// itself into the requests tab; its values are remembered for the next time.
fn send(ui: &mut Ui, controller: &mut Controller, form: Form) {
    if let Some(field) = form.fields.iter().find(|field| field.required && field.value.trim().is_empty()) {
        controller.add_log(format!("{} needs a value before it's sent.", field.name), Hue::Yellow);
        if let Some(Overlay::Requests(view)) = &mut ui.overlay {
            view.form = Some(form);
        }
        return;
    }
    let values = form.fields.iter().map(|field| field.value.clone()).collect();
    ui.spec_memory.values.insert(memory_key(&form.app, &form.operation), values);
    let (values, headers, body) = parts(&form);
    let url = openapi::url(form.port, &form.operation, &values);
    let method = form.operation.method.clone();
    let shown = url.trim_start_matches(&format!("http://localhost:{}", form.port)).to_string();
    let (sender, results) = channel();
    let (answer, answered) = channel();
    let (said_method, said_shown) = (method.clone(), shown.clone());
    std::thread::spawn(move || {
        let sent = openapi::send(&method, &url, &headers, body.as_deref());
        let outcome = match &sent {
            Ok(openapi::Sent { status, ms, .. }) => {
                let hue = if *status >= 400 { Hue::Yellow } else { Hue::Cyan };
                (hue, format!("↗ {method} {shown} sent → {status} in {} ms", ms.round()))
            }
            Err(why) => (Hue::Red, format!("✗ {method} {shown} wasn't sent: {why}")),
        };
        let _ = sender.send(outcome);
        let _ = answer.send(sent);
    });
    ui.replays.push(results);
    let name = controller.app(&form.app).map_or(form.app.clone(), |app| app.name.clone());
    controller.add_log(format!("Sending {} {} to {name}…", form.operation.method, form.operation.path), Hue::DarkGray);
    ui.spec_memory.response = Some(Response {
        app: form.app,
        method: said_method,
        path: form.operation.path,
        shown: said_shown,
        at: ui.now,
        got: Got::Waiting(answered),
    });
}

/// l: the last send's request in the requests tab, its details open.
fn open_answered(ui: &mut Ui, controller: &mut Controller) {
    let Some(response) = &ui.spec_memory.response else {
        controller.add_log("Send one first (Enter), then l shows its request.", Hue::DarkGray);
        return;
    };
    let found = controller.requests.items.iter().rev().find(|request| {
        request.app == response.app && same(&request.method, &request.endpoint, &response.method, &response.path)
    });
    let Some(seq) = found.map(|request| request.seq) else {
        controller.add_log("Its request isn't in the app's log yet.", Hue::DarkGray);
        return;
    };
    if let Some(Overlay::Requests(view)) = &mut ui.overlay {
        (view.tab, view.index, view.follow, view.selected, view.details) = (Tab::Requests, 0, false, seq, true);
    }
}

/// y: the picked endpoint, with its values, as curl; Y as a .http file's request.
fn copy(ui: &mut Ui, controller: &mut Controller, kind: CopyAs) {
    let Some(Overlay::Requests(view)) = &ui.overlay else { return };
    let Some((app, port, operation)) = picked(&ui.specs, controller, view.spec_index) else { return };
    let form = fill_in(app, port, operation, &ui.spec_memory);
    let (values, headers, body) = parts(&form);
    let url = openapi::url(port, &form.operation, &values);
    let content_type = if body.is_some() { JSON } else { "" };
    let (text, what) = if kind == CopyAs::Http {
        let mut lines = vec![format!("### {}", form.operation.summary), format!("{} {url}", form.operation.method)];
        lines.extend(headers.iter().map(|(name, value)| format!("{name}: {value}")));
        if let Some(body) = &body {
            lines.extend([format!("Content-Type: {JSON}"), String::new(), body.clone()]);
        }
        (lines.join(super::copy::NEWLINE), ".http request")
    } else {
        (command_with(kind, &form.operation.method, &url, content_type, body.as_deref().unwrap_or("")), "curl")
    };
    let what = format!("{what} for {} {}", form.operation.method, form.operation.path);
    super::copy::put(ui.clipboard, controller, &text, &what);
}
