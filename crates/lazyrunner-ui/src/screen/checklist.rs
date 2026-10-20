//! The startup checklist: a short animated splash that checks the tools, ports, the database server and the
//! frontend packages before lazyrunner starts anything ("startupChecklist": false turns it off; , then r runs it
//! any time). The checks run one after the other in a thread of their own, so the screen stays live.

use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::{Duration, Instant};

use lazyrunner_core::changelog::{self, Entry, Line};
use lazyrunner_core::output::Hue;
use lazyrunner_core::run::Controller;
use lazyrunner_core::run::checks::{self, Outcome, Status};

/// Each check shows its spinner at least this long, so every tick can be seen.
const MIN_CHECK: Duration = Duration::from_millis(150);

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
    phase: Phase,
    results: Receiver<(usize, Option<Outcome>)>,
}

impl Checklist {
    pub fn start(controller: &Controller, manual: bool, whats_new: &[Entry], start_after: bool) -> Self {
        let checks = checks::checks(controller, changelog::VERSION);
        let items = checks
            .iter()
            .map(|check| Item { name: check.name.clone(), status: Status::Pending, detail: String::new() })
            .collect();
        let (sender, results) = channel();
        std::thread::spawn(move || {
            for (index, check) in checks.into_iter().enumerate() {
                let started = Instant::now();
                if sender.send((index, None)).is_err() {
                    return;
                }
                let outcome = (check.run)();
                if let Some(rest) = MIN_CHECK.checked_sub(started.elapsed()) {
                    std::thread::sleep(rest);
                }
                if sender.send((index, Some(outcome))).is_err() {
                    return;
                }
            }
        });
        Self {
            items,
            footer: "checking…".into(),
            news: changelog::format(whats_new),
            manual,
            skip: false,
            start_after,
            phase: Phase::Checking,
            results,
        }
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
                        None => item.status = Status::Running,
                        Some((status, detail)) => {
                            item.status = status;
                            item.detail = detail;
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
            // Linger a moment: longer when something needs a look or there is news; any key continues.
            let hold = if self.manual {
                60.0
            } else if !self.news.is_empty() {
                20.0
            } else if !problems.is_empty() {
                8.0
            } else {
                1.2
            };
            self.phase = Phase::Holding { until: now + Duration::from_secs_f64(hold), problems: problems.len() };
        }
        let Phase::Holding { until, problems } = self.phase else { return false };
        if self.skip || now >= until {
            return true;
        }
        let left = until.saturating_duration_since(now).as_secs_f64().ceil() as u64;
        self.footer = if self.manual {
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

#[cfg(test)]
impl Checklist {
    /// A checklist with these results, without running any check (the snapshots).
    pub fn fixed(items: Vec<Item>, news: Vec<Line>, footer: &str) -> Self {
        let (_, results) = channel();
        Self {
            items,
            footer: footer.into(),
            news,
            manual: false,
            skip: false,
            start_after: false,
            phase: Phase::Checking,
            results,
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
