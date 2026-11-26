//! `replay_request` (#112): a request from get_requests sent again to its app. lazyrunner checks it and says where it
//! goes (`replay`) - asking the user first when it repeats what the request changed - then `lazyrunner mcp` sends it,
//! so the headers and body it's given never reach the screen or the log, and asks whether the app logged it yet
//! (`replayed`): the new request against the one it repeats.

use lazyrunner_core::output::Hue;
use lazyrunner_core::run::Controller;
use lazyrunner_core::run::requests::{self, REPLAYABLE, Request};
use serde_json::{Value, json};

use super::drive::{confirm_first, confirmed};
use super::{Answer, CLIENT_CHARS, Refusal, clean, ms, views};

fn find(controller: &Controller, args: &Value) -> Result<Request, Refusal> {
    let Some(id) = args["id"].as_u64() else {
        return Err(("bad_args", "which request? (\"id\", from get_requests)".into()));
    };
    let found = controller.requests.items.iter().find(|request| request.seq == id).cloned();
    found.ok_or_else(|| ("not_found", format!("no request {id}: lazyrunner keeps the newest ones while it runs")))
}

/// What the agent sends along, in words for the log: the headers' names (never their values) and the body's size.
fn along(args: &Value) -> String {
    let names: Vec<String> = args["headers"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(|name| clean(name, CLIENT_CHARS))
        .collect();
    let mut parts = Vec::new();
    if !names.is_empty() {
        parts.push(format!("headers {}", names.join(", ")));
    }
    if let Some(bytes) = args["body_bytes"].as_u64() {
        parts.push(format!("a {bytes}-byte body"));
    }
    if parts.is_empty() { String::new() } else { format!(" ({})", parts.join("; ")) }
}

/// Where a request is sent again and how - its app running, on its own port; one that isn't a GET, HEAD or OPTIONS
/// after `"confirm": true` - with the log saying so.
pub fn plan(controller: &mut Controller, args: &Value, client: &str) -> Answer {
    let request = find(controller, args)?;
    let (method, path) = (request.method.as_str(), request.path.as_str());
    let name = controller.app(&request.app).map_or(request.app.clone(), |app| app.name.clone());
    if !controller.is_running(&request.app) {
        return Err(("refused", format!("{name} isn't running - start_apps it, then send {method} {path} again")));
    }
    let Some(url) = requests::local_url(&request, controller.port(&request.app)) else {
        return Err(("not_found", format!("lazyrunner doesn't know {name}'s port, so it can't send {method} {path}")));
    };
    if !REPLAYABLE.contains(&method) && !confirmed(args) {
        return Err(confirm_first(&format!("sending {method} {path} again (it repeats what the request changed)")));
    }
    let doing = format!("send {method} {path} again{}", along(args));
    controller.add_log(format!("▸ {client}: {doing}"), Hue::Magenta);
    Ok(json!({
        "doing": doing,
        "app": request.app,
        "method": method,
        "url": url,
        "since": controller.requests.seq,
        "content_type": request.sent_content,
        "recorded_body": request.sent_length,
    }))
}

/// The request sent again, once its app has logged it - its id, status and time, and how it compares with the one it
/// repeats; `id` null until then.
pub fn replayed(controller: &Controller, args: &Value) -> Answer {
    let original = find(controller, args)?;
    let since = args["since"].as_u64().unwrap_or(original.seq);
    let found = controller.requests.items.iter().find(|request| {
        request.seq > since
            && request.app == original.app
            && request.method == original.method
            && request.path == original.path
    });
    let Some(again) = found else { return Ok(json!({ "id": null })) };
    let compare = views::compare(again, &original);
    let differs: Vec<Value> = compare["rows"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|row| row["differs"] == true)
        .map(|row| json!({ "what": row["what"], "this": row["this"], "baseline": row["baseline"] }))
        .collect();
    Ok(json!({
        "id": again.seq,
        "status": again.status,
        "ms": ms(again.ms),
        "compare": { "baseline": original.seq, "digest": compare["digest"], "differs": differs },
    }))
}
