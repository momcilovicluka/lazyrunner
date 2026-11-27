//! The add-source form (#114): `a` in the Sources section (or `p` with no source yet), and `E` on a source, filled
//! in. Its fields are data ([`FIELDS`]: each one's key, label and how it's edited), so the settings editor (#115)
//! can take them over. A connection string pasted (`Ctrl+V`, or typed into the first field) fills the form: the
//! server, login and database go to their fields, the password straight to the credential store on saving, and what
//! a pull sets itself is left out and listed once. `Ctrl+G` (or `g` on a choice) tries it: the check step's sign-in,
//! then its databases, which `Database` becomes a pick of - the whole server first; picking several makes a database
//! source each. `Enter` saves it for the team (the profile, only its key changed) or only you (this machine's
//! config). Its motion: a paste's parts glide into their fields one after another, the password's characters turning
//! to `•` as they come; a try's dot breathes, then the databases cascade in; a failure shakes the form once, the
//! server's message under it; a save folds the form away into its new row, which glints. With reduced motion
//! everything steps.

use std::time::{Duration, Instant};

use crossterm::event::KeyEvent;
use lazyrunner_core::output::Hue;
use lazyrunner_core::profile::pasted;
use lazyrunner_core::profile::sources::{self, Auth, DatabaseSource, Reach};
use lazyrunner_core::run::Controller;
use lazyrunner_core::run::catalog::Listed;
use lazyrunner_core::run::controller::sources::Place;
use lazyrunner_core::run::credentials::Password;
use lazyrunner_core::run::database_files as files;
use unicode_width::UnicodeWidthStr;

use super::icons::{self, Icon};
use super::overlays::boxed_in;
use super::pull_view::Card;
use super::seg::{Seg, focus_row, pad, seg};
use super::theme::C;
use super::{Overlay, Ui};

/// How a field is edited: typed text, a secret typed (never shown), one of a few choices, or the database (the whole
/// server, one typed, or picks once tried).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Text,
    Secret,
    Choice(&'static [&'static str]),
    Database,
}

/// A field of the form: its key (as the profile names it), its label, and how it's edited.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Field {
    pub key: &'static str,
    pub label: &'static str,
    pub kind: Kind,
}

/// The logins, the database's two choices and where it's saved, as the form offers them.
pub const LOGINS: [&str; 3] = ["SQL", "Windows", "from an environment variable"];
pub const DATABASES: [&str; 2] = ["the whole server", "one:"];
pub const SAVES: [&str; 2] = ["the team", "only me"];

pub const NAME: usize = 0;
pub const SERVER: usize = 1;
pub const LOGIN: usize = 2;
pub const USER: usize = 3;
pub const VARIABLE: usize = 4;
pub const PASSWORD: usize = 5;
pub const DATABASE: usize = 6;
pub const SAVE: usize = 7;

/// The form's fields, in order.
pub const FIELDS: [Field; 8] = [
    Field { key: "name", label: "Name", kind: Kind::Text },
    Field { key: "server", label: "Server", kind: Kind::Text },
    Field { key: "auth", label: "Login", kind: Kind::Choice(&LOGINS) },
    Field { key: "user", label: "User", kind: Kind::Text },
    Field { key: "connectionStringEnv", label: "Variable", kind: Kind::Text },
    Field { key: "password", label: "Password", kind: Kind::Secret },
    Field { key: "database", label: "Database", kind: Kind::Database },
    Field { key: "saveFor", label: "Save for", kind: Kind::Choice(&SAVES) },
];

