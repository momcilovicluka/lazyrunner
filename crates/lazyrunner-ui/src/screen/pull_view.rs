//! Pulling on the database screen (#110): `p` pulls a copy from a source (picked from a card when there are several),
//! `P` again from the last one, `x` cancels, `,` changes or forgets a source's password; `Tab` gives the pull graph
//! (`pull_graph`) the keys, where `h` `l` pick a step and `Enter` opens its whole output. A source's password is asked
//! for on a small centred card - a lock, the source's server, each character a `•` with a soft pop - and kept in the
//! OS's credential store: the card says so with one glint, then folds away. Once a pull is done, `keepPulled`'s offer
//! to drop the older copies comes as a y/n box, and the new database's row glints. The same card asks for the
//! password an export, an import, a listing or an update needs (#114), and holds the add-source form
//! (`source_form`) and the link picker (`link_card`); `U` updates a linked database and `L` links one.

use std::time::{Duration, Instant};

use crossterm::event::KeyEvent;
use lazyrunner_core::output::Hue;
use lazyrunner_core::profile::sources::{DatabaseSource, Reach};
use lazyrunner_core::run::Controller;
use lazyrunner_core::run::controller::save_points::db_key;
use lazyrunner_core::run::credentials::Password;
use lazyrunner_core::run::pull::{Mode, Step};

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

/// What a source is picked for: a pull, an export, an import, its databases listed, a linked database updated, or
/// its password.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Purpose {
    Pull,
    Export,
    Import,
    List,
    Update,
    Password,
}

impl Purpose {
    /// The pull's mode it starts, when it starts one.
    fn mode(self) -> Option<Mode> {
        match self {
            Self::Pull => Some(Mode::Pull),
            Self::Export => Some(Mode::Export),
            Self::Import => Some(Mode::Import),
            Self::Update => Some(Mode::Update),
            Self::List | Self::Password => None,
        }
    }

    /// What `Enter` does once the password is typed.
    fn then(self) -> &'static str {
        match self {
            Self::Pull => "keeps it and pulls",
            Self::Export => "keeps it and exports",
            Self::Import => "keeps it and imports",
            Self::List => "keeps it and lists the databases",
            Self::Update => "keeps it and updates",
            Self::Password => "keeps it",
        }
    }
}

/// What the user asked of a source: what for, which source, which of its server's databases, and for an update the
/// linked local database (its server and name).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Want {
    pub purpose: Purpose,
    pub source: String,
    pub database: Option<String>,
    pub local: Option<(String, String)>,
}

impl Want {
    pub fn new(purpose: Purpose, source: &str, database: Option<&str>) -> Self {
        Self { purpose, source: source.into(), database: database.map(String::from), local: None }
    }
}

