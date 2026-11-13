//! `Alt+g`, or a click on the branch in the status line: what changed for running on the last branch switch (#104) -
//! the profile, an app's settings (which keys), launch settings and project file, an npm app's packages, migrations,
//! `.env` files. `j` `k` pick one, `Enter` runs what fixes it (npm ci, the migrations view, reloading the profile) and
//! ticks it off once it's done, `d` shows a settings file's keys that changed (never their values).

use std::time::{Duration, Instant};

use lazyrunner_core::run::Controller;
use lazyrunner_core::run::branch_changes::{Change, Fix, What};

use super::actions::Run;
use super::icons::{self, Icon};
use super::keys;
use super::overlays::boxed;
use super::seg::{Seg, on, pad, seg};
use super::theme::C;
use super::{Overlay, Ui};

const INNER: usize = 96;
const FOOTER: &str = "j k pick · Enter runs its fix · d its keys · Esc closes";
/// What a changed key's value shows as: never the value itself.
const MASKED: &str = "•••";
/// The status line's chip slides in over this long, then glows amber this long.
const SLIDE_IN: Duration = Duration::from_millis(300);
const GLOW: Duration = Duration::from_secs(1);

pub fn open(ui: &mut Ui) {
    ui.overlay = Some(Overlay::BranchChanges { index: 0, open: false });
}

fn changes(controller: &Controller) -> &[Change] {
    controller.branch_changes.as_ref().map_or(&[], |(_, _, changes)| changes.as_slice())
}

