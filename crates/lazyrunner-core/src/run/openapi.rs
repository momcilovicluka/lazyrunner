//! A running .NET app's endpoints from its OpenAPI document (#100): fetched from where Swashbuckle or ASP.NET Core's
//! own OpenAPI serve it, each operation with its route, summary, tag, parameters and an example body made from its
//! schema; and sending one, filled in, to the app - any method, with a body.

use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};

/// Where an app serves its document, tried in turn after the profile's Swagger path.
pub const DOCUMENTS: [&str; 3] = ["/swagger/v1/swagger.json", "/openapi/v1.json", "/swagger.json"];
const SWAGGER_DOCUMENT: &str = "/v1/swagger.json";
const FETCH_TIMEOUT: Duration = Duration::from_secs(4);
const SEND_TIMEOUT: Duration = Duration::from_secs(30);
const METHODS: [&str; 7] = ["get", "post", "put", "patch", "delete", "head", "options"];
/// How deep an example body follows nested schemas.
const EXAMPLE_DEPTH: usize = 4;
const JSON: &str = "application/json";

/// Where a parameter goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    Path,
    Query,
    Header,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Param {
    pub name: String,
    pub place: Place,
    pub required: bool,
    /// Its example or default, as text ("" when it has none).
    pub example: String,
}

/// An endpoint as the document describes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Operation {
    pub method: String,
    pub path: String,
    pub summary: String,
    pub tag: String,
    pub params: Vec<Param>,
    /// An example JSON body, when it takes one.
    pub body: Option<String>,
}

/// Fetches app's document from `port` (its profile's Swagger path first) and reads its operations; where it was found.
pub fn fetch(port: u16, swagger: Option<&str>) -> Result<(String, Vec<Operation>), String> {
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(FETCH_TIMEOUT)).build().into();
    let own = swagger.map(|path| format!("{}{SWAGGER_DOCUMENT}", path.trim_end_matches('/')));
    let mut tried = Vec::new();
    for path in own.into_iter().chain(DOCUMENTS.iter().map(|path| path.to_string())) {
        let url = format!("http://localhost:{port}{path}");
        let Ok(mut response) = agent.get(&url).call() else {
            tried.push(path);
            continue;
        };
        let text = response.body_mut().read_to_string().map_err(|error| error.to_string())?;
        match serde_json::from_str::<Value>(crate::json::strip_bom(&text)) {
            Ok(document) if document.get("paths").is_some() => return Ok((path, parse(&document))),
            _ => tried.push(path),
        }
    }
    Err(format!("no OpenAPI document at {}", tried.join(", ")))
}

/// The document's operations, by tag, then route, then method.
pub fn parse(document: &Value) -> Vec<Operation> {
    let mut operations = Vec::new();
    for (path, item) in document["paths"].as_object().into_iter().flatten() {
        let shared: Vec<&Value> = item["parameters"].as_array().into_iter().flatten().collect();
        for method in METHODS {
            let Some(operation) = item.get(method).filter(|operation| operation.is_object()) else { continue };
            let own = operation["parameters"].as_array().into_iter().flatten();
            let params = shared.iter().copied().chain(own).filter_map(|param| to_param(document, param)).collect();
            let summary = operation["summary"].as_str().or_else(|| operation["operationId"].as_str()).unwrap_or("");
            operations.push(Operation {
                method: method.to_uppercase(),
                path: path.clone(),
                summary: summary.to_string(),
                tag: operation["tags"][0].as_str().unwrap_or("other").to_string(),
                params,
                body: body_example(document, &operation["requestBody"]),
            });
        }
    }
    let rank =
        |method: &str| METHODS.iter().position(|each| each.eq_ignore_ascii_case(method)).unwrap_or(METHODS.len());
    operations.sort_by(|a, b| (&a.tag, &a.path, rank(&a.method)).cmp(&(&b.tag, &b.path, rank(&b.method))));
    operations
}

/// A `$ref` within the document, followed.
fn resolve<'a>(document: &'a Value, value: &'a Value) -> &'a Value {
    match value["$ref"].as_str().and_then(|reference| reference.strip_prefix('#')) {
        Some(pointer) => document.pointer(pointer).unwrap_or(value),
        None => value,
    }
}

