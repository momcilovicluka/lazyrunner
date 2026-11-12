//! OpenTelemetry spans (#97): a small OTLP/HTTP receiver on localhost. With `"openTelemetry"` on, the .NET apps start
//! with `OTEL_EXPORTER_OTLP_ENDPOINT` pointing at it - a path per app, so each span says which app sent it - and an app
//! that already uses OpenTelemetry (ASP.NET Core, EF Core, HttpClient instrumentation) sends its real spans: nested,
//! timed exactly, with their SQL and URLs. A request's timeline then comes from its trace instead of its log.

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};

use tokio::sync::mpsc::UnboundedSender;

use super::Event;
use super::requests::Request;
use super::waterfall::{Kind, Span as Bar, Waterfall};

/// The spans kept, the newest last.
const KEPT: usize = 20_000;
/// Where an exporter posts its traces, after its app's own path.
const TRACES: &str = "/v1/traces";
pub const ENDPOINT: &str = "OTEL_EXPORTER_OTLP_ENDPOINT";
pub const PROTOCOL: &str = "OTEL_EXPORTER_OTLP_PROTOCOL";
pub const PROTOBUF: &str = "http/protobuf";
/// A span's kind: one serving a request, one making a call.
const SERVER: u64 = 2;
const CLIENT: u64 = 3;
/// A status code that says it failed.
const FAILED: u64 = 2;
/// How far apart a request's finish and its server span's end may be.
const MATCH_NANOS: u64 = 2_000_000_000;
const NANOS_PER_MS: f64 = 1_000_000.0;
const OK: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Type: application/x-protobuf\r\nContent-Length: 0\r\n\r\n";

/// A span as an app sent it: its app (by the path it posted to), its trace, itself and its parent, what it was, its
/// kind, when it started and ended (Unix nanoseconds), its attributes, and whether it failed.
#[derive(Clone, Debug, PartialEq)]
pub struct Span {
    pub app: String,
    pub trace: String,
    pub id: String,
    pub parent: Option<String>,
    pub name: String,
    pub kind: u64,
    pub start: u64,
    pub end: u64,
    pub attributes: Vec<(String, String)>,
    pub failed: bool,
}

impl Span {
    fn attribute(&self, names: &[&str]) -> Option<&str> {
        names
            .iter()
            .find_map(|name| self.attributes.iter().find(|(key, _)| key == name).map(|(_, value)| value.as_str()))
    }
}

/// A protobuf field's value: a varint, 8 bytes, length-delimited bytes, or 4 bytes.
enum Field<'a> {
    Varint(u64),
    Fixed64(u64),
    Bytes(&'a [u8]),
    Fixed32,
}

fn varint(bytes: &[u8], at: &mut usize) -> Option<u64> {
    let mut value = 0u64;
    for shift in (0..64).step_by(7) {
        let byte = *bytes.get(*at)?;
        *at += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Some(value);
        }
    }
    None
}

/// A message's fields, in order; what doesn't read ends it.
fn fields(bytes: &[u8]) -> Vec<(u64, Field<'_>)> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        let Some(key) = varint(bytes, &mut at) else { break };
        let field = match key & 7 {
            0 => varint(bytes, &mut at).map(Field::Varint),
            1 => bytes.get(at..at + 8).map(|eight| {
                at += 8;
                Field::Fixed64(u64::from_le_bytes(eight.try_into().unwrap_or_default()))
            }),
            2 => varint(bytes, &mut at).and_then(|length| {
                let slice = bytes.get(at..at + length as usize)?;
                at += length as usize;
                Some(Field::Bytes(slice))
            }),
            5 => bytes.get(at..at + 4).map(|_| {
                at += 4;
                Field::Fixed32
            }),
            _ => None,
        };
        let Some(field) = field else { break };
        out.push((key >> 3, field));
    }
    out
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// An attribute's value as text: a string, a bool, an integer or a double.
fn any_value(bytes: &[u8]) -> String {
    for (number, field) in fields(bytes) {
        match (number, field) {
            (1, Field::Bytes(text)) => return String::from_utf8_lossy(text).into_owned(),
            (2, Field::Varint(flag)) => return (flag != 0).to_string(),
            (3, Field::Varint(whole)) => return (whole as i64).to_string(),
            (4, Field::Fixed64(bits)) => return f64::from_bits(bits).to_string(),
            _ => {}
        }
    }
    String::new()
}

