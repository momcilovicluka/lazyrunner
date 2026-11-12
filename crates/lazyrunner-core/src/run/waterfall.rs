//! A request's waterfall (#38): where its time went, from its own log lines and when each arrived - each EF Core
//! command placed by its line's arrival less its duration, each outbound HttpClient call from its start and end lines,
//! the moment it logged an exception, and the time no line accounts for. A query run over and over (an N+1) is one comb.

use std::time::{Duration, Instant};

use super::calls::{END, START};
use super::requests::{N_PLUS_ONE, Request, db_command_ms};
use crate::output::{Level, Output, OutputLine};

/// The longest SQL kept as a span's label.
const SQL: usize = 200;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Db,
    Http,
    Exception,
}

/// Something the request did: when it started (ms from the request's start) and how long it took, what it was (the
/// SQL, the call's method, URL and status, the exception), the output line it came from, and for a comb its repeats.
#[derive(Clone, Debug, PartialEq)]
pub struct Span {
    pub kind: Kind,
    pub start: f64,
    pub ms: f64,
    pub label: String,
    pub line: u64,
    pub repeats: Vec<(f64, f64)>,
}

impl Span {
    pub fn end(&self) -> f64 {
        self.start + self.ms
    }
}

/// A request's waterfall: how long it took, its spans in the order they started, and its time in the app itself.
#[derive(Clone, Debug, PartialEq)]
pub struct Waterfall {
    pub ms: f64,
    pub spans: Vec<Span>,
    pub unaccounted: f64,
}

impl Waterfall {
    /// The span that took longest (a database command or a call).
    pub fn longest(&self) -> Option<usize> {
        let timed = self.spans.iter().enumerate().filter(|(_, span)| span.kind != Kind::Exception);
        timed.max_by(|(_, a), (_, b)| a.ms.total_cmp(&b.ms)).map(|(at, _)| at)
    }

    /// The stretches no span covers, from the request's start: where the time went in the app itself.
    pub fn gaps(&self) -> Vec<(f64, f64)> {
        let mut covered: Vec<(f64, f64)> = self
            .spans
            .iter()
            .filter(|span| span.kind != Kind::Exception)
            .flat_map(|span| if span.repeats.is_empty() { vec![(span.start, span.end())] } else { repeats(span) })
            .collect();
        covered.sort_by(|a, b| a.0.total_cmp(&b.0));
        let (mut gaps, mut at) = (Vec::new(), 0.0);
        for (start, end) in covered {
            if start > at {
                gaps.push((at, start));
            }
            at = f64::max(at, end);
        }
        if at < self.ms {
            gaps.push((at, self.ms));
        }
        gaps
    }
}

fn repeats(span: &Span) -> Vec<(f64, f64)> {
    span.repeats.iter().map(|&(start, ms)| (start, start + ms)).collect()
}

/// The waterfall of `request` from the lines its app's `output` still keeps; none when they're unknown or gone.
pub fn of(request: &Request, output: &Output) -> Option<Waterfall> {
    let numbers: Vec<u64> = match (&request.exact, request.logs) {
        (Some(exact), _) => exact.clone(),
        (None, Some((first, last))) if last >= first => (first..=last).collect(),
        _ => return None,
    };
    let lines: Vec<(u64, &OutputLine)> =
        numbers.iter().filter_map(|&number| Some((number, output.numbered(number, number).next()?))).collect();
    let end = request.ended.or_else(|| lines.last().map(|(_, line)| line.born))?;
    let start = end.checked_sub(Duration::from_secs_f64(request.ms.max(0.0) / 1000.0)).unwrap_or(end);
    Some(from_lines(request.ms, start, &lines))
}