fn to_param(document: &Value, param: &Value) -> Option<Param> {
    let param = resolve(document, param);
    let place = match param["in"].as_str()? {
        "path" => Place::Path,
        "query" => Place::Query,
        "header" => Place::Header,
        _ => return None,
    };
    let schema = resolve(document, &param["schema"]);
    let example = [&param["example"], &schema["example"], &schema["default"]]
        .into_iter()
        .find(|value| !value.is_null())
        .map_or(String::new(), |value| value.as_str().map_or_else(|| value.to_string(), str::to_string));
    let required = place == Place::Path || param["required"].as_bool().unwrap_or(false);
    Some(Param { name: param["name"].as_str()?.to_string(), place, required, example })
}

/// A JSON request body's example: the document's own, else one made from its schema.
fn body_example(document: &Value, body: &Value) -> Option<String> {
    let body = resolve(document, body);
    let content = body["content"].as_object()?;
    let media = content.iter().find(|(kind, _)| kind.contains("json")).map(|(_, media)| media)?;
    let named = media["examples"].as_object().and_then(|all| all.values().next()).map(|first| first["value"].clone());
    let given = Some(media["example"].clone()).filter(|example| !example.is_null());
    let given = given.or(named.filter(|example| !example.is_null()));
    let example = given.unwrap_or_else(|| from_schema(document, &media["schema"], EXAMPLE_DEPTH));
    serde_json::to_string(&example).ok()
}

/// A value shaped like `schema`: its example, else a blank of its type, objects with their properties.
fn from_schema(document: &Value, schema: &Value, depth: usize) -> Value {
    let schema = resolve(document, schema);
    if let Some(example) = schema.get("example") {
        return example.clone();
    }
    if depth == 0 {
        return Value::Null;
    }
    match schema["type"].as_str() {
        Some("object") | None if schema.get("properties").is_some() => {
            let fields = schema["properties"].as_object().into_iter().flatten();
            let filled = fields.map(|(name, field)| (name.clone(), from_schema(document, field, depth - 1)));
            Value::Object(filled.collect::<Map<_, _>>())
        }
        Some("array") => json!([from_schema(document, &schema["items"], depth - 1)]),
        Some("integer" | "number") => json!(0),
        Some("boolean") => json!(false),
        Some("string") => json!(""),
        _ => {
            let first = schema.get("allOf").and_then(|all| all.get(0));
            first.map_or(Value::Null, |first| from_schema(document, first, depth - 1))
        }
    }
}

/// The URL `operation` goes to on `port` with these values (by parameter name): path parameters put in, query ones
/// added when given.
pub fn url(port: u16, operation: &Operation, values: &[(String, String)]) -> String {
    let value = |name: &str| values.iter().find(|(each, _)| each == name).map_or("", |(_, value)| value.as_str());
    let mut path = operation.path.clone();
    for param in operation.params.iter().filter(|param| param.place == Place::Path) {
        path = path.replace(&format!("{{{}}}", param.name), &encode(value(&param.name)));
    }
    let query: Vec<String> = operation
        .params
        .iter()
        .filter(|param| param.place == Place::Query && !value(&param.name).is_empty())
        .map(|param| format!("{}={}", encode(&param.name), encode(value(&param.name))))
        .collect();
    let query = if query.is_empty() { String::new() } else { format!("?{}", query.join("&")) };
    format!("http://localhost:{port}{path}{query}")
}

