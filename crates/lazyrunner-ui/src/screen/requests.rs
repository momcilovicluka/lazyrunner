//! H: the HTTP requests the .NET apps served, newest (or slowest) first, or summed up per endpoint: count, errors,
//! average, 95th percentile, slowest. Errors only, one app, a path filter; y copies a row, w opens a GET request.
//! Enter (or Space) shows the selected row's details in a panel on the right: a request's log lines, database
//! commands and how it compares with its endpoint; an endpoint's times, statuses, slowest and failing requests.

use crossterm::event::KeyEvent;
use lazyrunner_core::output::Hue;
use lazyrunner_core::run::Controller;
use lazyrunner_core::run::requests::{Endpoint, Request, endpoints};

use super::columns::{contains_ignore_case, fit, line_color};
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
/// The details panel takes at least this many columns; on a screen too narrow for it the selected request fits the
/// last row.
const PANEL_MIN: usize = 40;
/// The list's share of a wide screen, in twentieths, and the least room it leaves the path (or endpoint).
const LIST_SHARE: usize = 9;
const LIST_PATH: usize = 20;
/// A log line wraps onto this many rows at most.
const LOG_WRAP: usize = 4;
/// An endpoint's slowest and failing requests listed, at most.
const LISTED: usize = 5;
/// An endpoint's latest request times in its sparkline, at most.
const SPARK_MAX: usize = 60;
/// The repeated query in an N+1's details, at most.
const N_PLUS_ONE_SQL: usize = 160;
/// An endpoint with fewer requests isn't compared with.
const USUAL_MIN: usize = 5;
const SPARKS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
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
    /// The details panel is open (Enter / Space).
    pub details: bool,
    /// Only one endpoint's requests (l on its row): app, method and endpoint; and its row, to go back to (h).
    pub endpoint: Option<(String, String, String)>,
    pub endpoint_row: usize,
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
            details: false,
            endpoint: None,
            endpoint_row: 0,
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
        .filter(|request| {
            view.endpoint.as_ref().is_none_or(|(app, method, endpoint)| {
                request.app == *app && request.method == *method && request.endpoint == *endpoint
            })
        })
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

/// Tab requests / endpoints, j k choose, Enter / Space details, l an endpoint's requests (h, Esc back to it), e errors
/// only, a app, s sort, / filter by path, y copy, w open a GET request, o the app's log, c clear, Esc the details or
/// back, q / H back; false for a key it doesn't use.
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
        "Escape" if view.details => view.details = false,
        "Escape" | "h" | "LeftArrow" | "Tab" if view.endpoint.is_some() => back_to_endpoints(view),
        "Escape" | "q" | "H" => ui.overlay = None,
        "Enter" | "Space" => view.details = !view.details,
        "l" | "RightArrow" if view.tab == Tab::Endpoints => {
            let Some(endpoint) = summed(view, controller).into_iter().nth(view.index) else { return true };
            view.endpoint = Some((endpoint.app, endpoint.method, endpoint.endpoint));
            view.endpoint_row = view.index;
            view.tab = Tab::Requests;
            (view.index, view.follow) = (0, true);
        }
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
        "w" => {
            let view = view.clone();
            open_row(ui, controller, &view);
        }
        "o" => {
            let view = view.clone();
            open_log(ui, controller, &view);
        }
        _ => return false,
    }
    true
}

/// h: from one endpoint's requests back to the endpoints, on its row.
fn back_to_endpoints(view: &mut View) {
    view.endpoint = None;
    view.tab = Tab::Endpoints;
    view.index = view.endpoint_row;
    view.follow = view.index == 0;
}

