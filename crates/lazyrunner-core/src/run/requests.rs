//! Requests (H): the HTTP requests the .NET apps served, from their own request log lines - ASP.NET's
//! "Request finished" (lazyrunner turns it on for the apps it starts) or Serilog's "HTTP GET /x responded 200 in
//! 12.3 ms" - kept out of the columns and summed up per endpoint.

use std::collections::{HashMap, VecDeque};
use std::sync::LazyLock;

use regex::Regex;

/// Requests kept, the newest.
pub const MAX_REQUESTS: usize = 5000;
/// Requests an app has in flight that are followed, the newest (one that never finishes is forgotten).
const MAX_IN_FLIGHT: usize = 256;
/// What `dotnet run` passes an app so ASP.NET logs each request it finished.
pub const LOG_REQUESTS: &str = "--Logging:LogLevel:Microsoft.AspNetCore.Hosting.Diagnostics=Information";

fn pattern(regex: &str) -> Regex {
    Regex::new(regex).expect("a valid pattern")
}

/// "Request finished HTTP/1.1 GET http://localhost:5000/api/x?y=1 - 200 52 application/json 12.3456ms" (.NET 8;
/// older ones put "- - -" before the status).
static FINISHED: LazyLock<Regex> = LazyLock::new(|| {
    pattern(
        r"Request finished (?P<proto>\S+) (?P<method>[A-Z]+) (?P<url>\S+)(?: -)+ (?P<status>\d{3}) (?P<rest>.*?) ?(?P<ms>[\d.,]+)ms\s*$",
    )
});
/// "Request starting HTTP/1.1 POST http://localhost:5000/api/x - application/json 52" (.NET 8; older ones leave out
/// the "-" before the content type).
static STARTING: LazyLock<Regex> =
    LazyLock::new(|| pattern(r"Request starting (?P<proto>\S+) (?P<method>[A-Z]+) (?P<url>\S+)(?P<rest>.*)$"));
/// Serilog's request logging: "HTTP GET /api/x responded 200 in 12.3456 ms".
static SERILOG: LazyLock<Regex> = LazyLock::new(|| {
    pattern(r"HTTP (?P<method>[A-Z]+) (?P<url>\S+) responded (?P<status>\d{3}) in (?P<ms>[\d.,]+) ms")
});
/// The console logger's header line of a request entry; its message is on the next line.
static HEADER: LazyLock<Regex> =
    LazyLock::new(|| pattern(r"^\s*\w+: Microsoft\.AspNetCore\.Hosting\.Diagnostics\[\d+\]\s*$"));
/// A path segment that is an id: a number, a GUID, a long hex string.
static ID: LazyLock<Regex> = LazyLock::new(|| {
    pattern(r"^(\d+|[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}|[0-9a-fA-F]{24,})$")
});

/// A request as its start line says it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Started {
    pub method: String,
    pub path: String,
    pub proto: String,
    /// The request body's content type and length, when it has one.
    pub content: String,
    pub length: String,
}

/// A request as its log lines say it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Finished {
    pub method: String,
    pub path: String,
    pub status: u16,
    pub ms: f64,
    /// The response's length and content type, when the line says them.
    pub length: String,
    pub content: String,
    pub proto: String,
    /// What its start line said: the request body's content type and length.
    pub sent_content: String,
    pub sent_length: String,
    /// The numbers of the app's output lines it logged while it ran (`first..=last`, none when `last < first`);
    /// `None` without a start line (Serilog's request log).
    pub logs: Option<(u64, u64)>,
    /// Other requests of the app ran at the same time, so some of those lines may be theirs.
    pub mixed: bool,
}

/// What an app's output line is to the requests view.
#[derive(Clone, Debug, PartialEq)]
pub enum Line {
    /// Not part of a request log.
    Other,
    /// Part of one, nothing to record (the header of an entry).
    Hidden,
    /// A request started.
    Started(Started),
    /// A finished request.
    Finished(Finished),
}

