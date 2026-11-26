//! One test's history for an agent (#112): `get_test_results` with `test` - the test by its full name or a part of it
//! only it has - answers its state as the tests view tells it (a new or known failure, fixed, passing, skipped, and
//! flaky besides), its last runs, the newest first, each with when, how it did, its time, its failure's first line
//! and the code it ran on, and where its outcome turned.

use std::collections::BTreeSet;

use lazyrunner_core::run::Controller;
use lazyrunner_core::run::impact::Baseline;
use lazyrunner_core::run::suites::{Outcome, Suite};
use lazyrunner_core::run::test_history::SuiteHistory;
use serde_json::{Value, json};

use super::{Answer, CLIENT_CHARS, Refusal, clean};

/// A test's runs in the answer at most, the newest.
pub const RUNS_SHOWN: usize = 10;
/// The tests a part matching several names at most.
const CANDIDATES: usize = 10;
/// A commit's short form, in characters.
const SHORT_HEAD: usize = 7;
const NO_CODE: &str = "code is null for runs kept before lazyrunner recorded their commit, or outside git";
const NO_DETAILS: &str = "ms and message are null for runs kept before lazyrunner recorded them";

/// Test `wanted`'s history in `suite`.
pub fn test(controller: &Controller, suite: &Suite, wanted: &str) -> Answer {
    let history = controller.test_history.suite(&suite.key);
    let name = find_test(suite, history, wanted)?;
    let story = history.map(|history| history.story(&name)).unwrap_or_default();
    let last = suite.tests.iter().find(|test| test.name == name).map(|test| test.outcome);
    let ran: Vec<usize> = history.map_or_else(Vec::new, |history| {
        (0..history.runs.len()).rev().filter(|run| history.outcome_in(&name, *run).is_some()).collect()
    });
    let last = last.or_else(|| history.zip(ran.first()).and_then(|(history, run)| history.outcome_in(&name, *run)));
    let state = match last {
        Some(Outcome::Failed) if history.is_none() => "failed",
        Some(Outcome::Failed) if story.new => "new failure",
        Some(Outcome::Failed) => "known failure",
        Some(Outcome::Passed) if story.fixed => "fixed",
        Some(Outcome::Passed) => "passing",
        Some(Outcome::Skipped) => "skipped",
        None => "not run",
    };
    let mut answer = json!({
        "suite": suite.name,
        "test": name,
        "state": state,
        "flaky": story.flaky.map(|(failures, runs)| format!("failed {failures} of its last {runs} runs")),
    });
    if last == Some(Outcome::Failed) {
        answer["likely_cause"] = super::likely_cause(controller, &suite.key, &name);
    }
    let Some(history) = history else { return Ok(answer) };
    let runs: Vec<Value> = ran.iter().take(RUNS_SHOWN).map(|run| run_row(history, &name, *run)).collect();
    let mut notes = Vec::new();
    if runs.iter().any(|run| run["code"].is_null()) {
        notes.push(NO_CODE);
    }
    if runs.iter().any(|run| run["outcome"] != "skipped" && run["ms"].is_null()) {
        notes.push(NO_DETAILS);
    }
    answer["runs"] = json!(runs);
    answer["runs_kept"] = json!(ran.len());
    answer["changes"] = json!(changes(history, &name));
    if !notes.is_empty() {
        answer["note"] = json!(notes.join("; "));
    }
    Ok(answer)
}

/// The test by its name, else the one name with `wanted` in it, among its suite's last results and its history.
fn find_test(suite: &Suite, history: Option<&SuiteHistory>, wanted: &str) -> Result<String, Refusal> {
    let kept = history.into_iter().flat_map(|history| history.tests.keys());
    let names: BTreeSet<&String> = suite.tests.iter().map(|test| &test.name).chain(kept).collect();
    if let Some(name) = names.iter().find(|name| name.as_str() == wanted) {
        return Ok((*name).clone());
    }
    let part = wanted.to_lowercase();
    let found: Vec<&&String> = names.iter().filter(|name| name.to_lowercase().contains(&part)).collect();
    let asked = clean(wanted, CLIENT_CHARS);
    match found.as_slice() {
        [name] => Ok((**name).clone()),
        [] => Err(("not_found", format!("no test in {} has '{asked}' in its name", suite.name))),
        many => {
            let shown: Vec<&str> = many.iter().take(CANDIDATES).map(|name| name.as_str()).collect();
            let more = many.len().saturating_sub(CANDIDATES);
            let more = if more > 0 { format!(" (+{more} more)") } else { String::new() };
            let why = format!("{} tests match '{asked}' - name one: {}{more}", many.len(), shown.join(", "));
            Err(("bad_args", why))
        }
    }
}

/// Run `run` of test `name`: when, how it did, its time, its failure's first line, the code.
fn run_row(history: &SuiteHistory, name: &str, run: usize) -> Value {
    let outcome = history.outcome_in(name, run);
    let mut row = json!({
        "at": history.when(run),
        "outcome": outcome.map(|outcome| format!("{outcome:?}").to_lowercase()),
        "ms": history.ms_in(name, run),
        "code": history.base_of(run).map(code),
    });
    if outcome == Some(Outcome::Failed) {
        row["message"] = json!(history.message_in(name, run));
    }
    row
}

/// The code a run ran on: its commit, short, and how many files differed from it.
fn code(base: &Baseline) -> String {
    let head: String = base.head.chars().take(SHORT_HEAD).collect();
    match base.dirty.len() {
        0 => head,
        1 => format!("{head} + 1 uncommitted file"),
        files => format!("{head} + {files} uncommitted files"),
    }
}

/// Where test `name`'s outcome turned between the runs it passed or failed in, the newest first:
/// "passed on a1b2c3d, failed since d4e5f6a + 2 uncommitted files (2026-11-02 09:41:07)".
fn changes(history: &SuiteHistory, name: &str) -> Vec<String> {
    let ran = (0..history.runs.len()).filter_map(|run| match history.outcome_in(name, run) {
        Some(outcome @ (Outcome::Passed | Outcome::Failed)) => Some((run, outcome)),
        _ => None,
    });
    let ran: Vec<(usize, Outcome)> = ran.collect();
    let said = |outcome: Outcome| if outcome == Outcome::Passed { "passed" } else { "failed" };
    let on = |run: usize| history.base_of(run).map_or_else(|| "unrecorded code".to_string(), code);
    let turns = ran.windows(2).filter(|pair| pair[0].1 != pair[1].1).map(|pair| {
        let ((before, was), (after, now)) = (pair[0], pair[1]);
        let at = history.when(after).map_or_else(String::new, |at| format!(" ({at})"));
        let (code_before, code_after) = (&history.runs[before], &history.runs[after]);
        let same = if !code_after.is_empty() && code_before == code_after { " - the same code" } else { "" };
        format!("{} on {}, {} since {}{at}{same}", said(was), on(before), said(now), on(after))
    });
    let mut turns: Vec<String> = turns.collect();
    turns.reverse();
    turns.truncate(RUNS_SHOWN);
    turns
}
