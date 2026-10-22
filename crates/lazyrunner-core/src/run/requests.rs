//! Requests (H): the HTTP requests the .NET apps served, from their own request log lines - ASP.NET's
//! "Request finished" (lazyrunner turns it on for the apps it starts) or Serilog's "HTTP GET /x responded 200 in
//! 12.3 ms" - kept out of the columns and summed up per endpoint.

use std::collections::{HashMap, VecDeque};
use std::sync::LazyLock;

use regex::Regex;

/// Requests kept, the newest.
pub const MAX_REQUESTS: usize = 5000;
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

/// A request as its log line says it.
#[derive(Clone, Debug, PartialEq)]
pub struct Finished {
    pub method: String,
    pub path: String,
    pub status: u16,
    pub ms: f64,
    /// The response's length and content type, when the line says them.
    pub length: String,
    pub content: String,
}

/// What an app's output line is to the requests view.
#[derive(Clone, Debug, PartialEq)]
pub enum Line {
    /// Not part of a request log.
    Other,
    /// Part of one, nothing to record (the request started; the header of an entry).
    Hidden,
    /// A finished request.
    Finished(Finished),
}

/// Reads one output line of a .NET app; `header` is whether the line before was a request entry's header (it is
/// updated for the next line).
pub fn classify(text: &str, header: &mut bool) -> Line {
    if std::mem::take(header) {
        return finished(text).map_or(Line::Hidden, Line::Finished);
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
    if text.contains("Request starting HTTP") { Line::Hidden } else { Line::Other }
}

fn finished(text: &str) -> Option<Finished> {
    let (found, rest) = match FINISHED.captures(text) {
        Some(found) => {
            let rest = found.name("rest").map_or("", |rest| rest.as_str()).trim().to_string();
            (found, rest)
        }
        None => (SERILOG.captures(text)?, String::new()),
    };
    let url = &found["url"];
    let path = match url.find("://") {
        Some(at) => url[at + 3..].find('/').map_or("/", |slash| &url[at + 3 + slash..]),
        None => url,
    };
    let (length, content) = rest.split_once(' ').unwrap_or((rest.as_str(), ""));
    Some(Finished {
        method: found["method"].to_string(),
        path: if path.is_empty() { "/".into() } else { path.to_string() },
        status: found["status"].parse().ok()?,
        ms: found["ms"].replace(',', ".").parse().ok()?,
        length: length.trim_matches('-').to_string(),
        content: content.trim_matches('-').trim().to_string(),
    })
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
        let Finished { method, path, status, ms, length, content } = request;
        let endpoint = endpoint(&path);
        let seq = self.seq;
        self.items.push_back(Request { seq, at, app: app.into(), method, path, endpoint, status, ms, length, content });
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
            Endpoint {
                app: app.into(),
                method: method.into(),
                endpoint: endpoint.into(),
                count,
                errors,
                avg: total / count as f64,
                p95: times[((count as f64 * 0.95).floor() as usize).min(count - 1)],
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
            "      Request starting HTTP/1.1 GET http://localhost:58201/api/orders/42?full=1 - - -",
            "info: Microsoft.AspNetCore.Hosting.Diagnostics[2]",
            "      Request finished HTTP/1.1 GET http://localhost:58201/api/orders/42?full=1 - 200 512 application/json;+charset=utf-8 12,3456ms",
            "info: Orders.Api[0]",
            "[10:00:00 INF] HTTP POST /api/orders responded 500 in 1532.1 ms",
            "      Request finished HTTP/1.1 GET http://localhost:58201/ - - - 404 0 - 0.5ms",
        ]);
        assert_eq!(&lines[..3], [Line::Hidden, Line::Hidden, Line::Hidden]);
        let Line::Finished(get) = &lines[3] else { panic!("{:?}", lines[3]) };
        assert_eq!((get.method.as_str(), get.path.as_str(), get.status), ("GET", "/api/orders/42?full=1", 200));
        assert_eq!(get.ms, 12.3456);
        assert_eq!((get.length.as_str(), get.content.as_str()), ("512", "application/json;+charset=utf-8"));
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
            let (method, path, length, content) = ("GET".into(), path.into(), String::new(), String::new());
            requests.add("orders", Finished { method, path, status, ms, length, content }, now);
        }
        let summed = endpoints(requests.items.iter());
        assert_eq!(summed.len(), 2);
        assert_eq!((summed[0].endpoint.as_str(), summed[0].count, summed[0].errors), ("/api/orders/{id}", 2, 1));
        assert_eq!((summed[0].avg, summed[0].p95, summed[0].max), (20.0, 30.0, 30.0));
        assert_eq!(requests.items.back().map(|request| request.seq), Some(3));
    }
}
