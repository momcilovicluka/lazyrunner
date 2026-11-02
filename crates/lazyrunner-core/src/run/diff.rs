//! What changed since an app's previous run (`Alt+d` on its column). A run is kept as its digest - the shapes of its
//! lines (`output::shape`: what changes every run blanked) and its errors and warnings, each one entry with the lines
//! that belong to it - when the next run starts, and in runs.json in the project's state folder when lazyrunner
//! quits, so the first run after it opens again compares too.

use std::collections::{HashMap, HashSet};
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::output::{self, Level, Output, OutputLine};

pub const FILE: &str = "runs.json";

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
    fn runs_are_kept_in_the_state_folder() {
        let temp = TempDir::new("runs");
        let digest = Digest::of(&run(), 2);
        save(temp.path(), &HashMap::from([("orders", &digest)])).unwrap();
        assert_eq!(load(temp.path()).get("orders"), Some(&digest));
        std::fs::write(temp.path().join(FILE), "{ not json").unwrap();
        assert!(load(temp.path()).is_empty(), "a broken file reads as none");
    }
}
