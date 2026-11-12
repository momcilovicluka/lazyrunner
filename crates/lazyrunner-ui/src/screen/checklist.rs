//! The startup checklist: a short animated splash that checks the tools, ports, the database server and the
//! frontend packages before lazyrunner starts anything ("startupChecklist": false turns it off; , then r runs it
//! any time). The checks run side by side, each in a thread of its own, so the screen stays live. At startup the
//! apps don't wait long for the update check: it goes on in the background (`StillChecking`). Once every check is
//! done, r runs the ones that failed or warned again (a failed update, say).
//!
//! It fixes what it finds too (the startup doctor): `j` / `k` move through the checks as they're drawn, by group, from
//! the first; `n` / `N` jump to the next and previous problem; `f` runs the picked check's fix (or the first
//! problem's) - npm ci for missing or stale packages, stopping what holds the apps' ports, the database picker, checking again,
//! or copying the commands that free ports Windows reserved - and `a` runs every safe one in turn (never one that
//! kills a process, needs admin rights or asks for a pick). `i` ignores a problem for this run; a click picks a check, and one on its `FIX` tag runs the fix. At startup a failed check holds the start until it's fixed or `Enter` starts
//! anyway; once nothing blocks it any more it counts down to the start (`Esc` holds it there). `c` copies it all as
//! a Markdown report for an issue.

use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::time::{Duration, Instant};

use lazyrunner_core::changelog::{self, Entry, Line};
use lazyrunner_core::output::Hue;
use lazyrunner_core::run::Controller;
use lazyrunner_core::run::checks::{self, Kind, Outcome, Status};
use lazyrunner_core::update;

/// Each check shows its spinner at least this long, so every tick can be seen.
const MIN_CHECK: Duration = Duration::from_millis(80);
/// Once the other checks are done, how long the startup waits for a background check (the update check, which
/// offline can take 20 s) before it goes on behind the scenes.
const BACKGROUND_GRACE: Duration = Duration::from_secs(2);
/// How long the checklist stays up when everything is fine.
const ALL_GOOD_HOLD: f64 = 0.5;
/// A hold this short closes too soon for a draining bar to mean anything.
const BAR_MIN_HOLD: Duration = Duration::from_secs(5);
/// How long a fixed check's edge takes to turn green, and a failed fix pulses.
pub const SETTLE: Duration = Duration::from_millis(300);
/// Once nothing blocks the start any more, it starts after this.
const COUNTDOWN: Duration = Duration::from_secs(3);
/// Checks that finish together tick up the header's counts this far apart.
const TICK: Duration = Duration::from_millis(40);
/// A finished check's mark is bright this long as it lands.
pub const LAND: Duration = Duration::from_millis(120);
/// What frees the ports Windows reserved: for an elevated terminal, which lazyrunner isn't.
pub const WINNAT: &str = "net stop winnat; net start winnat";

/// What fixes a check's problem.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fix {
    /// npm ci for the app, by its key: its packages are missing or stale.
    NpmCi(String),
    /// Stop what holds the ports of the apps that aren't running.
    FreePorts,
    /// Check again (the update check).
    Retry,
    /// Another database: the picker, once the checklist closes.
    Database,
    /// Copy the commands that free ports Windows reserved.
    Winnat,
    /// The audit view (A), once the checklist closes.
    Audit,
    /// Install dotnet-trace, for Alt+p's profiles (#83).
    DotnetTrace,
}

impl Fix {
    /// Whether `a` runs it: it kills nothing, needs no admin rights and asks for no pick.
    pub fn safe(&self) -> bool {
        matches!(self, Self::NpmCi(_) | Self::Retry)
    }

    /// What the fix does, for the report.
    pub fn advice(&self) -> String {
        match self {
            Self::NpmCi(_) => "`npm ci` in the app's folder".into(),
            Self::FreePorts => "stop what holds the port (`!` in lazyrunner)".into(),
            Self::Retry => "check again (`r`)".into(),
            Self::Database => "pick another database (`d`)".into(),
            Self::Winnat => format!("`{WINNAT}` in an elevated terminal"),
            Self::Audit => "see them and fix them in the audit view (`A`)".into(),
            Self::DotnetTrace => format!("`{}`", lazyrunner_core::run::profile::INSTALL),
        }
    }

    /// What `f` does, for the footer.
    pub fn label(&self) -> &'static str {
        match self {
            Self::NpmCi(_) => "f runs npm ci",
            Self::FreePorts => "f frees them",
            Self::Retry => "f checks again",
            Self::Database => "f picks a database",
            Self::Winnat => "f copies the fix",
            Self::Audit => "f opens the audit",
            Self::DotnetTrace => "f installs it",
        }
    }
}

/// A check's row beyond its result: what it looks at, how long it took, and a fix running for it.
#[derive(Clone, Debug)]
pub struct Row {
    pub kind: Kind,
    pub started: Option<Instant>,
    pub took: Option<Duration>,
    /// The fix running for it - npm ci goes on in the controller - or its check running again after one.
    pub fixing: Option<Fix>,
    /// When a fix last ended, and whether the check came back fine.
    pub settled: Option<(Instant, bool)>,
    /// When its check last finished.
    pub done_at: Option<Instant>,
    /// The language of the command app it's about: its icon, or its chip.
    pub language: Option<&'static str>,
}

impl Row {
    fn new(kind: Kind) -> Self {
        Self { kind, started: None, took: None, fixing: None, settled: None, done_at: None, language: None }
    }
}

#[derive(Clone, Debug)]
pub struct Item {
    pub name: String,
    pub status: Status,
    pub detail: String,
}

enum Phase {
    Checking,
    /// Done: it closes at `until`, or waits for a key without one.
    Holding {
        from: Instant,
        until: Option<Instant>,
    },
}

