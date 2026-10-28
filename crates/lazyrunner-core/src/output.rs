//! What an app printed: a bounded buffer of lines with their level and arrival time, and a lines-per-second meter.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Debug,
    Info,
    Warn,
    Error,
}

impl Level {
    /// Guesses a line's level from the words servers usually print.
    pub fn of(text: &str) -> Self {
        let head = text.char_indices().nth(40).map_or(text, |(end, _)| &text[..end]);
        if head.contains("ERR") || head.contains("error") || head.contains("fail") || head.contains("FATAL") {
            Self::Error
        } else if head.contains("WRN") || head.contains("WARN") || head.contains("warn") {
            Self::Warn
        } else if head.contains("DBG") || head.contains("DEBUG") || head.contains("TRACE") {
            Self::Debug
        } else {
            Self::Info
        }
    }
}

#[derive(Clone, Debug)]
pub struct OutputLine {
    pub text: String,
    pub level: Level,
    pub born: Instant,
    /// The colour lazyrunner gave the line (app output, its own notes); `None`: by its level.
    pub hue: Option<Hue>,
}

impl OutputLine {
    /// How serious the line is: by the colour lazyrunner gave it (an entry's level, carried onto its indented lines),
    /// else by its own words.
    pub fn severity(&self) -> Level {
        match self.hue {
            Some(Hue::Red) => Level::Error,
            Some(Hue::Yellow | Hue::DarkYellow) => Level::Warn,
            Some(Hue::DarkGray) => Level::Debug,
            Some(_) => Level::Info,
            None => self.level,
        }
    }
}

/// Which lines a column shows (`f` cycles it): all of them, errors only, warnings and errors, or info and up.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LevelFilter {
    #[default]
    All,
    Errors,
    Warnings,
    Info,
}

impl LevelFilter {
    /// The next one `f` gives: all, errors, warnings and errors, info and up, all again.
    pub fn next(self) -> Self {
        match self {
            Self::All => Self::Errors,
            Self::Errors => Self::Warnings,
            Self::Warnings => Self::Info,
            Self::Info => Self::All,
        }
    }

    /// Whether the column shows `line`.
    pub fn admits(self, line: &OutputLine) -> bool {
        let least = match self {
            Self::All => return true,
            Self::Errors => Level::Error,
            Self::Warnings => Level::Warn,
            Self::Info => Level::Info,
        };
        line.severity() >= least
    }

    /// Its name in the settings file (`"apps": { "<key>": { "filter": … } }`); all lines have none.
    pub fn key(self) -> &'static str {
        match self {
            Self::All => "",
            Self::Errors => "errors",
            Self::Warnings => "warnings",
            Self::Info => "info",
        }
    }

    pub fn from_key(key: &str) -> Self {
        match key {
            "errors" => Self::Errors,
            "warnings" => Self::Warnings,
            "info" => Self::Info,
            _ => Self::All,
        }
    }

    /// What it shows, as the column and the log say it.
    pub fn label(self) -> &'static str {
        match self {
            Self::All => "all lines",
            Self::Errors => "errors only",
            Self::Warnings => "warnings and errors",
            Self::Info => "info and up",
        }
    }
}

/// The colours lazyrunner gives lines, named as the themes name them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Hue {
    Gray,
    DarkGray,
    Cyan,
    DarkCyan,
    Green,
    DarkGreen,
    Yellow,
    DarkYellow,
    Red,
    Magenta,
    White,
}

#[derive(Debug)]
pub struct Output {
    lines: VecDeque<OutputLine>,
    capacity: usize,
    total: u64,
    errors: u64,
    warnings: u64,
    last_line_at: Option<Instant>,
}

impl Output {
    pub fn new(capacity: usize) -> Self {
        Self {
            lines: VecDeque::with_capacity(capacity.min(4096)),
            capacity: capacity.max(1),
            total: 0,
            errors: 0,
            warnings: 0,
            last_line_at: None,
        }
    }

    pub fn push(&mut self, text: &str, now: Instant) {
        self.push_hued(text, None, now);
    }

    /// A line in the colour lazyrunner chose for it.
    pub fn push_hued(&mut self, text: &str, hue: Option<Hue>, now: Instant) {
        let text = strip_ansi(text.trim_end_matches(['\r', '\n']));
        // A progress line rewrites itself with \r: keep what the terminal would end up showing.
        let shown = text.rsplit('\r').next().unwrap_or_default().replace('\t', "    ");
        let text: String = shown.chars().filter(|c| !c.is_control()).collect();
        let level = Level::of(&text);
        match level {
            Level::Error => self.errors += 1,
            Level::Warn => self.warnings += 1,
            _ => {}
        }
        if self.lines.len() == self.capacity {
            self.lines.pop_front();
        }
        self.lines.push_back(OutputLine { text, level, born: now, hue });
        self.total += 1;
        self.last_line_at = Some(now);
    }