/// A field's text at most this long; the form's text at most this wide; its labels this wide.
const TEXT_MOST: usize = 2048;
const INNER: usize = 66;
const LABEL_CELLS: usize = 9;
/// A paste typed into the first field is read once typing has stopped this long.
const SETTLE: Duration = Duration::from_millis(150);
/// A paste's parts glide in this long each, one after another; its dropped parts fade this long.
const GLIDE: Duration = Duration::from_millis(220);
const GLIDE_STAGGER: Duration = Duration::from_millis(90);
const FADE: Duration = Duration::from_millis(1200);
/// A try's dot breathes once in this long; its databases cascade in this far apart.
const BREATH: Duration = Duration::from_millis(1200);
const CASCADE: Duration = Duration::from_millis(45);
/// A failure shakes the form this long, by these cells; a save folds it this long.
const SHAKE: Duration = Duration::from_millis(320);
const SHAKE_CELLS: [isize; 6] = [2, -2, 2, -1, 1, 0];
const FOLD: Duration = Duration::from_millis(220);
/// The databases shown under the field at most, after a try.
const PICKS_MOST: usize = 8;

/// The form: the source it edits (`E`), each field's text and choice, the field typed in, the password typed, and
/// what moves - a paste, a try and its databases, a failure, a save.
#[derive(Debug)]
pub struct Form {
    pub editing: Option<String>,
    pub text: [String; 8],
    pub choice: [usize; 8],
    pub at: usize,
    pub password: Password,
    pub password_kept: bool,
    name_typed: bool,
    typed_at: Option<Instant>,
    pasted: Option<Instant>,
    pub dropped: Vec<String>,
    pub trying: Option<(u64, Instant)>,
    pub databases: Option<(Vec<Listed>, Instant)>,
    pub picks: Vec<bool>,
    pub pick_at: usize,
    pub signed_in: Option<String>,
    pub error: Option<(String, Instant)>,
    pub saved: Option<Instant>,
}

impl Form {
    fn empty() -> Self {
        Self {
            editing: None,
            text: Default::default(),
            choice: [0; 8],
            at: NAME,
            password: Password::new(""),
            password_kept: false,
            name_typed: false,
            typed_at: None,
            pasted: None,
            dropped: Vec::new(),
            trying: None,
            databases: None,
            picks: Vec::new(),
            pick_at: 0,
            signed_in: None,
            error: None,
            saved: None,
        }
    }

    /// Whether keys are text in it: a text field has the keys.
    pub fn typing(&self) -> bool {
        let text = matches!(FIELDS[self.at].kind, Kind::Text | Kind::Secret);
        let database = self.at == DATABASE && self.choice[DATABASE] == 1 && self.databases.is_none();
        self.saved.is_none() && (text || database)
    }

    /// Whether a field shows, as the login has it.
    pub fn shows(&self, field: usize) -> bool {
        let login = self.choice[LOGIN];
        match field {
            USER | PASSWORD => login == 0,
            VARIABLE => login == 2,
            SERVER | DATABASE => login != 2,
            _ => true,
        }
    }

    fn step(&mut self, forward: bool) {
        let count = FIELDS.len();
        let mut at = self.at;
        for _ in 0..count {
            at = if forward { (at + 1) % count } else { (at + count - 1) % count };
            if self.shows(at) {
                break;
            }
        }
        self.at = at;
    }

    /// The source as filled in, its database as chosen (none for the whole server).
    pub fn draft(&self) -> DatabaseSource {
        let name = self.text[NAME].trim().to_lowercase();
        let reach = match self.choice[LOGIN] {
            2 => Reach::Env(self.text[VARIABLE].trim().to_string()),
            login => {
                let database = Some(self.text[DATABASE].trim().to_string())
                    .filter(|db| !db.is_empty() && self.choice[DATABASE] == 1);
                let user = Some(self.text[USER].trim().to_string()).filter(|user| !user.is_empty() && login == 0);
                let auth = if login == 0 { Auth::Sql } else { Auth::Windows };
                Reach::Server { server: self.text[SERVER].trim().to_string(), database, auth, user }
            }
        };
        DatabaseSource { name, reach, mine: self.choice[SAVE] == 1 }
    }