/// The spans of lines that arrived from `start` on, in a request of `total` ms.
pub fn from_lines(total: f64, start: Instant, lines: &[(u64, &OutputLine)]) -> Waterfall {
    let at = |born: Instant| (born.saturating_duration_since(start).as_secs_f64() * 1000.0).min(total);
    let mut spans = Vec::new();
    let mut pending: Vec<(String, f64, u64)> = Vec::new();
    let mut exception = false;
    for (index, &(number, line)) in lines.iter().enumerate() {
        let text = line.text.as_str();
        if let Some(ms) = db_command_ms(text) {
            let sql: Vec<&str> = lines[index + 1..]
                .iter()
                .map(|(_, line)| line.text.as_str())
                .take_while(|text| text.starts_with(char::is_whitespace) && db_command_ms(text).is_none())
                .map(str::trim)
                .collect();
            let label: String = sql.join(" ").chars().take(SQL).collect();
            let start = (at(line.born) - ms).max(0.0);
            spans.push(Span { kind: Kind::Db, start, ms, label, line: number, repeats: Vec::new() });
        } else if let Some(found) = START.captures(text) {
            pending.push((format!("{} {}", &found["method"], &found["url"]), at(line.born), number));
        } else if let Some(found) = END.captures(text) {
            let ms: f64 = found["ms"].replace(',', "").parse().unwrap_or(0.0);
            let (label, started, from) = if pending.is_empty() {
                ("HTTP call".to_string(), at(line.born) - ms, number)
            } else {
                pending.remove(0)
            };
            let label = format!("{label} → {}", &found["status"]);
            spans.push(Span { kind: Kind::Http, start: started.max(0.0), ms, label, line: from, repeats: Vec::new() });
        } else if !exception && line.severity() == Level::Error {
            exception = true;
            let named = lines[index..].iter().map(|(_, line)| line.text.trim()).find(|text| text.contains("Exception"));
            let label = named.unwrap_or(text.trim()).to_string();
            spans.push(Span {
                kind: Kind::Exception,
                start: at(line.born),
                ms: 0.0,
                label,
                line: number,
                repeats: Vec::new(),
            });
        }
    }
    for (label, started, line) in pending {
        let span = Span { kind: Kind::Http, start: started, ms: total - started, label, line, repeats: Vec::new() };
        spans.push(span);
    }
    let mut spans = combed(spans);
    spans.sort_by(|a, b| a.start.total_cmp(&b.start));
    let mut waterfall = Waterfall { ms: total, spans, unaccounted: 0.0 };
    waterfall.unaccounted = waterfall.gaps().iter().map(|(start, end)| end - start).sum();
    waterfall
}

/// The same query run [`N_PLUS_ONE`] times or more becomes one comb: from its first run to its last, each run kept.
fn combed(spans: Vec<Span>) -> Vec<Span> {
    let count = |label: &str| spans.iter().filter(|span| span.kind == Kind::Db && span.label == label).count();
    let mut out: Vec<Span> = Vec::new();
    for span in &spans {
        if span.kind != Kind::Db || span.label.is_empty() || count(&span.label) < N_PLUS_ONE {
            out.push(span.clone());
            continue;
        }
        match out.iter_mut().find(|comb| comb.kind == Kind::Db && comb.label == span.label && !comb.repeats.is_empty())
        {
            Some(comb) => {
                comb.repeats.push((span.start, span.ms));
                let end = f64::max(comb.end(), span.end());
                comb.start = comb.start.min(span.start);
                comb.ms = end - comb.start;
            }
            None => out.push(Span { repeats: vec![(span.start, span.ms)], ..span.clone() }),
        }
    }
    out
}

/// A row of two requests side by side: what it is, how the request says it, how the baseline does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub what: &'static str,
    pub this: String,
    pub baseline: String,
}

impl Row {
    pub fn differs(&self) -> bool {
        self.this != self.baseline
    }
}

/// How a comparison's row says a request.
type Say = fn(&Request) -> String;

/// How many lines a request logged, as far as it's known.
fn logged(request: &Request) -> String {
    match (&request.exact, request.logs) {
        (Some(exact), _) => format!("{} lines", exact.len()),
        (None, Some((first, last))) => format!("{} lines", (last + 1).saturating_sub(first)),
        _ => "not known".into(),
    }
}