pub struct Checklist {
    pub items: Vec<Item>,
    pub footer: String,
    /// What's new after an update (the changelog's new entries).
    pub news: Vec<Line>,
    /// , then r: it waits for a key; at startup it moves on by itself.
    pub manual: bool,
    /// Any key: continue now.
    pub skip: bool,
    /// r was pressed: from then on it waits for a key while there is something to retry.
    retried: bool,
    /// Start everything once it is done (auto-start, `lazyrunner start`).
    pub start_after: bool,
    /// The lazyrunner version this checklist installed (it reopens on it).
    pub updated_to: Option<String>,
    installed_before: Option<String>,
    phase: Phase,
    results: Receiver<(usize, Message)>,
    /// Per check: may it finish after the checklist closes; is it done.
    background: Vec<bool>,
    done: Vec<bool>,
    /// When everything but the background checks was done.
    others_done_at: Option<Instant>,
    /// The background checks the startup stopped waiting for, and those of them that finished since.
    continuing: Vec<usize>,
    late: Vec<(String, Status, String)>,
    /// Each check's row: its kind, timing and fix.
    pub rows: Vec<Row>,
    /// The check `j` / `k` picked, which `f` fixes.
    pub selected: usize,
    /// The checks `a` will fix next, and how many it took on.
    queue: Vec<usize>,
    queued: usize,
    /// Checks running again after a fix, each lot with its own channel.
    again: Vec<(Results, Vec<usize>)>,
    /// A fix ran, or a failed check held the start: it no longer closes by itself, but counts down once nothing
    /// blocks the start.
    touched: bool,
    /// When that countdown ends; `Esc` holds it.
    countdown: Option<Instant>,
    held: bool,
    /// When `c` copied it: its rows flash.
    pub flashed: Option<Instant>,
    /// The problems `i` ignored for this run: they hold nothing, and count as skipped.
    ignored: Vec<bool>,
    /// The project it checks, for its header.
    pub project: String,
    /// When the checks started, and when the last of them was done.
    started: Option<Instant>,
    finished: Option<Instant>,
}

/// The doctor header's numbers: the checks done, and how many passed, warned, are blocked and were skipped, each
/// with when it last went up.
#[derive(Default)]
pub struct Tally {
    pub done: usize,
    pub counts: [usize; 4],
    pub bumped: [Option<Instant>; 4],
}

/// The channel a lot of checks report on.
type Results = Receiver<(usize, Message)>;

/// From the checks' thread: a check began, says what it is doing, or is done.
enum Message {
    Running,
    Detail(String),
    Done(Outcome),
}

impl Checklist {
    pub fn start(controller: &Controller, manual: bool, whats_new: &[Entry], start_after: bool) -> Self {
        let checks = checks::checks(controller);
        let items = checks
            .iter()
            .map(|check| Item { name: check.name.clone(), status: Status::Pending, detail: String::new() })
            .collect();
        let rows: Vec<Row> =
            checks.iter().map(|check| Row { language: check.language, ..Row::new(check.kind.clone()) }).collect();
        let first = first_drawn(&rows);
        let background: Vec<bool> = checks.iter().map(|check| check.background).collect();
        let done = vec![false; checks.len()];
        let (sender, results) = channel();
        for (index, check) in checks.into_iter().enumerate() {
            let sender = sender.clone();
            std::thread::spawn(move || run_check(index, check, &sender));
        }
        // Dropped here: the channel closes once every check's thread is done.
        drop(sender);
        Self {
            items,
            footer: "checking…".into(),
            news: changelog::format(whats_new),
            manual,
            skip: false,
            retried: false,
            start_after,
            updated_to: None,
            installed_before: update::installed(),
            phase: Phase::Checking,
            results,
            background,
            done,
            others_done_at: None,
            continuing: Vec::new(),
            late: Vec::new(),
            ignored: vec![false; rows.len()],
            rows,
            selected: first,
            queue: Vec::new(),
            queued: 0,
            again: Vec::new(),
            touched: false,
            countdown: None,
            held: false,
            flashed: None,
            project: controller.project.name.clone(),
            started: Some(Instant::now()),
            finished: None,
        }
    }

    /// Whether every check is done and it holds: what `j` `k` `f` `a` act on.
    pub fn is_holding(&self) -> bool {
        matches!(self.phase, Phase::Holding { .. })
    }

    /// The checks in the order they're drawn: by group, and in a group lazyrunner, then the tools, the profile, the
    /// ports and the branch.
    pub fn order(&self) -> Vec<usize> {
        drawn(&self.rows)
    }

    /// `j` / `k`: the next or previous check as they're drawn, round the ends; true when it went round one.
    pub fn select(&mut self, step: isize) -> bool {
        let order = self.order();
        let at = order.iter().position(|&index| index == self.selected).unwrap_or(0);
        let next = super::keys::wrapped(at, step, order.len());
        self.selected = order.get(next).copied().unwrap_or(0);
        (step > 0 && next < at) || (step < 0 && next > at)
    }

    /// Whether check `index` warned or failed, and `i` didn't ignore it.
    pub fn is_problem(&self, index: usize) -> bool {
        let shows = self.items.get(index).is_some_and(|item| matches!(item.status, Status::Warn | Status::Fail));
        shows && !self.is_ignored(index)
    }

    pub fn is_ignored(&self, index: usize) -> bool {
        self.ignored.get(index).copied().unwrap_or(false)
    }

    /// `n` / `N`: the next or previous problem as they're drawn, round the ends; false when there's none.
    pub fn next_problem(&mut self, forward: bool) -> bool {
        let order = self.order();
        let at = order.iter().position(|&index| index == self.selected).unwrap_or(0);
        let count = order.len();
        let found = (1..=count)
            .map(|step| if forward { (at + step) % count } else { (at + count - step % count) % count })
            .map(|at| order[at])
            .find(|&index| self.is_problem(index));
        if let Some(index) = found {
            self.selected = index;
        }
        found.is_some()
    }

    /// `i`: the picked check's problem ignored for this run, or no longer.
    pub fn ignore(&mut self) {
        let index = self.selected;
        let shows = self.items.get(index).is_some_and(|item| matches!(item.status, Status::Warn | Status::Fail));
        if let Some(ignored) = self.ignored.get_mut(index).filter(|_| shows) {
            *ignored = !*ignored;
        }
    }

    /// The check `f` fixes: the picked one, or the first problem with a fix when it has none.
    pub fn fix_target(&self) -> Option<usize> {
        let picked = Some(self.selected).filter(|&index| self.fix_of(index).is_some());
        picked.or_else(|| self.order().into_iter().find(|&index| self.fix_of(index).is_some()))
    }

    /// How many passed, warned, are blocked, and were skipped (or ignored).
    pub fn counts(&self) -> (usize, usize, usize, usize) {
        let [passed, warned, blocked, skipped] = self.tally(None).counts;
        (passed, warned, blocked, skipped)
    }

    /// The header's numbers at `now`: checks that finished together count a `TICK` apart, so they tick up one at a
    /// time. With no `now` (reduced motion) every finished check counts at once.
    pub fn tally(&self, now: Option<Instant>) -> Tally {
        let mut tally = Tally::default();
        for (index, counted) in self.counted().into_iter().enumerate() {
            let Some(at) = self.category(index) else { continue };
            if counted.zip(now).is_some_and(|(counted, now)| counted > now) {
                continue;
            }
            tally.done += 1;
            tally.counts[at] += 1;
            tally.bumped[at] = tally.bumped[at].max(counted);
        }
        tally
    }

