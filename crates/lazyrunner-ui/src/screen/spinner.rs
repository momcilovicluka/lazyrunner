//! The spinner shown while something runs: lazyrunner's own, or `"spinner"` from the settings - for every project,
//! or one project's own in its section - as lazygit's `gui.spinner`: `{ "frames": ["◐", "◓", "◑", "◒"], "rate": 120 }`
//! (milliseconds a frame).

use std::time::Duration;

use lazyrunner_core::output::Hue;
use lazyrunner_core::run::Controller;
use serde_json::Value;
use unicode_width::UnicodeWidthStr;

use super::Ui;

const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const RATE: Duration = Duration::from_millis(100);
/// A frame's time beyond these is a typo, not a wish.
const FASTEST_MS: u64 = 16;
const SLOWEST_MS: u64 = 2000;
/// The widest frame, in cells: a spinner sits before a line's text.
const WIDEST: usize = 8;

#[derive(Clone, Debug, PartialEq)]
pub struct Spinner {
    /// Each as wide as the widest, so the text after it holds still.
    frames: Vec<String>,
    rate: Duration,
    /// Set in the settings: it spins where lazyrunner's orbit would too.
    pub custom: bool,
    /// The setting it was read from: it's read again only when that changes.
    source: Value,
}

impl Default for Spinner {
    fn default() -> Self {
        Self { frames: FRAMES.map(str::to_string).to_vec(), rate: RATE, custom: false, source: Value::Null }
    }
}

impl Spinner {
    /// The frame `elapsed` into the run.
    pub fn frame(&self, elapsed: Duration) -> &str {
        let at = elapsed.as_millis() / self.rate.as_millis().max(1);
        &self.frames[(at % self.frames.len() as u128) as usize]
    }
}

/// Reads `"spinner"` again when the settings changed it; one that can't be used is said, and lazyrunner's own spins.
pub fn refresh(ui: &mut Ui, controller: &mut Controller) {
    let setting = &controller.config["spinner"];
    if *setting == ui.spinner.source {
        return;
    }
    let (spinner, problem) = read(setting);
    ui.spinner = spinner;
    if let Some(problem) = problem {
        controller.add_log(format!("\"spinner\": {problem} - using lazyrunner's own"), Hue::Yellow);
    }
}

fn read(setting: &Value) -> (Spinner, Option<String>) {
    let own = Spinner { source: setting.clone(), ..Spinner::default() };
    if setting.is_null() {
        return (own, None);
    }
    let frames = setting["frames"]
        .as_array()
        .and_then(|list| list.iter().map(|frame| frame.as_str().map(str::to_string)).collect::<Option<Vec<_>>>())
        .filter(|frames| !frames.is_empty() && frames.iter().all(|frame| (1..=WIDEST).contains(&frame.width())));
    let Some(frames) = frames else {
        return (own, Some(format!("\"frames\" is a list of texts, each 1 to {WIDEST} cells wide")));
    };
    let rate = match &setting["rate"] {
        Value::Null => RATE.as_millis() as u64,
        rate => match rate.as_u64().filter(|ms| (FASTEST_MS..=SLOWEST_MS).contains(ms)) {
            Some(ms) => ms,
            None => return (own, Some(format!("\"rate\" is milliseconds a frame, {FASTEST_MS} to {SLOWEST_MS}"))),
        },
    };
    let widest = frames.iter().map(|frame| frame.width()).max().unwrap_or(1);
    let frames = frames.iter().map(|frame| format!("{frame}{}", " ".repeat(widest - frame.width()))).collect();
    (Spinner { frames, rate: Duration::from_millis(rate), custom: true, source: setting.clone() }, None)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn frames_step_at_their_rate_and_are_padded_to_the_widest() {
        let (spinner, problem) = read(&json!({ "frames": ["a", "bbb"], "rate": 50 }));
        assert_eq!(problem, None);
        assert!(spinner.custom);
        assert_eq!(spinner.frame(Duration::from_millis(0)), "a  ");
        assert_eq!(spinner.frame(Duration::from_millis(60)), "bbb");
        assert_eq!(spinner.frame(Duration::from_millis(100)), "a  ");
    }

    #[test]
    fn one_that_cant_be_used_says_why_and_lazyrunners_own_spins() {
        for (setting, why) in [
            (json!({ "frames": [] }), "\"frames\""),
            (json!({ "frames": ["|", 3] }), "\"frames\""),
            (json!({ "frames": ["toolongtext"] }), "\"frames\""),
            (json!({ "frames": ["|"], "rate": 0 }), "\"rate\""),
        ] {
            let (spinner, problem) = read(&setting);
            assert!(problem.is_some_and(|problem| problem.starts_with(why)), "{setting}");
            assert!(!spinner.custom && spinner.frame(Duration::ZERO) == FRAMES[0]);
        }
        assert_eq!(read(&json!({ "frames": ["|", "/"] })).0.rate, RATE, "no rate: lazyrunner's");
    }
}