    /// The sources a save makes: one, or one per database picked (`<name>-<database>`).
    pub fn drafts(&self) -> Vec<DatabaseSource> {
        let draft = self.draft();
        let Some((listed, _)) = &self.databases else { return vec![draft] };
        let picked: Vec<&str> = listed
            .iter()
            .enumerate()
            .filter(|(at, _)| self.picks.get(at + 1).copied().unwrap_or(false))
            .map(|(_, listed)| listed.name.as_str())
            .collect();
        let whole = DatabaseSource {
            reach: match &draft.reach {
                Reach::Server { server, auth, user, .. } => {
                    Reach::Server { server: server.clone(), database: None, auth: *auth, user: user.clone() }
                }
                other => other.clone(),
            },
            ..draft.clone()
        };
        match picked.as_slice() {
            [] => vec![whole],
            [one] => vec![whole.on(one)],
            many => many
                .iter()
                .map(|database| {
                    let name: String = format!("{}-{}", draft.name, database.to_lowercase())
                        .chars()
                        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
                        .collect();
                    DatabaseSource { name, ..whole.on(database) }
                })
                .collect(),
        }
    }

    /// Fills the form from a pasted connection string; whether it was one.
    pub fn paste(&mut self, text: &str, taken: &[String], now: Instant) -> bool {
        if !pasted::looks_like(text) {
            return false;
        }
        let read = pasted::read(text);
        if self.text[NAME].contains('=') {
            self.text[NAME].clear();
        }
        self.text[SERVER] = read.server.clone().unwrap_or_default();
        self.text[USER] = read.user.clone().unwrap_or_default();
        self.choice[LOGIN] = match read.auth {
            Some(Auth::Sql) | None => 0,
            Some(Auth::Windows | Auth::Entra) => 1,
        };
        match &read.database {
            Some(database) => (self.choice[DATABASE], self.text[DATABASE]) = (1, database.clone()),
            None => (self.choice[DATABASE], self.text[DATABASE]) = (0, String::new()),
        }
        if let Some(password) = read.password {
            self.password = password;
        }
        if !self.name_typed || self.text[NAME].is_empty() {
            self.text[NAME] = sources::suggest_name(&self.text[SERVER], taken);
        }
        (self.dropped, self.pasted, self.databases, self.error) = (read.dropped, Some(now), None, None);
        self.at = SERVER;
        true
    }
}

/// Opens the empty form.
pub fn open_new(ui: &mut Ui, _controller: &mut Controller) {
    super::pull_view::open(ui, Card::Form(Box::new(Form::empty())));
}

/// `E`: opens the form on a source, filled in.
pub fn open_edit(ui: &mut Ui, controller: &mut Controller, name: &str) {
    let Some(source) = controller.source(name) else { return };
    let mut form = Form::empty();
    form.editing = Some(source.name.clone());
    form.text[NAME] = source.name.clone();
    form.name_typed = true;
    form.choice[SAVE] = usize::from(source.mine);
    match &source.reach {
        Reach::Env(variable) => (form.choice[LOGIN], form.text[VARIABLE]) = (2, variable.clone()),
        Reach::Server { server, database, auth, user } => {
            form.text[SERVER] = server.clone();
            form.choice[LOGIN] = usize::from(*auth != Auth::Sql);
            form.text[USER] = user.clone().unwrap_or_default();
            if let Some(database) = database {
                (form.choice[DATABASE], form.text[DATABASE]) = (1, database.clone());
            }
        }
    }
    form.password_kept = controller.password_kept(&source.name);
    super::pull_view::open(ui, Card::Form(Box::new(form)));
}

fn form_mut(ui: &mut Ui) -> Option<&mut Form> {
    match ui.overlay.as_mut() {
        Some(Overlay::Pull(card)) => match card.as_mut() {
            Card::Form(form) => Some(form),
            _ => None,
        },
        _ => None,
    }
}

fn still(ui: &Ui) -> bool {
    ui.reduced_motion || !ui.motion.enabled
}

fn since(ui: &Ui, at: Instant) -> Duration {
    ui.now.saturating_duration_since(at)
}

/// The names the sources have, so a suggested one is free.
fn taken(controller: &Controller, editing: Option<&str>) -> Vec<String> {
    let names = controller.sources().into_iter().map(|source| source.name);
    names.filter(|name| editing.is_none_or(|editing| !name.eq_ignore_ascii_case(editing))).collect()
}

