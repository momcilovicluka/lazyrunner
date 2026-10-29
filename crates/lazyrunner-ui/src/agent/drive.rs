//! What an agent does beyond starting, stopping, building and the tests: any action the palette has (`run_action`),
//! keys as the key reference names them (`press_keys`) for what no tool covers, and the database, packages,
//! migrations, ports and settings by name. What can't be undone or takes something away from the user - quitting, a
//! port's holder killed, a migration reverted, the database switched, the output cleared, lazyrunner reopened, the git
//! tool over the screen - needs `"confirm": true`, after asking the user.

use std::time::Duration;

use lazyrunner_core::config::Toggle;
use lazyrunner_core::profile::AppKind;
use lazyrunner_core::run::migrations::Status;
use lazyrunner_core::run::secrets::{self, Entry};
use lazyrunner_core::run::{Command, Controller, Origin};
use serde_json::{Value, json};

use super::{CLIENT_CHARS, NO_MIGRATIONS, Refusal, app_index, clean};
use crate::screen::actions::{self, Action, Run};
use crate::screen::{Overlay, Ui, keys};

/// The user pressed a key this lately in a prompt: they're typing in it, so press_keys waits.
const TYPING: Duration = Duration::from_secs(10);
/// Keys that quit, kill, clear or take the screen over where nothing else takes them.
const RISKY_KEYS: [&str; 7] = ["q", "Q", "Ctrl+C", "L", "!", "Ctrl+L", "Ctrl+R"];

fn needs_confirm(run: &Run) -> bool {
    matches!(run, Run::Quit | Run::KillPortUser | Run::Clear | Run::Reload | Run::ReloadProfile | Run::GitTool)
}

fn confirmed(args: &Value) -> bool {
    args["confirm"].as_bool() == Some(true)
}

fn confirm_first(what: &str) -> Refusal {
    let why = format!(
        "{what} can't be undone or takes something from the user - ask them, then call again with \"confirm\": true"
    );
    ("needs_confirm", why)
}

fn execute(controller: &mut Controller, command: Command) -> Result<(), Refusal> {
    controller.execute(command, Origin::Agent).map_err(|why| ("refused", why))
}

/// The palette's actions (not the recent ones it shows twice, nor the settings: set_setting sets those).
fn palette(ui: &Ui, controller: &Controller) -> Vec<Action> {
    actions::palette(ui, controller, "")
        .into_iter()
        .filter(|action| !action.recent && !matches!(action.run, Run::Setting { .. }))
        .collect()
}

/// Everything lazyrunner can do: every action as the palette lists it, and every setting with its state.
pub fn actions(ui: &Ui, controller: &Controller) -> Value {
    let list: Vec<Value> = palette(ui, controller)
        .iter()
        .map(|action| {
            json!({
                "key": action.key,
                "label": action.label,
                "hint": action.hint,
                "group": action.group,
                "while_building": action.safe,
                "focused_app": action.focused,
                "needs_confirm": needs_confirm(&action.run),
            })
        })
        .collect();
    let settings: Vec<Value> = Toggle::ALL
        .iter()
        .map(|toggle| json!({ "setting": toggle.key(), "on": controller.setting(*toggle) }))
        .collect();
    json!({
        "actions": list,
        "settings": settings,
        "hint": "run_action runs an action by its key (\"app\" focuses that app first); set_setting sets a setting; \
                 press_keys for what no tool covers",
    })
}

/// Runs a palette action by its key, on `app` when it acts on one, as the user's key would.
pub fn run_action(ui: &mut Ui, controller: &mut Controller, args: &Value) -> Result<String, Refusal> {
    let Some(key) = args["key"].as_str() else {
        return Err(("bad_args", "which action? (\"key\", from list_actions)".into()));
    };
    let focus = if args["app"].is_null() { ui.focus } else { app_index(controller, args)? };
    let before = std::mem::replace(&mut ui.focus, focus);
    let Some(action) = palette(ui, controller).into_iter().find(|action| action.key == key) else {
        ui.focus = before;
        return Err(("bad_args", format!("no action '{}' (list_actions has them)", clean(key, CLIENT_CHARS))));
    };
    if needs_confirm(&action.run) && !confirmed(args) {
        ui.focus = before;
        return Err(confirm_first(&action.label));
    }
    if controller.building.is_some() && !action.safe {
        ui.focus = before;
        return Err(("busy", format!("an app is building - wait for build_done, then {}", action.label)));
    }
    keys::run(ui, controller, &action.run);
    Ok(format!("{} ({key})", action.label))
}

