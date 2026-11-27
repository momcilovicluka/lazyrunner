//! Pulling on the database screen (#110): `p` pulls a copy from a source (picked from a card when there are several),
//! `P` again from the last one, `x` cancels, `,` changes or forgets a source's password; `Tab` gives the pull graph
//! (`pull_graph`) the keys, where `h` `l` pick a step and `Enter` opens its whole output. A source's password is asked
//! for on a small centred card - a lock, the source's server, each character a `•` with a soft pop - and kept in the
//! OS's credential store: the card says so with one glint, then folds away. Once a pull is done, `keepPulled`'s offer
//! to drop the older copies comes as a y/n box, and the new database's row glints.

use std::time::{Duration, Instant};

use crossterm::event::KeyEvent;
use lazyrunner_core::output::Hue;
use lazyrunner_core::profile::sources::{DatabaseSource, Reach};
use lazyrunner_core::run::Controller;
use lazyrunner_core::run::controller::save_points::db_key;
use lazyrunner_core::run::credentials::Password;
use lazyrunner_core::run::pull::Step;

use super::icons::{self, Icon};
use super::migrations::{Confirm, Risk, SavePointYes};
use super::mouse::Target;
use super::overlays::boxed_in;
use super::seg::{Seg, seg, sweep};
use super::theme::C;
use super::{Overlay, Ui};

/// A typed character pops this long; the card says where the password went this long, then folds this long.
const POP: Duration = Duration::from_millis(120);
const SAVED: Duration = Duration::from_millis(900);
const FOLD: Duration = Duration::from_millis(150);
const GLINT: Duration = super::motion::SWEEP;
/// The card's text at most this wide.
const CARD_INNER: usize = 56;
/// A password is at most this long.
const PASSWORD_MOST: usize = 256;

/// The pull's part of the database screen: whether the graph has the keys, the step picked (`h` `l`), whether its
/// output shows (and how far back), the failed pull put away, the pull whose done is handled, and the dots.
#[derive(Debug, Default)]
pub struct View {
    pub focused: bool,
    pub picked: Option<usize>,
    pub output: bool,
    pub scroll: usize,
    pub dismissed: Option<u64>,
    handled: Option<u64>,
    pub dots: super::pull_graph::Dots,
}

/// What a source is picked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Purpose {
    Pull,
    Password,
}

/// The card: a source to pick, or its password being typed (the time of each character, when it was kept).
#[derive(Debug)]
pub enum Card {
    Choose {
        purpose: Purpose,
        index: usize,
    },
    Password {
        source: String,
        purpose: Purpose,
        typed: String,
        last: Option<Instant>,
        saved: Option<(Instant, &'static str)>,
    },
}

impl Card {
    /// Whether keys are text in it: the password is being typed.
    pub fn typing(&self) -> bool {
        matches!(self, Self::Password { saved: None, .. })
    }
}

/// Whether the card asks for a password: an agent's keys never reach it.
pub fn asking_password(ui: &Ui) -> bool {
    matches!(&ui.overlay, Some(Overlay::Pull(card)) if matches!(**card, Card::Password { .. }))
}

fn still(ui: &Ui) -> bool {
    ui.reduced_motion || !ui.motion.enabled
}

/// The database screen's pull keys; whether the key was one of them.
pub fn key(ui: &mut Ui, controller: &mut Controller, name: &str) -> bool {
    if ui.pull_view.focused && focused_key(ui, controller, name) {
        return true;
    }
    match name {
        "p" => choose(ui, controller, Purpose::Pull),
        "P" if !controller.pull_sources().is_empty() => match controller.last_pull_source() {
            Some(source) => proceed(ui, controller, Purpose::Pull, &source),
            None => controller.add_log("Nothing pulled yet - p pulls a copy from a source", Hue::Gray),
        },
        "x" if controller.pull.as_ref().is_some_and(|pull| !pull.is_over()) => match controller.cancel_pull() {
            Ok(said) => controller.add_log(format!("■ {}{}", said[..1].to_uppercase(), &said[1..]), Hue::Yellow),
            Err(why) => controller.add_log(format!("✗ {why}"), Hue::Red),
        },
        "," => choose(ui, controller, Purpose::Password),
        _ => return false,
    }
    true
}

/// The keys while the graph has them; whether the key was one of them.
fn focused_key(ui: &mut Ui, controller: &mut Controller, name: &str) -> bool {
    let Some(pull) = controller.pull.as_ref() else {
        ui.pull_view.focused = false;
        return false;
    };
    let at = super::pull_graph::picked(ui, pull).index();
    let view = &mut ui.pull_view;
    match name {
        "h" | "LeftArrow" if !view.output => view.picked = Some(at.saturating_sub(1)),
        "l" | "RightArrow" if !view.output => view.picked = Some((at + 1).min(Step::ALL.len() - 1)),
        "j" | "DownArrow" if view.output => view.scroll = view.scroll.saturating_sub(1),
        "k" | "UpArrow" if view.output => view.scroll += 1,
        "Enter" => (view.output, view.scroll) = (!view.output, 0),
        "Escape" if view.output => view.output = false,
        "Escape" => {
            view.focused = false;
            if pull.is_over() {
                view.dismissed = Some(pull.job);
            }
        }
        _ => return false,
    }
    true
}

/// The screen's keys while the graph has them.
pub fn hint(ui: &Ui) -> Option<&'static str> {
    match (ui.pull_view.focused, ui.pull_view.output) {
        (true, true) => Some(" j k scroll · Enter or Esc back to the steps · x cancel the pull"),
        (true, false) => {
            Some(" h l pick a step · Enter its whole output · x cancel the pull · Tab databases · Esc back")
        }
        _ => None,
    }
}