fn key_value(bytes: &[u8]) -> Option<(String, String)> {
    let mut key = None;
    let mut value = String::new();
    for (number, field) in fields(bytes) {
        match (number, field) {
            (1, Field::Bytes(text)) => key = Some(String::from_utf8_lossy(text).into_owned()),
            (2, Field::Bytes(any)) => value = any_value(any),
            _ => {}
        }
    }
    Some((key?, value))
}

fn span(bytes: &[u8], app: &str) -> Span {
    let mut span = Span {
        app: app.to_string(),
        trace: String::new(),
        id: String::new(),
        parent: None,
        name: String::new(),
        kind: 0,
        start: 0,
        end: 0,
        attributes: Vec::new(),
        failed: false,
    };
    for (number, field) in fields(bytes) {
        match (number, field) {
            (1, Field::Bytes(id)) => span.trace = hex(id),
            (2, Field::Bytes(id)) => span.id = hex(id),
            (4, Field::Bytes(id)) if !id.is_empty() => span.parent = Some(hex(id)),
            (5, Field::Bytes(name)) => span.name = String::from_utf8_lossy(name).into_owned(),
            (6, Field::Varint(kind)) => span.kind = kind,
            (7, Field::Fixed64(nanos)) => span.start = nanos,
            (8, Field::Fixed64(nanos)) => span.end = nanos,
            (9, Field::Bytes(pair)) => span.attributes.extend(key_value(pair)),
            (15, Field::Bytes(status)) => {
                span.failed = fields(status)
                    .into_iter()
                    .any(|(number, field)| number == 3 && matches!(field, Field::Varint(code) if code == FAILED));
            }
            _ => {}
        }
    }
    span
}

/// The spans of an OTLP `ExportTraceServiceRequest`, sent by app `app`.
pub fn decode(body: &[u8], app: &str) -> Vec<Span> {
    let mut spans = Vec::new();
    for (number, field) in fields(body) {
        let (1, Field::Bytes(resource_spans)) = (number, field) else { continue };
        for (number, field) in fields(resource_spans) {
            let (2, Field::Bytes(scope_spans)) = (number, field) else { continue };
            for (number, field) in fields(scope_spans) {
                if let (2, Field::Bytes(bytes)) = (number, field) {
                    spans.push(span(bytes, app));
                }
            }
        }
    }
    spans
}

/// Listens on a localhost port for OTLP/HTTP traces, each posted to `/<app key>/v1/traces`: their spans as events.
pub fn listen(events: UnboundedSender<Event>) -> Option<u16> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).ok()?;
    let port = listener.local_addr().ok()?.port();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let events = events.clone();
            std::thread::spawn(move || serve(stream, &events));
        }
    });
    Some(port)
}

/// One connection: request after request, each answered at once, its spans sent on.
fn serve(stream: TcpStream, events: &UnboundedSender<Event>) {
    let Ok(mut writer) = stream.try_clone() else { return };
    let mut reader = BufReader::new(stream);
    loop {
        let mut first = String::new();
        if reader.read_line(&mut first).unwrap_or(0) == 0 {
            return;
        }
        let path = first.split_whitespace().nth(1).unwrap_or_default().to_string();
        let mut length = 0usize;
        loop {
            let mut header = String::new();
            if reader.read_line(&mut header).unwrap_or(0) == 0 {
                return;
            }
            if header.trim().is_empty() {
                break;
            }
            if let Some((name, value)) = header.split_once(':')
                && name.trim().eq_ignore_ascii_case("content-length")
            {
                length = value.trim().parse().unwrap_or(0);
            }
        }
        let mut body = vec![0; length];
        if reader.read_exact(&mut body).is_err() {
            return;
        }
        if let Some(app) = path.strip_suffix(TRACES).map(|app| app.trim_matches('/').to_string()) {
            let spans = decode(&body, &app);
            if !spans.is_empty() {
                let _ = events.send(Event::Spans(spans));
            }
        }
        if writer.write_all(OK).is_err() {
            return;
        }
    }
}

/// The spans received, and the apps that sent any.
#[derive(Debug, Default)]
pub struct Traces {
    spans: VecDeque<Span>,
    pub apps: BTreeSet<String>,
}

impl Traces {
    pub fn add(&mut self, spans: Vec<Span>) {
        for span in spans {
            self.apps.insert(span.app.clone());
            self.spans.push_back(span);
        }
        while self.spans.len() > KEPT {
            self.spans.pop_front();
        }
    }

