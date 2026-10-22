//! H: the HTTP requests the .NET apps served, newest (or slowest) first, or summed up per endpoint: count, errors,
//! average, 95th percentile, slowest. Errors only, one app, a path filter; y copies a row, w opens a GET request.

use crossterm::event::KeyEvent;
use lazyrunner_core::output::Hue;
use lazyrunner_core::run::Controller;
use lazyrunner_core::run::requests::{Endpoint, Request, endpoints};

use super::columns::{contains_ignore_case, fit};
use super::icons::{self, Icon};
use super::mouse::Target;
use super::seg::{Seg, aim, on, pad, seg, take_cells};
use super::theme::C;
use super::{Overlay, Ui, copy};

/// Ctrl+d / Ctrl+u move this far.
const PAGE: usize = 10;
/// Responses this slow are yellow, and this slow red.
const SLOW_MS: f64 = 1000.0;
const VERY_SLOW_MS: f64 = 3000.0;
/// The app's name in a row, at most.
const APP_CELLS: usize = 8;
/// What the rows take besides the path (requests) and the endpoint (endpoints).
const REQUEST_FIXED: usize = 46;
const ENDPOINT_FIXED: usize = 54;
const SORTS: [&str; 2] = ["time", "slowest"];
const ENDPOINT_SORTS: [&str; 5] = ["total", "avg", "max", "count", "errors"];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Requests,
    Endpoints,
}

/// What the view shows: the tab, the selected row, the filters and the sort.
#[derive(Clone, Debug)]
pub struct View {
    pub tab: Tab,
    pub index: usize,
    /// The top row follows new requests; any other row stays on its request (`selected`) as new ones arrive.
    pub follow: bool,
    pub selected: u64,
    pub errors_only: bool,
    /// Only this app's requests (its key).
    pub app: Option<String>,
    pub sort: usize,
    pub endpoint_sort: usize,
    pub filter: String,
    pub typing: bool,
}

impl Default for View {
    fn default() -> Self {
        Self {
            tab: Tab::Requests,
            index: 0,
            follow: true,
            selected: 0,
            errors_only: false,
            app: None,
            sort: 0,
            endpoint_sort: 0,
            filter: String::new(),
            typing: false,
        }
    }
}

/// H.
pub fn open(ui: &mut Ui) {
    ui.overlay = Some(Overlay::Requests(Box::default()));
}

/// The requests the filters let through, newest (or slowest) first.
fn shown<'a>(view: &View, controller: &'a Controller) -> Vec<&'a Request> {
    let mut list: Vec<&Request> = controller
        .requests
        .items
        .iter()
        .rev()
        .filter(|request| !view.errors_only || request.status >= 400)
        .filter(|request| view.app.as_ref().is_none_or(|app| *app == request.app))
        .filter(|request| view.filter.is_empty() || contains_ignore_case(&request.path, &view.filter))
        .collect();
    if SORTS[view.sort] == "slowest" {
        list.sort_by(|a, b| b.ms.total_cmp(&a.ms));
    }
    list
}

/// The shown requests per endpoint, by the chosen sort (the most first).
fn summed(view: &View, controller: &Controller) -> Vec<Endpoint> {
    let mut list = endpoints(shown(&View { sort: 0, ..view.clone() }, controller).into_iter());
    let key = |endpoint: &Endpoint| match ENDPOINT_SORTS[view.endpoint_sort] {
        "avg" => endpoint.avg,
        "max" => endpoint.max,
        "count" => endpoint.count as f64,
        "errors" => endpoint.errors as f64,
        _ => endpoint.total,
    };
    list.sort_by(|a, b| key(b).total_cmp(&key(a)));
    list
}

