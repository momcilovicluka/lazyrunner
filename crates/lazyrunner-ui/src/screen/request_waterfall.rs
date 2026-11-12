//! In the requests view's details (#38): the request's timeline - the request as a bar, then each database command
//! and outbound call where it ran, an N+1 as a comb, the time no line accounts for hatched, the exception's moment, the
//! endpoint's p95 dotted across and an axis - and, with a baseline marked (`m`), the two requests row against row, the
//! baseline's bars as ghosts behind this one's on one scale.

use lazyrunner_core::run::Controller;
use lazyrunner_core::run::requests::{Request, endpoints};
use lazyrunner_core::run::waterfall::{self, Kind, Span, Waterfall};

use super::icons::{self, Icon};
use super::seg::{Seg, seg};
use super::theme::C;

const EIGHTHS: [&str; 8] = ["▏", "▎", "▍", "▌", "▋", "▊", "▉", "█"];
/// The spans listed at most; the rest are counted.
const MOST_SPANS: usize = 8;
/// A row's label, and its time's cells.
const LABEL: usize = 22;
const TOOK: usize = 10;
/// Narrower than this the bars say too little to draw.
const LEAST_BAR: usize = 12;
/// The fewest requests of an endpoint its p95 is drawn from.
const USUAL: usize = 5;
/// Time in the app itself worth profiling (#83): this many ms, and this share of the request.
const IN_THE_APP_MS: f64 = 100.0;
const IN_THE_APP_SHARE: f64 = 0.3;
/// An axis tick at least this many cells from the last.
const TICK_ROOM: usize = 8;
/// The ghost of the baseline's bar, the time no line accounts for, the endpoint's p95, a comb's tooth and its spine.
const GHOST: &str = "▒";
const HATCH: &str = "░";
const P95: &str = "┊";
const TOOTH: &str = "┃";
const SPINE: &str = "─";

/// A bar's cells: how many eighths of each it fills (none: empty).
type Cells = Vec<Option<usize>>;

/// The bars' scale: so many ms over so many cells.
struct Scale {
    ms: f64,
    cells: usize,
}

impl Scale {
    fn eighths(&self, ms: f64) -> usize {
        ((ms / self.ms * (self.cells * 8) as f64).round().max(0.0) as usize).min(self.cells * 8)
    }

    fn cell(&self, ms: f64) -> usize {
        (self.eighths(ms) / 8).min(self.cells.saturating_sub(1))
    }

    /// A bar from `start` to `end`: whole cells from where it starts, its end to an eighth, at least one eighth.
    fn bar(&self, start: f64, end: f64) -> Cells {
        let from = self.eighths(start).min(self.cells * 8 - 1);
        let to = self.eighths(end).max(from + 1);
        (0..self.cells).map(|cell| (cell >= from / 8 && cell * 8 < to).then(|| (to - cell * 8).min(8))).collect()
    }

    /// Several bars as one row's cells.
    fn bars(&self, stretches: &[(f64, f64)]) -> Cells {
        let mut all = vec![None; self.cells];
        for &(start, end) in stretches {
            for (cell, filled) in self.bar(start, end).into_iter().enumerate() {
                all[cell] = all[cell].max(filled);
            }
        }
        all
    }
}

fn waterfall_of(controller: &Controller, request: &Request) -> Option<Waterfall> {
    waterfall::of(request, &controller.state(&request.app)?.output)
}

/// The p95 of the request's endpoint, over the requests kept, once there are a few.
fn p95(controller: &Controller, request: &Request) -> Option<f64> {
    let same = controller.requests.items.iter().filter(|other| {
        other.app == request.app && other.method == request.method && other.endpoint == request.endpoint
    });
    endpoints(same).pop().filter(|usual| usual.count >= USUAL).map(|usual| usual.p95)
}

/// A bar's cells as segments - eighths in `color` (or `glyph` for each filled cell) - over the baseline's ghost and
/// the p95 line.
fn bar(cells: &Cells, color: C, glyph: Option<&'static str>, ghost: &Cells, p95: Option<usize>) -> Vec<Seg> {
    cells
        .iter()
        .enumerate()
        .map(|(cell, filled)| match (filled, ghost.get(cell).copied().flatten()) {
            (Some(eighths), _) => seg(glyph.unwrap_or(EIGHTHS[eighths.saturating_sub(1)]), color),
            (None, Some(_)) => seg(GHOST, C::DarkGray),
            (None, None) if p95 == Some(cell) => seg(P95, C::Yellow),
            _ => seg(" ", C::Gray),
        })
        .collect()
}