/// `request` and `baseline` row against row: status, time, protocol, what each sent and returned, its database
/// commands, its log lines.
pub fn compare(request: &Request, baseline: &Request) -> Vec<Row> {
    let say: [(&'static str, Say); 7] = [
        ("status", |request| request.status.to_string()),
        ("time", |request| format!("{:.0} ms", request.ms)),
        ("protocol", |request| request.proto.clone()),
        ("sent", |request| format!("{} {}", request.sent_content, request.sent_length).trim().to_string()),
        ("returned", |request| format!("{} {}", request.content, request.length).trim().to_string()),
        ("database", |request| {
            request
                .db
                .as_ref()
                .map_or("no commands".into(), |db| format!("{} commands, {:.0} ms", db.commands, db.total_ms))
        }),
        ("log", logged),
    ];
    say.iter().map(|(what, say)| Row { what, this: say(request), baseline: say(baseline) }).collect()
}

/// The comparison in a line: how many rows differ, the time it gained or lost, its database commands before and now.
pub fn digest(request: &Request, baseline: &Request) -> String {
    let differ = compare(request, baseline).iter().filter(|row| row.differs()).count();
    let commands = |request: &Request| request.db.as_ref().map_or(0, |db| db.commands);
    format!(
        "{differ} row{} differ · {:+.0} ms · DB commands {} → {}",
        if differ == 1 { "" } else { "s" },
        request.ms - baseline.ms,
        commands(baseline),
        commands(request)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::Output;

    #[test]
    fn a_requests_lines_place_its_commands_calls_and_exception_and_leave_the_rest_to_the_app() {
        let start = Instant::now();
        let mut output = Output::new(100);
        let at = |ms: u64| start + Duration::from_millis(ms);
        output.push("info: Microsoft.EntityFrameworkCore.Database.Command[20101]", at(300));
        output.push("      Executed DbCommand (200ms) [Parameters=[], CommandType='Text']", at(300));
        output.push("      SELECT * FROM Orders", at(300));
        output.push("info: System.Net.Http.HttpClient.billing.LogicalHandler[100]", at(400));
        output.push("      Start processing HTTP request GET http://localhost:5002/api/invoices", at(400));
        output.push("      End processing HTTP request after 350.4ms - 200", at(750));
        output.push("fail: Microsoft.AspNetCore.Diagnostics[1]", at(900));
        output.push_hued("      System.InvalidOperationException: no currency", Some(crate::output::Hue::Red), at(900));
        let lines: Vec<(u64, &OutputLine)> =
            (1..=8).map(|number| (number, output.numbered(number, number).next().unwrap())).collect();
        let waterfall = from_lines(1000.0, start, &lines);
        let kinds: Vec<Kind> = waterfall.spans.iter().map(|span| span.kind).collect();
        assert_eq!(kinds, [Kind::Db, Kind::Http, Kind::Exception]);
        let db = &waterfall.spans[0];
        assert_eq!((db.start, db.ms, db.label.as_str()), (100.0, 200.0, "SELECT * FROM Orders"));
        let call = &waterfall.spans[1];
        assert!((call.start - 400.0).abs() < 1.0 && (call.ms - 350.4).abs() < 0.01, "{call:?}");
        assert_eq!(call.label, "GET http://localhost:5002/api/invoices → 200");
        assert!(waterfall.spans[2].label.contains("InvalidOperationException"));
        assert_eq!(waterfall.longest(), Some(1));
        assert!((waterfall.unaccounted - 449.6).abs() < 1.0, "{}", waterfall.unaccounted);
    }

    #[test]
    fn a_query_run_over_and_over_is_one_comb() {
        let start = Instant::now();
        let mut output = Output::new(100);
        for run in 0..12u64 {
            let at = start + Duration::from_millis(10 + run * 10);
            output.push("      Executed DbCommand (5ms) [Parameters=[@p0='1']]", at);
            output.push("      SELECT * FROM Lines WHERE Id = @p0", at);
        }
        let lines: Vec<(u64, &OutputLine)> =
            (1..=24).map(|number| (number, output.numbered(number, number).next().unwrap())).collect();
        let waterfall = from_lines(200.0, start, &lines);
        assert_eq!(waterfall.spans.len(), 1, "{:?}", waterfall.spans);
        assert_eq!(waterfall.spans[0].repeats.len(), 12);
        assert!((waterfall.spans[0].start - 5.0).abs() < 1.0 && (waterfall.spans[0].end() - 120.0).abs() < 1.0);
    }
}