    /// When each finished check counts: as it finished, but a `TICK` after the one before at the soonest; none: at
    /// once (it ended without finishing - skipped, say).
    fn counted(&self) -> Vec<Option<Instant>> {
        let mut finished: Vec<(Instant, usize)> = (0..self.rows.len())
            .filter(|&index| self.category(index).is_some())
            .filter_map(|index| Some((self.rows[index].done_at?, index)))
            .collect();
        finished.sort();
        let mut counted = vec![None; self.rows.len()];
        let mut last: Option<Instant> = None;
        for (at, index) in finished {
            let at = last.map_or(at, |last| at.max(last + TICK));
            (counted[index], last) = (Some(at), Some(at));
        }
        counted
    }

    /// Where check `index` counts: passed, warned, blocked, or skipped (ignored ones too); none while it's to do.
    fn category(&self, index: usize) -> Option<usize> {
        match self.items.get(index)?.status {
            Status::Pending | Status::Running => None,
            _ if self.is_ignored(index) => Some(3),
            Status::Ok => Some(0),
            Status::Warn => Some(1),
            Status::Fail => Some(2),
            Status::Skip => Some(3),
        }
    }

    /// The countdown to the start while it runs: how much of it is left (1 to 0), and the seconds.
    pub fn counting_down(&self, now: Instant) -> Option<(f64, u64)> {
        let left = self.countdown?.saturating_duration_since(now);
        Some((left.as_secs_f64() / COUNTDOWN.as_secs_f64(), left.as_secs_f64().ceil() as u64))
    }

    /// How long the checks took (so far, while they run); none for a checklist made up in a test.
    pub fn elapsed(&self, now: Instant) -> Option<Duration> {
        Some(self.finished.unwrap_or(now).saturating_duration_since(self.started?))
    }

    /// What fixes check `index`'s problem, when lazyrunner can.
    pub fn fix_of(&self, index: usize) -> Option<Fix> {
        if self.is_ignored(index) {
            return None;
        }
        let (item, row) = (self.items.get(index)?, self.rows.get(index)?);
        match (&row.kind, item.status) {
            (Kind::Packages(key), Status::Warn | Status::Fail) => Some(Fix::NpmCi(key.clone())),
            (Kind::Ports, Status::Fail) if item.detail.contains("reserved by Windows") => Some(Fix::Winnat),
            (Kind::Ports, Status::Warn) => Some(Fix::FreePorts),
            (Kind::Database, Status::Warn) if item.detail.contains("nothing answers") => Some(Fix::Database),
            (Kind::Update, Status::Warn | Status::Fail) => Some(Fix::Retry),
            (Kind::Audit, Status::Warn) => Some(Fix::Audit),
            (Kind::Tool, Status::Skip) if item.detail.contains(lazyrunner_core::run::profile::INSTALL) => {
                Some(Fix::DotnetTrace)
            }
            _ => None,
        }
    }

    /// `f` (or `a`): runs `fix` for check `index` - npm ci in the background, the others at once - and checks it
    /// again once it's done. The database picker and the copied commands are the caller's.
    pub fn start_fix(&mut self, controller: &mut Controller, index: usize, fix: Fix) {
        self.touched = true;
        self.countdown = None;
        match &fix {
            Fix::NpmCi(key) => {
                controller.npm_ci(Some(key));
                if !controller.npm_ci.as_ref().is_some_and(|job| job.key == *key) {
                    return; // it couldn't start: the log says why
                }
                self.items[index].detail = "npm ci…".into();
                self.rows[index].fixing = Some(fix);
            }
            Fix::FreePorts | Fix::Retry => {
                if fix == Fix::FreePorts {
                    controller.free_ports();
                }
                self.rows[index].fixing = Some(fix);
                self.check_again(controller, &[index]);
            }
            Fix::DotnetTrace => {
                controller.install_trace();
                self.items[index].detail = "installing…".into();
                self.rows[index].fixing = Some(fix);
            }
            Fix::Database | Fix::Winnat | Fix::Audit => {}
        }
    }

    /// `a`: every safe fix, one after another.
    pub fn fix_safe(&mut self, controller: &mut Controller) {
        self.queue = (0..self.items.len())
            .filter(|&index| self.rows[index].fixing.is_none() && self.fix_of(index).is_some_and(|fix| fix.safe()))
            .collect();
        self.queued = self.queue.len();
        self.next_fix(controller);
    }

    /// The next of `a`'s fixes, once none runs.
    fn next_fix(&mut self, controller: &mut Controller) {
        if self.rows.iter().any(|row| row.fixing.is_some()) {
            return;
        }
        while !self.queue.is_empty() {
            let index = self.queue.remove(0);
            if let Some(fix) = self.fix_of(index) {
                self.start_fix(controller, index, fix);
                return;
            }
        }
    }

    /// The winnat fix: says on the check's line whether the commands were copied.
    pub fn copied(&mut self, index: usize, copied: Result<(), String>) {
        let Some(item) = self.items.get_mut(index) else { return };
        item.detail = match copied {
            Ok(()) => format!("copied `{WINNAT}` - paste it into an elevated terminal, then r"),
            Err(why) => format!("couldn't copy ({why}) - run `{WINNAT}` in an elevated terminal"),
        };
    }

    /// `Esc` while it counts down to the start: holds it there. False when it isn't counting down.
    pub fn hold(&mut self) -> bool {
        if self.countdown.take().is_none() {
            return false;
        }
        self.held = true;
        true
    }

    /// Runs checks `indexes` again, after a fix.
    fn check_again(&mut self, controller: &Controller, indexes: &[usize]) {
        let (sender, results) = channel();
        for check in checks::checks(controller) {
            let same = |index: usize| self.items[index].name == check.name && self.rows[index].kind == check.kind;
            let Some(index) = indexes.iter().copied().find(|&index| same(index)) else {
                continue;
            };
            let sender = sender.clone();
            std::thread::spawn(move || run_check(index, check, &sender));
        }
        drop(sender);
        for &index in indexes {
            let item = &mut self.items[index];
            (item.status, item.detail) = (Status::Pending, String::new());
            if let Some(done) = self.done.get_mut(index) {
                *done = false;
            }
        }
        self.again.push((results, indexes.to_vec()));
    }