/// Reads one output line of a .NET app; `header` is whether the line before was a request entry's header (it is
/// updated for the next line).
pub fn classify(text: &str, header: &mut bool) -> Line {
    if std::mem::take(header) {
        return finished(text)
            .map(Line::Finished)
            .or_else(|| starting(text).map(Line::Started))
            .unwrap_or(Line::Hidden);
    }
    if !text.contains("Hosting.Diagnostics") && !text.contains("Request ") && !text.contains(" responded ") {
        return Line::Other;
    }
    if HEADER.is_match(text) {
        *header = true;
        return Line::Hidden;
    }
    if let Some(request) = finished(text) {
        return Line::Finished(request);
    }
    starting(text).map_or(Line::Other, Line::Started)
}

fn starting(text: &str) -> Option<Started> {
    let found = STARTING.captures(text)?;
    let mut rest: Vec<&str> = found["rest"].split_whitespace().collect();
    if rest.len() >= 3 && rest[0] == "-" {
        rest.remove(0);
    }
    let length = rest.pop().unwrap_or_default();
    let dash = |text: &str| if text == "-" { String::new() } else { text.to_string() };
    Some(Started {
        method: found["method"].to_string(),
        path: path_of(&found["url"]),
        proto: found["proto"].to_string(),
        content: dash(&rest.join(" ")),
        length: dash(length),
    })
}

/// A logged url's path and query.
fn path_of(url: &str) -> String {
    let path = match url.find("://") {
        Some(at) => url[at + 3..].find('/').map_or("/", |slash| &url[at + 3 + slash..]),
        None => url,
    };
    if path.is_empty() { "/".into() } else { path.to_string() }
}

fn finished(text: &str) -> Option<Finished> {
    let (found, rest) = match FINISHED.captures(text) {
        Some(found) => {
            let rest = found.name("rest").map_or("", |rest| rest.as_str()).trim().to_string();
            (found, rest)
        }
        None => (SERILOG.captures(text)?, String::new()),
    };
    let (length, content) = rest.split_once(' ').unwrap_or((rest.as_str(), ""));
    Some(Finished {
        method: found["method"].to_string(),
        path: path_of(&found["url"]),
        status: found["status"].parse().ok()?,
        ms: found["ms"].replace(',', ".").parse().ok()?,
        length: length.trim_matches('-').to_string(),
        content: content.trim_matches('-').trim().to_string(),
        proto: found.name("proto").map_or_else(String::new, |proto| proto.as_str().to_string()),
        ..Finished::default()
    })
}

/// An app's requests started and not finished yet, the oldest first, each with the output line count at its start
/// and whether another one ran beside it.
#[derive(Debug, Default)]
pub struct InFlight {
    items: VecDeque<(Started, u64, bool)>,
}

impl InFlight {
    /// A request started when the app had printed `lines` lines.
    pub fn start(&mut self, started: Started, lines: u64) {
        let mixed = !self.items.is_empty();
        for item in &mut self.items {
            item.2 = true;
        }
        self.items.push_back((started, lines, mixed));
        if self.items.len() > MAX_IN_FLIGHT {
            self.items.pop_front();
        }
    }

    /// The request finished when the app had printed `lines` lines: what its start line said, and which lines it
    /// logged meanwhile.
    pub fn finish(&mut self, request: &mut Finished, lines: u64) {
        let same =
            |(started, ..): &(Started, u64, bool)| started.method == request.method && started.path == request.path;
        let Some((started, from, mixed)) = self.items.iter().position(same).and_then(|at| self.items.remove(at)) else {
            return;
        };
        if request.proto.is_empty() {
            request.proto = started.proto;
        }
        (request.sent_content, request.sent_length) = (started.content, started.length);
        request.logs = Some((from + 1, lines));
        request.mixed = mixed;
    }

    pub fn clear(&mut self) {
        self.items.clear();
    }
}

/// The endpoint a path belongs to: without the query, ids (numbers, GUIDs, long hex) as `{id}`.
pub fn endpoint(path: &str) -> String {
    let path = path.split('?').next().unwrap_or_default();
    path.split('/').map(|part| if ID.is_match(part) { "{id}" } else { part }).collect::<Vec<_>>().join("/")
}

