//! What changed since an app's previous run (`Alt+d` on its column). A run is kept as its digest - the shapes of its
//! lines (`output::shape`: what changes every run blanked) and its errors and warnings, each one entry with the lines
//! that belong to it - when the next run starts, and in runs.json in the project's state folder when lazyrunner
//! quits, so the first run after it opens again compares too. A good build's warnings are compared with the last good
//! build's the same way (builds.json).

use std::collections::{HashMap, HashSet};
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::output::{self, Level, Output, OutputLine};

pub const FILE: &str = "runs.json";
pub const BUILDS_FILE: &str = "builds.json";

/// A run as the next one compares with it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Digest {
    /// Its lines' shapes.
    pub shapes: HashSet<u64>,
    /// Its errors and warnings, oldest first.
    pub problems: Vec<Problem>,
}

/// An error or a warning a run printed: the shape of all its lines, and its first line.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Problem {
    pub shape: u64,
    pub text: String,
    pub error: bool,
}

/// Lines printed together as one error or warning: the numbers of the first and the last, how serious it is, and the
/// shape of all of them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Block {
    pub first: u64,
    pub last: u64,
    pub level: Level,
    pub shape: u64,
}

impl Digest {
    /// The run whose first line is line `from` of `output` (those of its lines still kept).
    pub fn of(output: &Output, from: u64) -> Self {
        let lines = numbered(output, from);
        let text = |number: u64| lines.iter().find(|(at, _)| *at == number).map(|(_, line)| line.text.clone());
        let problems = blocks(&lines)
            .into_iter()
            .map(|block| Problem {
                shape: block.shape,
                text: text(block.first).unwrap_or_default(),
                error: block.level == Level::Error,
            })
            .collect();
        Self { shapes: lines.iter().map(|(_, line)| line.shape).collect(), problems }
    }
}

/// A good build's warnings, as the next build compares with them: how many, and their shapes.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Warnings {
    pub count: usize,
    pub shapes: HashSet<u64>,
}

/// A good build's `warnings` (each once) next to the last good build's (`previous`): how many more or fewer - none
/// with no build to compare with - the new ones, as they came, and these to compare the next build with.
pub fn warning_change<'a>(
    previous: Option<&Warnings>,
    warnings: &'a [String],
) -> (Option<isize>, Vec<&'a str>, Warnings) {
    let now =
        Warnings { count: warnings.len(), shapes: warnings.iter().map(|warning| output::shape(warning)).collect() };
    let Some(previous) = previous else { return (None, Vec::new(), now) };
    let new = warnings
        .iter()
        .filter(|warning| !previous.shapes.contains(&output::shape(warning)))
        .map(String::as_str)
        .collect();
    (Some(now.count as isize - previous.count as isize), new, now)
}

/// How many errors and warnings the run of `output` from line `from` on has that `previous` didn't.
pub fn new_problems(previous: &Digest, output: &Output, from: u64) -> (usize, usize) {
    let known: HashSet<u64> = previous.problems.iter().map(|problem| problem.shape).collect();
    let fresh = blocks(&numbered(output, from)).into_iter().filter(|block| !known.contains(&block.shape));
    fresh.fold(
        (0, 0),
        |(errors, warnings), block| {
            if block.level == Level::Error { (errors + 1, warnings) } else { (errors, warnings + 1) }
        },
    )
}

/// The lines of `output` from line `from` on that are still kept, with their numbers.
pub fn numbered(output: &Output, from: u64) -> Vec<(u64, &OutputLine)> {
    let oldest = output.total() + 1 - output.len() as u64;
    (from.max(oldest)..).zip(output.numbered(from, output.total())).collect()
}

/// The errors and warnings among `lines`, as entries: a line in an error's or a warning's colour starts one, and the
/// indented lines after it in the same colour - an entry's message, a stack - belong to it.
pub fn blocks(lines: &[(u64, &OutputLine)]) -> Vec<Block> {
    let mut blocks: Vec<Block> = Vec::new();
    for &(number, line) in lines {
        let level = line.severity();
        if level < Level::Warn {
            continue;
        }
        let shape = line.shape.to_le_bytes();
        match blocks.last_mut() {
            Some(block)
                if block.last + 1 == number && block.level == level && line.text.starts_with(char::is_whitespace) =>
            {
                block.last = number;
                block.shape = output::mix(block.shape, &shape);
            }
            _ => blocks.push(Block {
                first: number,
                last: number,
                level,
                shape: output::mix(output::FNV_OFFSET, &shape),
            }),
        }
    }
    blocks
}