/// A row: its label, its bar, its time.
fn row(label: Vec<Seg>, bar: Vec<Seg>, ms: f64) -> Vec<Seg> {
    let mut line = super::columns::fit(label, LABEL);
    line.push(seg(" ", C::Gray));
    line.extend(bar);
    line.push(seg(format!("{:>width$}", format!("{ms:.0} ms"), width = TOOK), C::Gray));
    line
}

/// A span's label: its kind's icon, how many runs for a comb, the SQL or the call.
fn label(nerd: bool, span: &Span, color: C) -> Vec<Seg> {
    let icon = match span.kind {
        Kind::Db => icons::icon(nerd, Icon::Database),
        _ => icons::icon(nerd, Icon::Requests),
    };
    let runs = if span.repeats.is_empty() { String::new() } else { format!("×{} ", span.repeats.len()) };
    vec![seg(format!("{icon} {runs}{}", span.label), color)]
}

/// The baseline's span that matches `span` (the same kind and what it ran), each used once.
fn matching<'a>(span: &Span, ghosts: &'a [Span], used: &mut Vec<usize>) -> Option<&'a Span> {
    let at = (0..ghosts.len())
        .find(|&at| !used.contains(&at) && ghosts[at].kind == span.kind && ghosts[at].label == span.label)?;
    used.push(at);
    ghosts.get(at)
}

/// What the timeline's title says: where the time went, the p95 line, the baseline's ghosts.
fn summary(fall: &Waterfall, usual: Option<f64>, baseline: Option<&Request>) -> String {
    let spent = |kind: Kind| -> f64 {
        let spans = fall.spans.iter().filter(|span| span.kind == kind);
        spans
            .map(|span| if span.repeats.is_empty() { span.ms } else { span.repeats.iter().map(|(_, ms)| ms).sum() })
            .sum::<f64>()
            + 0.0
    };
    let mut summary = format!(
        "Timeline  {:.0} ms in the database · {:.0} ms in calls · {:.0} ms in the app",
        spent(Kind::Db),
        spent(Kind::Http),
        fall.unaccounted
    );
    if let Some(usual) = usual {
        summary.push_str(&format!(" · {P95} p95 {usual:.0} ms"));
    }
    if let Some(baseline) = baseline {
        summary.push_str(&format!(" · {GHOST} ★ #{}", baseline.seq));
    }
    summary
}