    /// A message from a check's thread.
    fn receive(&mut self, index: usize, message: Message, now: Instant) {
        let Some(item) = self.items.get_mut(index) else { return };
        match message {
            Message::Running => {
                item.status = Status::Running;
                if let Some(row) = self.rows.get_mut(index) {
                    row.started = Some(now);
                }
            }
            Message::Detail(detail) => item.detail = detail,
            Message::Done((status, detail)) => {
                if self.continuing.contains(&index) {
                    // The startup stopped waiting for it: StillChecking reports it.
                    self.late.push((item.name.clone(), status, detail.clone()));
                }
                item.status = status;
                item.detail = detail;
                if let Some(done) = self.done.get_mut(index) {
                    *done = true;
                }
                if let Some(row) = self.rows.get_mut(index) {
                    row.took = row.started.map(|started| now.saturating_duration_since(started));
                    row.done_at = Some(now);
                    if row.fixing.take().is_some() {
                        row.settled = Some((now, matches!(status, Status::Ok | Status::Skip)));
                    }
                }
            }
        }
    }

    /// The checks running again after a fix: their results, and what didn't finish fails.
    fn receive_again(&mut self, now: Instant) {
        let mut again = std::mem::take(&mut self.again);
        again.retain(|(results, indexes)| {
            loop {
                match results.try_recv() {
                    Ok((index, message)) => self.receive(index, message, now),
                    Err(TryRecvError::Empty) => return true,
                    Err(TryRecvError::Disconnected) => {
                        for &index in indexes {
                            if matches!(self.items[index].status, Status::Pending | Status::Running) {
                                self.receive(
                                    index,
                                    Message::Done((Status::Fail, "the check didn't finish".into())),
                                    now,
                                );
                            }
                        }
                        return false;
                    }
                }
            }
        });
        self.again = again;
    }

    /// npm ci running for a check: its last line on the check's; once it's done, the check runs again.
    /// dotnet-trace's install done: its line is checked again.
    fn follow_install(&mut self, controller: &Controller) {
        let installed: Vec<usize> = (0..self.rows.len())
            .filter(|&index| self.rows[index].fixing == Some(Fix::DotnetTrace))
            .filter(|&index| !matches!(self.items[index].status, Status::Pending | Status::Running))
            .filter(|_| !controller.installing_trace)
            .collect();
        if !installed.is_empty() {
            self.check_again(controller, &installed);
        }
    }

    fn follow_npm_ci(&mut self, controller: &Controller) {
        let mut done = Vec::new();
        for index in 0..self.rows.len() {
            let Some(Fix::NpmCi(key)) = &self.rows[index].fixing else { continue };
            if matches!(self.items[index].status, Status::Pending | Status::Running) {
                continue; // checking again already
            }
            match controller.npm_ci.as_ref().filter(|job| job.key == *key) {
                Some(job) => {
                    if let Some(line) = job.lines.last().map(|line| line.trim()).filter(|line| !line.is_empty()) {
                        self.items[index].detail = format!("npm ci: {line}");
                    }
                }
                None => done.push(index),
            }
        }
        if !done.is_empty() {
            self.check_again(controller, &done);
        }
    }

    /// Whether a fix runs, or `a` has more to run.
    fn fixing(&self) -> bool {
        !self.queue.is_empty() || self.rows.iter().any(|row| row.fixing.is_some())
    }

    /// Whether checks are running: at first, or again after r.
    pub fn is_checking(&self) -> bool {
        matches!(self.phase, Phase::Checking)
    }

    /// r: every check is done, none goes on in the background, some failed or warned, and nothing is about to reopen
    /// lazyrunner on a new version.
    pub fn can_retry(&self) -> bool {
        matches!(self.phase, Phase::Holding { .. })
            && self.continuing.is_empty()
            && (self.manual || self.updated_to.is_none())
            && self.items.iter().any(|item| matches!(item.status, Status::Warn | Status::Fail))
    }

    /// r: runs the checks that failed or warned again, in place; the others keep their results. False when there is
    /// nothing to retry.
    pub fn retry(&mut self, controller: &mut Controller) -> bool {
        if !self.can_retry() {
            return false;
        }
        let failed: Vec<usize> = (0..self.items.len())
            .filter(|&index| matches!(self.items[index].status, Status::Warn | Status::Fail))
            .collect();
        let (sender, results) = channel();
        for check in checks::checks(controller) {
            let same = |index: usize| self.items[index].name == check.name && self.rows[index].kind == check.kind;
            let Some(index) = failed.iter().copied().find(|&index| same(index)) else {
                continue;
            };
            let sender = sender.clone();
            std::thread::spawn(move || run_check(index, check, &sender));
        }
        drop(sender);
        let names: Vec<&str> = failed.iter().map(|&index| self.items[index].name.as_str()).collect();
        controller.add_log(format!("Checking again: {}", names.join(", ")), Hue::Cyan);
        for &index in &failed {
            let item = &mut self.items[index];
            (item.status, item.detail) = (Status::Pending, String::new());
            if let Some(done) = self.done.get_mut(index) {
                *done = false;
            }
        }
        self.results = results;
        self.phase = Phase::Checking;
        self.retried = true;
        self.others_done_at = None;
        self.footer = "checking again…".into();
        true
    }

    /// What r would retry, for the footer: the one check's name, or how many.
    fn retry_hint(&self) -> String {
        let failed: Vec<&Item> =
            self.items.iter().filter(|item| matches!(item.status, Status::Warn | Status::Fail)).collect();
        match failed.as_slice() {
            [one] => format!("r retries {}", one.name),
            many => format!("r retries those {}", many.len()),
        }
    }

    /// How much of a timed hold is left (1 to 0), for the bar that drains along the box; none while it waits for a
    /// key, or for a hold too short to show one.
    pub fn draining(&self, now: Instant) -> Option<f64> {
        let Phase::Holding { from, until: Some(until), .. } = self.phase else { return None };
        let total = until.saturating_duration_since(from);
        if total < BAR_MIN_HOLD || self.skip {
            return None;
        }
        Some(until.saturating_duration_since(now).as_secs_f64() / total.as_secs_f64())
    }

    /// The background checks the startup didn't wait for: still running, or finished while it closed.
    pub fn still_checking(self) -> Option<StillChecking> {
        if self.continuing.is_empty() {
            return None;
        }
        let waiting: Vec<usize> =
            self.continuing.iter().copied().filter(|&index| !self.done.get(index).copied().unwrap_or(true)).collect();
        let names = waiting.iter().map(|&index| self.items[index].name.clone()).collect();
        Some(StillChecking {
            results: self.results,
            waiting,
            names,
            late: self.late,
            installed_before: self.installed_before,
        })
    }

    /// At startup, once only background checks are left, they get `BACKGROUND_GRACE` more; then the checklist
    /// moves on without them.
    fn let_background_continue(&mut self, now: Instant) {
        let unfinished: Vec<usize> = (0..self.items.len())
            .filter(|&index| matches!(self.items[index].status, Status::Pending | Status::Running))
            .collect();
        let only_background = unfinished.iter().all(|&index| self.background.get(index).copied().unwrap_or(false));
        if unfinished.is_empty() || !only_background {
            return;
        }
        let since = *self.others_done_at.get_or_insert(now);
        if now.saturating_duration_since(since) < BACKGROUND_GRACE {
            return;
        }
        for &index in &unfinished {
            let item = &mut self.items[index];
            item.status = Status::Skip;
            item.detail = "still checking - it goes on in the background".into();
        }
        self.continuing = unfinished;
    }

