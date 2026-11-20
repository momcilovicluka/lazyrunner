//! An app's output as log entries, for a column's parsed view (#27): an ASP.NET console entry with its indented
//! lines, a Serilog line, a plain line - each one with its level, the time it says, its category and its message; a
//! stack's frames counted, an EF Core command's time and SQL on one line, and a line that comes again and again one
//! entry `×N`. Built as the lines arrive and kept beside them, as many as their lines.

use std::collections::VecDeque;
use std::sync::LazyLock;

use regex::Regex;

use super::requests;
use crate::output::{FNV_OFFSET, Level, OutputLine, mix, shape};

/// EF Core's category for the commands it runs.
pub const EF_COMMAND: &str = "Microsoft.EntityFrameworkCore.Database.Command";
/// An EF Core command's SQL kept on its row, at most.
const SQL_CHARS: usize = 400;

/// One log entry: its lines, and what they say.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    /// Its lines' numbers, `first..=last` (the first line ever pushed is 1); the oldest go with the buffer's.
    pub first: u64,
    pub last: u64,
    pub level: Level,
    /// The time the line itself says (Serilog's `[10:00:00 INF]`).
    pub stamp: Option<String>,
    /// Its logger's category in full (`Microsoft.EntityFrameworkCore.Database.Command`), when it has one.
    pub category: Option<String>,
    pub message: String,
    /// Its stack frames.
    pub frames: usize,
    /// Its lines after the message that aren't frames: an exception's, or an EF Core command's SQL.
    pub details: usize,
    /// An EF Core command: how long it took, and its SQL on one line.
    pub sql: Option<Sql>,
    /// How many times it came, one after the other (1: once).
    pub repeats: usize,
    /// Indented lines go on it (an ASP.NET or Serilog entry); a plain line takes only a stack's.
    structured: bool,
    /// Its header came without a message: the next indented line is it.
    awaiting: bool,
}

/// An EF Core command: how long it took and the SQL it ran.
#[derive(Clone, Debug, PartialEq)]
pub struct Sql {
    pub ms: f64,
    pub command: String,
}

impl Entry {
    fn new(first: u64, level: Level) -> Self {
        Self {
            first,
            last: first,
            level,
            stamp: None,
            category: None,
            message: String::new(),
            frames: 0,
            details: 0,
            sql: None,
            repeats: 1,
            structured: false,
            awaiting: false,
        }
    }

    /// How many lines it has.
    pub fn lines(&self) -> u64 {
        self.last + 1 - self.first
    }

    /// Whether line `number` is one of its own.
    pub fn holds(&self, number: u64) -> bool {
        (self.first..=self.last).contains(&number)
    }

    /// Whether its lines are more than the row it is folded to shows.
    pub fn folds(&self) -> bool {
        self.repeats == 1 && self.frames + self.details > 0
    }

    /// The entry on one line, as an agent reads it (`get_logs` with `parsed`): its level, the time it says, its
    /// category, its message - or an EF Core command's time and SQL - and its frames and repeats.
    pub fn said(&self) -> String {
        let mut said = level_name(self.level).to_string();
        for part in [self.stamp.as_deref(), self.category.as_deref()].into_iter().flatten() {
            said.push(' ');
            said.push_str(part);
        }
        match &self.sql {
            Some(sql) => said.push_str(&format!(" SQL {} ms · {}", sql.ms, sql.command)),
            None => {
                said.push(' ');
                said.push_str(&self.message);
            }
        }
        if self.frames > 0 {
            said.push_str(&format!(" (+{} frames)", self.frames));
        }
        if self.repeats > 1 {
            said.push_str(&format!(" ×{}", self.repeats));
        }
        said
    }

    /// What another of it looks like: its level, category and message with what changes each time blanked.
    fn kind(&self) -> u64 {
        let category = self.category.as_deref().unwrap_or_default();
        let message = match NPM_NOISE.find(&self.message) {
            Some(prefix) => shape(prefix.as_str()),
            None => shape(&self.message),
        };
        let sql = self.sql.as_ref().map_or(0, |sql| shape(&sql.command));
        let level = level_name(self.level);
        [level.as_bytes(), category.as_bytes(), &message.to_le_bytes(), &sql.to_le_bytes()]
            .iter()
            .fold(FNV_OFFSET, |hash, bytes| mix(hash, bytes))
    }

