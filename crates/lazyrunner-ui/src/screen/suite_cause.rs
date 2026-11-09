//! A failure's likely cause in the tests view (#34): under each failure, the sources that changed since its test
//! last passed - those in its stack first, then those of its project and the ones it references (the "why" chain's
//! look), then the rest - each a link; `y` copies them with the failure.

use lazyrunner_core::links;
use lazyrunner_core::run::Controller;
use lazyrunner_core::run::cause::{Cause, Lead};

use super::Ui;
use super::mouse::Target;
use super::seg::{Seg, seg};
use super::theme::C;

/// Changed files shown under a failure, and copied with it, at most.
const SHOWN: usize = 5;
const COPIED: usize = 20;
const ACCENT: C = C::Magenta;

/// Why a changed file may be the cause, in words: "in its stack", "Orders.Tests ← Orders.Core", "changed".
pub fn lead(lead: &Lead) -> String {
    match lead {
        Lead::Stack => "in its stack".into(),
        Lead::Chain(chain) => chain.join(" ← "),
        Lead::Changed => "changed".into(),
    }
}

/// When it last passed, as the block says it: "09:41" today, "11-02 09:41" before.
fn when(at: &str) -> String {
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let (day, time) = at.split_once(' ').unwrap_or(("", at));
    let time: String = time.chars().take(5).collect();
    if day == today || day.is_empty() { time } else { format!("{} {time}", day.get(5..).unwrap_or(day)) }
}

/// The block under a failure, indented like its message; nothing when it never passed in the runs kept, or git
/// can't say what changed.
pub fn rows(ui: &mut Ui, controller: &Controller, key: &str, test: &str, width: usize) -> Vec<Vec<Seg>> {
    let Some(found) = controller.likely_cause(key, test) else { return Vec::new() };
    let since = found.passed_at.as_deref().map_or(String::new(), |at| format!(" ({})", when(at)));
    if found.causes.is_empty() {
        let text = format!("    LIKELY CAUSE · no source changed since it last passed{since} - flaky?");
        return vec![ui.scrolling_row(vec![seg(text, C::DarkGray)], 0, width)];
    }
    let title = format!("    LIKELY CAUSE · changed since it last passed{since}");
    let mut lines = vec![ui.scrolling_row(vec![seg(title, ACCENT)], 0, width)];
    for cause in found.causes.iter().take(SHOWN) {
        lines.push(row(ui, controller, cause, width));
    }
    if found.causes.len() > SHOWN {
        lines.push(vec![seg(format!("      +{} more", found.causes.len() - SHOWN), C::DarkGray)]);
    }
    lines
}

/// A changed file, a link, and why it may be the cause.
fn row(ui: &mut Ui, controller: &Controller, cause: &Cause, width: usize) -> Vec<Seg> {
    ui.links.push(links::Target::File {
        path: controller.root.join(&cause.file).display().to_string(),
        line: None,
        column: None,
    });
    let color = if cause.lead == Lead::Stack { C::White } else { C::Gray };
    let mut link = seg(cause.file.clone(), color).at(Target::Link { column: ui.focus, link: ui.links.len() - 1 });
    link.underline = true;
    let why = if cause.lead == Lead::Stack { ACCENT } else { C::DarkGray };
    ui.scrolling_row(vec![seg("      ", C::Gray), link, seg(format!("  {}", lead(&cause.lead)), why)], 2, width)
}

/// The likely cause as `y` copies it with the failure.
pub fn text(controller: &Controller, key: &str, test: &str) -> Option<String> {
    let found = controller.likely_cause(key, test)?;
    let since = found.passed_at.as_deref().map_or(String::new(), |at| format!(" at {at}"));
    if found.causes.is_empty() {
        return Some(format!("Likely cause: no source changed since it last passed{since}"));
    }
    let mut lines = vec![format!("Likely cause (changed since it last passed{since}):")];
    lines.extend(found.causes.iter().take(COPIED).map(|cause| format!("  {} ({})", cause.file, lead(&cause.lead))));
    if found.causes.len() > COPIED {
        lines.push(format!("  +{} more", found.causes.len() - COPIED));
    }
    Some(lines.join(super::copy::NEWLINE))
}