    /// What didn't finish ends as `status` - all but the checks running again after a fix, which have their own
    /// channel.
    fn finish_unfinished(&mut self, status: Status, detail: &str) {
        let again: Vec<usize> = self.again.iter().flat_map(|(_, indexes)| indexes.iter().copied()).collect();
        let unfinished =
            self.items.iter_mut().enumerate().filter(|(index, item)| {
                !again.contains(index) && matches!(item.status, Status::Pending | Status::Running)
            });
        for (_, item) in unfinished {
            item.status = status;
            item.detail = detail.into();
        }
    }

    /// Takes the results that came in and moves on; true once it is done (the overlay closes).
    pub fn update(&mut self, now: Instant, controller: &mut Controller) -> bool {
        loop {
            match self.results.try_recv() {
                Ok((index, message)) => self.receive(index, message, now),
                Err(TryRecvError::Empty) => break,
                // Done, or the checks' thread died: what didn't finish never will.
                Err(TryRecvError::Disconnected) => {
                    self.finish_unfinished(Status::Fail, "the check didn't finish");
                    break;
                }
            }
        }
        self.receive_again(now);
        self.follow_npm_ci(controller);
        self.follow_install(controller);
        self.next_fix(controller);
        if self.skip {
            self.finish_unfinished(Status::Skip, "skipped");
        }
        if !self.manual && matches!(self.phase, Phase::Checking) {
            self.let_background_continue(now);
        }
        let checking = self.items.iter().any(|item| matches!(item.status, Status::Pending | Status::Running));
        if matches!(self.phase, Phase::Checking) {
            if checking {
                return false;
            }
            let problems: Vec<&Item> =
                self.items.iter().filter(|item| matches!(item.status, Status::Warn | Status::Fail)).collect();
            for problem in &problems {
                let (mark, hue) = if problem.status == Status::Fail { ("✗", Hue::Red) } else { ("!", Hue::Yellow) };
                controller.add_log(format!("{mark} {}: {}", problem.name, problem.detail), hue);
            }
            if problems.is_empty() {
                controller.add_log("✓ Startup checklist: all good", Hue::Green);
            }
            self.finished.get_or_insert(now);
            self.updated_to = update::installed().filter(|version| self.installed_before.as_ref() != Some(version));
            // Linger a moment: longer when something needs a look or there is news; any key continues.
            let hold = if self.updated_to.is_some() && !self.manual {
                1.5
            } else if self.manual {
                60.0
            } else if !self.news.is_empty() {
                20.0
            } else if !problems.is_empty() {
                8.0
            } else {
                ALL_GOOD_HOLD
            };
            let until = Some(now + Duration::from_secs_f64(hold));
            self.phase = Phase::Holding { from: now, until };
            // Opened by hand, or after r: what can be retried stays up until a key, never closing under the next r.
            if (self.manual || self.retried) && self.can_retry() {
                self.phase = Phase::Holding { from: now, until: None };
            }
        }
        self.steer_hold(now);
        let Phase::Holding { until, .. } = self.phase else { return false };
        if self.skip || until.is_some_and(|until| now >= until) {
            return true;
        }
        let problems = (0..self.items.len()).filter(|&index| self.is_problem(index)).count();
        let blocking = (0..self.items.len())
            .filter(|&index| self.is_problem(index) && self.items[index].status == Status::Fail)
            .count();
        let left = until.map_or(String::new(), |until| {
            format!(" ({} s)", until.saturating_duration_since(now).as_secs_f64().ceil() as u64)
        });
        let retry = if self.can_retry() { format!(" · {}", self.retry_hint()) } else { String::new() };
        let seconds = until.map_or(0, |until| until.saturating_duration_since(now).as_secs_f64().ceil() as u64);
        self.footer = if let Some(version) = self.updated_to.as_ref().filter(|_| !self.manual) {
            format!("v{version} installed - reopening lazyrunner…")
        } else if self.fixing() {
            self.fixing_footer()
        } else if self.countdown.is_some() {
            format!("nothing blocks the start - starting in {seconds} s · Esc holds it")
        } else if self.held {
            "held - Enter starts".into()
        } else if self.starts() && blocking > 0 {
            format!("{blocking} blocking the start · Enter starts anyway{retry}")
        } else if self.manual {
            format!("any key closes{left}{retry}")
        } else if !self.news.is_empty() {
            format!("what's new above (, then c shows the whole changelog) - any key continues{left}{retry}")
        } else if problems > 0 {
            format!("{problems} to look at - any key continues{left}{retry}")
        } else {
            "all good - starting…".into()
        };
        false
    }

    /// Whether the apps start once it closes (and not a new version of lazyrunner).
    fn starts(&self) -> bool {
        self.start_after && !self.manual && self.updated_to.is_none()
    }

    /// While it holds: it never closes during a fix; a failed check holds the start; once a fix ran (or a failed
    /// check held the start) and nothing blocks it, it counts down to the start - or waits for a key without one.
    fn steer_hold(&mut self, now: Instant) {
        let blocking =
            (0..self.items.len()).any(|index| self.is_problem(index) && self.items[index].status == Status::Fail);
        let fixing = self.fixing();
        let starts = self.starts();
        let Phase::Holding { from, until, .. } = &mut self.phase else { return };
        if fixing {
            *until = None;
        } else if starts && blocking {
            (*until, self.touched, self.countdown) = (None, true, None);
        } else if starts && self.touched && !self.held {
            let end = *self.countdown.get_or_insert(now + COUNTDOWN);
            if *until != Some(end) {
                (*from, *until) = (now, Some(end));
            }
        } else if self.touched {
            *until = None;
        }
    }

    /// How many problems `a` would fix.
    pub fn safe_fixes(&self) -> usize {
        (0..self.items.len()).filter(|&index| self.fix_of(index).is_some_and(|fix| fix.safe())).count()
    }

