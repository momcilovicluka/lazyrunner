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
    Readme,
    Changelog,
    Releases,
    License,
    Bug,
    Idea,
    Source,
    Update,
    Person,
    Heart,
    Rust,
    /// The command palette's groups.
    Run,
    Tools,
    View,
    Help,
    Settings,
    /// An app up, or all of them.
    Up,
    /// Built, watching for changes (no port to ask).
    Watching,
    Stopped,
    /// An app that exited or couldn't start.
    Exited,
    /// A column's warnings count.
    Warnings,
    /// A column's level filter.
    Filter,
    Search,
    /// A duration's trend.
    Slower,
    Faster,
    /// The boxes' titles, and what they hold.
    Terminal,
    Bookmark,
    Keyboard,
    Package,
    Clipboard,
    Copy,
    Hourglass,
    Brush,
    /// A project's own theme.
    Pin,
    Sun,
    Moon,
    Globe,
    JavaScript,
    Link,
    /// The startup checklist's checks.
    Git,
    Plug,
    Folder,
    /// The jump list in the panel.
    Back,
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
        (Icon::Readme, false) => "¶",
        (Icon::Readme, true) => "\u{f02d}", // book
        (Icon::Changelog, false) => "✦",
        (Icon::Changelog, true) => "\u{f1da}", // history
        (Icon::Releases, false) => "⇩",
        (Icon::Releases, true) => "\u{f02b}", // tag
        (Icon::License, false) => "§",
        (Icon::License, true) => "\u{f24e}", // balance scale
        (Icon::Bug, false) => "⚑",
        (Icon::Bug, true) => "\u{f188}", // bug
        (Icon::Idea, false) => "☼",
        (Icon::Idea, true) => "\u{f0eb}", // light bulb
        (Icon::Source, false) => "⑂",
        (Icon::Source, true) => "\u{f09b}", // GitHub
        (Icon::Update, false) => "⇣",
        (Icon::Update, true) => "\u{f019}", // download
        (Icon::Person, false) => "•",
        (Icon::Person, true) => "\u{f007}", // user
        (Icon::Heart, false) => "♥",
        (Icon::Heart, true) => "\u{f004}", // heart
        (Icon::Rust, false) => "⚙",
        (Icon::Rust, true) => "\u{e7a8}", // Rust
        (Icon::Run, false) => "▸",
        (Icon::Run, true) => "\u{f04b}", // play
        (Icon::Tools, false) => "✱",
        (Icon::Tools, true) => "\u{f0ad}", // wrench
        (Icon::View, false) => "◧",
        (Icon::View, true) => "\u{f06e}", // eye
        (Icon::Help, false) => "?",
        (Icon::Help, true) => "\u{f059}", // question
        (Icon::Settings, false) => "≡",
        (Icon::Settings, true) => "\u{f013}", // cog
        (Icon::Up, false) => "●",
        (Icon::Up, true) => "\u{f04b}", // play
        (Icon::Watching, false) => "●",
        (Icon::Watching, true) => "\u{f06e}", // eye
        (Icon::Stopped, false) => "○",
        (Icon::Stopped, true) => "\u{f04d}", // stop
        (Icon::Exited, false) => "✗",
        (Icon::Exited, true) => "\u{f057}", // times circle
        (Icon::Warnings, false) => "△",
        (Icon::Warnings, true) => "\u{f071}", // warning
        (Icon::Filter, false) => "⧩",
        (Icon::Filter, true) => "\u{f0b0}", // filter
        (Icon::Search, false) => "⌕",
        (Icon::Search, true) => "\u{f002}", // search
        (Icon::Slower, false) => "▲",
        (Icon::Slower, true) => "\u{f062}", // arrow up
        (Icon::Faster, false) => "▼",
        (Icon::Faster, true) => "\u{f063}", // arrow down
        (Icon::Terminal, false) => "$",
        (Icon::Terminal, true) => "\u{f120}", // terminal
        (Icon::Bookmark, false) => "▌",
        (Icon::Bookmark, true) => "\u{f02e}", // bookmark
        (Icon::Keyboard, false) => "?",
        (Icon::Keyboard, true) => "\u{f11c}", // keyboard
        (Icon::Package, false) => "▣",
        (Icon::Package, true) => "\u{f1b2}", // cube
        (Icon::Clipboard, false) => "⧉",
        (Icon::Clipboard, true) => "\u{f0ea}", // clipboard
        (Icon::Copy, false) => "⧉",
        (Icon::Copy, true) => "\u{f0c5}", // copy
        (Icon::Hourglass, false) => "⧗",
        (Icon::Hourglass, true) => "\u{f252}", // hourglass
        (Icon::Brush, false) => "◐",
        (Icon::Brush, true) => "\u{f1fc}", // paint brush
        (Icon::Pin, false) => "◆",
        (Icon::Pin, true) => "\u{f08d}", // thumb tack
        (Icon::Sun, false) => "○",
        (Icon::Sun, true) => "\u{f185}", // sun
        (Icon::Moon, false) => "●",
        (Icon::Moon, true) => "\u{f186}", // moon
        (Icon::Globe, false) => "@",
        (Icon::Globe, true) => "\u{f0ac}", // globe
        (Icon::JavaScript, false) => "J",
        (Icon::JavaScript, true) => "\u{e74e}", // JavaScript
        (Icon::Link, false) => "~",
        (Icon::Link, true) => "\u{f0c1}", // link
        (Icon::Git, false) => "±",
        (Icon::Git, true) => "\u{e702}", // git
        (Icon::Plug, false) => "#",
        (Icon::Plug, true) => "\u{f1e6}", // plug
        (Icon::Folder, false) => "/",
        (Icon::Folder, true) => "\u{f07b}", // folder
        (Icon::Back, false) => "↶",
        (Icon::Back, true) => "\u{f0e2}", // undo
    }
}

