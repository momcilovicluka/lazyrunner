//! set_log_level (#112): a .NET app's log category at another level for a while, live - through its user secrets,
//! without a restart - or put back now (`"level": "reset"`). The log says who set what; the app's title shows it
//! while it lasts.

use std::time::Duration;

use lazyrunner_core::output::Hue;
use lazyrunner_core::run::Controller;
use lazyrunner_core::run::live_level::{self, DEFAULT, LEVELS, MINUTES, MOST_MINUTES};
use serde_json::{Value, json};

use super::{Answer, CLIENT_CHARS, Refusal, app_index, clean};

const RESET: &str = "reset";
const UNTIL: &str = "%H:%M";
const SERILOG: &str = "it logs through Serilog, so Serilog:MinimumLevel was set too - Serilog reads that live only when \
                       it reloads its configuration, else from a restart, which ends this";
const MADE: &str = "its user secrets file was made for this: an app reads a new one from its next start when its \
                    folder wasn't there as it started";

/// Sets `"category"` of `"app"` at `"level"` for `"minutes"`, or puts its live levels back (`"level": "reset"`; one
/// category's, or all of them without `"category"`).
pub fn set(controller: &mut Controller, args: &Value, client: &str) -> Answer {
    let index = app_index(controller, args)?;
    let (key, name) = (controller.apps[index].key.clone(), controller.apps[index].name.clone());
    let category = category(args)?;
    let Some(level) = args["level"].as_str() else { return Err(bad_level()) };
    if level.trim().eq_ignore_ascii_case(RESET) {
        return reset(controller, (&key, &name), category.as_deref(), client);
    }
    let Some(category) = category else {
        return Err(("bad_args", "which \"category\"? e.g. Microsoft.EntityFrameworkCore.Database.Command".into()));
    };
    let level = live_level::level(level).ok_or_else(bad_level)?;
    let minutes = match &args["minutes"] {
        Value::Null => MINUTES,
        value => value
            .as_u64()
            .filter(|minutes| (1..=MOST_MINUTES).contains(minutes))
            .ok_or_else(|| ("bad_args", format!("\"minutes\" is 1 to {MOST_MINUTES}")))?,
    };
    let set = controller.set_live_level(&key, &category, level, minutes).map_err(|why| ("refused", why))?;
    controller.add_log(format!("▸ {client}: {}", set.said), Hue::Magenta);
    let mut notes = Vec::new();
    if set.serilog {
        notes.push(SERILOG.to_string());
    }
    if set.made {
        notes.push(MADE.to_string());
    }
    if !set.shared.is_empty() {
        let verb = if set.shared.len() == 1 { "reads" } else { "read" };
        notes.push(format!("{} {verb} the same user secrets: the level is theirs too", set.shared.join(" and ")));
    }
    let until = jiff::ZonedDateTime::now().checked_add(Duration::from_secs(minutes * 60)).ok();
    Ok(json!({
        "doing": set.said,
        "until": until.map(|until| until.strftime(UNTIL).to_string()),
        "note": (!notes.is_empty()).then(|| notes.join("; ")),
    }))
}

/// `"category"`, checked: a dotted name, `Default` spelled as .NET does.
fn category(args: &Value) -> Result<Option<String>, Refusal> {
    let category = match &args["category"] {
        Value::Null => return Ok(None),
        Value::String(category) => category.trim(),
        _ => return Err(("bad_args", "\"category\" is a log category's name".into())),
    };
    if !live_level::is_category(category) {
        let why = format!(
            "'{}' isn't a log category: a dotted name such as Microsoft.EntityFrameworkCore.Database.Command, or \
             Default",
            clean(category, CLIENT_CHARS)
        );
        return Err(("bad_args", why));
    }
    Ok(Some(if category.eq_ignore_ascii_case(DEFAULT) { DEFAULT.to_string() } else { category.to_string() }))
}

fn bad_level() -> Refusal {
    ("bad_args", format!("\"level\" is one of {}, or \"reset\"", LEVELS.join(", ")))
}

/// Puts app `key`'s live levels back now: `category`'s, or every one.
fn reset(controller: &mut Controller, (key, name): (&str, &str), category: Option<&str>, client: &str) -> Answer {
    let live = controller
        .live_levels()
        .iter()
        .filter(|live| live.app == key && category.is_none_or(|category| live.category.eq_ignore_ascii_case(category)))
        .count();
    let which = category.map_or(String::new(), |category| format!(" for {category}"));
    if live == 0 {
        return Err(("not_found", format!("{name} has no live log level{which}")));
    }
    let doing = match category {
        Some(category) => format!("takes {name} back to its own level for {category}"),
        None => format!("takes {name} back to its own log levels"),
    };
    controller.add_log(format!("▸ {client}: {doing}"), Hue::Magenta);
    controller.reset_live_levels(key, category);
    Ok(json!({ "doing": doing }))
}