    /// The span that served `request`: its app's, its method and path, ending nearest its finish (within 2 s).
    fn server_of(&self, request: &Request) -> Option<&Span> {
        let finished = request.at.timestamp_nanos_opt()? as u64;
        let path = request.path.split('?').next().unwrap_or_default();
        self.spans
            .iter()
            .filter(|span| span.kind == SERVER && span.app == request.app)
            .filter(|span| {
                span.attribute(&["http.request.method", "http.method"]).is_none_or(|method| method == request.method)
            })
            .filter(|span| {
                span.attribute(&["url.path", "http.target"]).is_none_or(|said| said.split('?').next() == Some(path))
            })
            .filter(|span| span.end.abs_diff(finished) <= MATCH_NANOS)
            .min_by_key(|span| span.end.abs_diff(finished))
    }

    /// `request`'s timeline from its trace: each span under the one that served it, nested, timed exactly, the chain
    /// that decided its length marked.
    pub fn waterfall(&self, request: &Request) -> Option<Waterfall> {
        let server = self.server_of(request)?;
        let trace: Vec<&Span> = self.spans.iter().filter(|span| span.trace == server.trace).collect();
        let by_id: HashMap<&str, &Span> = trace.iter().map(|span| (span.id.as_str(), *span)).collect();
        let depth = |span: &Span| {
            let mut depth = 0;
            let mut parent = span.parent.as_deref();
            while let Some(id) = parent {
                depth += 1;
                if id == server.id {
                    return Some(depth);
                }
                parent = by_id.get(id).and_then(|span| span.parent.as_deref());
            }
            None
        };
        let ms = |nanos: u64| nanos as f64 / NANOS_PER_MS;
        let mut bars: Vec<(Bar, String, Option<String>)> = trace
            .iter()
            .filter_map(|span| Some((*span, depth(span)?)))
            .map(|(span, depth)| {
                let (kind, label) = describe(span);
                let bar = Bar {
                    kind,
                    start: ms(span.start.saturating_sub(server.start)),
                    ms: ms(span.end.saturating_sub(span.start)),
                    label: if span.failed { format!("{label} ✗") } else { label },
                    line: 0,
                    repeats: Vec::new(),
                    depth,
                    critical: false,
                };
                (bar, span.id.clone(), span.parent.clone())
            })
            .collect();
        bars.sort_by(|a, b| a.0.start.total_cmp(&b.0.start));
        let mut on = Some(server.id.clone());
        while let Some(id) = on.take() {
            let latest = bars
                .iter_mut()
                .filter(|(_, _, parent)| parent.as_deref() == Some(id.as_str()))
                .max_by(|a, b| a.0.end().total_cmp(&b.0.end()));
            if let Some((bar, child, _)) = latest {
                bar.critical = true;
                on = Some(child.clone());
            }
        }
        let mut fall = Waterfall {
            ms: ms(server.end.saturating_sub(server.start)),
            spans: bars.into_iter().map(|(bar, ..)| bar).collect(),
            unaccounted: 0.0,
            exact: true,
        };
        fall.unaccounted = fall.gaps().iter().map(|(start, end)| end - start).sum();
        Some(fall)
    }
}