/// A box's title, after its icon with Nerd Font icons; plain, as it is.
pub fn titled(nerd: bool, icon: Icon, title: &str) -> String {
    if nerd { format!("{} {title}", self::icon(true, icon)) } else { title.to_string() }
}

/// A startup check's icon, by its name.
pub fn check(name: &str) -> Option<Icon> {
    match name {
        "lazyrunner" => Some(Icon::Rust),
        "git" => Some(Icon::Git),
        ".NET SDK" => Some(Icon::Dotnet),
        "Node.js / npm" => Some(Icon::Npm),
        "Branch" => Some(Icon::Branch),
        "Ports" => Some(Icon::Plug),
        "Database" => Some(Icon::Database),
        "Project" => Some(Icon::Folder),
        _ if name.ends_with(" packages") => Some(Icon::Package),
        _ => None,
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
            Icon::Readme,
            Icon::Changelog,
            Icon::Releases,
            Icon::License,
            Icon::Bug,
            Icon::Idea,
            Icon::Source,
            Icon::Update,
            Icon::Person,
            Icon::Heart,
            Icon::Rust,
            Icon::Run,
            Icon::Tools,
            Icon::View,
            Icon::Help,
            Icon::Settings,
            Icon::Up,
            Icon::Watching,
            Icon::Stopped,
            Icon::Exited,
            Icon::Warnings,
            Icon::Filter,
            Icon::Search,
            Icon::Slower,
            Icon::Faster,
            Icon::Terminal,
            Icon::Bookmark,
            Icon::Keyboard,
            Icon::Package,
            Icon::Clipboard,
            Icon::Copy,
            Icon::Hourglass,
            Icon::Brush,
            Icon::Pin,
            Icon::Sun,
            Icon::Moon,
            Icon::Globe,
            Icon::JavaScript,
            Icon::Link,
            Icon::Git,
            Icon::Plug,
            Icon::Folder,
            Icon::Back,
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
