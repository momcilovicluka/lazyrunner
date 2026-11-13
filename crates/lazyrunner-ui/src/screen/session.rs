//! The screen kept with the session (#85): the focused and zoomed column, the open screen - on the app it looked at,
//! with its filters - each column's search and `Alt+d` mode. It comes back once the startup checklist closes; the
//! resumed columns fade in from the background one after another as their apps start (not with reduced motion).

use std::time::{Duration, Instant};

use lazyrunner_core::run::Controller;
use serde_json::{Value, json};

use super::screens::{self, Screen};
use super::seg::Seg;
use super::stack::Tab;
use super::theme::C;
use super::{Overlay, Search, Ui, diff, requests};

/// How long a resumed column takes to fade in, and how long after the one before it starts.
const FADE: Duration = Duration::from_millis(450);
const STAGGER: Duration = Duration::from_millis(150);
const STACK_TABS: [(Tab, &str); 3] = [(Tab::Map, "map"), (Tab::Code, "code"), (Tab::Health, "health")];

/// What the screen shows now, to keep with the session.
pub fn snapshot(ui: &Ui, controller: &Controller) -> Value {
    let key_of = |index: usize| controller.apps.get(index).map(|app| app.key.clone());
    let screen = screens::current(ui).filter(|screen| *screen != Screen::Database);
    let changes: Vec<&str> =
        controller.apps.iter().filter(|app| diff::is_on(ui, &app.key)).map(|app| app.key.as_str()).collect();
    let tab = STACK_TABS.iter().find(|(tab, _)| *tab == ui.stack_tab).map(|(_, name)| *name);
    let mut view = json!({
        "focus": key_of(ui.focus),
        "zoom": ui.zoom,
        "changes": changes,
        "screen": screen.map(Screen::key),
        "app": screen.and_then(|_| screens::looked_at(ui, controller)).and_then(key_of),
        "stackTab": tab,
    });
    if let Some(search) = ui.search.as_ref().filter(|search| !search.query.is_empty()) {
        view["search"] = json!({
            "app": search.key, "query": search.query, "regex": search.regex, "case": search.case, "filter": search.filter,
        });
    }
    match &ui.overlay {
        Some(Overlay::Requests(open)) => {
            view["requests"] = json!({
                "endpoints": open.tab == requests::Tab::Endpoints,
                "filter": open.filter,
                "errorsOnly": open.errors_only,
                "endpoint": open.endpoint,
            });
        }
        Some(Overlay::Tests { .. }) => view["tests"] = json!({ "query": ui.tests_query }),
        Some(Overlay::Audit { filter, only, .. }) => view["audit"] = json!({ "filter": filter, "only": only }),
        _ => {}
    }
    view
}

/// The session's screen back, once the start is under way; its apps fade in as they run.
pub fn restore(ui: &mut Ui, controller: &mut Controller) {
    let Some(snapshot) = controller.resume.take() else { return };
    let view = snapshot.view;
    let keys: Vec<String> = controller.apps.iter().map(|app| app.key.clone()).collect();
    let place = |key: &str| keys.iter().position(|each| each == key);
    if let Some(at) = view["focus"].as_str().and_then(place) {
        ui.focus = at;
    }
    ui.zoom = view["zoom"].as_bool().unwrap_or(false);
    let changes: Vec<usize> =
        view["changes"].as_array().into_iter().flatten().filter_map(Value::as_str).filter_map(place).collect();
    for at in changes {
        diff::turn_on(ui, controller, at);
    }
    let search = &view["search"];
    if let Some(key) = search["app"].as_str().filter(|key| place(key).is_some()) {
        let mut found = Search {
            key: key.to_string(),
            query: search["query"].as_str().unwrap_or_default().to_string(),
            regex: search["regex"].as_bool().unwrap_or(false),
            case: search["case"].as_bool().unwrap_or(false),
            filter: search["filter"].as_bool().unwrap_or(false),
            ..Search::default()
        };
        found.refresh();
        ui.search = Some(found);
    }
    if let Some(tab) = STACK_TABS.iter().find(|(_, name)| view["stackTab"].as_str() == Some(name)) {
        ui.stack_tab = tab.0;
    }
    if let Some(screen) = view["screen"].as_str().and_then(Screen::by_key) {
        let app = view["app"].as_str().and_then(place);
        screens::open_on(ui, controller, screen, app);
        filters(ui, &view);
    }
    if super::stack::moving(ui) {
        ui.restoring = snapshot.running.into_iter().filter(|key| place(key).is_some()).collect();
    }
}

/// The open screen's filters as they were.
fn filters(ui: &mut Ui, view: &Value) {
    let text = |value: &Value| value.as_str().unwrap_or_default().to_string();
    match &mut ui.overlay {
        Some(Overlay::Requests(open)) => {
            let kept = &view["requests"];
            if kept["endpoints"].as_bool().unwrap_or(false) {
                open.tab = requests::Tab::Endpoints;
            }
            open.filter = text(&kept["filter"]);
            open.errors_only = kept["errorsOnly"].as_bool().unwrap_or(false);
            open.endpoint = serde_json::from_value(kept["endpoint"].clone()).ok().flatten();
        }
        Some(Overlay::Tests { .. }) => ui.tests_query = text(&view["tests"]["query"]),
        Some(Overlay::Audit { filter, only, .. }) => {
            *filter = text(&view["audit"]["filter"]);
            *only = view["audit"]["only"].as_bool().unwrap_or(false);
        }
        _ => {}
    }
}

/// A resumed app that runs now starts its fade, a `STAGGER` after the one before.
pub fn step(ui: &mut Ui, controller: &Controller) {
    if ui.restoring.is_empty() && ui.fades.is_empty() {
        return;
    }
    let now = ui.now;
    ui.fades.retain(|_, at| now.saturating_duration_since(*at) < FADE);
    let running: Vec<String> = ui.restoring.iter().filter(|key| controller.is_running(key)).cloned().collect();
    for key in running {
        let last = ui.fades.values().max().copied();
        let at = last.map_or(now, |last| (last + STAGGER).max(now));
        ui.fades.insert(key.clone(), at);
        ui.restoring.retain(|waiting| *waiting != key);
    }
}

/// Whether a resumed column is fading in.
pub fn animating(ui: &Ui) -> bool {
    !ui.fades.is_empty()
}

/// A resumed column's rows as far as it has faded in: dim at first, then its colours' darker shades, then its own.
pub fn faded(ui: &Ui, key: &str, rows: Vec<Vec<Seg>>) -> Vec<Vec<Seg>> {
    let Some(at) = ui.fades.get(key) else { return rows };
    let shade: fn(C) -> C = match fade_share(ui.now, *at) {
        share if share < 1.0 / 3.0 => |_| C::DarkGray,
        share if share < 2.0 / 3.0 => darker,
        _ => return rows,
    };
    rows.into_iter().map(|row| row.into_iter().map(|part| Seg { fg: shade(part.fg), ..part }).collect()).collect()
}

/// How far a fade that starts `at` has come (none yet before it starts).
fn fade_share(now: Instant, at: Instant) -> f64 {
    now.checked_duration_since(at).map_or(0.0, |since| since.as_secs_f64() / FADE.as_secs_f64())
}

fn darker(color: C) -> C {
    match color {
        C::White => C::Gray,
        C::Gray => C::DarkGray,
        C::Red => C::DarkRed,
        C::Green => C::DarkGreen,
        C::Yellow => C::DarkYellow,
        C::Blue => C::DarkBlue,
        C::Magenta => C::DarkMagenta,
        C::Cyan => C::DarkCyan,
        other => other,
    }
}