/// The form's keys.
pub fn key(ui: &mut Ui, controller: &mut Controller, name: &str, key: &KeyEvent) {
    let now = ui.now;
    let paste = ui.paste;
    let Some(form) = form_mut(ui) else { return };
    if form.saved.is_some() {
        return;
    }
    let choice = matches!(FIELDS[form.at].kind, Kind::Choice(_)) || (form.at == DATABASE && !form.typing());
    let listing = form.at == DATABASE && form.databases.is_some();
    match name {
        "Escape" => ui.overlay = None,
        "Tab" => form.step(true),
        "Shift+Tab" => form.step(false),
        "DownArrow" if listing && form.pick_at + 1 < form.picks.len() => form.pick_at += 1,
        "UpArrow" if listing && form.pick_at > 0 => form.pick_at -= 1,
        "DownArrow" => form.step(true),
        "UpArrow" => form.step(false),
        "Space" if listing => toggle_pick(form),
        "LeftArrow" | "RightArrow" | "Space" if choice => cycle(form, name != "LeftArrow"),
        "Ctrl+G" => try_it(ui, controller),
        "g" if choice => try_it(ui, controller),
        "Ctrl+V" => match paste() {
            Ok(text) => {
                let taken = taken(controller, form.editing.as_deref());
                if !form.paste(&text, &taken, now) {
                    type_text(form, &text.replace(['\r', '\n'], ""), now);
                }
            }
            Err(why) => controller.add_log(format!("Couldn't read the clipboard: {why}"), Hue::Yellow),
        },
        "Enter" => {
            let typed_paste = form.text[NAME].clone();
            let taken = taken(controller, form.editing.as_deref());
            if form.at == NAME && form.paste(&typed_paste, &taken, now) {
                return;
            }
            save(ui, controller);
        }
        "Backspace" => {
            match FIELDS[form.at].kind {
                Kind::Secret => {
                    let mut typed = form.password.expose().to_string();
                    typed.pop();
                    form.password = Password::new(typed);
                }
                _ => {
                    form.text[form.at].pop();
                }
            }
            changed(form, controller, now);
        }
        _ => {
            if let Some(c) = super::keys::typed(key).filter(|_| form.typing()) {
                type_text(form, &c.to_string(), now);
                changed(form, controller, now);
            }
        }
    }
}

/// Text typed into the field that has the keys.
fn type_text(form: &mut Form, text: &str, now: Instant) {
    match FIELDS[form.at].kind {
        Kind::Secret => {
            let typed = format!("{}{text}", form.password.expose());
            form.password = Password::new(typed.chars().take(TEXT_MOST).collect::<String>());
        }
        _ => {
            let field = &mut form.text[form.at];
            field.push_str(text);
            *field = field.chars().take(TEXT_MOST).collect();
        }
    }
    if form.at == NAME {
        form.name_typed = !form.text[NAME].is_empty();
    }
    form.typed_at = Some(now);
}

/// After a field changed: a name suggested from the server until one is typed; a list from an earlier try forgotten
/// once the server or login changes.
fn changed(form: &mut Form, controller: &Controller, _now: Instant) {
    if form.at == SERVER {
        form.databases = None;
        if !form.name_typed {
            let taken = taken(controller, form.editing.as_deref());
            form.text[NAME] = if form.text[SERVER].trim().is_empty() {
                String::new()
            } else {
                sources::suggest_name(&form.text[SERVER], &taken)
            };
        }
    }
    form.error = None;
}

fn cycle(form: &mut Form, forward: bool) {
    let count = match FIELDS[form.at].kind {
        Kind::Choice(choices) => choices.len(),
        _ => DATABASES.len(),
    };
    let now = form.choice[form.at];
    form.choice[form.at] = if forward { (now + 1) % count } else { (now + count - 1) % count };
    if form.at == LOGIN {
        form.databases = None;
    }
}