    /// Whether `next` is this entry again: nothing more to it than a message (or an EF Core command) like its own.
    fn repeated_by(&self, next: &Entry) -> bool {
        let plain = |entry: &Entry| entry.frames == 0 && !entry.awaiting && (entry.details == 0 || entry.sql.is_some());
        plain(self) && plain(next) && self.details == next.details && self.kind() == next.kind()
    }
}

/// A level's badge: `INF`, `WRN`, `ERR`, `DBG`.
pub fn level_name(level: Level) -> &'static str {
    match level {
        Level::Debug => "DBG",
        Level::Info => "INF",
        Level::Warn => "WRN",
        Level::Error => "ERR",
    }
}

/// A category as a column has room for: the namespace trimmed to its last parts - `EFCore.Command` for EF Core's
/// commands, `Hosting.Lifetime` for Microsoft's, `Orders.Api.Checkout` for an app's own.
pub fn short_category(full: &str) -> String {
    let full = full.split('[').next().unwrap_or(full);
    let (vendor, rest) = match full.split_once('.') {
        Some(("Microsoft" | "System", rest)) => (true, rest),
        _ => (false, full),
    };
    let rest = rest.strip_prefix("EntityFrameworkCore").map_or(rest.to_string(), |tail| format!("EFCore{tail}"));
    let parts: Vec<&str> = rest.split('.').filter(|part| !part.is_empty()).collect();
    match parts.as_slice() {
        [first, .., last] if vendor && parts.len() > 2 => format!("{first}.{last}"),
        _ if parts.len() > APP_PARTS => parts[parts.len() - APP_PARTS..].join("."),
        _ => parts.join("."),
    }
}

/// An app's own category keeps this many of its last parts.
const APP_PARTS: usize = 3;

/// An app's entries, oldest first.
#[derive(Debug, Default)]
pub struct Entries {
    list: VecDeque<Entry>,
    /// The entries there have been, a repeat not counting as another: a scrolled-back view moves by as many.
    counted: u64,
}

