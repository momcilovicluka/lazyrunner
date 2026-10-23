//! One list of every panel action. It drives the single-key shortcuts, the : command palette, the x menu and the
//! ? key reference, so they never drift apart. Keys follow vim and lazygit and are case-sensitive: lowercase acts on
//! the focused column, uppercase on every app. Safe actions also work while a build runs.

use lazyrunner_core::config::Toggle;
use lazyrunner_core::run::Controller;

use super::Ui;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Run {
    Toggle,
    StartFocused,
    StopFocused,
    RestartFocused,
    BuildFocused,
    StartAll,
    StopAll,
    RestartAll,
    BuildAll,
    KillPortUser,
    Quit,
    Database,
    NpmCi,
    NpmInstall,
    /// i: npm ci or npm install.
    Install,
    OpenFocused,
    OpenAll,
    OpenLog,
    Copy,
    Durations,
    Tests,
    Requests,
    Migrations,
    GitTool,
    Menu,
    Search,
    Zoom,
    Widen,
    Narrow,
    EqualWidths,
    Layout,
    Clear,
    Reload,
    Theme,
    Settings,
    Help,
    Palette,
    Setting {
        toggle: Toggle,
        label: &'static str,
    },
    Checklist,
    Changelog,
    About,
    ReloadProfile,
}

#[derive(Clone, Debug)]
pub struct Action {
    pub group: &'static str,
    pub key: String,
    pub alt: Vec<&'static str>,
    pub label: String,
    pub hint: String,
    pub run: Run,
    pub safe: bool,
    pub focused: bool,
}

fn action(group: &'static str, key: &str, label: &str, hint: &str, run: Run) -> Action {
    Action {
        group,
        key: key.into(),
        alt: Vec::new(),
        label: label.into(),
        hint: hint.into(),
        run,
        safe: false,
        focused: false,
    }
}