    /// What check `index`'s problem holds up, for its details.
    pub fn blocks(&self, index: usize) -> &'static str {
        let (Some(item), Some(row)) = (self.items.get(index), self.rows.get(index)) else { return "" };
        match &row.kind {
            Kind::Version | Kind::Branch => "nothing",
            Kind::Update => "nothing - this version goes on running",
            Kind::Project => "nothing until the profile is reloaded (, then p)",
            Kind::Ports => "the apps whose ports are taken: they can't listen on them",
            Kind::Database => "the APIs' database calls, and the migrations (M)",
            Kind::Packages(_) => "the app's start, and its tests",
            Kind::Audit => "nothing - the apps run, with packages that have known vulnerabilities",
            Kind::Tool if item.name == "git" => {
                "the branch, the source checks that skip builds, the test history's code"
            }
            Kind::Tool if item.name == ".NET SDK" => "building and starting the .NET apps, and their tests",
            Kind::Tool => "the npm apps, and their tests",
        }
    }

    /// While fixes run: which of `a`'s, as a bar.
    fn fixing_footer(&self) -> String {
        if self.queued < 2 {
            return "fixing…".into();
        }
        let at = (self.queued - self.queue.len()).min(self.queued);
        format!("fixing {at} of {} {}{}", self.queued, "▰".repeat(at), "▱".repeat(self.queued - at))
    }
}

/// The checks' order on screen: by group, then by kind within one, else as they came.
fn drawn(rows: &[Row]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..rows.len()).collect();
    order.sort_by_key(|&index| (rows[index].kind.group(), rows[index].kind.rank()));
    order
}

/// The check drawn first, which the selection starts on.
fn first_drawn(rows: &[Row]) -> usize {
    drawn(rows).first().copied().unwrap_or(0)
}

/// Runs one check in its thread, saying when it starts, what it is doing and how it ended.
fn run_check(index: usize, check: checks::Check, sender: &Sender<(usize, Message)>) {
    let started = Instant::now();
    if sender.send((index, Message::Running)).is_err() {
        return;
    }
    let detail = sender.clone();
    let mut report = |text: &str| {
        let _ = detail.send((index, Message::Detail(text.to_string())));
    };
    // A check that panics fails (the panic itself goes to the log) instead of holding the checklist up.
    let run = std::panic::AssertUnwindSafe(|| (check.run)(&mut report));
    let outcome = std::panic::catch_unwind(run).unwrap_or_else(|_| (Status::Fail, "the check crashed".into()));
    if let Some(rest) = MIN_CHECK.checked_sub(started.elapsed()) {
        std::thread::sleep(rest);
    }
    let _ = sender.send((index, Message::Done(outcome))); // the checklist may have closed
}

/// A background check the startup didn't wait for (the update check): its outcome, when it comes, goes to the
/// log, and a version it installed is switched to.
pub struct StillChecking {
    results: Receiver<(usize, Message)>,
    waiting: Vec<usize>,
    names: Vec<String>,
    /// Outcomes that came in while the checklist was closing: (name, status, detail).
    late: Vec<(String, Status, String)>,
    installed_before: Option<String>,
}

impl StillChecking {
    /// Once every check is done: `Some` with the lazyrunner version that got installed, if one did.
    pub fn poll(&mut self, controller: &mut Controller) -> Option<Option<String>> {
        for (name, status, detail) in std::mem::take(&mut self.late) {
            log_outcome(controller, &name, status, &detail);
        }
        loop {
            match self.results.try_recv() {
                Ok((index, Message::Done((status, detail)))) => {
                    let Some(at) = self.waiting.iter().position(|&waiting| waiting == index) else { continue };
                    self.waiting.remove(at);
                    let name = self.names.remove(at);
                    log_outcome(controller, &name, status, &detail);
                }
                Ok(_) => {}
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.waiting.clear();
                    break;
                }
            }
        }
        if !self.waiting.is_empty() {
            return None;
        }
        Some(update::installed().filter(|version| self.installed_before.as_ref() != Some(version)))
    }
}

/// A background check's problem, in the log.
fn log_outcome(controller: &mut Controller, name: &str, status: Status, detail: &str) {
    match status {
        Status::Fail => controller.add_log(format!("✗ {name}: {detail}"), Hue::Red),
        Status::Warn => controller.add_log(format!("! {name}: {detail}"), Hue::Yellow),
        _ => {}
    }
}

#[cfg(test)]
impl Checklist {
    /// A checklist with these results, without running any check (the snapshots).
    pub fn fixed(items: Vec<Item>, news: Vec<Line>, footer: &str) -> Self {
        let (_, results) = channel();
        let items_len = items.len();
        let rows: Vec<Row> = items.iter().map(|item| Row::new(kind_of(&item.name))).collect();
        let first = first_drawn(&rows);
        Self {
            items,
            footer: footer.into(),
            news,
            manual: false,
            skip: false,
            retried: false,
            start_after: false,
            updated_to: None,
            installed_before: None,
            phase: Phase::Checking,
            results,
            background: vec![false; items_len],
            done: vec![false; items_len],
            others_done_at: None,
            continuing: Vec::new(),
            late: Vec::new(),
            ignored: vec![false; rows.len()],
            rows,
            selected: first,
            queue: Vec::new(),
            queued: 0,
            again: Vec::new(),
            touched: false,
            countdown: None,
            held: false,
            flashed: None,
            project: String::new(),
            started: None,
            finished: None,
        }
    }
}