/// Moves the selection; the top row follows new requests, another one stays on its request.
fn step(view: &mut View, controller: &Controller, by: isize, count: usize) {
    view.index = super::keys::wrapped(view.index, by, count);
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

/// o: the log of the selected row's app - all it printed, the request lines too.
fn open_log(ui: &Ui, controller: &mut Controller, view: &View) {
    let app = match view.tab {
        Tab::Requests => shown(view, controller).get(view.index).map(|request| request.app.clone()),
        Tab::Endpoints => summed(view, controller).get(view.index).map(|endpoint| endpoint.app.clone()),
    };
    let Some(state) = app.and_then(|app| controller.states.iter_mut().find(|state| state.key == app)) else { return };
    state.flush_log();
    match state.log_path.clone().filter(|path| path.exists()) {
        Some(path) if (ui.opener)(&path.display().to_string()) => {
            controller.add_log(format!("Opened {}", path.display()), Hue::DarkGray);
        }
        Some(path) => controller.add_log(format!("✗ Couldn't open {}", path.display()), Hue::Red),
        None => controller.add_log("No log yet.", Hue::DarkGray),
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
    let least = if view.tab == Tab::Requests { REQUEST_FIXED } else { ENDPOINT_FIXED } + LIST_PATH;
    let side_width = (width * LIST_SHARE / 20).max(least);
    let side = view.details && width >= side_width + 2 + PANEL_MIN;
    let list_width = if side { side_width } else { width };
    let panel = (width.saturating_sub(list_width + 2), height + 2);
    match view.tab {
        Tab::Requests if side => {
            view.index = view.index.min(list.len().saturating_sub(1));
            let left = request_rows(view, controller, &list, list_width, height + 1);
            let right = request_details(controller, list.get(view.index).copied(), panel.0, panel.1);
            out.extend(beside(left, right, list_width, panel));
        }
        Tab::Requests => {
            view.index = view.index.min(list.len().saturating_sub(1));
            out.extend(request_rows(view, controller, &list, width, height));
            out.resize(rows.saturating_sub(1), Vec::new());
            out.push(vec![seg(detail(controller, list.get(view.index).copied()), C::White)]);
        }
        Tab::Endpoints => {
            let summed = summed(view, controller);
            view.index = view.index.min(summed.len().saturating_sub(1));
            let left = endpoint_rows(view, controller, &summed, list_width, height + 1);
            if side {
                let right = endpoint_details(view, controller, summed.get(view.index), panel.0, panel.1);
                out.extend(beside(left, right, list_width, panel));
            } else {
                out.extend(left);
            }
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

/// The list on the left, the details on the right.
fn beside(
    left: Vec<Vec<Seg>>,
    right: Vec<Vec<Seg>>,
    list_width: usize,
    (width, rows): (usize, usize),
) -> Vec<Vec<Seg>> {
    (0..rows)
        .map(|row| {
            let mut line = fit(left.get(row).cloned().unwrap_or_default(), list_width);
            line.push(seg("│ ", C::FocusBorder));
            line.extend(fit(right.get(row).cloned().unwrap_or_default(), width));
            line
        })
        .collect()
}

/// `text` in lines of `cells`, at most `most` of them (the last one cut with "…").
fn wrap(text: &str, cells: usize, most: usize) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut lines: Vec<String> = chars.chunks(cells.max(1)).map(|chunk| chunk.iter().collect()).collect();
    if lines.len() > most {
        let rest: String = lines[most - 1..].concat();
        lines.truncate(most - 1);
        lines.push(cut(&rest, cells));
    }
    lines
}

/// `text` in lines of at most `cells`, broken between words.
fn words(text: &str, cells: usize) -> Vec<String> {
    let mut lines = vec![String::new()];
    for word in text.split_whitespace() {
        let line = lines.last_mut().expect("a line");
        if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > cells {
            lines.push(word.to_string());
        } else {
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
    }
    lines
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        204 => "No Content",
        301 => "Moved Permanently",
        302 => "Found",
        304 => "Not Modified",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        415 => "Unsupported Media Type",
        422 => "Unprocessable Entity",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => "",
    }
}

/// A body's content type and length, or that there was none.
fn body(content: &str, length: &str) -> String {
    let length = if length.is_empty() || length == "0" { String::new() } else { format!("{length} bytes") };
    match (content.is_empty(), length.is_empty()) {
        (true, true) => "no body".into(),
        (false, false) => format!("{content} · {length}"),
        _ => format!("{content}{length}"),
    }
}

/// The selected request: what it was, how it compares with its endpoint, what it sent and returned, what it logged.
fn request_details(controller: &Controller, request: Option<&Request>, width: usize, height: usize) -> Vec<Vec<Seg>> {
    let Some(request) = request else { return vec![vec![seg("No request selected.", C::DarkGray)]] };
    let mut lines: Vec<Vec<Seg>> = wrap(&format!("{} {}", request.method, request.path), width, 3)
        .into_iter()
        .map(|line| vec![seg(line, C::White)])
        .collect();
    let proto = if request.proto.is_empty() { String::new() } else { format!(" · {}", request.proto) };
    let (name, at) = (app_name(controller, &request.app), request.at.format("%H:%M:%S"));
    lines.push(vec![
        seg(
            format!("{} {}", request.status, reason(request.status)).trim_end().to_string(),
            status_color(request.status),
        ),
        seg(format!(" · {} ms", ms(request.ms)), ms_color(request.ms)),
    ]);
    lines.push(vec![seg(format!("{name} · {at}{proto}"), C::DarkGray)]);
    let same = controller.requests.items.iter().filter(|other| {
        other.app == request.app && other.method == request.method && other.endpoint == request.endpoint
    });
    if let Some(usual) = endpoints(same).pop().filter(|usual| usual.count >= USUAL_MIN) {
        let (verdict, color) = if request.ms > usual.p95 {
            ("slower than 95% of them", C::Yellow)
        } else if request.ms <= usual.p50 {
            ("faster than half of them", C::Green)
        } else {
            ("as usual", C::DarkGray)
        };
        lines.push(vec![
            seg(format!("{} requests · p50 {} · p95 {} ms · ", usual.count, ms(usual.p50), ms(usual.p95)), C::DarkGray),
            seg(verdict, color),
        ]);
    }
    lines.push(Vec::new());
    lines.push(vec![seg("sent      ", C::DarkGray), seg(body(&request.sent_content, &request.sent_length), C::Gray)]);
    lines.push(vec![seg("returned  ", C::DarkGray), seg(body(&request.content, &request.length), C::Gray)]);
    lines.push(Vec::new());
    let Some((first, last)) = request.logs else {
        let why = "Its log lines aren't known: the app logs no \"Request starting\" line for it (Serilog's request log \
                   says only when one finished).";
        lines.extend(words(why, width).into_iter().map(|line| vec![seg(line, C::DarkGray)]));
        return lines;
    };
    let logged: Vec<_> =
        controller.state(&request.app).map_or_else(Vec::new, |state| state.output.numbered(first, last).collect());
    let expected = (last + 1).saturating_sub(first) as usize;
    let errors = logged.iter().filter(|line| line_color(line) == C::Red).count();
    lines.push(vec![
        seg("Log ", C::White),
        seg(format!(" {expected} lines"), C::Gray),
        seg(format!(" · {errors} errors"), if errors > 0 { C::Red } else { C::DarkGray }),
    ]);
    if let Some(db) = &request.db {
        let share = if request.ms > 0.0 {
            format!(" · {:.0}% of its time", db.total_ms / request.ms * 100.0)
        } else {
            String::new()
        };
        let (commands, total, slowest) = (db.commands, ms(db.total_ms), ms(db.slowest_ms));
        let text = format!("DB {commands} commands · {total} ms{share} · slowest {slowest} ms");
        lines.extend(words(&text, width).into_iter().map(|line| vec![seg(line, C::Cyan)]));
        if let Some(repeated) = &db.repeated {
            let text = format!(
                "N+1? The same query ran {} times ({} ms) - a loop loading one row at a time; Include() or one query \
                 for all of them fixes it: {}",
                repeated.count,
                ms(repeated.total_ms),
                cut(&repeated.sql, N_PLUS_ONE_SQL)
            );
            lines.extend(words(&text, width).into_iter().map(|line| vec![seg(line, C::Yellow)]));
        }
    }
    if request.mixed {
        let text = "Possibly mixed: other requests of the app ran meanwhile, some lines may be theirs.";
        lines.extend(words(text, width).into_iter().map(|line| vec![seg(line, C::Yellow)]));
    }
    if expected == 0 {
        lines.push(vec![seg("It logged nothing.", C::DarkGray)]);
        return lines;
    }
    if logged.len() < expected {
        lines.push(vec![seg(format!("{} older lines aren't kept any more.", expected - logged.len()), C::DarkGray)]);
    }
    let wrapped: Vec<Vec<Vec<Seg>>> = logged
        .iter()
        .map(|line| {
            let text = line.text.strip_prefix("      ").map_or_else(|| line.text.clone(), |rest| format!("  {rest}"));
            wrap(&text, width, LOG_WRAP).into_iter().map(|row| vec![seg(row, line_color(line))]).collect()
        })
        .collect();
    // The newest lines that fit, below a note of how many more there are.
    let room = height.saturating_sub(lines.len()).max(1);
    let mut kept = wrapped.len();
    if wrapped.iter().map(Vec::len).sum::<usize>() > room {
        let mut used = 1;
        kept = wrapped
            .iter()
            .rev()
            .take_while(|rows| (used + rows.len() <= room).then(|| used += rows.len()).is_some())
            .count();
        lines.push(vec![seg(format!("… {} more lines above (o opens the log)", wrapped.len() - kept), C::DarkGray)]);
    }
    let skip = wrapped.len() - kept;
    lines.extend(wrapped.into_iter().skip(skip).flatten());
    lines
}

/// The selected endpoint: its times, a sparkline of the latest, its statuses, its slowest and latest failing requests.
fn endpoint_details(
    view: &View,
    controller: &Controller,
    endpoint: Option<&Endpoint>,
    width: usize,
    height: usize,
) -> Vec<Vec<Seg>> {
    let Some(endpoint) = endpoint else { return vec![vec![seg("No endpoint selected.", C::DarkGray)]] };
    let all = shown(&View { sort: 0, ..view.clone() }, controller);
    let requests: Vec<&Request> = all
        .into_iter()
        .filter(|request| {
            request.app == endpoint.app && request.method == endpoint.method && request.endpoint == endpoint.endpoint
        })
        .collect();
    let mut lines: Vec<Vec<Seg>> = wrap(&format!("{} {}", endpoint.method, endpoint.endpoint), width, 2)
        .into_iter()
        .map(|line| vec![seg(line, C::White)])
        .collect();
    let share = endpoint.errors * 100 / endpoint.count.max(1);
    lines.push(vec![
        seg(format!("{} · {} requests · ", app_name(controller, &endpoint.app), endpoint.count), C::DarkGray),
        seg(format!("{} errors ({share}%)", endpoint.errors), if endpoint.errors > 0 { C::Red } else { C::DarkGray }),
    ]);
    let mut times = vec![seg("avg", C::DarkGray)];
    for (label, value) in
        [("", endpoint.avg), (" · p50", endpoint.p50), (" · p95", endpoint.p95), (" · max", endpoint.max)]
    {
        times.push(seg(format!("{label} "), C::DarkGray));
        times.push(seg(ms(value), ms_color(value)));
    }
    times.push(seg(" ms", C::DarkGray));
    lines.push(times);
    let latest: Vec<f64> = requests.iter().take(width.min(SPARK_MAX)).rev().map(|request| request.ms).collect();
    let top = latest.iter().copied().fold(1.0, f64::max);
    let spark: String = latest.iter().map(|ms| SPARKS[((ms / top * 7.999).floor() as usize).min(7)]).collect();
    lines.push(vec![seg(spark, C::Cyan), seg("  latest", C::DarkGray)]);
    let mut statuses: Vec<(u16, usize)> = Vec::new();
    for request in &requests {
        match statuses.iter_mut().find(|(status, _)| *status == request.status) {
            Some((_, count)) => *count += 1,
            None => statuses.push((request.status, 1)),
        }
    }
    statuses.sort_unstable();
    let mut row = vec![seg("statuses", C::DarkGray)];
    row.extend(statuses.iter().map(|(status, count)| seg(format!("  {status} ×{count}"), status_color(*status))));
    lines.push(row);
    if endpoint.n_plus_one > 0 {
        let text = format!(
            "N+1 in {} of its {} requests: the same query over and over (a request's details show it)",
            endpoint.n_plus_one, endpoint.count
        );
        lines.extend(words(&text, width).into_iter().map(|line| vec![seg(line, C::Yellow)]));
    }
    let listed = |request: &Request, first: Seg| {
        let at = request.at.format("%H:%M:%S");
        let path = cut(&request.path, width.saturating_sub(20));
        vec![seg("  ", C::Gray), first, seg(format!("  {at}  "), C::DarkGray), seg(path, C::Gray)]
    };
    let mut slowest = requests.clone();
    slowest.sort_by(|a, b| b.ms.total_cmp(&a.ms));
    lines.push(Vec::new());
    lines.push(vec![seg("Slowest", C::White)]);
    for request in slowest.iter().take(LISTED) {
        lines.push(listed(request, seg(format!("{:>7} ms", ms(request.ms)), ms_color(request.ms))));
    }
    let failing: Vec<&&Request> = requests.iter().filter(|request| request.status >= 400).take(LISTED).collect();
    if !failing.is_empty() {
        lines.push(Vec::new());
        lines.push(vec![seg("Latest failing", C::White)]);
        for request in failing {
            lines.push(listed(request, seg(format!("{:>7}   ", request.status), status_color(request.status))));
        }
    }
    lines.truncate(height);
    lines
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
    if let Some((app, method, endpoint)) = &view.endpoint {
        filters.push(format!("{method} {endpoint} · {}", app_name(controller, app)));
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
    let text = match (view.tab, view.endpoint.is_some()) {
        (Tab::Endpoints, _) => format!(
            " Tab requests/endpoints · j k choose · l its requests · Enter details · e errors only · a app · \
             s sort ({sort}) · / filter · y copy · o log · c clear · Esc back"
        ),
        (Tab::Requests, filtered) => format!(
            " {} · j k choose · Enter details · e errors only · a app · s sort ({sort}) · / filter · y copy · \
             w open · o log · c clear · Esc back",
            if filtered { "h endpoints" } else { "Tab requests/endpoints" }
        ),
    };
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
        if controller.requests.items.is_empty() {
            out.extend(silent_apps(controller));
        }
    }
    let top = view.index.saturating_sub(height / 2).min(list.len().saturating_sub(height));
    for (index, request) in list.iter().enumerate().skip(top).take(height) {
        let time = request.at.format("%H:%M:%S").to_string();
        let app = pad(&cut(&app_name(controller, &request.app), APP_CELLS), APP_CELLS);
        let n_plus_one = request.db.as_ref().is_some_and(|db| db.repeated.is_some());
        let tag = if n_plus_one { "  N+1" } else { "" };
        let path = cut(&request.path, path_cells.saturating_sub(tag.len()));
        let (method, status, took) = (&request.method, request.status, ms(request.ms));
        let row = if index == view.index {
            let text = format!(" {time:<8}  {app}  {method:<6} {status:>6} {took:>7}  {path}{tag}");
            vec![on(pad(&text, width), C::FocusText, C::FocusBack)]
        } else {
            vec![
                seg(format!(" {time:<8}  {app}  {method:<6}"), C::DarkGray),
                seg(format!(" {status:>6}"), status_color(status)),
                seg(format!(" {took:>7}"), ms_color(request.ms)),
                seg(format!("  {path}"), C::Gray),
                seg(tag, C::Yellow),
            ]
        };
        out.push(aim(row, Target::Item(index)));
    }
    out
}

/// The .NET apps up but without ASP.NET's own log in their output ("Now listening on"): they log somewhere else
/// (Serilog to Seq, say), so their requests can't show here - and what to do about it.
fn silent_apps(controller: &Controller) -> Vec<Vec<Seg>> {
    let silent: Vec<&str> = controller
        .apps
        .iter()
        .zip(&controller.states)
        .filter(|(app, state)| {
            app.traits().builds
                && state.is_up()
                && !state.output.newest_first(0).any(|line| line.text.contains("Now listening on"))
        })
        .map(|(app, _)| app.name.as_str())
        .collect();
    if silent.is_empty() {
        return Vec::new();
    }
    let (names, print) =
        if silent.len() == 1 { (silent[0].to_string(), "prints") } else { (silent.join(", "), "print") };
    vec![
        Vec::new(),
        vec![seg(
            format!("  {names} {print} no ASP.NET log to the console: it logs somewhere else (Serilog to Seq, say),"),
            C::Yellow,
        )],
        vec![seg(
            "  so its requests can't show here. \"args\" in its profile can switch that off for local runs,",
            C::Yellow,
        )],
        vec![seg("  e.g. \"args\": [\"--Seq:ApiKey=\"] - README, Requests.", C::Yellow)],
    ]
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
