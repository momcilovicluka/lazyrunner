//! replay_request (#112): a request from get_requests sent again to its app on this machine, with the headers and body
//! the agent gives - the app's log has neither. lazyrunner says where it goes (and asks first when it repeats what the
//! request changed); it's sent from here, so the values never reach lazyrunner's screen or log, and the answer waits a
//! moment for the app to log it, to compare the two. Secrets in the headers are masked in the answer.

use std::time::{Duration, Instant};

use lazyrunner_core::run::requests;
use lazyrunner_core::run::secrets::secret_header;
use serde_json::{Map, Value, json};

use super::{Channel, Server};

/// How long the app has to log the request sent again, and how often lazyrunner is asked whether it has.
const LOGGED_WAIT: Duration = Duration::from_secs(3);
const LOGGED_POLL: Duration = Duration::from_millis(200);
const CONTENT_TYPE: &str = "Content-Type";
const MASKED: &str = "***";
const NO_HEADERS: &str = "headers aren't recorded, so none were sent: pass headers (an Authorization header the user \
                          gives you) when the endpoint needs them";

/// What the agent sends along: headers, each its name and value, and a body.
#[derive(Debug, Default, PartialEq)]
struct Along {
    headers: Vec<(String, String)>,
    body: Option<String>,
}

/// The headers and body in a call's arguments, checked before lazyrunner is asked.
fn along(args: &Value) -> Result<Along, String> {
    let headers = match &args["headers"] {
        Value::Null => Vec::new(),
        Value::Object(map) => map
            .iter()
            .map(|(name, value)| match value.as_str() {
                Some(value) => Ok((name.clone(), value.to_string())),
                None => Err(format!("bad_args: the header {name}'s value is a string")),
            })
            .collect::<Result<_, _>>()?,
        _ => return Err("bad_args: \"headers\" is an object of names and values".into()),
    };
    let body = match &args["body"] {
        Value::Null => None,
        Value::String(body) => Some(body.clone()),
        _ => return Err("bad_args: \"body\" is a string (JSON as its text)".into()),
    };
    Ok(Along { headers, body })
}

/// What lazyrunner is told: the request and `confirm`, the headers' names and the body's size - never their values.
fn told(args: &Value, along: &Along) -> Value {
    let mut told = json!({ "id": args["id"], "confirm": args["confirm"] });
    if !along.headers.is_empty() {
        told["headers"] = json!(along.headers.iter().map(|(name, _)| name).collect::<Vec<_>>());
    }
    if let Some(body) = &along.body {
        told["body_bytes"] = json!(body.len());
    }
    told
}

/// The headers sent: the agent's, and with a body and no content type of its own, the one the request was logged with.
fn headers(along: &Along, logged_type: &str) -> Vec<(String, String)> {
    let mut headers = along.headers.clone();
    let typed = headers.iter().any(|(name, _)| name.eq_ignore_ascii_case(CONTENT_TYPE));
    if along.body.is_some() && !typed && !logged_type.is_empty() {
        headers.push((CONTENT_TYPE.to_string(), logged_type.to_string()));
    }
    headers
}

/// The headers as an answer says them: each secret one masked.
fn shown(headers: &[(String, String)]) -> Value {
    let shown: Map<String, Value> = headers
        .iter()
        .map(|(name, value)| (name.clone(), json!(if secret_header(name) { MASKED } else { value.as_str() })))
        .collect();
    Value::Object(shown)
}

/// Milliseconds to one decimal.
fn ms(ms: f64) -> f64 {
    (ms * 10.0).round() / 10.0
}

/// What the answer notes: headers or a body the log doesn't have and that weren't given; the app not having logged it.
fn notes(along: &Along, recorded_body: &str, since: &Value, logged: bool) -> Vec<String> {
    let mut notes = Vec::new();
    if along.headers.is_empty() {
        notes.push(NO_HEADERS.to_string());
    }
    if along.body.is_none() && recorded_body.parse::<u64>().is_ok_and(|bytes| bytes > 0) {
        notes.push(format!("its {recorded_body}-byte body isn't recorded, so none was sent: pass body"));
    }
    if !logged {
        notes.push(format!("its app hasn't logged it yet: get_requests with since {since} lists it"));
    }
    notes
}

