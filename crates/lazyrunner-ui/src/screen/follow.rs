//! Follow mode (`F`): the screen follows an agent working over MCP - the column, the lines, the requests, the suite
//! or the migrations it reads come into view, what it read outlined for a few seconds - while your keys rest (all but
//! `F` and `Ctrl+C`; the mouse too). `F` again gives them back and puts the screen back as it was; the places the agent
//! took you stay in the jump list (`Ctrl+O`). With `followAgent` (`, F`) an agent's call starts it - not again while
//! that agent works, once you've taken the keys back.

use std::collections::{HashSet, VecDeque};
use std::time::{Duration, Instant};

use lazyrunner_core::config::Toggle;
use lazyrunner_core::output::Hue;
use lazyrunner_core::run::Controller;
use serde_json::Value;

use super::jumps::{self, Place};
use super::motion::Spot;
use super::requests::{self, Tab};
use super::seg::{Seg, on, seg};
use super::suites::{self, Row};
use super::theme::C;
use super::{Overlay, Ui, columns};

/// The agent's calls the panel keeps in words.
const TICKER: usize = 4;
/// An agent quiet this long is done: `followAgent` follows the next call again.
const QUIET: Duration = Duration::from_secs(120);
const KEYS_BACK: &str = "  your keys back   ";

#[derive(Debug, Default)]
pub struct Follow {
    pub on: bool,
    /// The agent's name, from its last call.
    pub client: String,
    /// Its last calls in words, oldest first.
    pub ticker: VecDeque<String>,
    /// The screen when following began: where you were, each column's scroll, the zoom.
    from: Option<(Place, Vec<usize>, bool)>,
    /// The keys were taken back while the agent worked: `followAgent` waits for it to go quiet.
    declined: bool,
    last_call: Option<Instant>,
}

/// F: follow the agent, or take the keys back - the screen back as it was.
pub fn toggle(ui: &mut Ui, controller: &mut Controller) {
    if !ui.follow.on {
        start(ui, controller);
        return;
    }
    ui.follow.on = false;
    ui.follow.declined = true;
    if let Some((place, scrolls, zoom)) = ui.follow.from.take() {
        for (state, scroll) in controller.states.iter_mut().zip(scrolls) {
            state.scroll = scroll;
        }
        jumps::go(ui, controller, &place);
        ui.zoom = zoom;
    }
    controller.add_log("Your keys again - Ctrl+O walks back through where the agent took you.", Hue::DarkGray);
}

fn start(ui: &mut Ui, controller: &mut Controller) {
    ui.follow.on = true;
    ui.follow.declined = false;
    let scrolls = controller.states.iter().map(|state| state.scroll).collect();
    ui.follow.from = Some((jumps::here(ui, controller), scrolls, ui.zoom));
    controller
        .add_log("Following the agent: the screen shows what it reads - F gives you the keys back.", Hue::Magenta);
}

/// An agent's call, answered: following, the screen goes where it looked.
pub fn agent_call(ui: &mut Ui, controller: &mut Controller, op: &str, args: &Value, result: &Value, client: &str) {
    let now = ui.now;
    if ui.follow.last_call.is_none_or(|at| now.saturating_duration_since(at) > QUIET) {
        ui.follow.declined = false;
    }
    ui.follow.last_call = Some(now);
    if !ui.follow.on && !ui.follow.declined && controller.setting(Toggle::FollowAgent) && jumps::can_travel(ui) {
        start(ui, controller);
    }
    if !ui.follow.on {
        return;
    }
    ui.follow.client = client.to_string();
    let before = jumps::before(ui, controller);
    let said = show(ui, controller, op, args, result);
    jumps::noticed(ui, controller, before);
    if let Some(said) = said {
        ui.follow.ticker.push_back(said);
        while ui.follow.ticker.len() > TICKER {
            ui.follow.ticker.pop_front();
        }
    }
}

