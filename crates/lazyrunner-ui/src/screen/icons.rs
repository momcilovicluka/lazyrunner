//! The icons in lazyrunner's chrome - title bars, the status line, the views' headers - in two sets: symbols every
//! font has (the default), or Nerd Font icons (`, i`) for a terminal whose font is a Nerd Font, a "Mono" one, so each
//! icon takes one cell. An icon an app's profile gives it, and text written into the columns, stay as they are.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    /// An app without an icon of its own, by its kind.
    Dotnet,
    Npm,
    Branch,
    Database,
    Tests,
    Requests,
    Migrations,
    /// A column scrolled back.
    Pause,
    /// Memory growing.
    Growing,
}

/// The icon, plain or (`nerd`) the Nerd Font's.
pub fn icon(nerd: bool, icon: Icon) -> &'static str {
    match (icon, nerd) {
        (Icon::Dotnet | Icon::Npm, false) => "■",
        (Icon::Dotnet, true) => "\u{e77f}", // .NET
        (Icon::Npm, true) => "\u{e718}",    // Node.js
        (Icon::Branch, false) => "⎇",
        (Icon::Branch, true) => "\u{e725}", // git branch
        (Icon::Database, false) => "DB",
        (Icon::Database | Icon::Migrations, true) => "\u{f1c0}", // database
        (Icon::Tests, false) => "⚗",
        (Icon::Tests, true) => "\u{f0c3}", // flask
        (Icon::Requests, false) => "⇄",
        (Icon::Requests, true) => "\u{f0ec}", // exchange
        (Icon::Migrations, false) => "⛁",
        (Icon::Pause, false) => "⏸",
        (Icon::Pause, true) => "\u{f04c}", // pause
        (Icon::Growing, false) => "↗",
        (Icon::Growing, true) => "\u{f201}", // line chart
    }
}

#[cfg(test)]
mod tests {
    use unicode_width::UnicodeWidthStr;

    use super::*;

    #[test]
    fn every_nerd_icon_takes_one_cell() {
        let all = [
            Icon::Dotnet,
            Icon::Npm,
            Icon::Branch,
            Icon::Database,
            Icon::Tests,
            Icon::Requests,
            Icon::Migrations,
            Icon::Pause,
            Icon::Growing,
        ];
        for each in all {
            assert_eq!(icon(true, each).width(), 1, "{each:?}");
        }
    }
}