pub fn key(ui: &mut Ui, controller: &mut Controller, name: &str) {
    let Some(Overlay::BranchChanges { index, open }) = &mut ui.overlay else { return };
    let count = changes(controller).len();
    match name {
        "Escape" | "q" | "Alt+g" => ui.overlay = None,
        "j" | "DownArrow" => (*index, *open) = (keys::wrapped(*index, 1, count.max(1)), false),
        "k" | "UpArrow" => (*index, *open) = (keys::wrapped(*index, -1, count.max(1)), false),
        "d" => *open = !*open,
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
    if change.fix.is_some() {
        ui.branch_fixes.insert(change.text.clone());
    }
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

/// Whether `change` is fixed: its fix ran, and what it fixes says so - the packages installed, no migration pending.
fn fixed(ui: &Ui, controller: &Controller, change: &Change) -> bool {
    if !ui.branch_fixes.contains(&change.text) {
        return false;
    }
    match change.fix {
        Some(Fix::NpmCi) => change.app.as_ref().is_some_and(|app| {
            controller.npm_ci.is_none()
                && !controller.lock_changed_keys().contains(app)
                && controller.lock_in_sync(app) != Some(false)
        }),
        Some(Fix::Migrations) => super::migrations_rail::pending(controller).is_none(),
        Some(Fix::ReloadProfile) => true,
        None => false,
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

/// Each kind's icon and colour.
fn kind_of(what: What) -> (&'static str, C) {
    match what {
        What::Profile => ("◎", C::Magenta),
        What::Settings => ("≡", C::Cyan),
        What::Launch => ("▷", C::Blue),
        What::Project => ("◇", C::Blue),
        What::Packages => ("⬢", C::Green),
        What::Migrations => ("⇅", C::Yellow),
        What::Env => ("$", C::Yellow),
    }
}

/// A settings change's keys, as `d` shows them: `+ Logging:LogLevel:Default •••`.
fn keys_of(change: &Change, inner: usize) -> Vec<Vec<Seg>> {
    if change.keys.is_empty() {
        return vec![vec![seg("     no keys to show for this one", C::DarkGray)]];
    }
    let room = inner.saturating_sub(MASKED.chars().count() + 2);
    change
        .keys
        .iter()
        .map(|(sign, key)| {
            let color = match sign {
                '+' => C::Green,
                '-' => C::Red,
                _ => C::Yellow,
            };
            vec![seg(pad(&format!("     {sign} {key}"), room), color), seg(format!("  {MASKED}"), C::DarkGray)]
        })
        .collect()
}

pub fn rows(ui: &Ui, controller: &Controller, cells: usize, rows: usize) -> Vec<Vec<Seg>> {
    let Some(Overlay::BranchChanges { index, open }) = &ui.overlay else { return Vec::new() };
    let inner = cells.saturating_sub(8).clamp(20, INNER);
    let mut lines = Vec::new();
    match &controller.branch_changes {
        None => lines.push(vec![seg(" No branch switch yet this session.", C::DarkGray)]),
        Some((from, to, changes)) => {
            let done = changes.iter().filter(|change| fixed(ui, controller, change)).count();
            let mut head = vec![seg(format!(" {from} → {to}"), C::Magenta)];
            if done > 0 {
                head.push(seg(format!("   ✓ {done} of {} fixed", changes.len()), C::Green));
            }
            lines.extend([head, Vec::new()]);
            if changes.is_empty() {
                lines.push(vec![seg(" Nothing that changes how the apps run - only their code.", C::DarkGray)]);
            }
            for (at, change) in changes.iter().enumerate() {
                let is_fixed = fixed(ui, controller, change);
                let (icon, color) = if is_fixed { ("✓", C::Green) } else { kind_of(change.what) };
                let fix = if is_fixed { "fixed" } else { fix_said(change.fix) };
                let text = pad(&format!(" {}", change.text), inner.saturating_sub(fix.len() + 5));
                lines.push(if at == *index {
                    vec![on(format!(" {icon} {text}  {fix}"), C::Black, C::Cyan)]
                } else {
                    vec![
                        seg(format!(" {icon} "), color),
                        seg(text, if is_fixed { C::DarkGray } else { C::White }),
                        seg(format!("  {fix}"), if is_fixed { C::Green } else { C::DarkCyan }),
                    ]
                });
                if at == *index && *open {
                    lines.extend(keys_of(change, inner));
                }
            }
        }
    }
    let title = icons::titled(ui.nerd, Icon::Branch, "What changed for running");
    boxed(&title, lines, FOOTER, cells, rows, inner)
}

/// The status line's `· 2 changed` after the branch: it slides in as a switch brings changes and glows amber a
/// moment; once each is fixed it says so in green.
pub fn chip(ui: &Ui, controller: &Controller) -> Option<Seg> {
    let (_, _, changes) = controller.branch_changes.as_ref()?;
    if changes.is_empty() {
        return None;
    }
    let left = changes.iter().filter(|change| !fixed(ui, controller, change)).count();
    let text = if left == 0 { " · ✓ changes fixed".to_string() } else { format!(" · {left} changed") };
    let since = ui.branch_chip.as_ref().map_or(SLIDE_IN + GLOW, |(_, at)| ui.now.saturating_duration_since(*at));
    if ui.reduced_motion || since >= SLIDE_IN + GLOW {
        return Some(seg(text, if left == 0 { C::Green } else { C::Yellow }));
    }
    if since >= SLIDE_IN {
        return Some(on(text, C::Black, C::Yellow));
    }
    let count = text.chars().count();
    let shown = ((since.as_secs_f64() / SLIDE_IN.as_secs_f64() * count as f64).ceil() as usize).max(1);
    Some(on(text.chars().take(shown).collect::<String>(), C::Black, C::Yellow))
}

/// Each tick: a switch that brought changes starts the chip sliding in.
pub fn tick(ui: &mut Ui, controller: &Controller) {
    let Some((_, to, changes)) = controller.branch_changes.as_ref().filter(|(_, _, changes)| !changes.is_empty())
    else {
        return;
    };
    if ui.branch_chip.as_ref().is_none_or(|(branch, _)| branch != to) {
        ui.branch_chip = Some((to.clone(), ui.now));
        ui.branch_fixes.retain(|text| changes.iter().any(|change| change.text == *text));
    }
}

/// Whether the chip is sliding in or glowing.
pub fn animating(ui: &Ui) -> bool {
    let moving = |at: &Instant| ui.now.saturating_duration_since(*at) < SLIDE_IN + GLOW;
    !ui.reduced_motion && ui.branch_chip.as_ref().is_some_and(|(_, at)| moving(at))
}
