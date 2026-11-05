//! What every full-screen view starts with: its header - the view's name in a chip, the same in every view - and, on
//! the row under it, its keys, which scroll sideways when they don't fit (cut with … with reduced motion), so no key is
//! ever lost off the edge.

use super::Ui;
use super::icons::{self, Icon};
use super::seg::{Seg, on, seg};
use super::theme::C;

/// A view's name in its header's chip.
pub fn chip(nerd: bool, icon: Icon, name: &str) -> Seg {
    on(format!(" {} {} ", icons::icon(nerd, icon), name.to_uppercase()), C::Black, C::Cyan)
}

/// A view's keys in exactly `cells`, scrolling sideways when they don't fit.
pub fn keys(ui: &Ui, text: &str, cells: usize) -> Vec<Seg> {
    vec![seg(ui.scrolling(text, cells), C::DarkGray)]
}