    /// Forgets the lines (Ctrl+L); the counters stay.
    pub fn clear(&mut self) {
        self.lines.clear();
    }

    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// Lines from the newest back, skipping the `skip` newest (scrolled back).
    pub fn newest_first(&self, skip: usize) -> impl Iterator<Item = &OutputLine> {
        self.lines.iter().rev().skip(skip)
    }

    pub fn total(&self) -> u64 {
        self.total
    }

    /// The number of the line at `position` among those kept (0 = the oldest); the first line ever pushed is 1.
    pub fn number_at(&self, position: usize) -> u64 {
        self.total + 1 - self.lines.len() as u64 + position as u64
    }

    /// Where line `number` is among those kept (0 = the oldest), while it still is.
    pub fn position_of(&self, number: u64) -> Option<usize> {
        let oldest = self.total + 1 - self.lines.len() as u64;
        (number >= oldest && number <= self.total).then(|| (number - oldest) as usize)
    }

    /// The lines numbered `first..=last` still kept (the first line ever pushed is 1).
    pub fn numbered(&self, first: u64, last: u64) -> impl Iterator<Item = &OutputLine> {
        let oldest = self.total + 1 - self.lines.len() as u64;
        let skip = first.saturating_sub(oldest) as usize;
        let take = (last + 1).saturating_sub(first.max(oldest)) as usize;
        self.lines.iter().skip(skip).take(take)
    }

    pub fn errors(&self) -> u64 {
        self.errors
    }

    pub fn warnings(&self) -> u64 {
        self.warnings
    }

    pub fn last_line_at(&self) -> Option<Instant> {
        self.last_line_at
    }
}

/// Drops terminal escape sequences (colours, cursor moves); the P3 output view renders colours itself.
pub fn strip_ansi(text: &str) -> String {
    if !text.contains('\x1b') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        if chars.peek() == Some(&'[') {
            chars.next();
            for c in chars.by_ref() {
                if ('@'..='~').contains(&c) {
                    break;
                }
            }
        } else if chars.peek() == Some(&']') {
            // An operating system command (window title, hyperlink): up to BEL or ESC \.
            chars.next();
            while let Some(c) = chars.next() {
                if c == '\x07' || (c == '\x1b' && chars.next_if_eq(&'\\').is_some()) {
                    break;
                }
            }
        } else {
            chars.next();
        }
    }
    out
}

/// Counts events per second over the last `seconds` whole seconds, for "lines/s" and its sparkline.
#[derive(Debug)]
pub struct RateMeter {
    started: Instant,
    /// Counts per second, oldest first; the last one is the second in progress.
    buckets: VecDeque<u32>,
    /// The second (since `started`) that the last bucket counts.
    current: u64,
    seconds: usize,
}

impl RateMeter {
    pub fn new(started: Instant, seconds: usize) -> Self {
        Self { started, buckets: VecDeque::from([0]), current: 0, seconds: seconds.max(2) }
    }

    pub fn add(&mut self, now: Instant, count: u32) {
        self.advance(now);
        if let Some(last) = self.buckets.back_mut() {
            *last = last.saturating_add(count);
        }
    }

    /// The last complete second's count.
    pub fn per_second(&mut self, now: Instant) -> u32 {
        self.advance(now);
        let len = self.buckets.len();
        if len < 2 { 0 } else { self.buckets[len - 2] }
    }

    /// Complete seconds, oldest first.
    pub fn history(&mut self, now: Instant) -> Vec<u32> {
        self.advance(now);
        let complete = self.buckets.len().saturating_sub(1);
        self.buckets.iter().take(complete).copied().collect()
    }

    fn advance(&mut self, now: Instant) {
        let second = now.saturating_duration_since(self.started).as_secs();
        let missing = second.saturating_sub(self.current).min(self.seconds as u64 + 1);
        for _ in 0..missing {
            self.buckets.push_back(0);
        }
        self.current = self.current.max(second);
        while self.buckets.len() > self.seconds + 1 {
            self.buckets.pop_front();
        }
    }
}