/// The runs kept in the state folder `state`, by app key; none when there's no runs.json or it doesn't read.
pub fn load(state: &Path) -> HashMap<String, Digest> {
    crate::json::read(&state.join(FILE)).and_then(|value| serde_json::from_value(value).ok()).unwrap_or_default()
}

/// Keeps `runs` (by app key) in the state folder `state`.
pub fn save(state: &Path, runs: &HashMap<&str, &Digest>) -> io::Result<()> {
    let text = serde_json::to_string(runs).map_err(io::Error::other)?;
    std::fs::create_dir_all(state)?;
    std::fs::write(state.join(FILE), text)
}

/// Each app's last good build's warnings kept in the state folder `state` (builds.json).
pub fn load_builds(state: &Path) -> HashMap<String, Warnings> {
    crate::json::read(&state.join(BUILDS_FILE)).and_then(|value| serde_json::from_value(value).ok()).unwrap_or_default()
}

/// Keeps each app's last good build's warnings in the state folder `state`.
pub fn save_builds(state: &Path, builds: &HashMap<String, Warnings>) -> io::Result<()> {
    let text = serde_json::to_string(builds).map_err(io::Error::other)?;
    std::fs::create_dir_all(state)?;
    std::fs::write(state.join(BUILDS_FILE), text)
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;
    use crate::output::Hue;
    use crate::testing::TempDir;

    fn run() -> Output {
        let now = Instant::now();
        let mut output = Output::new(100);
        output.push("an earlier run", now);
        for (text, hue) in [
            ("──── ▶ 12:00:00 · main · dotnet run ────", Hue::Cyan),
            ("info: Now listening on: http://localhost:5001", Hue::Cyan),
            ("fail: Orders.Api[0]", Hue::Red),
            ("      Invoice 7 has no currency.", Hue::Red),
            ("fail: Billing.Api[0]", Hue::Red),
            ("warn: slow for 40 ms", Hue::Yellow),
        ] {
            output.push_hued(text, Some(hue), now);
        }
        output
    }

    #[test]
    fn a_run_keeps_its_lines_shapes_and_its_errors_as_entries() {
        let digest = Digest::of(&run(), 2);
        assert!(!digest.shapes.contains(&output::shape("an earlier run")), "only this run's lines");
        assert!(digest.shapes.contains(&output::shape("info: Now listening on: http://localhost:5002")));
        let problems: Vec<(&str, bool)> =
            digest.problems.iter().map(|problem| (problem.text.as_str(), problem.error)).collect();
        assert_eq!(
            problems,
            [("fail: Orders.Api[0]", true), ("fail: Billing.Api[0]", true), ("warn: slow for 40 ms", false)],
            "an entry's indented lines belong to it; the next entry starts another"
        );
    }

    #[test]
    fn a_runs_new_errors_and_warnings_are_those_the_last_one_didnt_have() {
        let output = run();
        let mut previous = Digest::of(&output, 2);
        assert_eq!(new_problems(&previous, &output, 2), (0, 0));
        previous.problems.retain(|problem| problem.text != "fail: Billing.Api[0]");
        assert_eq!(new_problems(&previous, &output, 2), (1, 0));
    }

    #[test]
    fn a_builds_warnings_are_compared_with_the_last_good_builds() {
        let first = ["A.cs(12,5): warning CS8618: Name is null".to_string()];
        let (change, new, kept) = warning_change(None, &first);
        assert_eq!((change, new.len(), kept.count), (None, 0, 1), "nothing to compare the first with");
        let second = [
            "A.cs(14,5): warning CS8618: Name is null".to_string(),
            "B.cs(3,1): warning CS0168: e is never used".to_string(),
        ];
        let (change, new, _) = warning_change(Some(&kept), &second);
        assert_eq!(change, Some(1));
        assert_eq!(new, ["B.cs(3,1): warning CS0168: e is never used"], "the one that moved two lines isn't new");
    }

    #[test]
    fn runs_are_kept_in_the_state_folder() {
        let temp = TempDir::new("runs");
        let digest = Digest::of(&run(), 2);
        save(temp.path(), &HashMap::from([("orders", &digest)])).unwrap();
        assert_eq!(load(temp.path()).get("orders"), Some(&digest));
        std::fs::write(temp.path().join(FILE), "{ not json").unwrap();
        assert!(load(temp.path()).is_empty(), "a broken file reads as none");
    }
}