/// Tab requests / endpoints, j k choose, e errors only, a app, s sort, / filter by path, y copy, w / Enter open a
/// GET request, c clear, Esc / q / H back; false for a key it doesn't use.
pub fn key(ui: &mut Ui, controller: &mut Controller, name: &str, key: &KeyEvent) -> bool {
    let Some(Overlay::Requests(view)) = &mut ui.overlay else { return false };
    if view.typing {
        match name {
            "Escape" => {
                view.filter.clear();
                view.typing = false;
            }
            "Enter" => view.typing = false,
            "Backspace" => {
                view.filter.pop();
            }
            _ => view.filter.extend(super::keys::typed(key)),
        }
        (view.index, view.follow) = (0, true);
        return true;
    }
    let count = match view.tab {
        Tab::Requests => shown(view, controller).len(),
        Tab::Endpoints => summed(view, controller).len(),
    };
    match name {
        "Escape" | "q" | "H" => ui.overlay = None,
        "Tab" => {
            view.tab = if view.tab == Tab::Requests { Tab::Endpoints } else { Tab::Requests };
            (view.index, view.follow) = (0, true);
        }
        "j" | "DownArrow" => step(view, controller, 1, count),
        "k" | "UpArrow" => step(view, controller, -1, count),
        "Ctrl+D" | "PageDown" => step(view, controller, PAGE as isize, count),
        "Ctrl+U" | "PageUp" => step(view, controller, -(PAGE as isize), count),
        "g" | "Home" | "<" => (view.index, view.follow) = (0, true),
        "G" | "End" | ">" => step(view, controller, count as isize, count),
        "e" => {
            view.errors_only = !view.errors_only;
            (view.index, view.follow) = (0, true);
        }
        "a" => {
            let apps: Vec<String> =
                controller.apps.iter().filter(|app| app.traits().counts_requests).map(|app| app.key.clone()).collect();
            let at = view.app.as_ref().and_then(|app| apps.iter().position(|key| key == app));
            view.app = match at {
                None => apps.first().cloned(),
                Some(at) => apps.get(at + 1).cloned(),
            };
            (view.index, view.follow) = (0, true);
        }
        "s" => {
            match view.tab {
                Tab::Requests => view.sort = (view.sort + 1) % SORTS.len(),
                Tab::Endpoints => view.endpoint_sort = (view.endpoint_sort + 1) % ENDPOINT_SORTS.len(),
            }
            (view.index, view.follow) = (0, true);
        }
        "/" => view.typing = true,
        "c" => {
            controller.requests.items.clear();
            (view.index, view.follow) = (0, true);
            controller.add_log("Requests cleared.", Hue::DarkGray);
        }
        "y" => {
            let view = view.clone();
            copy_row(ui, controller, &view);
        }
        "w" | "Enter" => {
            let view = view.clone();
            open_row(ui, controller, &view);
        }
        _ => return false,
    }
    true
}

/// Moves the selection; the top row follows new requests, another one stays on its request.
fn step(view: &mut View, controller: &Controller, by: isize, count: usize) {
    view.index = view.index.saturating_add_signed(by).min(count.saturating_sub(1));
    view.follow = view.index == 0;
    view.selected = match view.tab {
        Tab::Requests => shown(view, controller).get(view.index).map_or(0, |request| request.seq),
        Tab::Endpoints => 0,
    };
}

fn app_name(controller: &Controller, key: &str) -> String {
    controller.app(key).map_or_else(|| key.to_string(), |app| app.name.clone())
}

fn url(controller: &Controller, request: &Request) -> String {
    let port = controller.port(&request.app).map_or(String::new(), |port| format!(":{port}"));
    format!("http://localhost{port}{}", request.path)
}

fn copy_row(ui: &Ui, controller: &mut Controller, view: &View) {
    let text = match view.tab {
        Tab::Requests => shown(view, controller).get(view.index).map(|request| {
            let (name, url) = (app_name(controller, &request.app), url(controller, request));
            let at = request.at.format("%Y-%m-%d %H:%M:%S");
            format!("{} {url} -> {} in {} ms ({name}, {at})", request.method, request.status, ms(request.ms))
        }),
        Tab::Endpoints => summed(view, controller).get(view.index).map(|endpoint| {
            let name = app_name(controller, &endpoint.app);
            format!(
                "{} {} ({name}): {} requests, {} errors, avg {} ms, p95 {} ms, max {} ms",
                endpoint.method,
                endpoint.endpoint,
                endpoint.count,
                endpoint.errors,
                ms(endpoint.avg),
                ms(endpoint.p95),
                ms(endpoint.max)
            )
        }),
    };
    if let Some(text) = text {
        copy::put(ui.clipboard, controller, &text, "the selected row");
    }
}

fn open_row(ui: &Ui, controller: &mut Controller, view: &View) {
    let found = match view.tab {
        Tab::Requests => shown(view, controller)
            .get(view.index)
            .filter(|request| request.method == "GET")
            .map(|request| url(controller, request)),
        Tab::Endpoints => None,
    };
    match found {
        Some(url) if (ui.opener)(&url) => controller.add_log(format!("Opening {url}"), Hue::DarkGray),
        Some(url) => controller.add_log(format!("✗ Couldn't open {url}"), Hue::Red),
        None => controller.add_log("w opens a GET request from the requests tab.", Hue::DarkGray),
    }
}

