//! `Alt+g`, or a click on the branch in the status line: what changed for running on the last branch switch (#104) -
//! the profile, an app's settings (which keys), launch settings and project file, an npm app's packages, migrations,
//! `.env` files. `j` `k` pick one, `Enter` runs what fixes it (npm ci, the migrations view, reloading the profile).

use lazyrunner_core::run::Controller;
use lazyrunner_core::run::branch_changes::{Change, Fix};

use super::actions::Run;
use super::icons::{self, Icon};
use super::keys;
use super::overlays::boxed;
use super::seg::{Seg, on, pad, seg};
use super::theme::C;
use super::{Overlay, Ui};

const INNER: usize = 96;
const FOOTER: &str = "j k pick · Enter runs its fix · Esc closes";

pub fn open(ui: &mut Ui) {
    ui.overlay = Some(Overlay::BranchChanges { index: 0 });
}

fn changes(controller: &Controller) -> &[Change] {
    controller.branch_changes.as_ref().map_or(&[], |(_, _, changes)| changes.as_slice())
}

pub fn key(ui: &mut Ui, controller: &mut Controller, name: &str) {
    let Some(Overlay::BranchChanges { index }) = &mut ui.overlay else { return };
    let count = changes(controller).len();
    match name {
        "Escape" | "q" | "Alt+g" => ui.overlay = None,
        "j" | "DownArrow" => *index = keys::wrapped(*index, 1, count.max(1)),
        "k" | "UpArrow" => *index = keys::wrapped(*index, -1, count.max(1)),
        "Enter" | "Space" => {
            let picked = changes(controller).get(*index).cloned();
            if let Some(change) = picked {
                fix(ui, controller, &change);
            }
        }
        _ => {}
    }
}

/// Runs what fixes `change`: npm ci for its app, or opens the migrations view, or reloads the profile.
fn fix(ui: &mut Ui, controller: &mut Controller, change: &Change) {
    match change.fix {
        Some(Fix::NpmCi) => {
            if let Some(app) = &change.app {
                controller.npm_ci(Some(app));
            }
        }
        Some(Fix::Migrations) => {
            ui.overlay = None;
            keys::run(ui, controller, &Run::Migrations);
        }
        Some(Fix::ReloadProfile) => {
            ui.overlay = None;
            keys::run(ui, controller, &Run::ReloadProfile);
        }
        None => {}
    }
}

fn fix_said(fix: Option<Fix>) -> &'static str {
    match fix {
        Some(Fix::NpmCi) => "Enter: npm ci",
        Some(Fix::Migrations) => "Enter: M",
        Some(Fix::ReloadProfile) => "Enter: reload",
        None => "",
    }
}

pub fn rows(ui: &Ui, controller: &Controller, cells: usize, rows: usize) -> Vec<Vec<Seg>> {
    let Some(Overlay::BranchChanges { index }) = &ui.overlay else { return Vec::new() };
    let inner = cells.saturating_sub(8).clamp(20, INNER);
    let mut lines = Vec::new();
    match &controller.branch_changes {
        None => lines.push(vec![seg(" No branch switch yet this session.", C::DarkGray)]),
        Some((from, to, changes)) => {
            lines.push(vec![seg(format!(" {from} → {to}"), C::Magenta)]);
            lines.push(Vec::new());
            if changes.is_empty() {
                lines.push(vec![seg(" Nothing that changes how the apps run - only their code.", C::DarkGray)]);
            }
            for (at, change) in changes.iter().enumerate() {
                let fix = fix_said(change.fix);
                let text = pad(&format!(" {}", change.text), inner.saturating_sub(fix.len() + 2));
                lines.push(if at == *index {
                    vec![on(format!("{text}  {fix}"), C::Black, C::Cyan)]
                } else {
                    vec![seg(text, C::White), seg(format!("  {fix}"), C::DarkCyan)]
                });
            }
        }
    }
    let title = icons::titled(ui.nerd, Icon::Branch, "What changed for running");
    boxed(&title, lines, FOOTER, cells, rows, inner)
}