impl<C: Channel> Server<C> {
    /// replay_request: asks lazyrunner where it goes, sends it, and answers with the request the app logged for it.
    pub(super) async fn replay(&self, args: Value) -> Result<Value, String> {
        let along = along(&args)?;
        let plan = self.agent("replay", told(&args, &along)).await?;
        let (method, url) = (plan["method"].as_str().unwrap_or_default(), plan["url"].as_str().unwrap_or_default());
        let headers = headers(&along, plan["content_type"].as_str().unwrap_or_default());
        let (how, to, with, body) = (method.to_string(), url.to_string(), headers.clone(), along.body.clone());
        let sent = tokio::task::spawn_blocking(move || requests::send(&how, &to, &with, body.as_deref()))
            .await
            .map_err(|error| format!("not_sent: {method} {url} - {error}"))?;
        let (status, took) = sent.map_err(|why| format!("not_sent: {method} {url} - {why}"))?;
        let mut answer = json!({
            "id": null,
            "status": status,
            "ms": ms(took),
            "sent": {
                "method": method,
                "url": url,
                "headers": shown(&headers),
                "body_bytes": along.body.as_ref().map_or(0, String::len),
            },
        });
        let logged = self.logged(&args["id"], &plan["since"]).await;
        if let Some(logged) = &logged {
            for field in ["id", "status", "ms", "compare"] {
                answer[field] = logged[field].clone();
            }
        }
        let recorded = plan["recorded_body"].as_str().unwrap_or_default();
        let notes = notes(&along, recorded, &plan["since"], logged.is_some());
        if !notes.is_empty() {
            answer["note"] = json!(notes.join("; "));
        }
        Ok(answer)
    }

    /// The request sent again as the app logged it, once it has (a moment at most); `None` when it hasn't or
    /// lazyrunner can't say.
    async fn logged(&self, id: &Value, since: &Value) -> Option<Value> {
        let started = Instant::now();
        loop {
            let answer = self.agent("replayed", json!({ "id": id, "since": since })).await;
            match answer {
                Ok(found) if !found["id"].is_null() => return Some(found),
                Ok(_) => {}
                Err(why) if why == super::NOT_ANSWERING => {}
                Err(_) => return None,
            }
            if started.elapsed() >= LOGGED_WAIT {
                return None;
            }
            tokio::time::sleep(LOGGED_POLL).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headers_and_a_body_are_checked_and_lazyrunner_hears_only_their_names_and_size() {
        let args = json!({ "id": 7, "confirm": true, "body": "{\"qty\":2}",
            "headers": { "Authorization": "Bearer abc", "X-Tenant": "shop" } });
        let along = along(&args).unwrap();
        assert_eq!(along.body.as_deref(), Some("{\"qty\":2}"));
        let told = told(&args, &along);
        assert_eq!(
            told,
            json!({ "id": 7, "confirm": true, "headers": ["Authorization", "X-Tenant"], "body_bytes": 9 })
        );
        assert!(!told.to_string().contains("Bearer"), "no value reaches lazyrunner");
        assert!(super::along(&json!({ "headers": ["Authorization"] })).unwrap_err().starts_with("bad_args:"));
        assert!(super::along(&json!({ "headers": { "X-Count": 2 } })).unwrap_err().contains("X-Count"));
        assert!(super::along(&json!({ "body": { "qty": 2 } })).unwrap_err().starts_with("bad_args: \"body\""));
        assert_eq!(super::along(&json!({ "id": 7 })).unwrap(), Along::default());
    }

    #[test]
    fn a_body_goes_with_its_logged_content_type_and_secrets_are_masked_in_the_answer() {
        let along = Along {
            headers: vec![("Authorization".into(), "Bearer abc".into()), ("X-Api-Key".into(), "k1".into())],
            body: Some("{}".into()),
        };
        let sent = headers(&along, "application/json");
        assert_eq!(sent.last().unwrap(), &("Content-Type".to_string(), "application/json".to_string()));
        assert_eq!(
            shown(&sent),
            json!({ "Authorization": "***", "X-Api-Key": "***", "Content-Type": "application/json" })
        );
        let typed = Along { headers: vec![("content-type".into(), "text/plain".into())], body: Some("hi".into()) };
        assert_eq!(headers(&typed, "application/json").len(), 1, "its own content type stays");
        assert!(headers(&Along::default(), "application/json").is_empty(), "no body, no content type");
    }

    #[test]
    fn the_notes_say_what_the_log_lacked_and_where_a_request_not_logged_yet_shows() {
        let said = notes(&Along::default(), "52", &json!(40), false);
        assert_eq!(said.len(), 3);
        assert!(said[0].starts_with("headers aren't recorded"));
        assert_eq!(said[1], "its 52-byte body isn't recorded, so none was sent: pass body");
        assert_eq!(said[2], "its app hasn't logged it yet: get_requests with since 40 lists it");
        let given = Along { headers: vec![("X-Tenant".into(), "shop".into())], body: None };
        assert!(notes(&given, "", &json!(40), true).is_empty());
    }
}