// ---------------------------------------------------------------- the view

fn ms(ms: f64) -> String {
    if ms >= 10_000.0 { format!("{:.1}s", ms / 1000.0) } else { format!("{ms:.0}") }
}

fn status_color(status: u16) -> C {
    match status {
        500.. => C::Red,
        400.. => C::Yellow,
        300.. => C::Cyan,
        _ => C::Green,
    }
}

fn ms_color(ms: f64) -> C {
    if ms >= VERY_SLOW_MS {
        C::Red
    } else if ms >= SLOW_MS {
        C::Yellow
    } else {
        C::Gray
    }
}

/// Cut to `cells`, with "…" when it was longer.
fn cut(text: &str, cells: usize) -> String {
    if text.chars().count() > cells { format!("{}…", take_cells(text, cells.saturating_sub(1))) } else { text.into() }
}

pub fn rows(ui: &mut Ui, controller: &Controller, width: usize, rows: usize) -> Vec<Vec<Seg>> {
    let (cursor, nerd) = (ui.cursor(), ui.nerd);
    let Some(Overlay::Requests(view)) = &mut ui.overlay else { return Vec::new() };
    let list = shown(view, controller);
    if !view.follow && view.selected != 0 {
        // Stay on the selected request while new ones arrive above it.
        if let Some(at) = list.iter().position(|request| request.seq == view.selected) {
            view.index = at;
        }
    }
    let height = rows.saturating_sub(4);
    let mut out = vec![header(view, controller, &list, width, cursor, nerd), hint(view, controller, width)];
    match view.tab {
        Tab::Requests => {
            view.index = view.index.min(list.len().saturating_sub(1));
            out.extend(request_rows(view, controller, &list, width, height));
            out.resize(rows.saturating_sub(1), Vec::new());
            out.push(vec![seg(detail(controller, list.get(view.index).copied()), C::White)]);
        }
        Tab::Endpoints => {
            let summed = summed(view, controller);
            view.index = view.index.min(summed.len().saturating_sub(1));
            out.extend(endpoint_rows(view, controller, &summed, width, height + 1));
        }
    }
    out.resize(rows, Vec::new());
    out.into_iter().map(|row| aim(fit(row, width), Target::Box)).collect()
}

/// The selected request in full, on the last row.
fn detail(controller: &Controller, request: Option<&Request>) -> String {
    let Some(request) = request else { return String::new() };
    let length = if request.length.is_empty() { String::new() } else { format!(" · {} bytes", request.length) };
    let content = if request.content.is_empty() { String::new() } else { format!(" · {}", request.content) };
    let (name, at) = (app_name(controller, &request.app), request.at.format("%H:%M:%S"));
    let (method, path, status) = (&request.method, &request.path, request.status);
    format!(" {method} {path}   {status} · {} ms{length}{content} · {name} · {at}", ms(request.ms))
}

fn header(view: &View, controller: &Controller, list: &[&Request], width: usize, cursor: &str, nerd: bool) -> Vec<Seg> {
    let errors = list.iter().filter(|request| request.status >= 400).count();
    let avg =
        if list.is_empty() { 0.0 } else { list.iter().map(|request| request.ms).sum::<f64>() / list.len() as f64 };
    let mut filters = Vec::new();
    if view.errors_only {
        filters.push("errors only".to_string());
    }
    if let Some(app) = &view.app {
        filters.push(app_name(controller, app));
    }
    if !view.filter.is_empty() || view.typing {
        filters.push(format!("/{}{}", view.filter, if view.typing { cursor } else { "" }));
    }
    let chip = format!(" {} Requests ", icons::icon(nerd, Icon::Requests));
    let mut segs = vec![on(chip, C::Black, C::Cyan), seg("  ", C::Gray)];
    for (tab, label) in [(Tab::Requests, " requests "), (Tab::Endpoints, " endpoints ")] {
        segs.push(if view.tab == tab { on(label, C::FocusText, C::FocusBack) } else { seg(label, C::DarkGray) });
    }
    segs.push(seg(format!("   {} shown · ", list.len()), C::DarkGray));
    segs.push(seg(format!("{errors} errors"), if errors > 0 { C::Red } else { C::DarkGray }));
    segs.push(seg(format!(" · avg {} ms", ms(avg)), C::DarkGray));
    if !filters.is_empty() {
        segs.push(seg(format!("   [{}]", filters.join(" · ")), C::Yellow));
    }
    fit(segs, width)
}