impl Entries {
    pub fn len(&self) -> usize {
        self.list.len()
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    /// The entries there have been since the first line, a repeat not counting as another.
    pub fn counted(&self) -> u64 {
        self.counted
    }

    /// The entries, oldest first.
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &Entry> + ExactSizeIterator {
        self.list.iter()
    }

    /// The entry line `number` is in.
    pub fn of(&self, number: u64) -> Option<&Entry> {
        let at = self.list.partition_point(|entry| entry.last < number);
        self.list.get(at).filter(|entry| entry.holds(number))
    }

    pub fn clear(&mut self) {
        self.list.clear();
    }

    /// The lines before `oldest` are gone from the buffer: the entries that were all theirs go too.
    pub fn trim(&mut self, oldest: u64) {
        while self.list.front().is_some_and(|entry| entry.last < oldest) {
            self.list.pop_front();
        }
        if let Some(front) = self.list.front_mut() {
            front.first = front.first.max(oldest);
        }
    }

    /// Line `number` came: it starts an entry, goes on the newest one, or is that one again.
    pub fn push(&mut self, number: u64, line: &OutputLine) {
        let text = line.text.as_str();
        if let Some(entry) = header(number, text) {
            self.settle();
            self.start(entry);
            return;
        }
        if let Some(newest) = self.list.back_mut().filter(|newest| continues(newest, text)) {
            newest.add(number, text);
            return;
        }
        self.settle();
        let mut entry = Entry::new(number, line.severity());
        entry.message = text.trim().to_string();
        self.start(entry);
        self.settle(); // a plain line is whole as it comes
    }

    fn start(&mut self, entry: Entry) {
        self.list.push_back(entry);
        self.counted += 1;
    }

    /// The newest entry is whole: when it's the one before it again, it counts as that one's repeat.
    fn settle(&mut self) {
        let count = self.list.len();
        if count < 2 || !self.list[count - 2].repeated_by(&self.list[count - 1]) {
            return;
        }
        let Some(newest) = self.list.pop_back() else { return };
        self.counted -= 1;
        let Some(before) = self.list.back_mut() else { return };
        before.last = newest.last;
        before.repeats += 1;
        (before.message, before.stamp, before.sql) = (newest.message, newest.stamp, newest.sql);
    }
}

impl Entry {
    /// An indented line of its own: the message its header waited for, a frame, or more of what it says.
    fn add(&mut self, number: u64, text: &str) {
        self.last = number;
        let trimmed = text.trim();
        if self.awaiting && !FRAME.is_match(text) {
            self.awaiting = false;
            self.message = trimmed.to_string();
            let command = self.category.as_deref() == Some(EF_COMMAND);
            if let Some(ms) = requests::db_command_ms(trimmed).filter(|_| command) {
                self.sql = Some(Sql { ms, command: String::new() });
            }
            return;
        }
        if FRAME.is_match(text) {
            self.frames += 1;
            return;
        }
        self.details += 1;
        if let Some(sql) = self.sql.as_mut().filter(|sql| sql.command.len() < SQL_CHARS) {
            if !sql.command.is_empty() {
                sql.command.push(' ');
            }
            sql.command.push_str(trimmed);
        }
    }
}

/// Whether `text` goes on the entry `newest`: a stack's frame, the end of an inner exception's stack, or - for an
/// ASP.NET or Serilog entry - an indented line or the exception printed after it.
fn continues(newest: &Entry, text: &str) -> bool {
    if FRAME.is_match(text) || text.starts_with("--- ") {
        return true;
    }
    newest.structured && (INDENTED.is_match(text) || EXCEPTION.is_match(text))
}

/// The entry a header line starts: ASP.NET's console logger (`info: Category[id]`, its message on the next line or
/// after it), or Serilog's console sink (`[10:00:00 INF] message`).
fn header(number: u64, text: &str) -> Option<Entry> {
    if let Some(found) = ASPNET.captures(text) {
        let level = match found[1].to_lowercase().as_str() {
            "info" => Level::Info,
            "warn" => Level::Warn,
            "fail" | "crit" => Level::Error,
            _ => Level::Debug,
        };
        let mut entry = Entry::new(number, level);
        entry.category = Some(found[2].to_string());
        entry.message = found.get(3).map_or("", |message| message.as_str()).trim().to_string();
        (entry.structured, entry.awaiting) = (true, entry.message.is_empty());
        return Some(entry);
    }
    let found = SERILOG.captures(text)?;
    let level = match &found[2] {
        "ERR" | "FTL" => Level::Error,
        "WRN" => Level::Warn,
        "INF" => Level::Info,
        _ => Level::Debug,
    };
    let mut entry = Entry::new(number, level);
    entry.stamp = Some(found[1].to_string());
    entry.message = found[3].trim().to_string();
    entry.structured = true;
    Some(entry)
}

/// Whether a line is a stack frame: .NET's, Node's and Java's `at …`, Python's `File "…"`.
pub fn is_frame(text: &str) -> bool {
    FRAME.is_match(text)
}

fn pattern(regex: &str) -> Regex {
    Regex::new(regex).expect("a valid pattern")
}

static ASPNET: LazyLock<Regex> =
    LazyLock::new(|| pattern(r"(?i)^(info|warn|fail|crit|dbug|trce): ([^\s\[]+)(?:\[\d+\])?:?(?:\s+(.*))?$"));
static SERILOG: LazyLock<Regex> =
    LazyLock::new(|| pattern(r"^\[(\d{2}:\d{2}:\d{2})(?:\.\d+)? (VRB|DBG|INF|WRN|ERR|FTL)\] ?(.*)$"));
static FRAME: LazyLock<Regex> = LazyLock::new(|| pattern(r#"^\s+(at \S|File ")"#));
static INDENTED: LazyLock<Regex> = LazyLock::new(|| pattern(r"^\s+\S"));
/// An exception's first line, as Serilog prints it under its entry: `System.InvalidOperationException: no currency`.
static EXCEPTION: LazyLock<Regex> = LazyLock::new(|| pattern(r"^[\w.`]+(Exception|Error)\b"));
/// npm's chatter: the lines after it with the same word repeat it, whatever they say.
static NPM_NOISE: LazyLock<Regex> = LazyLock::new(|| pattern(r"(?i)^npm (verbose|info|timing|http|sill)\b"));

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;
    use crate::output::{Hue, Output};

    fn entries(lines: &[(&str, Hue)]) -> Vec<Entry> {
        let mut output = Output::new(100);
        for (text, hue) in lines {
            output.push_hued(text, Some(*hue), Instant::now());
        }
        output.entries().iter().cloned().collect()
    }

    fn gray(lines: &[&str]) -> Vec<Entry> {
        entries(&lines.iter().map(|line| (*line, Hue::Gray)).collect::<Vec<_>>())
    }

    #[test]
    fn an_aspnet_entry_takes_its_indented_lines_and_its_message_from_the_first() {
        let found = gray(&[
            "info: Microsoft.Hosting.Lifetime[14]",
            "      Now listening on: http://localhost:5000",
            "warn: Orders.Api.Checkout[3]",
            "      Card declined",
            "      for order 42",
            "fail: Orders.Api.Payments[0] Payment gateway timed out",
            "plain line",
        ]);
        type Summary<'a> = (u64, u64, Level, Option<&'a str>, &'a str, usize);
        let summary: Vec<Summary> = found
            .iter()
            .map(|entry| {
                (entry.first, entry.last, entry.level, entry.category.as_deref(), entry.message.as_str(), entry.details)
            })
            .collect();
        assert_eq!(
            summary,
            [
                (1, 2, Level::Info, Some("Microsoft.Hosting.Lifetime"), "Now listening on: http://localhost:5000", 0),
                (3, 5, Level::Warn, Some("Orders.Api.Checkout"), "Card declined", 1),
                (6, 6, Level::Error, Some("Orders.Api.Payments"), "Payment gateway timed out", 0),
                (7, 7, Level::Info, None, "plain line", 0),
            ]
        );
    }

    #[test]
    fn a_serilog_line_says_its_own_time_and_level_and_keeps_its_exception() {
        let found = gray(&[
            "[10:00:00 INF] Starting web host",
            "[10:00:01.123 ERR] Unhandled exception",
            "System.InvalidOperationException: no currency",
            "   at Orders.Api.Checkout.Pay() in C:\\src\\Orders\\Checkout.cs:line 42",
            "   at Microsoft.AspNetCore.Routing.EndpointMiddleware.Invoke(HttpContext context)",
            "[10:00:02 DBG] Cache warm",
        ]);
        assert_eq!(found.len(), 3);
        assert_eq!((found[0].stamp.as_deref(), found[0].level), (Some("10:00:00"), Level::Info));
        let error = &found[1];
        assert_eq!(
            (error.stamp.as_deref(), error.level, error.message.as_str()),
            (Some("10:00:01"), Level::Error, "Unhandled exception")
        );
        assert_eq!((error.first, error.last, error.frames, error.details), (2, 5, 2, 1));
        assert!(error.folds());
        assert_eq!(found[2].level, Level::Debug);
    }

    #[test]
    fn plain_lines_take_their_colours_level_as_angular_and_vite_print_them() {
        let found = entries(&[
            ("ERROR in src/app/app.component.ts:3:5", Hue::Red),
            ("WARNING in budgets: initial exceeded maximum budget", Hue::Yellow),
            ("  ➜  Local:   http://localhost:5173/", Hue::Gray),
            ("npm verbose cli node", Hue::DarkGray),
            ("Compiled successfully.", Hue::Green),
        ]);
        let levels: Vec<Level> = found.iter().map(|entry| entry.level).collect();
        assert_eq!(levels, [Level::Error, Level::Warn, Level::Info, Level::Debug, Level::Info]);
        assert_eq!(found[2].message, "➜  Local:   http://localhost:5173/", "an indented plain line is its own");
    }

    #[test]
    fn a_node_stack_goes_on_the_error_above_it() {
        let found = entries(&[
            ("Error: connect ECONNREFUSED 127.0.0.1:5432", Hue::Red),
            ("    at TCPConnectWrap.afterConnect [as oncomplete] (node:net:1555:16)", Hue::Gray),
            ("    at Server.listen (/work/web/server.js:12:7)", Hue::Gray),
        ]);
        assert_eq!(found.len(), 1);
        assert_eq!((found[0].frames, found[0].level, found[0].last), (2, Level::Error, 3));
    }

    #[test]
    fn an_ef_core_command_is_its_time_and_its_sql_on_one_line() {
        let found = gray(&[
            "info: Microsoft.EntityFrameworkCore.Database.Command[20101]",
            "      Executed DbCommand (12ms) [Parameters=[@__id_0='5'], CommandType='Text', CommandTimeout='30']",
            "      SELECT [o].[Id], [o].[Total]",
            "      FROM [Orders] AS [o]",
        ]);
        let sql = found[0].sql.as_ref().expect("an EF Core command");
        assert_eq!((sql.ms, sql.command.as_str()), (12.0, "SELECT [o].[Id], [o].[Total] FROM [Orders] AS [o]"));
        assert_eq!(
            found[0].said(),
            format!("INF {EF_COMMAND} SQL 12 ms · SELECT [o].[Id], [o].[Total] FROM [Orders] AS [o]")
        );
        assert!(found[0].folds(), "its SQL lines unfold");
    }

    #[test]
    fn the_same_line_again_and_again_is_one_entry_with_its_count() {
        let found = gray(&[
            "webpack compiled successfully in 120 ms",
            "webpack compiled successfully in 98 ms",
            "webpack compiled successfully in 101 ms",
            "npm verbose cli node",
            "npm verbose title npm run start",
            "info: Orders.Worker[1]",
            "      Polled the queue: 0 messages",
            "info: Orders.Worker[1]",
            "      Polled the queue: 3 messages",
            "info: Orders.Worker[1]",
            "      Polled the queue: 1 messages",
            "something else",
        ]);
        let counted: Vec<(&str, usize, u64, u64)> =
            found.iter().map(|entry| (entry.message.as_str(), entry.repeats, entry.first, entry.last)).collect();
        assert_eq!(
            counted,
            [
                ("webpack compiled successfully in 101 ms", 3, 1, 3),
                ("npm verbose title npm run start", 2, 4, 5),
                ("Polled the queue: 1 messages", 3, 6, 11),
                ("something else", 1, 12, 12),
            ],
            "the newest of them is shown"
        );
        assert!(found[2].said().ends_with("×3"));
    }

    #[test]
    fn repeated_ef_commands_count_as_one_only_when_their_sql_is_the_same() {
        let command = |ms: u32, sql: &'static str| {
            vec![
                "info: Microsoft.EntityFrameworkCore.Database.Command[20101]".to_string(),
                format!("      Executed DbCommand ({ms}ms) [Parameters=[@__id_0='{ms}'], CommandType='Text']"),
                format!("      {sql}"),
            ]
        };
        let lines: Vec<String> = [
            command(3, "SELECT [l].[Id] FROM [Lines] AS [l]"),
            command(4, "SELECT [l].[Id] FROM [Lines] AS [l]"),
            command(9, "SELECT [o].[Id] FROM [Orders] AS [o]"),
        ]
        .concat();
        let found = gray(&lines.iter().map(String::as_str).collect::<Vec<_>>());
        let counts: Vec<(usize, f64)> =
            found.iter().map(|entry| (entry.repeats, entry.sql.as_ref().map_or(0.0, |sql| sql.ms))).collect();
        assert_eq!(counts, [(2, 4.0), (1, 9.0)], "the repeated query, then another one; settled as the next one came");
    }

    #[test]
    fn categories_are_trimmed_to_their_last_parts() {
        assert_eq!(short_category(EF_COMMAND), "EFCore.Command");
        assert_eq!(short_category("Microsoft.Hosting.Lifetime"), "Hosting.Lifetime");
        assert_eq!(short_category("Microsoft.AspNetCore.Hosting.Diagnostics[1]"), "AspNetCore.Diagnostics");
        assert_eq!(short_category("Orders.Api.Checkout"), "Orders.Api.Checkout");
        assert_eq!(short_category("Contoso.Shop.Orders.Api.Checkout"), "Orders.Api.Checkout");
    }

    #[test]
    fn entries_go_with_their_lines_and_a_line_finds_its_entry() {
        let mut output = Output::new(3);
        let now = Instant::now();
        for text in ["fail: Boom[1]", "      it broke", "      at A.B() in a.cs:line 1", "next", "after"] {
            output.push(text, now);
        }
        let firsts: Vec<(u64, u64)> = output.entries().iter().map(|entry| (entry.first, entry.last)).collect();
        assert_eq!(firsts, [(3, 3), (4, 4), (5, 5)], "the stack's frame is all that is left of the first");
        assert_eq!(output.entries().of(4).map(|entry| entry.message.as_str()), Some("next"));
        assert_eq!(output.entries().of(1), None);
        output.clear();
        assert!(output.entries().is_empty());
    }
}
