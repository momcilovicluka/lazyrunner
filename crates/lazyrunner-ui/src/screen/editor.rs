//! `O` (#107): the place in code a view shows opens in your editor (`"editor"`) at its line - a stack's own frame, a
//! failed test's, a passing test's file at its method, an endpoint's controller, a project. A stack with more than
//! one place of the app's own asks which: its places listed, the app's own bright and the libraries' dim, the first
//! own one picked; `Enter` (or `O` again) opens it. `Ctrl+Enter` does what `O` does where the terminal tells it apart.
//!
//! It moves (not with reduced motion): the place opened lights up and a `↗` rises from it before the box closes.
//! With Nerd Font icons each place has its language's icon.

use std::path::{Path, PathBuf};
use std::time::Duration;

use lazyrunner_core::links::{self, Target as Link};
use lazyrunner_core::output::Hue;
use lazyrunner_core::run::Controller;
use lazyrunner_core::run::suites::Outcome;

use super::copy::{Pick, gather_app};
use super::icons::{self, Icon};
use super::mouse::Target;
use super::overlays::{box_inner, boxed};
use super::seg::{Seg, aim, on, pad, seg};
use super::theme::C;
use super::{Overlay, Ui};

/// The chooser's widest text, and what its footer says.
const INNER: usize = 100;
const FOOTER: &str = "j k choose · Enter or O opens it · Esc closes";
/// A route's words that name no controller (`/api/v2/orders`), what a controller's file ends with, a C# file's.
const NOT_CONTROLLERS: [&str; 1] = ["api"];
const CONTROLLER: &str = "Controller.cs";
const CSHARP: &str = ".cs";
/// How long the place opened shows before the box closes, and how many rows its `↗` rises.
const FLY: Duration = Duration::from_millis(450);
const FLY_ROWS: usize = 4;
const ARROW: &str = " ↗";
const ARROW_CELLS: usize = 2;

/// A place in code: a file, its line and column when known, whether it's the app's own (not a library's), and the
/// line it was found in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place {
    pub file: PathBuf,
    pub line: Option<u32>,
    pub column: Option<u32>,
    pub own: bool,
    pub text: String,
}

/// The places in code in `lines` (a stack, an error), each once, in order: found as the columns find links (`app`'s
/// own frame pattern), looked up in the repo (the app's folder first).
pub fn places(controller: &Controller, app: Option<usize>, lines: &[String]) -> Vec<Place> {
    let frames = app.and_then(|app| controller.states.get(app)).and_then(|state| state.frames());
    let mut found: Vec<Place> = Vec::new();
    for text in lines {
        for link in links::find_with(text, frames) {
            let Link::File { path, line, column } = link.target else { continue };
            let Some(file) = super::links::locate(controller, app, &path) else { continue };
            if found.iter().any(|place| place.file == file && place.line == line) {
                continue;
            }
            let own = !links::is_library(&file.to_string_lossy()) && !links::library_frame(text, frames);
            found.push(Place { file, line, column, own, text: text.trim().to_string() });
        }
    }
    found
}

/// Opens `places` (found in `what`, as the log says when there's none): the one place, or the one of the app's own,
/// straight away; else the chooser, over the view.
pub fn open(ui: &mut Ui, controller: &mut Controller, places: Vec<Place>, what: &str) {
    let Some(first) = places.first() else {
        controller.add_log(format!("No place in code in {what}"), Hue::DarkGray);
        return;
    };
    let own = places.iter().filter(|place| place.own).count();
    if places.len() == 1 || own == 1 {
        let place = places.iter().find(|place| place.own).unwrap_or(first).clone();
        go(ui, controller, &place);
        return;
    }
    let index = places.iter().position(|place| place.own).unwrap_or(0);
    if !matches!(ui.overlay, Some(Overlay::Places { .. })) {
        ui.behind = ui.overlay.take();
    }
    ui.overlay = Some(Overlay::Places { places, index, opening: None });
}

/// Whether the chooser moves: the place it opened lighting up, its `↗` rising.
pub fn animating(ui: &Ui) -> bool {
    matches!(ui.overlay, Some(Overlay::Places { opening: Some(_), .. }))
}

/// Closes the chooser once the place it opened has shown (each frame, before the view behind it comes back).
pub fn settle(ui: &mut Ui) {
    if let Some(Overlay::Places { opening: Some(at), .. }) = &ui.overlay
        && ui.now.saturating_duration_since(*at) >= FLY
    {
        ui.overlay = None;
    }
}

fn go(ui: &Ui, controller: &mut Controller, place: &Place) {
    super::links::open_file(ui, controller, &place.file, place.line, place.column);
}

/// The chooser's keys: j k choose, Enter or O opens the picked place, Esc closes; none while the place opened shows.
pub fn key(ui: &mut Ui, controller: &mut Controller, name: &str) {
    let moving = ui.motion.enabled && !ui.reduced_motion;
    let now = ui.now;
    let Some(Overlay::Places { places, index, opening }) = &mut ui.overlay else { return };
    if opening.is_some() {
        return;
    }
    match name {
        "j" | "DownArrow" => *index = super::keys::wrapped(*index, 1, places.len()),
        "k" | "UpArrow" => *index = super::keys::wrapped(*index, -1, places.len()),
        "Enter" | "O" => {
            let place = places.get(*index).cloned();
            if moving {
                *opening = Some(now);
            } else {
                ui.overlay = None;
            }
            if let Some(place) = place {
                go(ui, controller, &place);
            }
        }
        "Escape" | "q" => ui.overlay = None,
        _ => {}
    }
}

