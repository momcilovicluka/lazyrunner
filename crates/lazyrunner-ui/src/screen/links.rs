//! A click on a link in a column: a URL opens in the browser, a place in a file in the editor (`"editor"`) at its
//! line.

use std::path::Path;
use std::process::{Command, Stdio};

use lazyrunner_core::links::{self, Target};
use lazyrunner_core::output::Hue;
use lazyrunner_core::run::Controller;

use super::Ui;

/// A page in the browser.
pub fn open_url(ui: &Ui, controller: &mut Controller, url: &str) {
    if (ui.opener)(url) {
        controller.add_log(format!("Opening {url}"), Hue::DarkGray);
    } else {
        controller.add_log(format!("✗ Couldn't open {url}"), Hue::Red);
    }
}

pub fn open(ui: &mut Ui, controller: &mut Controller, link: usize) {
    let Some(target) = ui.links.get(link).cloned() else { return };
    match target {
        Target::Url(url) => open_url(ui, controller, &url),
        Target::File { path, line, column } => {
            let Some(file) = links::resolve(&controller.root, &path) else {
                let root = controller.root.display().to_string();
                controller.add_log(format!("{path} isn't in {root} (outside build output and packages)"), Hue::Yellow);
                return;
            };
            let name = file.file_name().map_or(path.clone(), |name| name.to_string_lossy().into_owned());
            let shown = line.map_or(name.clone(), |line| format!("{name}:{line}"));
            let editor = controller.config["editor"].as_str().unwrap_or("auto").to_string();
            match links::editor_command(&editor, &file, line, column) {
                Some((program, args)) => {
                    let by = program.file_stem().map_or(String::new(), |stem| stem.to_string_lossy().into_owned());
                    let batch = program.extension().is_some_and(|extension| {
                        extension.eq_ignore_ascii_case("cmd") || extension.eq_ignore_ascii_case("bat")
                    });
                    if batch && file.to_string_lossy().contains(['%', '!']) {
                        // cmd.exe, which runs it, would read %…% and !…! in the name as variables.
                        controller
                            .add_log(format!("{name} has % or ! in its name - {by} can't open it safely"), Hue::Yellow);
                        return;
                    }
                    if (ui.launcher)(&program, &args) {
                        controller.add_log(format!("Opening {shown} in {by}"), Hue::DarkGray);
                    } else {
                        controller.add_log(format!("✗ {by} didn't start (\"editor\": \"{editor}\")"), Hue::Red);
                    }
                }
                None if (ui.opener)(&file.display().to_string()) => {
                    controller.add_log(format!("Opening {shown}"), Hue::DarkGray);
                }
                None => controller.add_log(format!("✗ Couldn't open {shown}"), Hue::Red),
            }
        }
    }
}

/// Starts a program without waiting for it (the editor); false when it doesn't start.
pub fn launch(program: &Path, args: &[String]) -> bool {
    Command::new(program).args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().is_ok()
}