/// A check's kind by its name, for checklists made up in tests.
#[cfg(test)]
fn kind_of(name: &str) -> Kind {
    match name {
        "lazyrunner" => Kind::Update,
        "Ports" => Kind::Ports,
        "Database" => Kind::Database,
        "Profile" => Kind::Project,
        "Branch" => Kind::Branch,
        _ if name.ends_with(" packages") => Kind::Packages(name.trim_end_matches(" packages").to_lowercase()),
        _ => Kind::Tool,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(status: Status) -> Item {
        item_named("git", status)
    }

    fn item_named(name: &str, status: Status) -> Item {
        Item { name: name.into(), status, detail: String::new() }
    }

    #[test]
    fn checks_whose_thread_is_gone_fail_instead_of_holding_the_start() {
        let (_temp, mut controller) = crate::screen::tests::controller("checklist-gone");
        let mut list = Checklist::fixed(vec![item(Status::Ok), item(Status::Running)], Vec::new(), "checking…");
        let now = Instant::now();
        assert!(!list.update(now, &mut controller), "it holds a moment to show the problem");
        assert_eq!(list.items[1].status, Status::Fail);
        assert_eq!(list.items[1].detail, "the check didn't finish");
        assert!(list.update(now + Duration::from_secs(9), &mut controller), "then it moves on");
    }

    #[test]
    fn at_startup_the_update_check_goes_on_in_the_background() {
        let (_temp, mut controller) = crate::screen::tests::controller("checklist-background");
        let (sender, results) = channel();
        let mut list = Checklist::fixed(
            vec![Item { name: "lazyrunner".into(), status: Status::Running, detail: String::new() }, item(Status::Ok)],
            Vec::new(),
            "checking…",
        );
        list.results = results;
        list.background = vec![true, false];
        let now = Instant::now();
        assert!(!list.update(now, &mut controller), "a moment more for the update check");
        assert!(!list.update(now + BACKGROUND_GRACE, &mut controller), "then it holds, all good");
        assert_eq!(list.items[0].status, Status::Skip);
        assert!(list.items[0].detail.contains("in the background"));
        assert!(list.update(now + BACKGROUND_GRACE + Duration::from_secs(1), &mut controller), "and moves on");

        let mut still = list.still_checking().expect("the update check is still going");
        assert_eq!(still.poll(&mut controller), None, "not done yet");
        sender.send((0, Message::Done((Status::Warn, "GitHub didn't answer".into())))).unwrap();
        assert_eq!(still.poll(&mut controller), Some(None), "done, nothing installed");
        assert_eq!(controller.log.back().unwrap().text, "! lazyrunner: GitHub didn't answer");
    }

    #[test]
    fn a_background_check_that_finishes_while_the_checklist_closes_is_still_reported() {
        let (_temp, mut controller) = crate::screen::tests::controller("checklist-late");
        let (sender, results) = channel();
        let mut list = Checklist::fixed(
            vec![Item { name: "lazyrunner".into(), status: Status::Running, detail: String::new() }, item(Status::Ok)],
            Vec::new(),
            "checking…",
        );
        list.results = results;
        list.background = vec![true, false];
        let now = Instant::now();
        list.update(now, &mut controller);
        assert!(!list.update(now + BACKGROUND_GRACE, &mut controller), "holding");
        sender.send((0, Message::Done((Status::Warn, "offline".into())))).unwrap();
        assert!(!list.update(now + BACKGROUND_GRACE, &mut controller), "it came in during the hold");
        let mut still = list.still_checking().expect("its outcome is still to be reported");
        assert_eq!(still.poll(&mut controller), Some(None));
        assert_eq!(controller.log.back().unwrap().text, "! lazyrunner: offline");
    }

    #[test]
    fn a_checklist_opened_by_hand_waits_for_every_check() {
        let (_temp, mut controller) = crate::screen::tests::controller("checklist-manual");
        let (_sender, results) = channel();
        let mut list = Checklist::fixed(vec![item(Status::Running), item(Status::Ok)], Vec::new(), "checking…");
        list.results = results;
        list.background = vec![true, false];
        list.manual = true;
        assert!(!list.update(Instant::now() + Duration::from_secs(30), &mut controller));
        assert_eq!(list.items[0].status, Status::Running);
        assert!(list.still_checking().is_none());
    }

    #[test]
    fn r_runs_the_checks_that_failed_again_and_keeps_the_others() {
        let (_temp, mut controller) = crate::screen::tests::controller("checklist-retry");
        let git = Item { name: "git".into(), status: Status::Fail, detail: "not found on PATH".into() };
        let mut list = Checklist::fixed(vec![item_named("lazyrunner", Status::Ok), git], Vec::new(), "checking…");
        list.manual = true;
        let now = Instant::now();
        assert!(!list.update(now, &mut controller), "holding, with the problem");
        assert_eq!(list.footer, "any key closes · r retries git");
        assert!(list.retry(&mut controller));
        assert_eq!((list.items[0].status, list.items[1].status), (Status::Ok, Status::Pending));
        assert_eq!(controller.log.back().unwrap().text, "Checking again: git");
        let deadline = Instant::now() + Duration::from_secs(20);
        while list.items[1].status != Status::Ok && list.items[1].status != Status::Fail && Instant::now() < deadline {
            list.update(Instant::now(), &mut controller);
            std::thread::sleep(Duration::from_millis(20));
        }
        list.update(Instant::now(), &mut controller);
        assert_eq!(list.items[1].status, Status::Ok, "git is there: {}", list.items[1].detail);
        assert_eq!(list.footer, "any key closes (60 s)", "nothing left to retry: it closes by itself again");
        assert!(!list.retry(&mut controller));
    }

    #[test]
    fn opened_by_hand_it_waits_for_a_key_while_something_can_be_retried() {
        let (_temp, mut controller) = crate::screen::tests::controller("checklist-waits");
        let update = Item { name: "lazyrunner".into(), status: Status::Warn, detail: "GitHub didn't answer".into() };
        let mut list = Checklist::fixed(vec![update, item(Status::Ok)], Vec::new(), "checking…");
        list.manual = true;
        let now = Instant::now();
        assert!(!list.update(now, &mut controller));
        assert!(!list.update(now + Duration::from_secs(600), &mut controller), "it never closes by itself");
        assert_eq!(list.footer, "any key closes · r retries lazyrunner");
        assert_eq!(list.draining(now), None, "so nothing drains");
    }

    #[test]
    fn at_startup_it_moves_on_by_itself_until_r_is_pressed() {
        let (_temp, mut controller) = crate::screen::tests::controller("checklist-retried");
        let mut list = Checklist::fixed(vec![item_named("Ports", Status::Warn)], Vec::new(), "checking…");
        let now = Instant::now();
        assert!(!list.update(now, &mut controller));
        let fix = "";
        assert_eq!(list.footer, format!("1 to look at - any key continues (8 s) · r retries Ports{fix}"));
        assert_eq!(list.draining(now), Some(1.0));
        assert_eq!(list.draining(now + Duration::from_secs(2)), Some(0.75), "the bar drains with the time left");
        assert!(list.update(now + Duration::from_secs(8), &mut controller), "then the apps start");

        let mut list = Checklist::fixed(vec![item_named("Ports", Status::Warn)], Vec::new(), "checking…");
        list.update(now, &mut controller);
        let (sender, results) = channel();
        list.retry(&mut controller);
        list.results = results;
        sender.send((0, Message::Done((Status::Warn, "47211 is in use".into())))).unwrap();
        assert!(!list.update(now, &mut controller));
        assert!(!list.update(now + Duration::from_secs(600), &mut controller), "after r it waits for a key");
        assert_eq!(list.footer, format!("1 to look at - any key continues · r retries Ports{fix}"));
    }

    #[test]
    fn a_short_hold_has_no_bar() {
        let (_temp, mut controller) = crate::screen::tests::controller("checklist-short");
        let mut list = Checklist::fixed(vec![item(Status::Ok)], Vec::new(), "checking…");
        let now = Instant::now();
        assert!(!list.update(now, &mut controller), "all good, a moment");
        assert_eq!(list.draining(now), None);
    }

    #[test]
    fn there_is_nothing_to_retry_while_a_check_goes_on_or_lazyrunner_reopens() {
        let (_temp, mut controller) = crate::screen::tests::controller("checklist-no-retry");
        let checking = Checklist::fixed(vec![item(Status::Running), item(Status::Fail)], Vec::new(), "checking…");
        assert!(!checking.can_retry(), "still checking");
        let mut updated = Checklist::fixed(vec![item(Status::Warn)], Vec::new(), "checking…");
        updated.update(Instant::now(), &mut controller);
        assert!(updated.can_retry());
        updated.updated_to = Some("9.9.9".into());
        assert!(!updated.can_retry(), "a new version is about to open");
    }

    #[test]
    fn each_problem_has_its_fix_and_a_runs_only_the_safe_ones() {
        let problem = |name: &str, status, detail: &str| Item { name: name.into(), status, detail: detail.into() };
        let list = Checklist::fixed(
            vec![
                problem("Web packages", Status::Fail, "node_modules is missing"),
                problem("Ports", Status::Warn, "47211 used by node"),
                problem("Database", Status::Warn, "Local · nothing answers on port 1433"),
                problem("lazyrunner", Status::Warn, "GitHub didn't answer"),
                problem("git", Status::Fail, "not found on PATH"),
            ],
            Vec::new(),
            "",
        );
        let fixes: Vec<Option<Fix>> = (0..5).map(|index| list.fix_of(index)).collect();
        assert_eq!(
            fixes,
            [Some(Fix::NpmCi("web".into())), Some(Fix::FreePorts), Some(Fix::Database), Some(Fix::Retry), None]
        );
        let safe: Vec<bool> = fixes.iter().flatten().map(Fix::safe).collect();
        assert_eq!(safe, [true, false, false, true], "a kills nothing and asks for no pick");
        let reserved = problem("Ports", Status::Fail, "47211 reserved by Windows (47200-47299) - net stop winnat");
        assert_eq!(Checklist::fixed(vec![reserved], Vec::new(), "").fix_of(0), Some(Fix::Winnat));
    }

    #[test]
    fn a_failed_check_holds_the_start_and_once_fixed_it_counts_down_to_it() {
        let (_temp, mut controller) = crate::screen::tests::controller("checklist-blocking");
        let mut list = Checklist::fixed(vec![item(Status::Fail), item_named("Ports", Status::Ok)], Vec::new(), "");
        list.start_after = true;
        let now = Instant::now();
        assert!(!list.update(now, &mut controller));
        assert!(!list.update(now + Duration::from_secs(600), &mut controller), "it never starts by itself");
        assert_eq!(list.footer, "1 blocking the start · Enter starts anyway · r retries git");

        let (sender, results) = channel();
        list.again.push((results, vec![0]));
        list.items[0].status = Status::Pending;
        sender.send((0, Message::Done((Status::Ok, "2.50".into())))).unwrap();
        assert!(!list.update(now, &mut controller), "fixed: it counts down");
        assert_eq!(list.footer, "nothing blocks the start - starting in 3 s · Esc holds it");
        assert!(list.hold(), "Esc holds it");
        assert!(!list.update(now + Duration::from_secs(600), &mut controller));
        assert_eq!(list.footer, "held - Enter starts");
        assert!(!list.hold(), "nothing to hold any more");
    }

    #[test]
    fn the_selection_starts_on_the_first_row_and_moves_as_the_rows_are_drawn() {
        let problem = |name: &str, status| Item { name: name.into(), status, detail: String::new() };
        let mut list = Checklist::fixed(
            vec![
                problem("lazyrunner", Status::Warn),
                problem("git", Status::Ok),
                problem("Ports", Status::Warn),
                problem("Web packages", Status::Fail),
                problem("Profile", Status::Ok),
            ],
            Vec::new(),
            "",
        );
        let names = |list: &Checklist| -> Vec<String> {
            list.order().into_iter().map(|index| list.items[index].name.clone()).collect()
        };
        assert_eq!(names(&list), ["git", "Profile", "Ports", "Web packages", "lazyrunner"], "by group, as drawn");
        assert_eq!(list.items[list.selected].name, "git", "the first row, not the first problem");
        let mut walked = Vec::new();
        for _ in 0..5 {
            list.select(1);
            walked.push(list.items[list.selected].name.clone());
        }
        assert_eq!(walked, ["Profile", "Ports", "Web packages", "lazyrunner", "git"], "down, round the end");
        assert!(list.next_problem(true));
        assert_eq!(list.items[list.selected].name, "Ports", "n: the next problem");
        list.ignore();
        assert!(
            list.next_problem(true) && list.items[list.selected].name == "Web packages",
            "an ignored one is passed by"
        );
        assert_eq!(list.counts(), (2, 1, 1, 1), "the ignored warning counts as skipped");
        assert_eq!(list.fix_target(), Some(list.selected), "f: the picked problem's fix");
    }

    #[test]
    fn the_counts_tick_up_a_check_at_a_time_the_bar_says_when_it_goes_round_and_the_countdown_drains() {
        let item = |name: &str, status| Item { name: name.into(), status, detail: String::new() };
        let items = vec![item("git", Status::Ok), item("Ports", Status::Warn), item("Profile", Status::Ok)];
        let mut list = Checklist::fixed(items, Vec::new(), "");
        let now = Instant::now();
        for row in &mut list.rows {
            row.done_at = Some(now);
        }
        let at = |ms: u64| list.tally(Some(now + Duration::from_millis(ms)));
        assert_eq!((at(0).done, at(0).counts), (1, [1, 0, 0, 0]), "finished together: one at a time");
        assert_eq!((at(40).done, at(40).counts), (2, [1, 1, 0, 0]));
        assert_eq!(at(40).bumped[1], Some(now + TICK), "the warnings went up then: they pulse");
        assert_eq!((at(80).done, at(80).counts), (3, [2, 1, 0, 0]));
        assert_eq!(list.tally(None).done, 3, "reduced motion: all at once");
        let went_round: Vec<bool> = [1, 1, 1, -1].into_iter().map(|step| list.select(step)).collect();
        assert_eq!(went_round, [false, false, true, true], "round the end, both ways");
        assert_eq!(list.counting_down(now), None);
        list.countdown = Some(now + COUNTDOWN);
        assert_eq!(list.counting_down(now + Duration::from_millis(1500)), Some((0.5, 2)));
    }

    #[test]
    fn a_key_skips_the_checks_still_running() {
        let (_temp, mut controller) = crate::screen::tests::controller("checklist-skip");
        let (_sender, results) = channel();
        let mut list = Checklist::fixed(vec![item(Status::Running), item(Status::Pending)], Vec::new(), "checking…");
        list.results = results; // the checks are still going
        list.skip = true;
        assert!(list.update(Instant::now(), &mut controller));
        assert!(list.items.iter().all(|item| item.status == Status::Skip));
    }
}