/// The user is typing in a prompt: the palette, a search, a filter, a note, the theme picker's search.
fn typing(ui: &Ui) -> bool {
    let prompt = ui.search.as_ref().is_some_and(|search| search.typing)
        || ui.picker.as_ref().is_some_and(|picker| picker.filtering || picker.new_name.is_some())
        || matches!(&ui.overlay, Some(Overlay::Palette { .. } | Overlay::Themes { .. }))
        || matches!(&ui.overlay, Some(Overlay::Marks { note: Some(_), .. }))
        || matches!(&ui.overlay, Some(Overlay::Requests(view)) if view.typing);
    prompt && ui.user_key_at.is_some_and(|at| ui.now.saturating_duration_since(at) < TYPING)
}

/// Presses keys as the user would - in follow mode too - for what no tool covers.
pub fn press_keys(ui: &mut Ui, controller: &mut Controller, args: &Value) -> Result<String, Refusal> {
    let Some(spec) = args["keys"].as_str() else {
        return Err(("bad_args", "which keys? (\"keys\": \"g g\", \"Ctrl+o\", \"Esc\"…)".into()));
    };
    let pressed = crate::keys::parse(spec).map_err(|why| ("bad_args", why))?;
    if typing(ui) {
        return Err(("user_typing", "the user is typing in a prompt - try again in a moment".into()));
    }
    let risky = pressed.iter().any(|key| RISKY_KEYS.contains(&keys::key_name(key).as_str()));
    if risky && !confirmed(args) {
        return Err(confirm_first(&format!(
            "{} (they quit, kill, clear or take the screen over)",
            RISKY_KEYS.join(" ")
        )));
    }
    let (following, user_key_at) = (std::mem::replace(&mut ui.follow.on, false), ui.user_key_at);
    for key in pressed {
        keys::press(ui, controller, key);
    }
    ui.follow.on |= following;
    ui.user_key_at = user_key_at;
    Ok(format!("pressed {}", clean(spec, CLIENT_CHARS)))
}

/// Turns a setting on or off (saved), as `,` does.
pub fn set_setting(controller: &mut Controller, args: &Value) -> Result<String, Refusal> {
    let names = || Toggle::ALL.iter().map(|toggle| toggle.key()).collect::<Vec<_>>().join(", ");
    let wanted = args["setting"].as_str().unwrap_or_default();
    let Some(toggle) = Toggle::ALL.into_iter().find(|toggle| toggle.key().eq_ignore_ascii_case(wanted)) else {
        return Err(("bad_args", format!("\"setting\" is one of {}", names())));
    };
    let Some(on) = args["on"].as_bool() else {
        return Err(("bad_args", "\"on\": true or false".into()));
    };
    execute(controller, Command::SetToggle { toggle, on })?;
    Ok(format!("set {} {}", toggle.key(), if on { "on" } else { "off" }))
}

/// npm ci (`clean`, the default) or npm install for an npm app.
pub fn install_packages(controller: &mut Controller, args: &Value) -> Result<String, Refusal> {
    let app = &controller.apps[app_index(controller, args)?];
    let (key, name) = (app.key.clone(), app.name.clone());
    if !matches!(app.kind, AppKind::Npm { .. }) {
        return Err(("bad_args", format!("{name} isn't an npm app")));
    }
    let fresh = args["clean"].as_bool().unwrap_or(true);
    execute(controller, if fresh { Command::NpmCi(key) } else { Command::NpmInstall(key) })?;
    Ok(format!("{} for {name}", if fresh { "npm ci" } else { "npm install" }))
}

/// Kills what holds a port an app needs, then starts, as `!` does.
pub fn kill_port_holder(controller: &mut Controller, args: &Value) -> Result<String, Refusal> {
    if controller.conflicts.is_empty() {
        return Err(("nothing_to_do", "no port an app needs is taken".into()));
    }
    if !confirmed(args) {
        return Err(confirm_first("killing whatever holds the apps' ports"));
    }
    execute(controller, Command::KillPortUser)?;
    Ok("kill what holds the apps' ports, then start".into())
}

