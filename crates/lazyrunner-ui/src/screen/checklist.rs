//! The startup checklist: a short animated splash that checks the tools, ports, the database server and the
//! frontend packages before lazyrunner starts anything ("startupChecklist": false turns it off; , then r runs it
//! any time). The checks run side by side, each in a thread of its own, so the screen stays live. At startup the
//! apps don't wait long for the update check: it goes on in the background (`StillChecking`). Once every check is
//! done, r runs the ones that failed or warned again (a failed update, say).

use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::time::{Duration, Instant};

use lazyrunner_core::changelog::{self, Entry, Line};
use lazyrunner_core::output::Hue;
use lazyrunner_core::run::Controller;
use lazyrunner_core::run::checks::{self, Outcome, Status};
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
        problems: usize,
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
}

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
        }
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
            let Some(index) = failed.iter().copied().find(|&index| self.items[index].name == check.name) else {
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

    fn finish_unfinished(&mut self, status: Status, detail: &str) {
        for item in self.items.iter_mut().filter(|item| matches!(item.status, Status::Pending | Status::Running)) {
            item.status = status;
            item.detail = detail.into();
        }
    }

    /// Takes the results that came in and moves on; true once it is done (the overlay closes).
    pub fn update(&mut self, now: Instant, controller: &mut Controller) -> bool {
        loop {
            match self.results.try_recv() {
                Ok((index, outcome)) => {
                    let Some(item) = self.items.get_mut(index) else { continue };
                    match outcome {
                        Message::Running => item.status = Status::Running,
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
                        }
                    }
                }
                Err(TryRecvError::Empty) => break,
                // Done, or the checks' thread died: what didn't finish never will.
                Err(TryRecvError::Disconnected) => {
                    self.finish_unfinished(Status::Fail, "the check didn't finish");
                    break;
                }
            }
        }
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
            self.phase = Phase::Holding { from: now, until, problems: problems.len() };
            // Opened by hand, or after r: what can be retried stays up until a key, never closing under the next r.
            if (self.manual || self.retried) && self.can_retry() {
                self.phase = Phase::Holding { from: now, until: None, problems: problems.len() };
            }
        }
        let Phase::Holding { until, problems, .. } = self.phase else { return false };
        if self.skip || until.is_some_and(|until| now >= until) {
            return true;
        }
        let left = until.map_or(String::new(), |until| {
            format!(" ({} s)", until.saturating_duration_since(now).as_secs_f64().ceil() as u64)
        });
        let retry = if self.can_retry() { format!(" · {}", self.retry_hint()) } else { String::new() };
        self.footer = if let Some(version) = self.updated_to.as_ref().filter(|_| !self.manual) {
            format!("v{version} installed - reopening lazyrunner…")
        } else if self.manual {
            format!("any key closes{left}{retry}")
        } else if !self.news.is_empty() {
            format!("what's new above (, then c shows the whole changelog) - any key continues{left}{retry}")
        } else if problems > 0 {
            format!("{problems} to look at (also in the log below) - any key continues{left}{retry}")
        } else {
            "all good - starting…".into()
        };
        false
    }
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
        }
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
        assert_eq!(list.footer, "1 to look at (also in the log below) - any key continues (8 s) · r retries Ports");
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
        assert_eq!(list.footer, "1 to look at (also in the log below) - any key continues · r retries Ports");
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