/// Space on a database after a try: picks it (the whole server unpicks the rest, a database unpicks the whole
/// server).
fn toggle_pick(form: &mut Form) {
    let at = form.pick_at;
    let on = !form.picks.get(at).copied().unwrap_or(false);
    if at == 0 {
        form.picks.iter_mut().for_each(|pick| *pick = false);
    } else if let Some(whole) = form.picks.first_mut() {
        *whole = false;
    }
    if let Some(pick) = form.picks.get_mut(at) {
        *pick = on;
    }
    if !form.picks.iter().any(|pick| *pick)
        && let Some(whole) = form.picks.first_mut()
    {
        *whole = true;
    }
}

/// `Ctrl+G`: tries the source as filled in - signs in and lists its databases - in the background.
fn try_it(ui: &mut Ui, controller: &mut Controller) {
    let now = ui.now;
    let Some(form) = form_mut(ui) else { return };
    let mut draft = form.draft();
    if draft.name.is_empty() {
        draft.name = sources::suggest_name(draft.server().unwrap_or_default(), &[]);
    }
    let typed = Some(form.password.clone()).filter(|password| !password.expose().is_empty());
    match controller.try_source(&draft, typed) {
        Ok(job) => (form.trying, form.error, form.databases, form.signed_in) = (Some((job, now)), None, None, None),
        Err(why) => form.error = Some((why, now)),
    }
}

/// `Enter`: saves what's filled in; the form folds away into the source's new row.
fn save(ui: &mut Ui, controller: &mut Controller) {
    let now = ui.now;
    let Some(form) = form_mut(ui) else { return };
    let drafts = form.drafts();
    let place = if form.choice[SAVE] == 1 { Place::Mine } else { Place::Team };
    let password = Some(form.password.clone()).filter(|password| !password.expose().is_empty());
    let editing = form.editing.clone();
    match controller.save_sources(&drafts, place, editing.as_deref(), password.as_ref()) {
        Ok(said) => {
            form.saved = Some(now);
            let whose = if place == Place::Mine {
                "only mine".to_string()
            } else {
                said.rsplit('(').next().unwrap_or_default().trim_end_matches(')').to_string()
            };
            let first = drafts.first().map(|draft| draft.name.clone()).unwrap_or_default();
            super::sources::arrived(ui, controller, &first, &whose);
        }
        Err(why) => form.error = Some((why, now)),
    }
}

/// Each frame: a paste typed into the first field read once typing stops; a try's answer taken in; a saved form
/// closed once it has folded away.
pub fn poll(ui: &mut Ui, controller: &Controller) {
    let (now, still) = (ui.now, still(ui));
    let Some(form) = form_mut(ui) else { return };
    if form.saved.is_some_and(|at| still || now.saturating_duration_since(at) >= FOLD) {
        ui.overlay = None;
        return;
    }
    if form.at == NAME && form.typed_at.is_some_and(|at| now.saturating_duration_since(at) >= SETTLE) {
        form.typed_at = None;
        let text = form.text[NAME].clone();
        let taken = taken(controller, form.editing.as_deref());
        form.paste(&text, &taken, now);
    }
    let Some((job, _)) = form.trying else { return };
    let Some((answered, result)) = controller.tried.as_ref().filter(|(answered, _)| *answered == job) else { return };
    let _ = answered;
    form.trying = None;
    match result {
        Ok(tried) => {
            let named = (form.choice[DATABASE] == 1).then(|| form.text[DATABASE].trim().to_lowercase());
            let mut picks = vec![named.is_none()];
            picks.extend(tried.databases.iter().map(|listed| Some(listed.name.to_lowercase()) == named));
            if !picks.iter().any(|pick| *pick) {
                picks[0] = true;
            }
            form.picks = picks;
            form.pick_at = 0;
            form.databases = Some((tried.databases.clone(), now));
            form.signed_in = Some(format!(
                "✓ signed in{} · {} ms · {} database{}",
                tried.login.as_ref().map(|login| format!(" as {login}")).unwrap_or_default(),
                tried.ms,
                tried.databases.len(),
                if tried.databases.len() == 1 { "" } else { "s" }
            ));
            form.at = DATABASE;
        }
        Err(why) => form.error = Some((why.lines().next().unwrap_or_default().to_string(), now)),
    }
}

