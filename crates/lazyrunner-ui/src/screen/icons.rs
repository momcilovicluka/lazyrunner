//! The icons in lazyrunner's chrome - title bars, the status line, the views' headers - in two sets: symbols every
//! font has (the default), or Nerd Font icons (`, i`) for a terminal whose font is a Nerd Font, a "Mono" one, so each
//! icon takes one cell. With them an app's title shows its `"nerdIcon"`, else .NET's or its framework's (Angular,
//! React, Vue) else Node.js's; text written into the columns and the log stays as it is.

use lazyrunner_core::apptypes::Framework;
use lazyrunner_core::profile::App;

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
    Ok,
    Fail,
    Warn,
    Skip,
    Queued,
    Cancelled,
    Reload,
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
        (Icon::Ok, false) => "✓",
        (Icon::Ok, true) => "\u{f00c}", // check
        (Icon::Fail, false) => "✗",
        (Icon::Fail, true) => "\u{f00d}", // times
        (Icon::Warn, false) => "!",
        (Icon::Warn, true) => "\u{f071}", // warning
        (Icon::Skip, false) => "–",
        (Icon::Skip, true) => "\u{f068}", // minus
        (Icon::Queued, false) => "◌",
        (Icon::Queued, true) => "\u{f017}", // clock
        (Icon::Cancelled, false) => "■",
        (Icon::Cancelled, true) => "\u{f04d}", // stop
        (Icon::Reload, false) => "↻",
        (Icon::Reload, true) => "\u{f021}", // refresh
    }
}

/// An app's icon in its title bar: its profile's, else one for its kind - with Nerd Font icons .NET's, or its
/// framework's (`framework`) else Node.js's.
pub fn app(nerd: bool, app: &App, framework: Option<Framework>) -> &str {
    let builds = app.traits().builds;
    if !nerd {
        return app.icon.as_deref().unwrap_or(icon(false, if builds { Icon::Dotnet } else { Icon::Npm }));
    }
    let kind = match framework {
        _ if builds => icon(true, Icon::Dotnet),
        Some(Framework::Angular) => "\u{e753}",
        Some(Framework::React) => "\u{e7ba}",
        Some(Framework::Vue) => "\u{e6a0}",
        None => icon(true, Icon::Npm),
    };
    app.nerd_icon.as_deref().unwrap_or(kind)
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
            Icon::Ok,
            Icon::Fail,
            Icon::Warn,
            Icon::Skip,
            Icon::Queued,
            Icon::Cancelled,
            Icon::Reload,
        ];
        for each in all {
            assert_eq!(icon(true, each).width(), 1, "{each:?}");
            assert_eq!(icon(false, each).width(), if each == Icon::Database { 2 } else { 1 }, "{each:?}");
        }
        for framework in ["\u{e753}", "\u{e7ba}", "\u{e6a0}"] {
            assert_eq!(framework.width(), 1);
        }
    }
}