/// Whether the pull's section can have the keys: there's a pull to show.
pub fn can_focus(controller: &Controller) -> bool {
    controller.pull.is_some()
}

/// A click on a step picks it (the graph gets the keys); a double click opens its output.
pub fn click(ui: &mut Ui, target: Option<Target>, double: bool) -> bool {
    let Some(Target::Step(at)) = target.filter(|_| ui.picker.is_some()) else { return false };
    (ui.save_view.on_rail, ui.files_view.on_files) = (false, false);
    let view = &mut ui.pull_view;
    (view.focused, view.picked) = (true, Some(at));
    if double {
        (view.output, view.scroll) = (true, 0);
    }
    true
}

/// `p` / `,`: the one source, else a card to pick one.
fn choose(ui: &mut Ui, controller: &mut Controller, purpose: Purpose) {
    let sources = controller.pull_sources();
    match sources {
        [] => controller.add_log(
            "No databaseSources in the profile - add one to pull a copy from another environment (projects/README.md)",
            Hue::Gray,
        ),
        [one] => {
            let name = one.name.clone();
            proceed(ui, controller, purpose, &name);
        }
        _ => open(ui, Card::Choose { purpose, index: 0 }),
    }
}

fn open(ui: &mut Ui, card: Card) {
    ui.overlay = Some(Overlay::Pull(Box::new(card)));
}

/// A source picked: its password asked for when one is needed and none is kept, else the pull starts.
fn proceed(ui: &mut Ui, controller: &mut Controller, purpose: Purpose, source: &str) {
    let Some(found) = controller.pull_sources().iter().find(|known| known.name == source).cloned() else {
        controller.add_log(format!("No source {source} in the profile"), Hue::Yellow);
        return;
    };
    ui.overlay = None;
    let password = || Card::Password { source: source.into(), purpose, typed: String::new(), last: None, saved: None };
    if purpose == Purpose::Password {
        if !found.needs_password() {
            controller.add_log(format!("{source} logs in without a password lazyrunner keeps"), Hue::Gray);
            return;
        }
        return open(ui, password());
    }
    if let Some(busy) = controller.database_busy() {
        controller.add_log(format!("lazyrunner is {busy} - pull once it's done"), Hue::Yellow);
        return;
    }
    if found.needs_password() && !controller.password_kept(source) {
        return open(ui, password());
    }
    start(ui, controller, source, None);
}

fn start(ui: &mut Ui, controller: &mut Controller, source: &str, typed: Option<Password>) {
    match controller.pull_database(source, typed) {
        Ok(_) => ui.pull_view = View { dismissed: ui.pull_view.dismissed, ..View::default() },
        Err(why) => controller.add_log(format!("✗ {why}"), Hue::Red),
    }
}

