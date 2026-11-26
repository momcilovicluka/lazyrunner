//! What `wait` asks for `request` and `error` (#112): the next request or error that comes after the wait began, so an
//! agent that asks the user to reproduce a bug gets the request or error the user made, not one it guesses. The first
//! ask (no `since`) says where they are now - and the log tells the user to go ahead; each one after, what came since.

use std::time::Instant;

use lazyrunner_core::output::Hue;
use lazyrunner_core::run::Controller;
use serde_json::{Map, Value, json};

use super::{Answer, CLIENT_CHARS, app_index, clean, matching, request_row};
use crate::screen::copy;

/// The request a wait is for, in words: `failing POST request to /api/orders in Orders`.
pub(crate) fn described(controller: &Controller, args: &Value) -> String {
    let mut said = String::new();
    if args["errors_only"].as_bool().unwrap_or(false) {
        said.push_str("failing ");
    }
    if let Some(method) = args["method"].as_str() {
        said.push_str(&format!("{} ", clean(method, CLIENT_CHARS).to_uppercase()));
    }
    said.push_str("request");
    if let Some(to) = args["endpoint"].as_str().or(args["path"].as_str()) {
        said.push_str(&format!(" to {}", clean(to, CLIENT_CHARS)));
    }
    if let Some(app) = app_name(controller, args) {
        said.push_str(&format!(" in {app}"));
    }
    said
}

fn app_name(controller: &Controller, args: &Value) -> Option<String> {
    args["app"].as_str()?;
    app_index(controller, args).ok().map(|index| controller.apps[index].name.clone())
}

/// The oldest request after `since` (a request's id) that get_requests' filters let through, and how many more came;
/// without `since`, the id the wait starts from, the log saying what it waits for.
pub fn next_request(controller: &mut Controller, args: &Value, client: &str) -> Answer {
    let found = matching(controller, args)?;
    let next = controller.requests.seq;
    if args["since"].as_u64().is_none() {
        let wanted = described(controller, args);
        controller.add_log(format!("▸ {client}: waits for your next {wanted} - reproduce it now"), Hue::Magenta);
        return Ok(json!({ "next": next, "doing": format!("waits for the next {wanted}") }));
    }
    let Some(first) = found.last() else { return Ok(json!({ "next": next, "request": null })) };
    Ok(json!({ "next": next, "request": request_row(first), "more": found.len() - 1 }))
}

/// The oldest error after `since` (each app's line number, by its key) in one app or any, and how many more came;
/// without `since`, each app's newest line, the log saying what it waits for.
pub fn next_error(now: Instant, controller: &mut Controller, args: &Value, client: &str) -> Answer {
    let apps: Vec<usize> = match &args["app"] {
        Value::Null => (0..controller.apps.len()).collect(),
        _ => vec![app_index(controller, args)?],
    };
    let next: Map<String, Value> = apps
        .iter()
        .map(|&app| (controller.apps[app].key.clone(), json!(controller.states[app].output.total())))
        .collect();
    let Some(since) = args["since"].as_object() else {
        let whose = match apps.as_slice() {
            [app] => controller.apps[*app].name.clone(),
            _ => "any app".to_string(),
        };
        controller.add_log(format!("▸ {client}: waits for the next error in {whose} - reproduce it now"), Hue::Magenta);
        return Ok(json!({ "next": next, "doing": format!("waits for the next error in {whose}") }));
    };
    let mut found: Vec<(usize, copy::Found)> = apps
        .iter()
        .flat_map(|&app| {
            let total = controller.states[app].output.total();
            let after = since.get(&controller.apps[app].key).and_then(Value::as_u64).unwrap_or(total);
            copy::errors_after(&controller.states[app], after).into_iter().map(move |error| (app, error))
        })
        .collect();
    found.sort_by_key(|(_, error)| error.born);
    let Some((app, first)) = found.first() else { return Ok(json!({ "next": next, "error": null })) };
    let ago = now.saturating_duration_since(first.born);
    let at = jiff::ZonedDateTime::now().checked_sub(ago).unwrap_or_else(|_| jiff::ZonedDateTime::now());
    let error = json!({
        "app": controller.apps[*app].key,
        "line": first.line,
        "at": at.strftime(lazyrunner_core::time::RFC_3339_SECONDS).to_string(),
        "text": first.lines.join("\n"),
    });
    Ok(json!({ "next": next, "error": error, "more": found.len() - 1 }))
}