/// A request one of the apps served.
#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    /// Counts up: tells a request from the ones after it.
    pub seq: u64,
    pub at: chrono::DateTime<chrono::Local>,
    /// The app's key.
    pub app: String,
    pub method: String,
    pub path: String,
    pub endpoint: String,
    pub status: u16,
    pub ms: f64,
    pub length: String,
    pub content: String,
    pub proto: String,
    pub sent_content: String,
    pub sent_length: String,
    /// See [`Finished::logs`].
    pub logs: Option<(u64, u64)>,
    pub mixed: bool,
}

/// The requests served, oldest first.
#[derive(Debug, Default)]
pub struct Requests {
    pub items: VecDeque<Request>,
    pub seq: u64,
}

impl Requests {
    pub fn add(&mut self, app: &str, request: Finished, at: chrono::DateTime<chrono::Local>) {
        self.seq += 1;
        let Finished { method, path, status, ms, length, content, proto, sent_content, sent_length, logs, mixed } =
            request;
        let endpoint = endpoint(&path);
        let seq = self.seq;
        self.items.push_back(Request {
            seq,
            at,
            app: app.into(),
            method,
            path,
            endpoint,
            status,
            ms,
            length,
            content,
            proto,
            sent_content,
            sent_length,
            logs,
            mixed,
        });
        while self.items.len() > MAX_REQUESTS {
            self.items.pop_front();
        }
    }
}

/// An endpoint's requests summed up.
#[derive(Clone, Debug, PartialEq)]
pub struct Endpoint {
    pub app: String,
    pub method: String,
    pub endpoint: String,
    pub count: usize,
    pub errors: usize,
    pub avg: f64,
    pub p50: f64,
    pub p95: f64,
    pub max: f64,
    pub total: f64,
}