/// Whether the form moves: the cursor, a paste gliding in, a try breathing, databases cascading, a shake, a fold.
pub fn animating(ui: &Ui, form: &Form) -> bool {
    if still(ui) {
        return false;
    }
    let recent = |at: Option<Instant>, long: Duration| at.is_some_and(|at| since(ui, at) < long);
    form.typing()
        || form.trying.is_some()
        || recent(form.pasted, FADE.max(GLIDE * 6))
        || recent(form.databases.as_ref().map(|(_, at)| *at), CASCADE * PICKS_MOST as u32 + GLIDE)
        || recent(form.error.as_ref().map(|(_, at)| *at), SHAKE)
        || recent(form.saved, FOLD)
        || form.typed_at.is_some()
}

/// How much of a pasted field shows: its share glided in, its place `order` among the parts.
fn glided(ui: &Ui, form: &Form, order: u32) -> f64 {
    let Some(at) = form.pasted.filter(|_| !still(ui)) else { return 1.0 };
    let after = since(ui, at).saturating_sub(GLIDE_STAGGER * order);
    super::motion::ease((after.as_secs_f64() / GLIDE.as_secs_f64()).min(1.0))
}

/// The first share of a text's characters.
fn share_of(text: &str, share: f64) -> String {
    let count = text.chars().count();
    text.chars().take((count as f64 * share).ceil() as usize).collect()
}

/// A choice field's options: `◉` on the one chosen.
fn options(choices: &[&str], chosen: usize, extra: &[String]) -> String {
    choices
        .iter()
        .enumerate()
        .map(|(at, choice)| {
            let more = extra.get(at).filter(|more| !more.is_empty()).map(|more| format!(" {more}")).unwrap_or_default();
            format!("{} {choice}{more}", if at == chosen { "◉" } else { "○" })
        })
        .collect::<Vec<_>>()
        .join("  ")
}

/// A field's row: its label, then its value (the cursor after it while it has the keys).
fn field_row(ui: &Ui, controller: &Controller, form: &Form, field: usize) -> Vec<Seg> {
    let focused = form.at == field && form.saved.is_none();
    let cursor = if focused && form.typing() { ui.cursor().to_string() } else { String::new() };
    let label = seg(format!(" {}", pad(FIELDS[field].label, LABEL_CELLS)), if focused { C::Cyan } else { C::Gray });
    let order = |field: usize| match field {
        SERVER => 0,
        USER => 1,
        PASSWORD => 2,
        DATABASE => 3,
        _ => 4,
    };
    let value = match FIELDS[field].kind {
        Kind::Text => {
            let text = share_of(&form.text[field], glided(ui, form, order(field)));
            let empty = text.is_empty() && cursor.is_empty();
            let hint = match field {
                NAME if form.editing.is_none() => "paste a connection string here, or a name",
                VARIABLE => "an environment variable holding its connection string",
                _ => "",
            };
            if empty { seg(hint, C::DarkGray) } else { seg(format!("{text}{cursor}"), C::White) }
        }
        Kind::Secret => {
            let count = form.password.expose().chars().count();
            let shown = (count as f64 * glided(ui, form, order(field))).ceil() as usize;
            let lock = icons::icon(ui.nerd, Icon::Lock);
            let kept = match (count, form.password_kept) {
                (0, true) => format!("kept in {} - type to change it", controller.credential_store()),
                _ => format!("{lock} kept in {}", controller.credential_store()),
            };
            seg(format!("{}{cursor}   {kept}", "•".repeat(shown)), if count == 0 { C::DarkGray } else { C::White })
        }
        Kind::Choice(choices) => {
            let extra: Vec<String> = match field {
                SAVE => {
                    let file = controller.project.path.file_name().map(|name| format!("({})", name.to_string_lossy()));
                    vec![file.unwrap_or_default(), String::new()]
                }
                _ => Vec::new(),
            };
            seg(options(choices, form.choice[field], &extra), if focused { C::White } else { C::Gray })
        }
        Kind::Database => {
            let typed = share_of(&form.text[DATABASE], glided(ui, form, order(field)));
            let one = if form.choice[DATABASE] == 1 { format!("{typed}{cursor}") } else { String::new() };
            let text = match &form.databases {
                Some((listed, _)) => {
                    let picked: Vec<&str> = listed
                        .iter()
                        .enumerate()
                        .filter(|(at, _)| form.picks.get(at + 1).copied().unwrap_or(false))
                        .map(|(_, listed)| listed.name.as_str())
                        .collect();
                    match picked.len() {
                        0 => "◉ the whole server".to_string(),
                        1 => format!("◉ one: {}", picked[0]),
                        many => format!("◉ {many} databases: a source each"),
                    }
                }
                None => options(&DATABASES, form.choice[DATABASE], &[String::new(), one]),
            };
            seg(text, if focused { C::White } else { C::Gray })
        }
    };
    let line = vec![label, value];
    if focused && !matches!(FIELDS[field].kind, Kind::Text | Kind::Secret) { focus_row(line, INNER) } else { line }
}