/// The card's keys.
pub fn card_key(ui: &mut Ui, controller: &mut Controller, name: &str, key: &KeyEvent) {
    let Some(Overlay::Pull(card)) = ui.overlay.as_mut() else { return };
    let now = ui.now;
    match card.as_mut() {
        Card::Choose { purpose, index } => {
            let count = controller.pull_sources().len();
            match name {
                "j" | "DownArrow" => *index = (*index + 1) % count.max(1),
                "k" | "UpArrow" => *index = (*index + count.max(1) - 1) % count.max(1),
                "Enter" => {
                    let (purpose, picked) = (*purpose, *index);
                    if let Some(source) = controller.pull_sources().get(picked).map(|source| source.name.clone()) {
                        proceed(ui, controller, purpose, &source);
                    }
                }
                "Escape" | "q" => ui.overlay = None,
                _ => {}
            }
        }
        Card::Password { saved: Some(_), .. } => {}
        Card::Password { source, purpose, typed, last, .. } => match name {
            "Escape" => ui.overlay = None,
            "Backspace" => {
                typed.pop();
            }
            "Delete" if typed.is_empty() && *purpose == Purpose::Password => {
                let source = source.clone();
                ui.overlay = None;
                if let Err(why) = controller.forget_password(&source) {
                    controller.add_log(format!("✗ {why}"), Hue::Red);
                }
            }
            "Enter" if typed.is_empty() => {}
            "Enter" => {
                let (source, purpose) = (source.clone(), *purpose);
                let password = Password::new(std::mem::take(typed));
                submit(ui, controller, &source, purpose, password);
            }
            _ => {
                if let Some(c) = super::keys::typed(key).filter(|_| typed.chars().count() < PASSWORD_MOST) {
                    typed.push(c);
                    *last = Some(now);
                }
            }
        },
    }
}

/// The password typed: kept in the credential store (the card says where, then folds away), and the pull started.
fn submit(ui: &mut Ui, controller: &mut Controller, source: &str, purpose: Purpose, password: Password) {
    let kept = controller.keep_password(source, &password);
    match &kept {
        Ok(store) => {
            if let Some(Overlay::Pull(card)) = ui.overlay.as_mut()
                && let Card::Password { saved, .. } = card.as_mut()
            {
                *saved = Some((ui.now, *store));
            }
        }
        Err(why) => {
            ui.overlay = None;
            controller.add_log(format!("! {why}"), Hue::Yellow);
        }
    }
    if purpose == Purpose::Pull {
        start(ui, controller, source, Some(password));
    }
}

/// Each frame: the card folded away once it has said where the password went; a pull that's done handled once - the
/// new database's row read in and glinting, `keepPulled`'s offer made.
pub fn poll(ui: &mut Ui, controller: &Controller) {
    if let Some(Overlay::Pull(card)) = &ui.overlay
        && let Card::Password { saved: Some((at, _)), .. } = **card
    {
        let lasts = if still(ui) { SAVED } else { SAVED + FOLD };
        if ui.now.saturating_duration_since(at) >= lasts {
            ui.overlay = None;
        }
    }
    let Some(pull) = controller.pull.as_ref().filter(|pull| pull.is_over()) else { return };
    if ui.pull_view.handled == Some(pull.job) || ui.picker.is_none() {
        return;
    }
    ui.pull_view.handled = Some(pull.job);
    if !pull.went_well() {
        return;
    }
    super::database_files::reload(ui, Some((&pull.server, &pull.database)));
    ui.files_view.glint = Some((db_key(&pull.server, &pull.database), ui.now));
    if controller.pull_stale.is_empty() || ui.overlay.is_some() {
        return;
    }
    let names: Vec<&str> = controller.pull_stale.iter().map(|old| old.database.as_str()).collect();
    let keep = controller.keep_pulled().unwrap_or(1);
    let lines = vec![
        format!("keepPulled keeps the newest {keep} pulled copies of {}.", pull.source),
        format!("Drop the older {} ({})?", if names.len() == 1 { "one" } else { "ones" }, names.join(", ")),
        "Their data is gone for good; the one in use is never dropped.".to_string(),
    ];
    let yes = SavePointYes::DropPulled(controller.pull_stale.clone());
    ui.overlay = Some(Overlay::Confirm(Box::new(Confirm {
        lines,
        jobs: Vec::new(),
        risk: Risk::LosesData,
        save_point: Some(yes),
    })));
}

/// Whether the card moves: the cursor while typing, a pop, the glint, the fold.
pub fn animating(ui: &Ui) -> bool {
    let Some(Overlay::Pull(card)) = &ui.overlay else { return false };
    match **card {
        Card::Password { saved: Some(_), .. } => true,
        Card::Password { .. } => !still(ui),
        Card::Choose { .. } => false,
    }
}

