//! wait for `request` or `error` (#112): the next one that comes after the wait began - the agent asks the user to
//! reproduce a bug and gets the request (narrowed as get_requests narrows) or the error the user made, not one it
//! guesses from the list afterwards. lazyrunner says where its requests or each app's output are when the wait begins,
//! and is asked what came after, until something did or the time is up.

use std::time::Instant;

use serde_json::{Map, Value, json};
use tokio::sync::mpsc::UnboundedSender;

use super::{Channel, NOT_ANSWERING, Server, WAIT_POLL, progress};

/// The conditions that wait for what comes next: each its lazyrunner op and the arguments that narrow it.
pub const NEXT: [(&str, &str, &[&str]); 2] = [
    ("request", "next_request", &["app", "method", "path", "endpoint", "errors_only"]),
    ("error", "next_error", &["app"]),
];

/// The answer once the request or error came: it, what to call for the rest, and how many more came.
fn found(waited: u64, answer: &Value) -> Option<Value> {
    let more = answer["more"].as_u64().unwrap_or_default();
    let mut said = if let Some(request) = answer["request"].as_object() {
        let id = &request["id"];
        format!(
            "{} {} answered {} in {} ms ({}) - get_request with id {id} has it in full",
            text(&request["method"]),
            text(&request["path"]),
            request["status"],
            request["ms"],
            text(&request["app"])
        )
    } else {
        let error = answer["error"].as_object()?;
        let app = text(&error["app"]);
        let first = error["text"].as_str().and_then(|text| text.lines().next()).unwrap_or_default().trim();
        let line = error["line"].as_u64().unwrap_or(1);
        format!(
            "{app}: {first} - get_errors with app {app} has it with its stack, get_logs with since {} its lines",
            line.saturating_sub(1)
        )
    };
    if more > 0 {
        said.push_str(&format!("; {more} more came after it"));
    }
    let mut done = json!({ "done": true, "waited_sec": waited, "summary": said, "more": more });
    for field in ["request", "error"] {
        if answer[field].is_object() {
            done[field] = answer[field].clone();
        }
    }
    Some(done)
}

fn text(value: &Value) -> &str {
    value.as_str().unwrap_or_default()
}

/// The answer when nothing came in time: how to wait again without missing one.
fn timed_out(condition: &str, waited: u64, next: &Value) -> Value {
    let summary = match condition {
        "request" => format!(
            "no matching request came in {waited}s - call wait again; get_requests with since {next} lists any that \
             come before it does"
        ),
        _ => format!("no error came in {waited}s - call wait again"),
    };
    json!({ "done": false, "waited_sec": waited, "summary": summary })
}

impl<C: Channel> Server<C> {
    /// Asks lazyrunner where things are now, then what came after, until what `op` looks for did or `limit` seconds
    /// passed (an ask lazyrunner didn't answer counts as nothing yet); tells a client that asked for progress what it
    /// waits for, every ask.
    pub(super) async fn next(
        &self,
        (condition, op, narrowed_by): (&str, &str, &[&str]),
        args: &Value,
        limit: u64,
        token: &Value,
        out: &UnboundedSender<Value>,
    ) -> Result<Value, String> {
        let mut asked: Map<String, Value> = narrowed_by
            .iter()
            .filter_map(|name| {
                args.get(*name).filter(|value| !value.is_null()).map(|value| (name.to_string(), value.clone()))
            })
            .collect();
        let start = self.agent(op, Value::Object(asked.clone())).await?;
        let doing = start["doing"].as_str().unwrap_or("waits").to_string();
        let message = doing.strip_prefix("waits").map_or(doing.clone(), |rest| format!("waiting{rest}"));
        asked.insert("since".into(), start["next"].clone());
        let (asked, started) = (Value::Object(asked), Instant::now());
        let mut next = start["next"].clone();
        let mut polls = 0u64;
        loop {
            tokio::time::sleep(WAIT_POLL).await;
            let waited = started.elapsed().as_secs();
            match self.agent(op, asked.clone()).await {
                Ok(answer) => {
                    if let Some(done) = found(waited, &answer) {
                        return Ok(done);
                    }
                    next = answer["next"].clone();
                }
                Err(why) if why == NOT_ANSWERING => {}
                Err(why) => return Err(why),
            }
            if waited >= limit {
                return Ok(timed_out(condition, waited, &next));
            }
            polls += 1;
            progress(out, token, polls, &message);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_or_an_error_that_came_says_what_it_was_and_where_the_rest_is() {
        let request = json!({ "id": 7, "app": "orders", "method": "POST", "path": "/api/orders", "status": 500,
            "ms": 48.5, "endpoint": "/api/orders" });
        let said = found(3, &json!({ "next": 9, "request": request, "more": 2 })).unwrap();
        assert_eq!(
            said["summary"],
            "POST /api/orders answered 500 in 48.5 ms (orders) - get_request with id 7 has it in full; 2 more came \
             after it"
        );
        assert_eq!((said["done"].clone(), said["request"]["id"].clone()), (json!(true), json!(7)));
        let error = json!({ "app": "billing", "line": 41, "at": "2026-11-26T09:41:07+01:00",
            "text": "fail: Billing.Api.Invoices[0]\n      System.InvalidOperationException: no currency" });
        let said = found(1, &json!({ "next": {}, "error": error, "more": 0 })).unwrap();
        assert_eq!(
            said["summary"],
            "billing: fail: Billing.Api.Invoices[0] - get_errors with app billing has it with its stack, get_logs \
             with since 40 its lines"
        );
        assert_eq!(said["error"]["line"], 41);
        assert_eq!(found(1, &json!({ "next": 9, "request": null })), None);
        assert_eq!(found(1, &json!({ "next": {}, "error": null })), None);
    }

    #[test]
    fn nothing_in_time_says_how_to_wait_again_without_missing_one() {
        let said = timed_out("request", 120, &json!(9));
        assert_eq!(said["done"], false);
        assert!(
            said["summary"].as_str().unwrap().ends_with("get_requests with since 9 lists any that come before it does")
        );
        assert_eq!(timed_out("error", 5, &json!({}))["summary"], "no error came in 5s - call wait again");
    }
}
