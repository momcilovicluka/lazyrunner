//! The tasks in the panel's header (#56): one running as a capsule - its spinner, its name, how long it has run and,
//! once it has passed before, a bar against its usual time; one that just passed glints green and goes (its result is
//! in the log); one that failed pulses red twice and stays with its first error line until it's clicked away.

use std::time::Duration;

use lazyrunner_core::run::{Controller, format_duration};

use super::Ui;
use super::icons::{self, Icon};
use super::mouse::Target;
use super::seg::{Seg, seg};
use super::theme::C;

/// How long a passed task's chip shows, glinting.
const PASSED: Duration = Duration::from_millis(1500);
/// How long the glint runs over it.
const GLINT: Duration = Duration::from_millis(400);
/// A failed task's chip pulses this long, a pulse each two steps.
const PULSE: Duration = Duration::from_millis(800);
const PULSE_STEP: u128 = 200;
/// The cells of the bar against the usual time.
const BAR_CELLS: usize = 4;
const GAP: &str = "   ";

/// `▰▰▱▱` as far as `seconds` is through `usual`, and whether it's past it.
pub fn bar(seconds: f64, usual: f64) -> (String, bool) {
    let through = if usual > 0.0 { seconds / usual } else { 1.0 };
    let filled = ((through * BAR_CELLS as f64).floor() as usize).min(BAR_CELLS);
    (format!("{}{}", "▰".repeat(filled), "▱".repeat(BAR_CELLS - filled)), through > 1.0)
}

/// The header's task chips.
pub fn chips(ui: &Ui, controller: &Controller) -> Vec<Seg> {
    let mut segs = Vec::new();
    for (task, started) in controller.tasks_running() {
        let seconds = ui.now.saturating_duration_since(started).as_secs_f64();
        let spinner = super::columns::spinner(ui);
        let text =
            format!("{GAP}{spinner} {} {} {}", icons::icon(ui.nerd, Icon::Run), task.name, format_duration(seconds));
        segs.push(seg(text, C::Cyan));
        if let Some(usual) = controller.task_usual(&task.key) {
            let (bar, over) = bar(seconds, usual);
            segs.push(seg(format!(" {bar}"), if over { C::Yellow } else { C::DarkCyan }));
            let hourglass = icons::icon(ui.nerd, Icon::Hourglass);
            segs.push(seg(format!(" {hourglass} {}", format_duration(usual)), C::DarkGray));
        }
    }
    for task in controller.tasks() {
        let Some(result) = controller.task_result(&task.key).filter(|_| controller.task_started(&task.key).is_none())
        else {
            continue;
        };
        let since = ui.now.saturating_duration_since(result.at);
        if result.ok && since < PASSED {
            let glint = !ui.reduced_motion && since < GLINT;
            let text =
                format!("{GAP}{} {} {}", icons::icon(ui.nerd, Icon::Ok), task.name, format_duration(result.seconds));
            segs.push(seg(text, if glint { C::White } else { C::Green }));
        }
    }
    for (at, (task, result)) in controller.task_failures().into_iter().enumerate() {
        let index = controller.tasks().iter().position(|each| each.key == task.key).unwrap_or(at);
        let since = ui.now.saturating_duration_since(result.at);
        let pulse = !ui.reduced_motion && since < PULSE && (since.as_millis() / PULSE_STEP).is_multiple_of(2);
        let why = result.error.clone().unwrap_or_default();
        let text = format!("{GAP}{} {}: {why}", icons::icon(ui.nerd, Icon::Fail), task.name);
        segs.push(seg(text, if pulse { C::White } else { C::Red }).at(Target::TaskResult(index)));
    }
    segs
}

/// Whether the chips move: a task running, a glint, a pulse.
pub fn animating(ui: &Ui, controller: &Controller) -> bool {
    let moving = |task: &lazyrunner_core::profile::Task| {
        controller.task_result(&task.key).is_some_and(|result| {
            let since = ui.now.saturating_duration_since(result.at);
            if result.ok { since < PASSED } else { !ui.reduced_motion && since < PULSE }
        })
    };
    !controller.tasks_running().is_empty() || controller.tasks().iter().any(moving)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bar_fills_against_the_usual_time_and_says_when_its_past_it() {
        assert_eq!(bar(0.0, 12.0), ("▱▱▱▱".to_string(), false));
        assert_eq!(bar(6.5, 12.0), ("▰▰▱▱".to_string(), false));
        assert_eq!(bar(13.0, 12.0), ("▰▰▰▰".to_string(), true));
    }

    #[test]
    fn a_running_tasks_tab_progress_is_against_its_usual_time() {
        use super::super::status::task_progress;
        assert_eq!(task_progress(6.0, Some(12.0)), "1;50");
        assert_eq!(task_progress(30.0, Some(12.0)), "1;99", "never done before it is");
        assert_eq!(task_progress(3.0, None), "3;0", "busy, without a figure");
    }
}