/// Where a source is and how it logs in, as the card says it - never a password.
fn reached(source: &DatabaseSource) -> String {
    match &source.reach {
        Reach::Server { server, database, auth, user } => {
            let login = user.as_ref().map_or_else(|| auth.name().to_string(), |user| format!("as {user}"));
            format!("{server} · {database} · {login}")
        }
        Reach::Env(name) => format!("${name}"),
    }
}

/// The card, centred over the database screen.
pub fn card_rows(ui: &Ui, controller: &Controller, width: usize, rows: usize) -> Vec<Vec<Seg>> {
    let Some(Overlay::Pull(card)) = &ui.overlay else { return Vec::new() };
    let lock = icons::icon(ui.nerd, Icon::Lock);
    match card.as_ref() {
        Card::Choose { purpose, index } => {
            let mut lines: Vec<Vec<Seg>> = controller
                .pull_sources()
                .iter()
                .enumerate()
                .map(|(at, source)| {
                    let kept = if !source.needs_password() {
                        ""
                    } else if controller.password_kept(&source.name) {
                        " · password kept"
                    } else {
                        " · asks for its password"
                    };
                    let text =
                        format!(" {} {}  {}{kept}", if at == *index { "▸" } else { " " }, source.name, reached(source));
                    let line = vec![seg(text, if at == *index { C::White } else { C::Gray })];
                    if at == *index { super::seg::focus_row(line, CARD_INNER) } else { line }
                })
                .collect();
            let what = if *purpose == Purpose::Pull { "pulls a copy of it" } else { "changes or forgets its password" };
            lines.extend([Vec::new(), vec![seg(format!(" j k pick · Enter {what} · Esc cancels"), C::DarkGray)]]);
            boxed_in(C::Cyan, &icons::titled(ui.nerd, Icon::Database, "Pull from"), lines, "", width, rows, CARD_INNER)
        }
        Card::Password { source, purpose, typed, last, saved } => {
            let found = controller.pull_sources().iter().find(|known| &known.name == source);
            let place = found.map(reached).unwrap_or_default();
            let mut lines = vec![vec![seg(format!(" {place}"), C::Gray)], Vec::new()];
            lines.push(dots(ui, typed.chars().count(), *last, saved.is_some()));
            lines.push(Vec::new());
            let store = controller.credential_store();
            match saved {
                Some((at, store)) => {
                    let mut said = vec![seg(format!(" ✓ saved in {store}"), C::Green)];
                    let after = ui.now.saturating_duration_since(*at);
                    if !still(ui) && after < GLINT {
                        said = sweep(said, after.as_secs_f64() / GLINT.as_secs_f64(), CARD_INNER, C::Black, C::Green);
                    }
                    lines.push(said);
                }
                None => {
                    lines.push(vec![seg(format!(" Kept in {store}, never in a file or a log"), C::DarkGray)]);
                    let forget = if *purpose == Purpose::Password { " · Del forgets the kept one" } else { "" };
                    let then = if *purpose == Purpose::Pull { "keeps it and pulls" } else { "keeps it" };
                    lines.push(vec![seg(format!(" Enter {then}{forget} · Esc cancels"), C::DarkGray)]);
                }
            }
            let title = format!("{lock} {source}'s password");
            let boxed = boxed_in(C::Cyan, &title, lines, "", width, rows, CARD_INNER);
            match saved.map(|(at, _)| ui.now.saturating_duration_since(at)).filter(|after| *after > SAVED && !still(ui))
            {
                Some(after) => {
                    super::reveal(boxed, 1.0 - ((after - SAVED).as_secs_f64() / FOLD.as_secs_f64()).min(1.0))
                }
                None => boxed,
            }
        }
    }
}

/// The password as typed: a `•` a character, the newest popping (bigger, lit) a moment; the cursor after.
fn dots(ui: &Ui, count: usize, last: Option<Instant>, saved: bool) -> Vec<Seg> {
    let popping = last.is_some_and(|at| !still(ui) && ui.now.saturating_duration_since(at) < POP) && count > 0;
    let settled = if popping { count - 1 } else { count };
    let mut line = vec![seg(format!(" {}", "•".repeat(settled)), C::White)];
    if popping {
        line.push(seg("●", C::Cyan));
    }
    if !saved {
        line.push(seg(ui.cursor().to_string(), C::Cyan));
    }
    line
}