/// A place's language, by its file's extension, for its icon with Nerd Font icons.
fn language(place: &Place) -> Option<Icon> {
    let extension = place.file.extension()?.to_string_lossy().to_lowercase();
    Some(match extension.as_str() {
        "cs" | "fs" | "vb" | "cshtml" | "razor" => Icon::Dotnet,
        "ts" | "tsx" | "mts" | "cts" => Icon::Npm,
        "js" | "jsx" | "mjs" | "cjs" => Icon::JavaScript,
        "py" => Icon::Python,
        "go" => Icon::Go,
        "rs" => Icon::Rust,
        "java" | "kt" | "scala" => Icon::Java,
        _ => return None,
    })
}

/// The chooser: each place's file and line, then its folder in the repo - the app's own bright, the libraries' dim.
pub fn rows(ui: &Ui, controller: &Controller, cells: usize, rows: usize) -> Vec<Vec<Seg>> {
    let Some(Overlay::Places { places, index, opening }) = &ui.overlay else { return Vec::new() };
    let inner = box_inner(cells, INNER);
    let flying = opening.map(|at| ui.now.saturating_duration_since(at).as_secs_f64() / FLY.as_secs_f64());
    let mut lines: Vec<Vec<Seg>> = places
        .iter()
        .enumerate()
        .map(|(at, place)| {
            let name = place.file.file_name().map_or_else(String::new, |name| name.to_string_lossy().into_owned());
            let line = place.line.map_or_else(String::new, |line| format!(":{line}"));
            let folder = place.file.parent().map_or_else(String::new, |folder| {
                folder.strip_prefix(&controller.root).unwrap_or(folder).display().to_string().replace('\\', "/")
            });
            let icon = language(place)
                .filter(|_| ui.nerd)
                .map_or_else(String::new, |icon| format!("{} ", icons::icon(true, icon)));
            let label = format!(" {icon}{name}{line}");
            if at == *index && flying.is_some() {
                return aim(vec![on(pad(&format!("{label}  {folder}"), inner), C::Black, C::Green)], Target::Item(at));
            }
            if at == *index {
                return aim(
                    vec![on(pad(&format!("{label}  {folder}"), inner), C::FocusText, C::FocusBack)],
                    Target::Item(at),
                );
            }
            let color = if place.own { C::White } else { C::DarkGray };
            aim(vec![seg(label, color), seg(format!("  {folder}"), C::DarkGray)], Target::Item(at))
        })
        .collect();
    if let Some(flown) = flying.filter(|flown| *flown < 1.0) {
        let row = index.saturating_sub((flown * FLY_ROWS as f64) as usize).min(lines.len().saturating_sub(1));
        if let Some(line) = lines.get_mut(row) {
            let kept = super::columns::fit(std::mem::take(line), inner.saturating_sub(ARROW_CELLS));
            *line = [kept, vec![seg(ARROW, C::Cyan)]].concat();
        }
    }
    let own = places.iter().filter(|place| place.own).count();
    let title = format!("Open in the editor · {own} of {} the app's own", places.len());
    boxed(&icons::titled(ui.nerd, Icon::Pencil, &title), lines, FOOTER, cells, rows, inner)
}

/// O on a column: the focused app's last error, at its own place.
pub fn column(ui: &mut Ui, controller: &mut Controller) {
    app_error(ui, controller, ui.focus);
}

/// App `app`'s last error (a failed build's errors), at its own place.
pub fn app_error(ui: &mut Ui, controller: &mut Controller, app: usize) {
    let (lines, _) = gather_app(controller, app, Pick::LastError);
    let name = controller.apps.get(app).map_or_else(String::new, |app| app.name.clone());
    let found = places(controller, Some(app), &lines);
    open(ui, controller, found, &format!("{name}'s last error"));
}

/// O on a suite: its first failed test, as the tests view lists them.
pub fn suite(ui: &mut Ui, controller: &mut Controller, key: &str) {
    let failed = controller.suite(key).and_then(|suite| {
        let list = super::suites::test_list(suite);
        list.into_iter().find(|test| test.outcome == Outcome::Failed).map(|test| test.name.clone())
    });
    match failed {
        Some(name) => test(ui, controller, key, &name),
        None => controller.add_log("No failed test in that suite - l picks one, O opens it", Hue::DarkGray),
    }
}