/// Makes a SQL connection string in the APIs' secrets.json the active one, as `d` does; the APIs restart on it.
pub fn switch_database(controller: &mut Controller, args: &Value) -> Result<String, Refusal> {
    let Some(path) = secrets::secrets_path(&controller.root, &controller.apps) else {
        return Err(("not_found", "no secrets.json: no API has a UserSecretsId".into()));
    };
    let text = std::fs::read_to_string(&path)
        .map_err(|error| ("not_found", format!("secrets.json couldn't be read: {error}")))?;
    let entries = secrets::connections(&text);
    let choices = entries.iter().map(|entry| format!("{} ({})", entry.label, entry.key)).collect::<Vec<_>>().join(", ");
    let Some(wanted) = args["name"].as_str() else {
        return Err(("bad_args", format!("which database? (\"name\"): {choices}")));
    };
    let key = args["key"].as_str();
    let found: Vec<&Entry> = entries
        .iter()
        .filter(|entry| entry.label.eq_ignore_ascii_case(wanted) || entry.database.eq_ignore_ascii_case(wanted))
        .filter(|entry| key.is_none_or(|key| entry.key.eq_ignore_ascii_case(key)))
        .collect();
    let entry = match found.as_slice() {
        [entry] => (*entry).clone(),
        [] => {
            return Err((
                "bad_args",
                format!("no database '{}' - the ones there: {choices}", clean(wanted, CLIENT_CHARS)),
            ));
        }
        _ => return Err(("bad_args", format!("several are called that - add \"key\": {choices}"))),
    };
    if entry.active {
        return Ok(format!("keep {} (already in use)", entry.label));
    }
    if !confirmed(args) {
        return Err(confirm_first(&format!("switching the database to {} (the APIs restart on it)", entry.label)));
    }
    controller.switch_database(&entry);
    Ok(format!("switch the database to {}", entry.label))
}

/// Applies a DbContext's pending migrations, or moves it to one (`to`): forward applies up to it, back reverts the
/// newer ones through their Down() - after `confirm`.
pub fn apply_migrations(controller: &mut Controller, args: &Value) -> Result<String, Refusal> {
    let Some(wanted) = args["context"].as_str() else {
        return Err(("bad_args", "which DbContext? (\"context\", from get_migrations)".into()));
    };
    if !controller.find_migrations() {
        return Err(("not_found", NO_MIGRATIONS.into()));
    }
    let Some(context) = controller
        .contexts
        .iter()
        .flatten()
        .find(|context| context.key().eq_ignore_ascii_case(wanted) || context.name.eq_ignore_ascii_case(wanted))
    else {
        let wanted = clean(wanted, CLIENT_CHARS);
        return Err(("bad_args", format!("no migration context '{wanted}' (get_migrations names them)")));
    };
    if context.is_busy() || context.status != Status::Ok || !context.known() {
        let why = format!("{} isn't checked yet - check_migrations it, then wait for migrations_checked", context.name);
        return Err(("not_checked", why));
    }
    let (target, what, back) = match args["to"].as_str() {
        None => {
            let pending = context.pending().len();
            if pending == 0 {
                return Err(("nothing_to_do", format!("{} has nothing pending", context.name)));
            }
            let plural = if pending == 1 { "" } else { "s" };
            (None, format!("applied {pending} pending migration{plural}"), false)
        }
        Some(to) => {
            let found = context
                .items
                .iter()
                .position(|migration| migration.id.eq_ignore_ascii_case(to) || migration.name.eq_ignore_ascii_case(to));
            let Some(at) = found else {
                return Err(("bad_args", format!("{} has no migration '{}'", context.name, clean(to, CLIENT_CHARS))));
            };
            let current = context.current();
            let name = &context.items[at].name;
            if current == Some(at) {
                return Err(("nothing_to_do", format!("{} is already at {name}", context.name)));
            }
            let back = current.is_some_and(|current| at < current);
            let what = if back { format!("reverted to {name}") } else { format!("applied up to {name}") };
            (Some(context.items[at].id.clone()), what, back)
        }
    };
    if back && !confirmed(args) {
        return Err(confirm_first("reverting migrations runs their Down(), which can drop tables and their data -"));
    }
    let (key, name) = (context.key(), context.name.clone());
    controller.migrate(&key, target, what.clone());
    Ok(format!("{name}: {what} - wait for migrations_checked"))
}