fn hint(view: &View, controller: &Controller, width: usize) -> Vec<Seg> {
    if !controller.apps.iter().any(|app| app.traits().counts_requests) {
        return fit(vec![seg(" No .NET app here: the requests come from ASP.NET's log.", C::DarkGray)], width);
    }
    let sort = match view.tab {
        Tab::Requests => SORTS[view.sort],
        Tab::Endpoints => ENDPOINT_SORTS[view.endpoint_sort],
    };
    let text = format!(
        " Tab requests/endpoints · j k choose · e errors only · a app · s sort ({sort}) · / filter · y copy · w open · \
         c clear · Esc back"
    );
    fit(vec![seg(text, C::DarkGray)], width)
}

fn request_rows(view: &View, controller: &Controller, list: &[&Request], width: usize, height: usize) -> Vec<Vec<Seg>> {
    let path_cells = width.saturating_sub(REQUEST_FIXED).max(10);
    let title = format!(" {:<8}  {:<8}  {:<6} {:>6} {:>7}  {}", "TIME", "APP", "METHOD", "STATUS", "MS", "PATH");
    let mut out = vec![vec![seg(title, C::DarkCyan)]];
    if list.is_empty() {
        let why = if controller.requests.items.is_empty() {
            "No requests yet - they show up here as the .NET apps serve them."
        } else {
            "No request matches the filters."
        };
        out.push(vec![seg(format!("  {why}"), C::DarkGray)]);
    }
    let top = view.index.saturating_sub(height / 2).min(list.len().saturating_sub(height));
    for (index, request) in list.iter().enumerate().skip(top).take(height) {
        let time = request.at.format("%H:%M:%S").to_string();
        let app = pad(&cut(&app_name(controller, &request.app), APP_CELLS), APP_CELLS);
        let path = cut(&request.path, path_cells);
        let (method, status, took) = (&request.method, request.status, ms(request.ms));
        let row = if index == view.index {
            let text = format!(" {time:<8}  {app}  {method:<6} {status:>6} {took:>7}  {path}");
            vec![on(pad(&text, width), C::FocusText, C::FocusBack)]
        } else {
            vec![
                seg(format!(" {time:<8}  {app}  {method:<6}"), C::DarkGray),
                seg(format!(" {status:>6}"), status_color(status)),
                seg(format!(" {took:>7}"), ms_color(request.ms)),
                seg(format!("  {path}"), C::Gray),
            ]
        };
        out.push(aim(row, Target::Item(index)));
    }
    out
}

fn endpoint_rows(
    view: &View,
    controller: &Controller,
    list: &[Endpoint],
    width: usize,
    height: usize,
) -> Vec<Vec<Seg>> {
    let name_cells = width.saturating_sub(ENDPOINT_FIXED).max(10);
    let title = format!(
        " {:<8}  {:<6} {:>6} {:>6} {:>7} {:>7} {:>7}  {}",
        "APP", "METHOD", "COUNT", "ERRORS", "AVG", "P95", "MAX", "ENDPOINT"
    );
    let mut out = vec![vec![seg(title, C::DarkCyan)]];
    if list.is_empty() {
        out.push(vec![seg("  No requests yet.", C::DarkGray)]);
    }
    let top = view.index.saturating_sub(height / 2).min(list.len().saturating_sub(height));
    for (index, endpoint) in list.iter().enumerate().skip(top).take(height) {
        let app = pad(&cut(&app_name(controller, &endpoint.app), APP_CELLS), APP_CELLS);
        let name = cut(&endpoint.endpoint, name_cells);
        let (count, errors) = (format!(" {:>6}", endpoint.count), format!(" {:>6}", endpoint.errors));
        let times = [endpoint.avg, endpoint.p95, endpoint.max];
        let row = if index == view.index {
            let times: String = times.iter().map(|time| format!(" {:>7}", ms(*time))).collect();
            let text = format!(" {app}  {:<6}{count}{errors}{times}  {name}", endpoint.method);
            vec![on(pad(&text, width), C::FocusText, C::FocusBack)]
        } else {
            let mut row = vec![
                seg(format!(" {app}  {:<6}", endpoint.method), C::DarkGray),
                seg(count, C::Gray),
                seg(errors, if endpoint.errors > 0 { C::Red } else { C::DarkGray }),
            ];
            row.extend(times.iter().map(|time| seg(format!(" {:>7}", ms(*time)), ms_color(*time))));
            row.push(seg(format!("  {name}"), C::Gray));
            row
        };
        out.push(aim(row, Target::Item(index)));
    }
    out
}