/// O on a test: its failure's own place; else (it passed, or its stack names none) its file at its method - a .NET
/// test's, found by its class's name.
pub fn test(ui: &mut Ui, controller: &mut Controller, key: &str, name: &str) {
    let Some(suite) = controller.suite(key) else { return };
    let Some(found) = suite.tests.iter().find(|test| test.name == name).cloned() else { return };
    let app = suite.app.as_deref().and_then(|owner| controller.apps.iter().position(|app| app.key == owner));
    let lines: Vec<String> = found.message.lines().chain(found.stack.lines()).map(str::to_string).collect();
    let mut found_places = places(controller, app, &lines);
    if found_places.is_empty() {
        found_places.extend(test_file(&controller.root, name));
    }
    open(ui, controller, found_places, &format!("the test {name}"));
}

/// A .NET test's file and its method's line, from its full name (`Shop.Tests.CartTests.Totals(2, 3)`): `CartTests.cs`.
fn test_file(root: &Path, name: &str) -> Option<Place> {
    let name = name.split('(').next().unwrap_or(name);
    let mut parts = name.rsplit('.');
    let (method, class) = (parts.next()?, parts.next()?);
    let file = links::resolve(root, &format!("{class}{CSHARP}"))?;
    let line = line_of(&file, &format!("{method}(")).or_else(|| line_of(&file, method));
    Some(Place { file, line, column: None, own: true, text: name.to_string() })
}

/// O on an endpoint: its .NET controller, from its route - named for its first word (`/api/orders/{id}` →
/// `OrdersController.cs`, `/api/workflow-engine` → `WorkflowEngineController.cs`), at the first line naming its last
/// word; said when there's none.
pub fn handler(ui: &mut Ui, controller: &mut Controller, endpoint: &str) {
    match controller_of(&controller.root, endpoint) {
        Some(place) => go(ui, controller, &place),
        None => controller.add_log(
            format!("No controller found for {endpoint} - O looks for one named for its route"),
            Hue::DarkGray,
        ),
    }
}

fn controller_of(root: &Path, endpoint: &str) -> Option<Place> {
    let route = endpoint.split_whitespace().last()?;
    let words: Vec<&str> = route
        .split('/')
        .filter(|word| {
            let version = word.starts_with(['v', 'V'])
                && word.get(1..).is_some_and(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()));
            let skipped = NOT_CONTROLLERS.iter().any(|skip| word.eq_ignore_ascii_case(skip));
            !word.is_empty() && !word.starts_with('{') && !version && !skipped
        })
        .collect();
    let first = words.first()?;
    let file = links::resolve(root, &format!("{}{CONTROLLER}", first.replace(['-', '_'], "")))?;
    let line = words.last().filter(|last| *last != first).and_then(|last| line_of(&file, &format!("\"{last}")));
    Some(Place { file, line, column: None, own: true, text: endpoint.to_string() })
}

/// O on the code map: a failed project's build errors there (the first own one, or which), else its file or folder.
pub fn project(ui: &mut Ui, controller: &mut Controller, path: &Path) {
    let folder = if path.is_dir() { path } else { path.parent().unwrap_or(path) };
    let errors: Vec<String> = controller.failed_build_errors.values().flatten().cloned().collect();
    let here: Vec<Place> =
        places(controller, None, &errors).into_iter().filter(|place| place.file.starts_with(folder)).collect();
    if here.is_empty() {
        super::links::open_file(ui, controller, path, None, None);
        return;
    }
    open(ui, controller, here, "the project's build errors");
}

/// The first line of `file` that holds `wanted` (any case), counted from 1.
fn line_of(file: &Path, wanted: &str) -> Option<u32> {
    let text = lazyrunner_core::json::read_text(file).ok()?;
    let wanted = wanted.to_lowercase();
    let at = text.lines().position(|line| line.to_lowercase().contains(&wanted))?;
    u32::try_from(at + 1).ok()
}

#[cfg(test)]
mod tests {
    use lazyrunner_core::testing::{TempDir, write};

    use super::*;

    #[test]
    fn an_endpoint_finds_the_controller_named_for_its_route_at_its_last_word() {
        let temp = TempDir::new("editor-controller");
        let text = "namespace Shop;\npublic class WorkflowEngineController\n{\n    [HttpGet(\"steps/{id}\")]\n}\n";
        write(temp.join("src/Api/Controllers/WorkflowEngineController.cs"), text);
        let found = controller_of(temp.path(), "/api/v2/workflow-engine/steps/{id}").expect("its controller");
        assert!(found.file.ends_with("WorkflowEngineController.cs"));
        assert_eq!(found.line, Some(4), "the line that names its last word");
        assert_eq!(controller_of(temp.path(), "GET /api/workflow-engine").and_then(|place| place.line), None);
        assert!(controller_of(temp.path(), "/health").is_none(), "no such controller");
    }

    #[test]
    fn a_dotnet_tests_file_is_found_by_its_class_at_its_method() {
        let temp = TempDir::new("editor-test-file");
        write(temp.join("tests/CartTests.cs"), "public class CartTests\n{\n    public void Adds() { }\n}\n");
        let found = test_file(temp.path(), "Shop.Tests.CartTests.Adds(2, 3)").expect("its file");
        assert_eq!((found.file.ends_with("CartTests.cs"), found.line), (true, Some(3)));
        assert!(test_file(temp.path(), "cart adds two").is_none(), "a jest name has no class");
    }
}