/// Percent-encodes what a URL can't carry as it is.
fn encode(text: &str) -> String {
    text.bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (byte as char).to_string(),
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// What came back from a send: its status, how long it took (ms), and the start of its body.
#[derive(Clone, Debug, PartialEq)]
pub struct Sent {
    pub status: u16,
    pub ms: f64,
    pub body: String,
}

/// How much of an answer's body is kept.
const MOST_BODY: u64 = 4096;

/// Sends `method` to `url` with these headers and, when given, a JSON body: what came back.
pub fn send(method: &str, url: &str, headers: &[(String, String)], body: Option<&str>) -> Result<Sent, String> {
    let agent: ureq::Agent =
        ureq::Agent::config_builder().timeout_global(Some(SEND_TIMEOUT)).http_status_as_error(false).build().into();
    let mut request = ureq::http::Request::builder().method(method).uri(url);
    for (name, value) in headers.iter().filter(|(_, value)| !value.is_empty()) {
        request = request.header(name.as_str(), value.as_str());
    }
    if body.is_some() {
        request = request.header("Content-Type", JSON);
    }
    let request = request.body(body.unwrap_or_default().to_string()).map_err(|error| error.to_string())?;
    let started = Instant::now();
    let mut response = agent.run(request).map_err(|error| error.to_string())?;
    let ms = started.elapsed().as_secs_f64() * 1000.0;
    let status = response.status().as_u16();
    let mut answer = String::new();
    let mut reader = std::io::Read::take(response.body_mut().as_reader(), MOST_BODY);
    let _ = std::io::Read::read_to_string(&mut reader, &mut answer);
    Ok(Sent { status, ms, body: answer })
}

/// A route as the requests view groups them, its parameters' names left out: `/api/orders/{}`.
pub fn route_key(path: &str) -> String {
    let mut key = String::new();
    let mut inside = false;
    for c in path.to_lowercase().chars() {
        match c {
            '{' => {
                inside = true;
                key.push('{');
            }
            '}' => {
                inside = false;
                key.push('}');
            }
            _ if inside => {}
            c => key.push(c),
        }
    }
    key.trim_end_matches('/').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document() -> Value {
        json!({
            "openapi": "3.0.1",
            "paths": {
                "/api/orders/{id}": {
                    "get": { "tags": ["Orders"], "summary": "One order", "parameters": [
                        { "name": "id", "in": "path", "required": true, "schema": { "type": "integer" } },
                        { "name": "include", "in": "query", "schema": { "type": "string", "default": "lines" } }
                    ] },
                    "put": { "tags": ["Orders"], "requestBody": { "content": { "application/json": {
                        "schema": { "$ref": "#/components/schemas/Order" } } } } }
                },
                "/api/health": { "get": { "operationId": "Health" } }
            },
            "components": { "schemas": { "Order": { "type": "object", "properties": {
                "name": { "type": "string" }, "total": { "type": "number" },
                "lines": { "type": "array", "items": { "type": "integer" } }
            } } } }
        })
    }

    #[test]
    fn operations_come_with_their_parameters_and_an_example_body_from_the_schema() {
        let operations = parse(&document());
        let (get, put) = (&operations[0], &operations[1]);
        assert_eq!(
            (get.method.as_str(), get.path.as_str(), get.summary.as_str()),
            ("GET", "/api/orders/{id}", "One order")
        );
        let include = Param { name: "include".into(), place: Place::Query, required: false, example: "lines".into() };
        assert_eq!(get.params[1], include);
        assert_eq!(put.body.as_deref(), Some(r#"{"name":"","total":0,"lines":[0]}"#), "in the schema's order");
        assert_eq!(operations[2].tag, "other", "untagged last");
        assert_eq!(operations[2].summary, "Health", "its operationId when it has no summary");
        let values = [("id".to_string(), "42".to_string()), ("include".to_string(), "lines and more".to_string())];
        assert_eq!(url(5001, get, &values), "http://localhost:5001/api/orders/42?include=lines%20and%20more");
        assert_eq!(route_key("/api/Orders/{orderId}/"), route_key("/api/orders/{id}"));
    }

    #[test]
    fn a_body_is_sent_with_its_method_and_headers_and_the_status_comes_back() {
        use std::io::{BufRead, BufReader, Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let asked = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            // The whole request, its body too, before answering: an answer mid-body resets the connection.
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let (mut asked, mut line) = (String::new(), String::new());
            while reader.read_line(&mut line).is_ok_and(|read| read > 0) && !line.trim_end().is_empty() {
                asked.push_str(&std::mem::take(&mut line));
            }
            let lower = asked.to_lowercase();
            let length = lower.lines().find_map(|line| line.strip_prefix("content-length:")?.trim().parse().ok());
            let mut body = vec![0; length.unwrap_or(0)];
            let _ = reader.read_exact(&mut body);
            let _ = stream.write_all(b"HTTP/1.1 201 Created\r\nContent-Length: 0\r\n\r\n");
            lower
        });
        let url = format!("http://127.0.0.1:{port}/api/orders");
        let sent = send("POST", &url, &[("X-Tenant".into(), "a".into())], Some(r#"{"name":"x"}"#)).unwrap();
        assert_eq!(sent.status, 201);
        let asked = asked.join().unwrap();
        assert!(asked.starts_with("post /api/orders"), "{asked}");
        assert!(asked.contains("x-tenant: a") && asked.contains("content-type: application/json"), "{asked}");
    }
}