/// Time since `then`, saturating at zero.
pub fn age(then: Instant, now: Instant) -> Duration {
    now.saturating_duration_since(then)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_keeps_its_number_as_older_ones_go() {
        let now = Instant::now();
        let mut output = Output::new(3);
        for text in ["one", "two", "three", "four"] {
            output.push(text, now);
        }
        assert_eq!(output.number_at(0), 2, "one went");
        assert_eq!(output.position_of(4), Some(2));
        assert_eq!(output.position_of(1), None, "gone");
        assert_eq!(output.numbered(3, 3).next().map(|line| line.text.as_str()), Some("three"));
    }

    #[test]
    fn a_level_filter_goes_by_the_lines_colour_and_cycles() {
        let line = |text: &str, hue: Option<Hue>| OutputLine {
            text: text.into(),
            level: Level::of(text),
            born: Instant::now(),
            hue,
        };
        let stack = line("      at Orders.Api.Checkout()", Some(Hue::Red));
        let warned = line("warn: Orders.Api[2]", Some(Hue::Yellow));
        let said = line("      Listing orders", Some(Hue::Gray));
        let noise = line("npm verb cli", Some(Hue::DarkGray));
        let note = line("error: it broke", None);
        assert!(
            LevelFilter::Errors.admits(&stack) && LevelFilter::Errors.admits(&note),
            "an entry's indented lines too"
        );
        assert!(!LevelFilter::Errors.admits(&warned) && LevelFilter::Warnings.admits(&warned));
        assert!(!LevelFilter::Warnings.admits(&said) && LevelFilter::Info.admits(&said));
        assert!(!LevelFilter::Info.admits(&noise) && LevelFilter::All.admits(&noise));
        let mut filter = LevelFilter::All;
        let cycle: Vec<LevelFilter> = (0..4)
            .map(|_| {
                filter = filter.next();
                filter
            })
            .collect();
        assert_eq!(cycle, [LevelFilter::Errors, LevelFilter::Warnings, LevelFilter::Info, LevelFilter::All]);
        for filter in cycle {
            assert_eq!(LevelFilter::from_key(filter.key()), filter);
        }
    }

    #[test]
    fn keeps_only_the_newest_lines_but_counts_them_all() {
        let now = Instant::now();
        let mut output = Output::new(3);
        for i in 0..5 {
            output.push(&format!("line {i}"), now);
        }
        assert_eq!(output.len(), 3);
        assert_eq!(output.total(), 5);
        let newest: Vec<&str> = output.newest_first(0).map(|line| line.text.as_str()).collect();
        assert_eq!(newest, ["line 4", "line 3", "line 2"]);
        assert_eq!(output.newest_first(2).next().unwrap().text, "line 2");
    }

    #[test]
    fn levels_and_counters_come_from_the_text() {
        let now = Instant::now();
        let mut output = Output::new(10);
        output.push("12:00:00.000 ERR  boom", now);
        output.push("12:00:00.000 WRN  slow", now);
        output.push("12:00:00.000 DBG  cache", now);
        output.push("12:00:00.000 INF  GET /", now);
        let levels: Vec<Level> = output.newest_first(0).map(|line| line.level).collect();
        assert_eq!(levels, [Level::Info, Level::Debug, Level::Warn, Level::Error]);
        assert_eq!((output.errors(), output.warnings()), (1, 1));
    }

    #[test]
    fn strips_colours_and_line_endings() {
        let mut output = Output::new(2);
        output.push("\x1b[32mready\x1b[0m in 5 ms\r\n", Instant::now());
        assert_eq!(output.newest_first(0).next().unwrap().text, "ready in 5 ms");
        output.push("\x1b]0;title\x07a\tb \x1b]8;;http://x\x1b\\link", Instant::now());
        assert_eq!(
            output.newest_first(0).next().unwrap().text,
            "a    b link",
            "titles and links go, tabs become spaces"
        );
    }

    #[test]
    fn a_progress_line_keeps_what_the_terminal_shows_and_no_control_characters() {
        let mut output = Output::new(2);
        output.push("10%\r50%\r100% done\x07", Instant::now());
        assert_eq!(output.newest_first(0).next().unwrap().text, "100% done");
    }

    #[test]
    fn the_rate_meter_reports_whole_seconds() {
        let start = Instant::now();
        let mut meter = RateMeter::new(start, 5);
        meter.add(start, 10);
        meter.add(start + Duration::from_millis(900), 5);
        assert_eq!(meter.per_second(start + Duration::from_millis(950)), 0, "the first second isn't over yet");
        meter.add(start + Duration::from_millis(1500), 7);
        assert_eq!(meter.per_second(start + Duration::from_millis(1600)), 15);
        assert_eq!(meter.per_second(start + Duration::from_millis(2100)), 7);
        assert_eq!(meter.per_second(start + Duration::from_millis(4100)), 0, "quiet seconds count as zero");
        assert_eq!(meter.history(start + Duration::from_secs(10)).len(), 5, "history is bounded");
    }
}