impl Action {
    fn safe(mut self) -> Self {
        self.safe = true;
        self
    }
    fn focused(mut self) -> Self {
        self.focused = true;
        self
    }
    fn alt(mut self, keys: &[&'static str]) -> Self {
        self.alt = keys.to_vec();
        self
    }
}

/// Every action shown now (some only when they apply).
pub fn actions(ui: &Ui, controller: &Controller) -> Vec<Action> {
    let installs = controller.apps.iter().any(|app| app.traits().installs);
    let mut list = vec![
        action("Run", "Space", "Start / stop the focused app", "whichever it is not", Run::Toggle).safe().focused(),
        action("Run", "u", "Start the focused app", "builds it first if it changed", Run::StartFocused)
            .safe()
            .focused(),
        action("Run", "s", "Stop the focused app", "or cancel its build / npm ci", Run::StopFocused).safe().focused(),
        action("Run", "r", "Restart the focused app", "rebuild if it changed", Run::RestartFocused).focused(),
        action("Run", "b", "Build the focused app", "full build, then it stays stopped", Run::BuildFocused).focused(),
        action("Run", "U", "Start all", "build if needed, start whatever is stopped", Run::StartAll),
        action("Run", "S", "Stop all", "also cancels a running build", Run::StopAll),
        action("Run", "R", "Restart all", "rebuild if anything changed", Run::RestartAll),
        action("Run", "B", "Build all", "full build of every app that builds, leaves them stopped", Run::BuildAll),
    ];
    if !controller.conflicts.is_empty() {
        list.push(action("Run", "!", "Kill whatever holds a needed port", "", Run::KillPortUser));
    }
    list.push(action("Run", "q", "Quit", "stop everything and exit", Run::Quit).alt(&["Q"]));
    list.push(action("Tools", "d", "Switch database", "connection strings in secrets.json", Run::Database).safe());
    if installs {
        list.push(
            action("Tools", "i", "Install packages: npm ci or npm install", "the focused npm app's", Run::Install)
                .safe(),
        );
        list.push(
            action("Tools", "i c", "npm ci", "a clean install of exactly what package-lock.json says", Run::NpmCi)
                .safe(),
        );
        list.push(
            action("Tools", "i n", "npm install", "adds and updates packages to match package.json", Run::NpmInstall)
                .safe(),
        );
    }
    list.extend([
        action("Tools", "w", "Open the focused app in the browser", "Swagger for an API", Run::OpenFocused)
            .safe()
            .focused(),
        action("Tools", "W", "Open everything in the browser", "every Swagger and npm app", Run::OpenAll).safe(),
        action("Tools", "o", "Open the focused app's log file", "its full output", Run::OpenLog).safe().focused(),
        action(
            "Tools",
            "y",
            "Copy (yank)",
            "the last error, every error or warning, the search match, the output",
            Run::Copy,
        )
        .safe()
        .focused(),
        action(
            "Tools",
            "D",
            "Durations",
            "build, start and npm ci times over the last runs: what got slower",
            Run::Durations,
        )
        .safe(),
        action(
            "Tools",
            "H",
            "Requests: what the .NET apps served",
            "newest or slowest, per endpoint; filter, copy, open",
            Run::Requests,
        )
        .safe(),
        action(
            "Tools",
            "M",
            "Migrations: what's applied to the database",
            "EF Core, per DbContext; apply or roll back",
            Run::Migrations,
        )
        .safe(),
        action(
            "Tools",
            "T",
            "Tests: run the project's test suites",
            "dotnet test, jest, vitest - several at once in the background",
            Run::Tests,
        )
        .safe(),
        action(
            "Tools",
            "L",
            &format!("{}: git without leaving lazyrunner", crate::git_tool::name(&controller.config)),
            if controller.setting(Toggle::GitBeside) {
                "beside it, in Windows Terminal"
            } else {
                "q comes back; the apps keep running"
            },
            Run::GitTool,
        )
        .safe(),
        action("Tools", "x", "Menu for the focused app", "", Run::Menu).safe(),
        action("View", "/", "Search the focused app's output", "n older match, N newer", Run::Search).safe().focused(),
        action("View", "z", "Zoom the focused column", "", Run::Zoom).safe().focused(),
        action("View", "+", "Widen the focused column", "taller when the apps are stacked", Run::Widen).safe(),
        action("View", "-", "Narrow the focused column", "shorter when the apps are stacked", Run::Narrow).safe(),
        action("View", "=", "Equal column widths", "", Run::EqualWidths).safe(),
        action(
            "View",
            "|",
            &format!("Layout: {} → next", super::columns::layout(&controller.config)),
            "auto (stacked when narrow) / columns side by side / stacked top to bottom",
            Run::Layout,
        )
        .safe(),
        action("View", "Ctrl+L", "Clear output", "", Run::Clear).safe(),
        action(
            "View",
            "Ctrl+R",
            "Reload window",
            "the settings and themes read again, the terminal set up again; the apps keep running",
            Run::Reload,
        )
        .safe(),
        action(
            "View",
            "t",
            &format!("Theme picker ({})", ui.theme.name),
            &format!("preview live, Enter keeps one - {} themes", ui.themes.len()),
            Run::Theme,
        )
        .safe(),
        action(
            "Help",
            ",",
            "Settings (on/off toggles, r checklist)",
            "auto-restart, auto-start, checklist…",
            Run::Settings,
        )
        .safe(),
        action("Help", "?", "Key reference", "", Run::Help).safe(),
        action("Help", ":", "Command palette", "also Ctrl+P", Run::Palette).safe().alt(&["Ctrl+P"]),
    ]);
    list
}

/// An on/off setting in the , menu: its letter there, and its label.
pub(crate) struct MenuToggle {
    pub letter: &'static str,
    pub toggle: Toggle,
    pub label: &'static str,
}

const MENU_TOGGLES: [MenuToggle; 11] = [
    MenuToggle { letter: "a", toggle: Toggle::AutoRestartOnBranchChange, label: "Auto-restart on branch change" },
    MenuToggle { letter: "u", toggle: Toggle::AutoStartOnOpen, label: "Auto-start when lazyrunner opens" },
    MenuToggle { letter: "s", toggle: Toggle::StopFrontendOnStop, label: "Stop/Restart all include the frontend" },
    MenuToggle { letter: "e", toggle: Toggle::RestartOnCodeChange, label: "Restart the APIs when code changes" },
    MenuToggle { letter: "w", toggle: Toggle::StartupChecklist, label: "Startup checklist when lazyrunner opens" },
    MenuToggle { letter: "v", toggle: Toggle::AutoUpdate, label: "Auto-update lazyrunner when it opens" },
    MenuToggle { letter: "m", toggle: Toggle::Mouse, label: "Mouse: click and scroll (off: select text)" },
    MenuToggle { letter: "f", toggle: Toggle::ReducedMotion, label: "Reduced motion: no blinking, spinners or fades" },
    MenuToggle {
        letter: "g",
        toggle: Toggle::GitBeside,
        label: "L opens the git tool beside lazyrunner (Windows Terminal)",
    },
    MenuToggle {
        letter: "i",
        toggle: Toggle::NerdIcons,
        label: "Nerd Font icons (the terminal's font a Nerd Font Mono)",
    },
    MenuToggle {
        letter: "o",
        toggle: Toggle::CheckMigrationsOnOpen,
        label: "Check the migrations against the database when M opens",
    },
];

/// The , menu's toggles that apply here (whether Stop/Restart all include the frontend only with one).
pub(crate) fn menu_toggles(controller: &Controller) -> impl Iterator<Item = &'static MenuToggle> {
    let frontend = controller.apps.iter().any(|app| app.traits().frontend);
    MENU_TOGGLES.iter().filter(move |item| frontend || item.toggle != Toggle::StopFrontendOnStop)
}