/// The card: a source to pick, its password being typed (the time of each character, when it was kept), the add or
/// edit form, or the link picker.
#[derive(Debug)]
pub enum Card {
    Choose { purpose: Purpose, index: usize },
    Password { want: Want, typed: String, last: Option<Instant>, saved: Option<(Instant, &'static str)> },
    Form(Box<super::source_form::Form>),
    Link(Box<super::link_card::LinkCard>),
}

impl Card {
    /// Whether keys are text in it: the password or the form being typed.
    pub fn typing(&self) -> bool {
        match self {
            Self::Password { saved, .. } => saved.is_none(),
            Self::Form(form) => form.typing(),
            _ => false,
        }
    }
}

/// Whether the card asks for a password or holds the add form: an agent's keys never reach it.
pub fn asking_password(ui: &Ui) -> bool {
    matches!(&ui.overlay, Some(Overlay::Pull(card)) if matches!(**card, Card::Password { .. } | Card::Form(_)))
}

fn still(ui: &Ui) -> bool {
    ui.reduced_motion || !ui.motion.enabled
}

/// The highlighted local database: its server and name.
fn highlighted(ui: &Ui) -> Option<(String, String)> {
    let picker = ui.picker.as_ref()?;
    picker.entries().get(picker.index).map(|entry| (entry.server.clone(), entry.database.clone()))
}

/// The database screen's pull keys; whether the key was one of them.
pub fn key(ui: &mut Ui, controller: &mut Controller, name: &str) -> bool {
    if ui.pull_view.focused && focused_key(ui, controller, name) {
        return true;
    }
    match name {
        "p" => choose(ui, controller, Purpose::Pull),
        "P" if !controller.pull_sources().is_empty() => match controller.last_pull() {
            Some((source, database)) => {
                proceed(ui, controller, Want { database, ..Want::new(Purpose::Pull, &source, None) })
            }
            None => controller.add_log("Nothing pulled yet - p pulls a copy from a source", Hue::Gray),
        },
        "x" if controller.pull.as_ref().is_some_and(|pull| !pull.is_over()) => match controller.cancel_pull() {
            Ok(said) => controller.add_log(format!("■ {}{}", said[..1].to_uppercase(), &said[1..]), Hue::Yellow),
            Err(why) => controller.add_log(format!("✗ {why}"), Hue::Red),
        },
        "," => choose(ui, controller, Purpose::Password),
        "U" => {
            let Some((server, database)) = highlighted(ui) else { return false };
            update(ui, controller, &server, &database);
        }
        "L" => {
            let Some((server, database)) = highlighted(ui) else { return false };
            super::link_card::open_for_local(ui, controller, &server, &database);
        }
        _ => return false,
    }
    true
}

/// `U`: updates the linked database from its twin, its password asked for first when one is needed.
pub fn update(ui: &mut Ui, controller: &mut Controller, server: &str, database: &str) {
    let Some(link) = controller.link_of(server, database).cloned() else {
        controller.add_log(format!("{database} isn't linked to a remote database - L links it"), Hue::Gray);
        return;
    };
    let want = Want {
        local: Some((server.to_string(), database.to_string())),
        ..Want::new(Purpose::Update, &link.source, Some(&link.remote))
    };
    proceed(ui, controller, want);
}

/// The keys while the graph has them; whether the key was one of them.
fn focused_key(ui: &mut Ui, controller: &mut Controller, name: &str) -> bool {
    let Some(pull) = controller.pull.as_ref() else {
        ui.pull_view.focused = false;
        return false;
    };
    let steps = pull.steps().len();
    let at = super::pull_graph::picked_at(ui, pull);
    let view = &mut ui.pull_view;
    match name {
        "h" | "LeftArrow" if !view.output => view.picked = Some(at.saturating_sub(1)),
        "l" | "RightArrow" if !view.output => view.picked = Some((at + 1).min(steps - 1)),
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
        (true, false) => Some(" h l pick a step · Enter its whole output · x cancel the pull · Tab sources · Esc back"),
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
    (ui.save_view.on_rail, ui.files_view.on_files, ui.sources_view.focused) = (false, false, false);
    let view = &mut ui.pull_view;
    (view.focused, view.picked) = (true, Some(at));
    if double {
        (view.output, view.scroll) = (true, 0);
    }
    true
}

/// `p` / `,`: the one source, else a card to pick one - and with none yet, the form that adds one.
fn choose(ui: &mut Ui, controller: &mut Controller, purpose: Purpose) {
    let sources = controller.pull_sources();
    match sources.as_slice() {
        [] if purpose == Purpose::Pull => super::source_form::open_new(ui, controller),
        [] => controller.add_log("No database sources yet - a in the Sources section adds one", Hue::Gray),
        [one] if purpose == Purpose::Password || !one.is_server() => {
            proceed(ui, controller, Want::new(purpose, &one.name.clone(), None));
        }
        _ => open(ui, Card::Choose { purpose, index: 0 }),
    }
}

pub(super) fn open(ui: &mut Ui, card: Card) {
    ui.overlay = Some(Overlay::Pull(Box::new(card)));
}

/// What a source was picked for: its password asked for when one is needed and none is kept, else it's done.
pub fn proceed(ui: &mut Ui, controller: &mut Controller, want: Want) {
    let Some(found) = controller.source(&want.source) else {
        controller.add_log(format!("No source {}", want.source), Hue::Yellow);
        return;
    };
    ui.overlay = None;
    let password = |want: Want| Card::Password { want, typed: String::new(), last: None, saved: None };
    if want.purpose == Purpose::Password {
        if !found.needs_password() {
            controller.add_log(format!("{} logs in without a password lazyrunner keeps", found.name), Hue::Gray);
            return;
        }
        return open(ui, password(want));
    }
    if want.purpose == Purpose::Pull && found.is_server() && want.database.is_none() {
        super::sources::open_source(ui, controller, &found.name);
        controller.add_log(format!("{} is a whole server - pick one of its databases, then p", found.name), Hue::Gray);
        return;
    }
    if let Some(busy) = controller.database_busy().filter(|_| want.purpose != Purpose::List) {
        controller.add_log(format!("lazyrunner is {busy} - once it's done"), Hue::Yellow);
        return;
    }
    if found.needs_password() && !controller.password_kept(&found.name) {
        return open(ui, password(want));
    }
    start(ui, controller, &want, None);
}

/// Does what was asked, with the password just typed (else the kept one).
fn start(ui: &mut Ui, controller: &mut Controller, want: &Want, typed: Option<Password>) {
    let database = want.database.as_deref();
    let result = match (want.purpose.mode(), &want.local) {
        (Some(Mode::Update), Some((server, local))) => controller.update_linked(server, local, typed),
        (Some(mode), _) => controller.pull_from(mode, &want.source, database, typed),
        (None, _) if want.purpose == Purpose::List => controller.list_databases(&want.source, typed),
        (None, _) => Ok(String::new()),
    };
    match result {
        Ok(_) if want.purpose.mode().is_some() => {
            ui.pull_view = View { dismissed: ui.pull_view.dismissed, ..View::default() };
        }
        Ok(_) => {}
        Err(why) => controller.add_log(format!("✗ {why}"), Hue::Red),
    }
}

/// The card's keys.
pub fn card_key(ui: &mut Ui, controller: &mut Controller, name: &str, key: &KeyEvent) {
    let Some(Overlay::Pull(card)) = ui.overlay.as_mut() else { return };
    let now = ui.now;
    match card.as_mut() {
        Card::Form(_) => super::source_form::key(ui, controller, name, key),
        Card::Link(_) => super::link_card::key(ui, controller, name),
        Card::Choose { purpose, index } => {
            let count = controller.pull_sources().len();
            match name {
                "j" | "DownArrow" => *index = (*index + 1) % count.max(1),
                "k" | "UpArrow" => *index = (*index + count.max(1) - 1) % count.max(1),
                "a" => super::source_form::open_new(ui, controller),
                "Enter" => {
                    let (purpose, picked) = (*purpose, *index);
                    if let Some(source) = controller.pull_sources().get(picked).map(|source| source.name.clone()) {
                        proceed(ui, controller, Want::new(purpose, &source, None));
                    }
                }
                "Escape" | "q" => ui.overlay = None,
                _ => {}
            }
        }
        Card::Password { saved: Some(_), .. } => {}
        Card::Password { want, typed, last, .. } => match name {
            "Escape" => ui.overlay = None,
            "Backspace" => {
                typed.pop();
            }
            "Delete" if typed.is_empty() && want.purpose == Purpose::Password => {
                let source = want.source.clone();
                ui.overlay = None;
                if let Err(why) = controller.forget_password(&source) {
                    controller.add_log(format!("✗ {why}"), Hue::Red);
                }
            }
            "Enter" if typed.is_empty() => {}
            "Enter" => {
                let want = want.clone();
                let password = Password::new(std::mem::take(typed));
                submit(ui, controller, &want, password);
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

/// The password typed: kept in the credential store (the card says where, then folds away), and what was asked done.
fn submit(ui: &mut Ui, controller: &mut Controller, want: &Want, password: Password) {
    let kept = controller.keep_password(&want.source, &password);
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
    if want.purpose != Purpose::Password {
        start(ui, controller, want, Some(password));
    }
}

/// Each frame: the card folded away once it has said where the password went; the form's paste and try followed;
/// a pull that's done handled once - the new database's row read in and glinting, `keepPulled`'s offer made.
pub fn poll(ui: &mut Ui, controller: &Controller) {
    if let Some(Overlay::Pull(card)) = &ui.overlay
        && let Card::Password { saved: Some((at, _)), .. } = **card
    {
        let lasts = if still(ui) { SAVED } else { SAVED + FOLD };
        if ui.now.saturating_duration_since(at) >= lasts {
            ui.overlay = None;
        }
    }
    super::source_form::poll(ui, controller);
    let Some(pull) = controller.pull.as_ref().filter(|pull| pull.is_over()) else { return };
    if ui.pull_view.handled == Some(pull.job) || ui.picker.is_none() {
        return;
    }
    ui.pull_view.handled = Some(pull.job);
    if !pull.went_well() || pull.mode == Mode::Export {
        return;
    }
    let shown = pull.target.as_deref().unwrap_or(&pull.database);
    super::database_files::reload(ui, Some((&pull.server, shown)));
    ui.files_view.glint = Some((db_key(&pull.server, shown), ui.now));
    if pull.mode == Mode::Update {
        ui.sources_view.updated = Some((db_key(&pull.server, shown), ui.now));
    }
    if controller.pull_stale.is_empty() || ui.overlay.is_some() {
        return;
    }
    let names: Vec<&str> = controller.pull_stale.iter().map(|old| old.database.as_str()).collect();
    let lines = match controller.keep_pulled() {
        Some(keep) => vec![
            format!("keepPulled keeps the newest {keep} pulled copies of {}.", pull.source),
            format!("Drop the older {} ({})?", if names.len() == 1 { "one" } else { "ones" }, names.join(", ")),
            "Their data is gone for good; the one in use is never dropped.".to_string(),
        ],
        None => vec![
            format!("{shown} is up to date with {} - its old copy is {}.", pull.twin(), names.join(", ")),
            "Drop the old copy? Its data is gone for good.".to_string(),
            "n keeps it (keepPulled keeps the newest so many by itself).".to_string(),
        ],
    };
    let yes = SavePointYes::DropPulled(controller.pull_stale.clone());
    ui.overlay = Some(Overlay::Confirm(Box::new(Confirm {
        lines,
        jobs: Vec::new(),
        risk: Risk::LosesData,
        save_point: Some(yes),
    })));
}

/// Whether the card moves: the cursor while typing, a pop, the glint, the fold, the form's own motion.
pub fn animating(ui: &Ui) -> bool {
    let Some(Overlay::Pull(card)) = &ui.overlay else { return false };
    match card.as_ref() {
        Card::Password { saved: Some(_), .. } => true,
        Card::Password { .. } => !still(ui),
        Card::Form(form) => super::source_form::animating(ui, form),
        Card::Link(link) => super::link_card::animating(ui, link),
        Card::Choose { .. } => false,
    }
}

/// Where a source is and how it logs in, as the card says it - never a password.
pub fn reached(source: &DatabaseSource) -> String {
    match &source.reach {
        Reach::Server { server, database, auth, user } => {
            let login = user.as_ref().map_or_else(|| auth.name().to_string(), |user| format!("as {user}"));
            match database {
                Some(database) => format!("{server} · {database} · {login}"),
                None => format!("{server} · the whole server · {login}"),
            }
        }
        Reach::Env(name) => format!("${name}"),
    }
}

/// The card, centred over the database screen.
pub fn card_rows(ui: &Ui, controller: &Controller, width: usize, rows: usize) -> Vec<Vec<Seg>> {
    let Some(Overlay::Pull(card)) = &ui.overlay else { return Vec::new() };
    let lock = icons::icon(ui.nerd, Icon::Lock);
    match card.as_ref() {
        Card::Form(form) => super::source_form::rows(ui, controller, form, width, rows),
        Card::Link(link) => super::link_card::rows(ui, link, width, rows),
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
            let what = match purpose {
                Purpose::Password => "changes or forgets its password",
                _ => "pulls a copy of it (a whole server opens)",
            };
            lines.extend([
                Vec::new(),
                vec![seg(format!(" j k pick · Enter {what} · a add one · Esc cancels"), C::DarkGray)],
            ]);
            boxed_in(C::Cyan, &icons::titled(ui.nerd, Icon::Database, "Pull from"), lines, "", width, rows, CARD_INNER)
        }
        Card::Password { want, typed, last, saved } => {
            let found = controller.source(&want.source);
            let place = found
                .as_ref()
                .map(|found| reached(&want.database.as_deref().map_or_else(|| found.clone(), |db| found.on(db))))
                .unwrap_or_default();
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
                    let forget = if want.purpose == Purpose::Password { " · Del forgets the kept one" } else { "" };
                    lines.push(vec![seg(format!(" Enter {}{forget} · Esc cancels", want.purpose.then()), C::DarkGray)]);
                }
            }
            let title = format!("{lock} {}'s password", want.source);
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
pub(super) fn dots(ui: &Ui, count: usize, last: Option<Instant>, saved: bool) -> Vec<Seg> {
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

/// Whether a step is the pull's last: the switch, or an update's swap.
pub fn is_last(step: Step) -> bool {
    matches!(step, Step::Switch | Step::Swap)
}