/// The databases a try listed, under the Database field: the whole server first, each with its size, cascading in.
fn picks_rows(ui: &Ui, form: &Form) -> Vec<Vec<Seg>> {
    let Some((listed, at)) = &form.databases else { return Vec::new() };
    let names: Vec<(String, String)> = std::iter::once(("the whole server".to_string(), String::new()))
        .chain(listed.iter().map(|listed| (listed.name.clone(), listed.bytes.map(files::size).unwrap_or_default())))
        .collect();
    let top = (form.pick_at + 1).saturating_sub(PICKS_MOST);
    let widest = names.iter().map(|(name, _)| name.width()).max().unwrap_or_default();
    names
        .iter()
        .enumerate()
        .skip(top)
        .take(PICKS_MOST)
        .map(|(row, (name, size))| {
            let comes = CASCADE * (row - top) as u32;
            if !still(ui) && since(ui, *at) < comes {
                return Vec::new();
            }
            let mark = if form.picks.get(row).copied().unwrap_or(false) { "☑" } else { "☐" };
            let text = format!("{}{mark} {}  {size}", " ".repeat(LABEL_CELLS + 3), pad(name, widest));
            let lit = form.at == DATABASE && row == form.pick_at;
            let line = vec![seg(text, if lit { C::White } else { C::Gray })];
            if lit { focus_row(line, INNER) } else { line }
        })
        .collect()
}