/// The on/off settings and the checklist, in the , menu.
pub fn settings(controller: &Controller) -> Vec<Action> {
    let mut list: Vec<Action> = menu_toggles(controller)
        .map(|item| {
            let check = if controller.setting(item.toggle) { "[x] " } else { "[ ] " };
            let run = Run::Setting { toggle: item.toggle, label: item.label };
            action("Settings", item.letter, &format!("{check}{}", item.label), "", run).safe()
        })
        .collect();
    list.extend([
        action("Settings", "r", "Run the startup checklist now (and check for an update)", "", Run::Checklist),
        action("Settings", "c", "What's new: the lazyrunner changelog", "", Run::Changelog).safe(),
        action("Settings", "A", "About lazyrunner: version, contributors, links", "", Run::About).safe(),
    ]);
    let changed = if controller.profile_changed { " (this branch changed it)" } else { "" };
    list.push(action("Settings", "p", &format!("Reload the project profile{changed}"), "", Run::ReloadProfile));
    list
}

/// The action a key runs.
pub fn find(ui: &Ui, controller: &Controller, name: &str) -> Option<Action> {
    actions(ui, controller).into_iter().find(|action| action.key == name || action.alt.contains(&name))
}

/// The x menu: the actions for the focused app (and npm ci and npm install for an npm app).
pub fn menu(ui: &Ui, controller: &Controller) -> Vec<Action> {
    let all = actions(ui, controller);
    let mut items: Vec<Action> = all.iter().filter(|action| action.focused).cloned().collect();
    let focused_installs = controller.apps.get(ui.focus).is_some_and(|app| app.traits().installs);
    if focused_installs {
        items.extend(all.iter().filter(|action| action.key == "i c" || action.key == "i n").cloned());
    }
    items
}

/// Fuzzy subsequence match: every typed character in order; consecutive and word-start hits score higher.
pub fn fuzzy_score(query: &str, text: &str) -> u32 {
    fuzzy_match(query, text).map_or(0, |(score, _)| score)
}

/// Where the typed characters matched in `text` (character positions), to light them; none when they don't.
pub fn fuzzy_positions(query: &str, text: &str) -> Vec<usize> {
    fuzzy_match(query, text).map_or_else(Vec::new, |(_, positions)| positions)
}

fn fuzzy_match(query: &str, text: &str) -> Option<(u32, Vec<usize>)> {
    let text: Vec<char> = text.to_lowercase().chars().collect();
    let mut score = 0;
    let mut from = 0;
    let mut positions = Vec::new();
    for c in query.to_lowercase().chars().filter(|c| *c != ' ') {
        let found = text[from.min(text.len())..].iter().position(|t| *t == c).map(|at| at + from)?;
        score += 1;
        if positions.last().is_some_and(|previous| found == previous + 1) {
            score += 3;
        }
        if found == 0 || matches!(text[found - 1], ' ' | '-' | '/' | '(' | '[' | '\\') {
            score += 2;
        }
        positions.push(found);
        from = found + 1;
    }
    Some((score, positions))
}

/// Every action plus the settings (as ", a" and so on), best matches first. What acts on the focused app names it
/// ("Start Orders").
pub fn palette(ui: &Ui, controller: &Controller, query: &str) -> Vec<Action> {
    let focused = controller.apps.get(ui.focus).map(|app| app.name.clone());
    let mut all: Vec<Action> = actions(ui, controller)
        .into_iter()
        .filter(|action| action.key != ":")
        .map(|mut action| {
            if let Some(name) = focused.as_deref().filter(|_| action.focused) {
                let owner = if name.ends_with('s') { format!("{name}'") } else { format!("{name}'s") };
                action.label = action
                    .label
                    .replace("the focused app's", &owner)
                    .replace("the focused app", name)
                    .replace("the focused column", name);
            }
            action
        })
        .collect();
    all.extend(settings(controller).into_iter().map(|mut setting| {
        setting.key = format!(", {}", setting.key);
        setting
    }));
    if query.trim().is_empty() {
        return all;
    }
    let mut scored: Vec<(u32, Action)> = all
        .into_iter()
        .filter_map(|action| {
            let label = fuzzy_score(query, &action.label);
            let score = if label > 0 {
                label + 10
            } else {
                fuzzy_score(query, &format!("{} {} {} {}", action.label, action.group, action.hint, action.key))
            };
            (score > 0).then_some((score, action))
        })
        .collect();
    scored.sort_by_key(|(score, _)| std::cmp::Reverse(*score)); // stable: ties keep the list's order
    scored.into_iter().map(|(_, action)| action).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_matching_prefers_word_starts_and_runs() {
        assert_eq!(fuzzy_score("xyz", "Stop all"), 0);
        assert!(fuzzy_score("sa", "Stop all") > 0);
        assert!(fuzzy_score("sto", "Stop all") > fuzzy_score("sto", "restore"));
        assert!(fuzzy_score("db", "Switch database") > 0);
    }

    #[test]
    fn the_matched_letters_are_where_the_score_found_them() {
        assert_eq!(fuzzy_positions("rso", "Restart Orders"), [0, 2, 8]);
        assert_eq!(fuzzy_positions("ORD", "Restart Orders"), [8, 9, 10], "ignoring case");
        assert!(fuzzy_positions("xyz", "Restart Orders").is_empty());
    }
}