/// What a span was: a database command (its SQL), a call (its method, URL and status), else its name.
fn describe(span: &Span) -> (Kind, String) {
    if span.attribute(&["db.system", "db.system.name"]).is_some() {
        let sql = span.attribute(&["db.query.text", "db.statement"]).unwrap_or(&span.name);
        return (Kind::Db, sql.to_string());
    }
    if span.kind == CLIENT
        && let Some(url) = span.attribute(&["url.full", "http.url"])
    {
        let method = span.attribute(&["http.request.method", "http.method"]).unwrap_or("GET");
        let status = span.attribute(&["http.response.status_code", "http.status_code"]).unwrap_or("?");
        return (Kind::Http, format!("{method} {url} → {status}"));
    }
    (Kind::Span, span.name.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(number: u64, wire: u64, bytes: &[u8]) -> Vec<u8> {
        let mut out = encode_varint(number << 3 | wire);
        if wire == 2 {
            out.extend(encode_varint(bytes.len() as u64));
        }
        out.extend(bytes);
        out
    }

    fn encode_varint(mut value: u64) -> Vec<u8> {
        let mut out = Vec::new();
        loop {
            let byte = (value & 0x7f) as u8;
            value >>= 7;
            if value == 0 {
                out.push(byte);
                return out;
            }
            out.push(byte | 0x80);
        }
    }

    fn attribute(key: &str, value: &str) -> Vec<u8> {
        let any = field(1, 2, value.as_bytes());
        [field(1, 2, key.as_bytes()), field(2, 2, &any)].concat()
    }

    fn span_bytes(
        id: u8,
        parent: Option<u8>,
        kind: u64,
        (start, end): (u64, u64),
        attributes: &[(&str, &str)],
    ) -> Vec<u8> {
        let mut out = [field(1, 2, &[9; 16]), field(2, 2, &[id; 8])].concat();
        if let Some(parent) = parent {
            out.extend(field(4, 2, &[parent; 8]));
        }
        out.extend(field(5, 2, b"span"));
        out.extend(field(6, 0, &encode_varint(kind)));
        out.extend(field(7, 1, &start.to_le_bytes()));
        out.extend(field(8, 1, &end.to_le_bytes()));
        for (key, value) in attributes {
            out.extend(field(9, 2, &attribute(key, value)));
        }
        out
    }

    #[tokio::test]
    async fn the_receiver_answers_an_apps_post_and_passes_its_spans_on_with_the_app() {
        let (events, mut received) = tokio::sync::mpsc::unbounded_channel();
        let port = listen(events).expect("a port");
        let span = span_bytes(1, None, SERVER, (1, 2), &[]);
        let body = field(1, 2, &field(2, 2, &field(2, 2, &span)));
        let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        let head = format!(
            "POST /orders/v1/traces HTTP/1.1
Host: x
Content-Type: application/x-protobuf
Content-Length: {}

",
            body.len()
        );
        stream.write_all(head.as_bytes()).unwrap();
        stream.write_all(&body).unwrap();
        let mut answer = [0u8; 15];
        stream.read_exact(&mut answer).unwrap();
        assert_eq!(&answer, b"HTTP/1.1 200 OK");
        let Some(Event::Spans(spans)) = received.recv().await else { panic!("its spans") };
        assert_eq!((spans.len(), spans[0].app.as_str()), (1, "orders"));
    }

    #[test]
    fn a_trace_is_decoded_and_its_server_span_gives_the_request_its_exact_nested_timeline() {
        let at = chrono::Local::now();
        let end = at.timestamp_nanos_opt().unwrap() as u64;
        let start = end - 100_000_000;
        let server =
            span_bytes(1, None, SERVER, (start, end), &[("http.request.method", "GET"), ("url.path", "/api/orders")]);
        let db = span_bytes(
            2,
            Some(1),
            CLIENT,
            (start + 10_000_000, start + 70_000_000),
            &[("db.system", "mssql"), ("db.query.text", "SELECT 1")],
        );
        let call = span_bytes(
            3,
            Some(1),
            CLIENT,
            (start + 75_000_000, start + 90_000_000),
            &[("url.full", "http://localhost:5002/x"), ("http.response.status_code", "200")],
        );
        let scope = [field(2, 2, &server), field(2, 2, &db), field(2, 2, &call)].concat();
        let body = field(1, 2, &field(2, 2, &scope));
        let spans = decode(&body, "orders");
        assert_eq!(spans.len(), 3);
        assert_eq!((spans[1].parent.as_deref(), spans[1].start), (Some("0101010101010101"), start + 10_000_000));
        let mut traces = Traces::default();
        traces.add(spans);
        let request = Request {
            seq: 1,
            at,
            app: "orders".into(),
            method: "GET".into(),
            path: "/api/orders?full=1".into(),
            endpoint: "/api/orders".into(),
            status: 200,
            ms: 100.0,
            length: String::new(),
            content: String::new(),
            proto: String::new(),
            sent_content: String::new(),
            sent_length: String::new(),
            logs: None,
            mixed: false,
            exact: None,
            db: None,
            ended: None,
        };
        let fall = traces.waterfall(&request).expect("its trace");
        assert!(fall.exact && (fall.ms - 100.0).abs() < 0.001);
        let kinds: Vec<Kind> = fall.spans.iter().map(|bar| bar.kind).collect();
        assert_eq!(kinds, [Kind::Db, Kind::Http]);
        assert_eq!((fall.spans[0].label.as_str(), fall.spans[0].depth), ("SELECT 1", 1));
        assert_eq!(fall.spans[1].label, "GET http://localhost:5002/x → 200");
        assert!(fall.spans[1].critical && !fall.spans[0].critical, "the call ended last");
        assert!((fall.unaccounted - 25.0).abs() < 0.001, "{}", fall.unaccounted);
        assert!(traces.apps.contains("orders"));
    }
}