/// The form, centred over the database screen: shaking once after a failure, folding away once saved.
pub fn rows(ui: &Ui, controller: &Controller, form: &Form, width: usize, rows: usize) -> Vec<Vec<Seg>> {
    let mut lines = Vec::new();
    if form.editing.is_none() {
        lines.push(vec![seg(" Paste a connection string (Ctrl+V), or fill in:", C::DarkGray)]);
    }
    for field in (0..FIELDS.len()).filter(|field| form.shows(*field)) {
        lines.push(field_row(ui, controller, form, field));
        if field == DATABASE {
            lines.extend(picks_rows(ui, form));
        }
    }
    if !form.dropped.is_empty() {
        let fading = form.pasted.is_some_and(|at| !still(ui) && since(ui, at) < FADE);
        let said = format!(" left out (a pull sets them itself): {}", form.dropped.join(", "));
        let wrapped = super::seg::wrap(&[seg(said, if fading { C::Gray } else { C::DarkGray })], INNER, 2, 3);
        lines.extend(wrapped);
    }
    lines.push(Vec::new());
    if let Some((_, at)) = form.trying {
        let phase = since(ui, at).as_millis() % BREATH.as_millis();
        let lit = still(ui) || phase < BREATH.as_millis() / 2;
        lines.push(vec![
            seg(" ● ", if lit { C::Cyan } else { C::DarkCyan }),
            seg("signing in and listing its databases…", C::Gray),
        ]);
    } else if let Some(said) = &form.signed_in {
        lines.push(vec![seg(format!(" {said}"), C::Green)]);
    }
    if let Some((why, _)) = &form.error {
        lines.extend(super::seg::wrap(&[seg(format!(" ✗ {why}"), C::Red)], INNER, 3, 3));
    }
    let keys = if form.databases.is_some() && form.at == DATABASE {
        " ↑ ↓ pick · Space choose · Tab next · Enter save · Esc cancel"
    } else {
        " Tab next · ← → choose · Ctrl+G try it · Enter save · Esc cancel"
    };
    lines.push(vec![seg(keys, C::DarkGray)]);
    let title = match &form.editing {
        Some(name) => format!("Edit {name}"),
        None => "New source".to_string(),
    };
    let boxed = boxed_in(C::Cyan, &icons::titled(ui.nerd, Icon::Database, &title), lines, "", width, rows, INNER);
    if let Some(at) = form.saved.filter(|_| !still(ui)) {
        return super::reveal(boxed, 1.0 - (since(ui, at).as_secs_f64() / FOLD.as_secs_f64()).min(1.0));
    }
    match form.error.as_ref().map(|(_, at)| since(ui, *at)).filter(|after| !still(ui) && *after < SHAKE) {
        Some(after) => {
            let phase = (after.as_secs_f64() / SHAKE.as_secs_f64() * SHAKE_CELLS.len() as f64) as usize;
            shift(boxed, SHAKE_CELLS[phase.min(SHAKE_CELLS.len() - 1)])
        }
        None => boxed,
    }
}

/// Rows moved sideways by `cells` (left when negative), as a shake moves the box.
fn shift(lines: Vec<Vec<Seg>>, cells: isize) -> Vec<Vec<Seg>> {
    lines
        .into_iter()
        .map(|mut line| {
            if cells > 0 {
                line.insert(0, seg(" ".repeat(cells as usize), C::Gray));
            } else if cells < 0
                && let Some(first) = line.first_mut()
            {
                let cut = first.text.chars().take_while(|c| *c == ' ').count().min(cells.unsigned_abs());
                first.text = first.text.chars().skip(cut).collect();
            }
            line
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pasted_string_fills_the_fields_and_names_the_source_from_its_server() {
        let mut form = Form::empty();
        let now = Instant::now();
        let pasted = "Server=qa-sql.example.com,1433;Database=Shop;User ID=reader;Password=s3cret;Encrypt=True";
        assert!(form.paste(pasted, &["qa".into()], now));
        assert_eq!(form.text[NAME], "qa-2", "free");
        assert_eq!((form.text[SERVER].as_str(), form.text[USER].as_str()), ("qa-sql.example.com,1433", "reader"));
        assert_eq!((form.choice[DATABASE], form.text[DATABASE].as_str()), (1, "Shop"));
        assert_eq!(form.password.expose(), "s3cret");
        assert_eq!(form.dropped, ["Encrypt"]);
        assert!(!format!("{form:?}").contains("s3cret"), "never shown");
        let draft = form.draft();
        assert_eq!(draft.label(), "Shop @ qa-sql.example.com,1433");
        assert!(!form.paste("just a name", &[], now));
    }

    #[test]
    fn picking_several_databases_makes_a_source_each() {
        let mut form = Form::empty();
        form.paste("Server=qa-sql;User ID=reader", &[], Instant::now());
        let listed = |name: &str| Listed { name: name.into(), ..Listed::default() };
        form.databases = Some((vec![listed("Shop"), listed("Shop Audit")], Instant::now()));
        form.picks = vec![true, false, false];
        assert!(form.drafts()[0].is_server(), "the whole server first");
        form.picks = vec![false, true, true];
        let names: Vec<String> =
            form.drafts().iter().map(|draft| format!("{} {}", draft.name, draft.database().unwrap())).collect();
        assert_eq!(names, ["qa-shop Shop", "qa-shop-audit Shop Audit"]);
        form.picks = vec![false, true, false];
        assert_eq!(form.drafts()[0].name, "qa", "one keeps the name");
    }
}
