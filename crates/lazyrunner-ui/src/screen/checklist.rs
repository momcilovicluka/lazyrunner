//! The startup checklist: a short animated splash that checks the tools, ports, the database server and the
//! frontend packages before lazyrunner starts anything ("startupChecklist": false turns it off; , then r runs it
//! any time). The checks run side by side, each in a thread of its own, so the screen stays live. At startup the
//! apps don't wait long for the update check: it goes on in the background (`StillChecking`).

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

pub struct Item {
    pub name: String,
    pub status: Status,
    pub detail: String,
}

enum Phase {
    Checking,
    Holding { until: Instant, problems: usize },
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
            self.phase = Phase::Holding { until: now + Duration::from_secs_f64(hold), problems: problems.len() };
        }
        let Phase::Holding { until, problems } = self.phase else { return false };
        if self.skip || now >= until {
            return true;
        }
        let left = until.saturating_duration_since(now).as_secs_f64().ceil() as u64;
        self.footer = if let Some(version) = self.updated_to.as_ref().filter(|_| !self.manual) {
            format!("v{version} installed - reopening lazyrunner…")
        } else if self.manual {
            "any key closes".into()
        } else if !self.news.is_empty() {
            format!("what's new above (, then c shows the whole changelog) - any key continues ({left} s)")
        } else if problems > 0 {
            format!("{problems} to look at (also in the log below) - any key continues ({left} s)")
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
        Item { name: "git".into(), status, detail: String::new() }
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