/// The timeline, `width` wide; nothing when its lines aren't known or kept, or there's no room.
pub(super) fn timeline(
    nerd: bool,
    controller: &Controller,
    request: &Request,
    baseline: Option<&Request>,
    width: usize,
) -> Vec<Vec<Seg>> {
    let Some(fall) = waterfall_of(controller, request) else { return Vec::new() };
    let ghost = baseline.and_then(|baseline| waterfall_of(controller, baseline));
    let usual = p95(controller, request);
    let cells = width.saturating_sub(LABEL + TOOK + 1);
    if cells < LEAST_BAR || fall.ms <= 0.0 {
        return Vec::new();
    }
    let longest = [fall.ms, usual.unwrap_or(0.0), ghost.as_ref().map_or(0.0, |ghost| ghost.ms)];
    let scale = Scale { ms: longest.into_iter().fold(1.0, f64::max), cells };
    let p95 = usual.map(|usual| scale.cell(usual));
    let none = vec![None; cells];
    let title = summary(&fall, usual, baseline.filter(|_| ghost.is_some()));
    let mut lines = vec![Vec::new()];
    lines.extend(super::seg::wrap(&[seg(title, C::White)], width, 2, 2));
    let whole_ghost = ghost.as_ref().map_or_else(|| none.clone(), |ghost| scale.bar(0.0, ghost.ms));
    let color = super::requests::status_color(request.status);
    let mut whole = bar(&scale.bar(0.0, fall.ms), color, None, &whole_ghost, p95);
    let exception = fall.spans.iter().find(|span| span.kind == Kind::Exception);
    if let Some(exception) = exception {
        whole[scale.cell(exception.start)] = seg(icons::icon(nerd, Icon::Fail), C::Red);
    }
    lines.push(row(vec![seg("request", C::Gray)], whole, fall.ms));
    let longest = fall.longest();
    let timed: Vec<(usize, &Span)> =
        fall.spans.iter().enumerate().filter(|(_, span)| span.kind != Kind::Exception).collect();
    let ghosts = ghost.as_ref().map_or(&[][..], |ghost| ghost.spans.as_slice());
    let mut used = Vec::new();
    for &(at, span) in timed.iter().take(MOST_SPANS) {
        let color = match (span.kind, Some(at) == longest) {
            (_, true) => C::Yellow,
            (Kind::Db, false) => C::DarkCyan,
            _ => C::Blue,
        };
        let shadow =
            matching(span, ghosts, &mut used).map_or_else(|| none.clone(), |ghost| scale.bar(ghost.start, ghost.end()));
        let cells = if span.repeats.is_empty() {
            bar(&scale.bar(span.start, span.end()), color, None, &shadow, p95)
        } else {
            let runs: Vec<(f64, f64)> = span.repeats.iter().map(|&(start, ms)| (start, start + ms)).collect();
            let mut comb = bar(&scale.bars(&runs), color, Some(TOOTH), &shadow, p95);
            for part in &mut comb[scale.cell(span.start)..=scale.cell(span.end())] {
                if part.text != TOOTH {
                    *part = seg(SPINE, C::DarkGray);
                }
            }
            comb
        };
        lines.push(row(label(nerd, span, color), cells, span.ms));
    }
    if timed.len() > MOST_SPANS {
        let more = timed.len() - MOST_SPANS;
        lines.push(vec![seg(format!("… {more} more (get_request lists them all)"), C::DarkGray)]);
    }
    if fall.unaccounted >= 1.0 {
        let gaps = bar(&scale.bars(&fall.gaps()), C::DarkYellow, Some(HATCH), &none, p95);
        lines.push(row(vec![seg("in the app", C::DarkYellow)], gaps, fall.unaccounted));
    }
    lines.push(axis(&scale));
    if fall.unaccounted >= IN_THE_APP_MS && fall.unaccounted >= fall.ms * IN_THE_APP_SHARE {
        let name = controller.app(&request.app).map_or(request.app.as_str(), |app| app.name.as_str());
        let text = format!(
            "◉ f profiles {name}: {:.0} ms of it ran in the app itself, where no line says what",
            fall.unaccounted
        );
        lines.push(super::columns::fit(vec![seg(text, C::Magenta)], width));
    }
    if let Some(exception) = exception {
        let text = format!("{} at {:.0} ms: {}", icons::icon(nerd, Icon::Fail), exception.start, exception.label);
        lines.push(super::columns::fit(vec![seg(text, C::Red)], width));
    }
    lines
}

/// The milliseconds under the bars: round ticks as far apart as they fit.
fn axis(scale: &Scale) -> Vec<Seg> {
    let room = (scale.cells / TICK_ROOM).max(1) as f64;
    let rough = scale.ms / room;
    let power = 10f64.powf(rough.log10().floor());
    let step =
        [1.0, 2.0, 2.5, 5.0, 10.0].iter().map(|times| times * power).find(|step| *step >= rough).unwrap_or(rough);
    let mut text = vec![' '; scale.cells];
    let (mut free, mut tick) = (0, 0.0);
    while tick <= scale.ms {
        let label = format!("{tick:.0}");
        let at = scale.cell(tick);
        if at >= free && at + label.len() <= scale.cells {
            text[at..at + label.len()].copy_from_slice(&label.chars().collect::<Vec<char>>());
            free = at + label.len() + 1;
        }
        tick += step;
    }
    let ticks: String = text.into_iter().collect();
    vec![seg(" ".repeat(LABEL + 1), C::Gray), seg(ticks, C::DarkGray), seg(" ms", C::DarkGray)]
}

/// This request against the baseline, row against row: what differs amber, what's the same dim, a digest on top.
pub(super) fn compare(request: &Request, baseline: &Request, width: usize) -> Vec<Vec<Seg>> {
    let column = width.saturating_sub(12) / 2;
    let digest = waterfall::digest(request, baseline);
    let them = format!("★ #{}", baseline.seq);
    let mut lines = vec![
        Vec::new(),
        vec![seg(format!("★ Against #{}  ", baseline.seq), C::White), seg(digest, C::Yellow)],
        vec![seg(format!("{:<10}  {:<column$}  {them}", "", "this"), C::DarkGray)],
    ];
    for row in waterfall::compare(request, baseline) {
        let cut = |text: &str| text.chars().take(column).collect::<String>();
        let (this, them) = if row.differs() { (C::Yellow, C::Gray) } else { (C::DarkGray, C::DarkGray) };
        lines.push(vec![
            seg(format!("{:<10}  ", row.what), C::DarkGray),
            seg(format!("{:<column$}  ", cut(&row.this)), this),
            seg(cut(&row.baseline), them),
        ]);
    }
    lines
}