/// The requests per app, method and endpoint, in the order they first appear.
pub fn endpoints<'a>(requests: impl Iterator<Item = &'a Request>) -> Vec<Endpoint> {
    let mut groups: Vec<(&str, &str, &str, Vec<f64>, usize)> = Vec::new();
    let mut index: HashMap<(&str, &str, &str), usize> = HashMap::new();
    for request in requests {
        let key = (request.app.as_str(), request.method.as_str(), request.endpoint.as_str());
        let at = *index.entry(key).or_insert_with(|| {
            groups.push((key.0, key.1, key.2, Vec::new(), 0));
            groups.len() - 1
        });
        groups[at].3.push(request.ms);
        groups[at].4 += usize::from(request.status >= 400);
    }
    groups
        .into_iter()
        .map(|(app, method, endpoint, mut times, errors)| {
            times.sort_by(f64::total_cmp);
            let total: f64 = times.iter().sum();
            let count = times.len();
            let percentile = |at: f64| times[((count as f64 * at).floor() as usize).min(count - 1)];
            Endpoint {
                app: app.into(),
                method: method.into(),
                endpoint: endpoint.into(),
                count,
                errors,
                avg: total / count as f64,
                p50: percentile(0.5),
                p95: percentile(0.95),
                max: times[count - 1],
                total,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(lines: &[&str]) -> Vec<Line> {
        let mut header = false;
        lines.iter().map(|line| classify(line, &mut header)).collect()
    }

    #[test]
    fn aspnet_and_serilog_request_lines_are_read() {
        let lines = read(&[
            "info: Microsoft.AspNetCore.Hosting.Diagnostics[1]",
            "      Request starting HTTP/1.1 GET http://localhost:47201/api/orders/42?full=1 - - -",
            "info: Microsoft.AspNetCore.Hosting.Diagnostics[2]",
            "      Request finished HTTP/1.1 GET http://localhost:47201/api/orders/42?full=1 - 200 512 application/json;+charset=utf-8 12,3456ms",
            "info: Orders.Api[0]",
            "[10:00:00 INF] HTTP POST /api/orders responded 500 in 1532.1 ms",
            "      Request finished HTTP/1.1 GET http://localhost:47201/ - - - 404 0 - 0.5ms",
        ]);
        assert_eq!(lines[0], Line::Hidden);
        let Line::Started(started) = &lines[1] else { panic!("{:?}", lines[1]) };
        assert_eq!((started.method.as_str(), started.path.as_str()), ("GET", "/api/orders/42?full=1"));
        assert_eq!((started.content.as_str(), started.length.as_str()), ("", ""));
        assert_eq!(lines[2], Line::Hidden);
        let Line::Finished(get) = &lines[3] else { panic!("{:?}", lines[3]) };
        assert_eq!((get.method.as_str(), get.path.as_str(), get.status), ("GET", "/api/orders/42?full=1", 200));
        assert_eq!(get.ms, 12.3456);
        assert_eq!((get.length.as_str(), get.content.as_str()), ("512", "application/json;+charset=utf-8"));
        assert_eq!(get.proto, "HTTP/1.1");
        assert_eq!(lines[4], Line::Other);
        let Line::Finished(post) = &lines[5] else { panic!("{:?}", lines[5]) };
        assert_eq!(
            (post.method.as_str(), post.path.as_str(), post.status, post.ms),
            ("POST", "/api/orders", 500, 1532.1)
        );
        let Line::Finished(old) = &lines[6] else { panic!("{:?}", lines[6]) };
        assert_eq!((old.path.as_str(), old.status), ("/", 404), "the older format");
    }

    #[test]
    fn endpoints_group_ids_and_sum_up() {
        let path = "/api/orders/42/lines/0f8fad5b-d9cb-469f-a165-70867728950e?x=1";
        assert_eq!(endpoint(path), "/api/orders/{id}/lines/{id}");
        let mut requests = Requests::default();
        let now = chrono::Local::now();
        for (path, status, ms) in [("/api/orders/1", 200, 10.0), ("/api/orders/2", 500, 30.0), ("/api/cart", 200, 5.0)]
        {
            requests.add(
                "orders",
                Finished { method: "GET".into(), path: path.into(), status, ms, ..Finished::default() },
                now,
            );
        }
        let summed = endpoints(requests.items.iter());
        assert_eq!(summed.len(), 2);
        assert_eq!((summed[0].endpoint.as_str(), summed[0].count, summed[0].errors), ("/api/orders/{id}", 2, 1));
        assert_eq!((summed[0].avg, summed[0].p50, summed[0].p95, summed[0].max), (20.0, 30.0, 30.0, 30.0));
        assert_eq!(requests.items.back().map(|request| request.seq), Some(3));
    }

    #[test]
    fn start_lines_say_what_was_sent() {
        let started = |line: &str| match read(&[line]).remove(0) {
            Line::Started(started) => started,
            other => panic!("{other:?}"),
        };
        let post =
            started("      Request starting HTTP/2 POST https://localhost:5001/api/orders - application/json 52");
        assert_eq!(
            (post.proto.as_str(), post.content.as_str(), post.length.as_str()),
            ("HTTP/2", "application/json", "52")
        );
        let old = started(
            "      Request starting HTTP/1.1 PUT http://localhost:5000/api/x application/json; charset=utf-8 7",
        );
        assert_eq!((old.content.as_str(), old.length.as_str()), ("application/json; charset=utf-8", "7"));
        assert_eq!(started("Request starting HTTP/1.1 GET http://localhost:5000/ - -").content, "");
    }

    #[test]
    fn a_finished_request_gets_the_lines_logged_while_it_ran() {
        let start = |path: &str| Started { method: "GET".into(), path: path.into(), ..Started::default() };
        let done = |path: &str| Finished { method: "GET".into(), path: path.into(), ..Finished::default() };
        let mut flights = InFlight::default();
        flights.start(start("/a"), 10);
        let mut a = done("/a");
        flights.finish(&mut a, 14);
        assert_eq!((a.logs, a.mixed), (Some((11, 14)), false));
        flights.start(start("/b"), 20);
        flights.start(start("/c"), 21);
        let (mut c, mut b) = (done("/c"), done("/b"));
        flights.finish(&mut c, 25);
        flights.finish(&mut b, 30);
        assert_eq!((b.logs, b.mixed, c.logs, c.mixed), (Some((21, 30)), true, Some((22, 25)), true));
        let mut unknown = done("/never-started");
        flights.finish(&mut unknown, 31);
        assert_eq!(unknown.logs, None, "Serilog logs no start line");
    }
}