/// Brings what the agent read into view; what it did, in words.
fn show(ui: &mut Ui, controller: &mut Controller, op: &str, args: &Value, result: &Value) -> Option<String> {
    match op {
        "logs" => {
            let app = app_of(controller, &result["app"])?;
            let lines = result["lines"].as_array()?.iter().filter_map(|line| line["text"].as_str());
            look_at(ui, controller, app, lines.map(str::to_string).collect());
            let grep = args["grep"].as_str().map_or(String::new(), |grep| format!(" matching \"{grep}\""));
            Some(format!("read {}'s output{grep}", controller.apps[app].name))
        }
        "errors" => {
            let app = app_of(controller, &result["app"])?;
            look_at(ui, controller, app, result["text"].as_str()?.lines().map(str::to_string).collect());
            let what = match args["what"].as_str() {
                Some("errors") => "errors",
                Some("warnings") => "warnings",
                Some("both") => "errors and warnings",
                _ => "last error",
            };
            Some(format!("read {}'s {what}", controller.apps[app].name))
        }
        "requests" => {
            requests_view(ui, controller, args, result);
            Some(format!("read the requests ({} matching)", result["matching"].as_u64().unwrap_or_default()))
        }
        "request" => {
            let id = args["id"].as_u64()?;
            let view = requests::View { follow: false, selected: id, details: true, ..Default::default() };
            ui.overlay = Some(Overlay::Requests(Box::new(view)));
            ui.motion.look(Spot::LookedRequests(HashSet::from([id])), ui.now);
            Some(format!("opened request {id}"))
        }
        "tests" | "run_tests" | "stop_tests" => {
            let suite = args["suite"].as_str();
            tests_view(ui, controller, suite);
            let read = suite.map_or("read the test suites".to_string(), |suite| format!("read {suite}'s results"));
            Some(result["doing"].as_str().map_or(read, str::to_string))
        }
        "migrations" | "check_migrations" => {
            if !matches!(ui.overlay, Some(Overlay::Migrations { .. })) {
                ui.overlay = Some(Overlay::Migrations { index: 0, picking: None });
            }
            Some(result["doing"].as_str().map_or("read the migrations".to_string(), str::to_string))
        }
        "start" | "stop" | "restart" | "build" => {
            close_view(ui);
            match app_of(controller, &args["app"]) {
                Some(app) => {
                    ui.focus = app;
                    ui.motion.flash(Spot::Title(app), ui.now);
                }
                None => ui.motion.flash(Spot::Titles, ui.now),
            }
            result["doing"].as_str().map(str::to_string)
        }
        "status" => Some("looked at the status".to_string()),
        _ => result["doing"].as_str().map(str::to_string),
    }
}

fn app_of(controller: &Controller, wanted: &Value) -> Option<usize> {
    let wanted = wanted.as_str()?;
    controller.apps.iter().position(|app| app.key.eq_ignore_ascii_case(wanted) || app.name.eq_ignore_ascii_case(wanted))
}

fn close_view(ui: &mut Ui) {
    if matches!(ui.overlay, Some(Overlay::Requests(_) | Overlay::Tests { .. } | Overlay::Migrations { .. })) {
        ui.overlay = None;
    }
}

/// The app's column, the newest line read at its bottom, the lines read outlined.
fn look_at(ui: &mut Ui, controller: &mut Controller, app: usize, lines: HashSet<String>) {
    close_view(ui);
    ui.focus = app;
    let shown = columns::shown_lines(&controller.states[app], ui.search.as_ref());
    let scroll = shown.iter().rposition(|item| lines.contains(&item.line.text)).map_or(0, |at| shown.len() - 1 - at);
    controller.states[app].scroll = scroll;
    ui.motion.look(Spot::Looked { app, lines }, ui.now);
}

/// The requests view with the agent's filters, the requests it got outlined.
fn requests_view(ui: &mut Ui, controller: &Controller, args: &Value, result: &Value) {
    let view = requests::View {
        app: app_of(controller, &args["app"]).map(|app| controller.apps[app].key.clone()),
        errors_only: args["errors_only"].as_bool().unwrap_or(false),
        sort: usize::from(args["sort"].as_str() == Some("slowest")),
        filter: args["path"].as_str().unwrap_or_default().to_string(),
        tab: if args["endpoints"].as_bool().unwrap_or(false) { Tab::Endpoints } else { Tab::Requests },
        ..Default::default()
    };
    ui.overlay = Some(Overlay::Requests(Box::new(view)));
    let rows = result["requests"].as_array().into_iter().flatten();
    ui.motion.look(Spot::LookedRequests(rows.filter_map(|row| row["id"].as_u64()).collect()), ui.now);
}

/// The tests view on the suite asked about, else on the first.
fn tests_view(ui: &mut Ui, controller: &Controller, suite: Option<&str>) {
    let row = suite.and_then(|wanted| {
        let suites = controller.suites.as_deref()?;
        let at = suites
            .iter()
            .position(|suite| suite.key.eq_ignore_ascii_case(wanted) || suite.name.eq_ignore_ascii_case(wanted))?;
        suites::list_rows(controller).iter().position(|row| matches!(row, Row::Suite(suite) if *suite == at))
    });
    ui.overlay = Some(Overlay::Tests { index: row.unwrap_or(suites::FIRST_SUITE), scroll: 0, picking: None });
}

/// The panel's keys row while following: F, and the agent's last calls in words, the newest on the right.
pub fn ticker(ui: &Ui, cells: usize) -> Vec<Seg> {
    let said = ui.follow.ticker.iter().map(String::as_str).collect::<Vec<_>>().join(" · ");
    let room = cells.saturating_sub(3 + KEYS_BACK.len());
    let count = said.chars().count();
    let said =
        if count > room { format!("…{}", said.chars().skip(count + 1 - room).collect::<String>()) } else { said };
    vec![on(" F ", C::Black, C::Magenta), seg(KEYS_BACK, C::DarkGray), seg(said, C::Magenta)]
}
