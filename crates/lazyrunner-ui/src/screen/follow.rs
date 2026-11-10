//! Follow mode (`F`): the screen follows an agent working over MCP - the column, the lines, the requests, the suite,
//! the migrations, the stack or code map, the ports, the audit, the durations, the timeline, the start sheet or the
//! marks it reads come into view, what it read outlined for a few seconds - while your keys rest (all but `F` and
//! `Ctrl+C`; the mouse too). `F` again gives them back and puts the screen back as it was; the places the agent took
//! you stay in the jump list (`Ctrl+O`); `F` once more follows again, on the screen the agent is at. With
//! `followAgent` (`, F`, on by default) an agent's call starts it - not again while that agent works, once you've
//! taken the keys back - and while the terminal is in the background, a notification says an agent is at work.

use std::collections::{HashSet, VecDeque};
use std::time::{Duration, Instant};

use lazyrunner_core::config::Toggle;
use lazyrunner_core::output::Hue;
use lazyrunner_core::run::Controller;
use serde_json::Value;

use super::actions::Run;
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
    /// The agent's last call - its op, arguments and answer: `F` follows again on its screen.
    last: Option<(String, Value, Value)>,
    /// A notification said an agent is at work while the terminal was in the background (again once it's been seen).
    pub notified: bool,
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
        if place.view.is_none() && !matches!(ui.overlay, Some(Overlay::Checklist(_))) {
            ui.overlay = None; // a view the jump list doesn't keep (the stack map, the durations...)
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
    // Where the agent is: its last call's screen.
    if let Some((op, args, result)) = ui.follow.last.clone() {
        let before = jumps::before(ui, controller);
        let said = show(ui, controller, &op, &args, &result);
        jumps::noticed(ui, controller, before);
        if let Some(said) = said {
            remember(ui, said);
        }
    }
}

/// A call in words, in the ticker.
fn remember(ui: &mut Ui, said: String) {
    ui.follow.ticker.push_back(said);
    while ui.follow.ticker.len() > TICKER {
        ui.follow.ticker.pop_front();
    }
}

/// While the terminal is in the background (it said so), the first call of an agent at work says so in a
/// notification - again once you've been back.
fn notify_away(ui: &mut Ui, controller: &mut Controller, client: &str, what: String) {
    if ui.terminal_focused != Some(false) || ui.follow.notified || !controller.setting(Toggle::FollowAgent) {
        return;
    }
    ui.follow.notified = true;
    controller.notifications.push((format!("{client} is working in lazyrunner"), what));
}

/// An agent's call, answered: following, the screen goes where it looked.
pub fn agent_call(ui: &mut Ui, controller: &mut Controller, op: &str, args: &Value, result: &Value, client: &str) {
    let now = ui.now;
    if ui.follow.last_call.is_none_or(|at| now.saturating_duration_since(at) > QUIET) {
        ui.follow.declined = false;
        ui.follow.notified = false;
    }
    ui.follow.last_call = Some(now);
    ui.follow.client = client.to_string();
    ui.follow.last = Some((op.to_string(), args.clone(), result.clone()));
    let what = result["doing"].as_str().map_or_else(|| op.replace('_', " "), str::to_string);
    if !ui.follow.on && !ui.follow.declined && controller.setting(Toggle::FollowAgent) && jumps::can_travel(ui) {
        start(ui, controller); // on this call's screen
        let said = ui.follow.ticker.back().cloned().unwrap_or(what);
        notify_away(ui, controller, client, said);
        return;
    }
    if !ui.follow.on {
        notify_away(ui, controller, client, what);
        return;
    }
    let before = jumps::before(ui, controller);
    let said = show(ui, controller, op, args, result);
    jumps::noticed(ui, controller, before);
    notify_away(ui, controller, client, said.clone().unwrap_or(what));
    if let Some(said) = said {
        remember(ui, said);
    }
}

/// Opens the view `kind` with its action - unless it's open already (an action that toggles would close it).
fn open_view(ui: &mut Ui, controller: &mut Controller, kind: &str, run: Run) {
    if ui.overlay.as_ref().map(|overlay| overlay.kind()) != Some(kind) {
        ui.overlay = None;
        super::keys::run(ui, controller, &run);
    }
}

/// Brings what the agent read into view; what it did, in words.
fn show(ui: &mut Ui, controller: &mut Controller, op: &str, args: &Value, result: &Value) -> Option<String> {
    match op {
        "logs" => {
            let app = app_of(controller, &result["app"])?;
            // Each line after its number and a space.
            let lines =
                result["text"].as_str()?.lines().map(|line| line.split_once(' ').map_or(line, |(_, text)| text));
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
        "status" => {
            close_view(ui);
            Some("looked at the status".to_string())
        }
        "audit" => {
            open_view(ui, controller, "audit", Run::Audit);
            Some("read the audit".to_string())
        }
        "ports" => {
            open_view(ui, controller, "ports", Run::Ports);
            Some("read the port map".to_string())
        }
        "stack" => {
            open_view(ui, controller, "stack", Run::Stack);
            Some("read the stack map".to_string())
        }
        "dependencies" => {
            let app = app_of(controller, &args["part"]).map(|app| controller.apps[app].key.clone());
            ui.overlay = None;
            super::code::open(ui, controller, app);
            let part = args["part"].as_str().map_or(String::new(), |part| format!(" for {part}"));
            Some(format!("read the code map{part}"))
        }
        "durations" => {
            open_view(ui, controller, "durations", Run::Durations);
            Some("read the durations".to_string())
        }
        "timeline" => {
            open_view(ui, controller, "timeline", Run::Timeline);
            Some(format!("read the timeline ({} lines)", result["shown"].as_u64().unwrap_or_default()))
        }
        "sheet" => {
            let app = app_of(controller, &result["app"])?;
            if ui.focus != app {
                ui.overlay = None;
                ui.focus = app;
            }
            open_view(ui, controller, "sheet", Run::Sheet);
            Some(format!("read {}'s start sheet", controller.apps[app].name))
        }
        "checks" => Some(if result["running"].as_bool() == Some(true) {
            "runs the startup checks".to_string()
        } else {
            "read the startup checks".to_string()
        }),
        "marks" => {
            open_view(ui, controller, "marks", Run::Marks);
            Some(format!("read your marks ({})", result["marks"].as_array().map_or(0, Vec::len)))
        }
        "changelog" => {
            open_view(ui, controller, "changelog", Run::Changelog);
            Some("read the changelog".to_string())
        }
        _ => result["doing"].as_str().map(str::to_string),
    }
}

fn app_of(controller: &Controller, wanted: &Value) -> Option<usize> {
    let wanted = wanted.as_str()?;
    controller.apps.iter().position(|app| app.key.eq_ignore_ascii_case(wanted) || app.name.eq_ignore_ascii_case(wanted))
}

/// Back to the columns: any view the agent took you to closes (the checklist finishes by itself).
fn close_view(ui: &mut Ui) {
    if !matches!(ui.overlay, Some(Overlay::Checklist(_))) {
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
