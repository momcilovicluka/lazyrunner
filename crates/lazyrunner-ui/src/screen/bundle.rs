//! `, A z`: the diagnostics zip, ready to attach to a bug report - lazyrunner's and the tools' versions, the apps and
//! their states, the settings and the project profile with their secrets masked, each app's recent output and
//! lazyrunner's log (passwords in connection strings, a URL's user:password and bearer tokens masked), and the screen
//! as a picture in the theme's colours. Written in the background (the tools take a moment to say their versions)
//! into the project's state folder; the log says where.

use std::io::Write as _;
use std::path::Path;
use std::sync::LazyLock;

use lazyrunner_core::changelog::VERSION;
use lazyrunner_core::links;
use lazyrunner_core::output::Hue;
use lazyrunner_core::run::{Controller, checks};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use regex::Regex;
use serde_json::{Value, json};

use super::Ui;
use crate::svg;

const FOLDER: &str = "diagnostics";
/// The log's line for a zip written: these around its path.
pub const WRITTEN: &str = "Diagnostics zip: ";
pub const SHARE: &str = " - look over its logs before you share it";
/// An app's output lines in the report, the newest.
const OUTPUT_LINES: usize = 1000;
const MASK: &str = "***";
/// Settings and profile keys whose values are masked.
const SECRET_KEYS: [&str; 8] = ["password", "pwd", "secret", "token", "apikey", "api_key", "credential", "privatekey"];
/// The screen's size when the terminal doesn't say.
const SCREEN: (u16, u16) = (160, 45);

/// Passwords in connection strings, a URL's user:password, bearer tokens.
static SECRETS: LazyLock<[(Regex, &str); 3]> = LazyLock::new(|| {
    let pattern = |pattern: &str| Regex::new(pattern).expect("a valid pattern");
    [
        (pattern(r"(?i)\b((?:password|pwd)\s*=\s*)[^;'\x22\s]*"), "${1}***"),
        (pattern(r"(?i)(://[^:/\s@]+:)[^@\s/]+@"), "${1}***@"),
        (pattern(r"(?i)\b(bearer\s+)[a-z0-9._~+/=-]+"), "${1}***"),
    ]
});

/// A line with its secrets masked.
fn mask_text(text: &str) -> String {
    SECRETS.iter().fold(text.to_string(), |text, (pattern, with)| pattern.replace_all(&text, *with).into_owned())
}

/// Settings or a profile with their secrets masked: the values of secret-sounding keys, and secrets inside strings.
fn mask(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| {
                    let lower = key.to_lowercase();
                    let secret = SECRET_KEYS.iter().any(|word| lower.contains(word));
                    let masked =
                        if secret && !value.is_object() && !value.is_array() { json!(MASK) } else { mask(value) };
                    (key.clone(), masked)
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(mask).collect()),
        Value::String(text) => Value::String(mask_text(text)),
        other => other.clone(),
    }
}

fn pretty(value: &Value) -> Vec<u8> {
    serde_json::to_vec_pretty(value).unwrap_or_default()
}

/// lazyrunner, the project and its apps, the terminal; the tools' versions come after.
fn about(ui: &Ui, controller: &Controller, size: (u16, u16)) -> String {
    let mut lines = vec![
        format!("lazyrunner {VERSION} ({} {})", std::env::consts::OS, std::env::consts::ARCH),
        format!("project: {}  ·  branch: {}", controller.project.name, controller.branch),
        format!("database: {}", mask_text(controller.db_label.as_deref().unwrap_or("none"))),
        format!("terminal: {}x{}  ·  theme: {}  ·  nerd icons: {}", size.0, size.1, ui.theme.id, ui.nerd),
        String::new(),
        "apps:".to_string(),
    ];
    for (app, state) in controller.apps.iter().zip(&controller.states) {
        let kind = if app.traits().builds { "dotnet" } else { "npm" };
        let up = if state.is_up() { ", up" } else { "" };
        lines.push(format!(
            "  {} ({kind}): {}{up}, {} errors, {} warnings",
            app.name,
            state.status.name(),
            state.errors,
            state.warnings
        ));
    }
    lines.join("\n")
}

/// The screen as it is now, drawn once more as a picture.
fn screen(ui: &mut Ui, controller: &Controller, size: (u16, u16)) -> String {
    let Ok(mut terminal) = Terminal::new(TestBackend::new(size.0, size.1));
    if terminal.draw(|frame| super::draw(frame, ui, controller)).is_err() {
        return String::new();
    }
    let (fg, bg) = ui.theme.terminal.clone().unwrap_or((None, None));
    let title = format!("lazyrunner — {}", controller.project.name);
    let (fg, bg) = (fg.unwrap_or_else(|| svg::DEFAULT_FG.into()), bg.unwrap_or_else(|| svg::DEFAULT_BG.into()));
    svg::render(terminal.backend().buffer(), &title, &fg, &bg)
}

/// `, b`: gathers the report, then writes it in the background; the log says where it went.
pub fn write(ui: &mut Ui, controller: &mut Controller) {
    let size = crossterm::terminal::size().unwrap_or(SCREEN);
    let mut files: Vec<(String, Vec<u8>)> = vec![
        ("screen.svg".into(), screen(ui, controller, size).into_bytes()),
        ("settings.json".into(), pretty(&mask(&controller.config))),
    ];
    let profile = lazyrunner_core::json::read(&controller.project.path).unwrap_or(Value::Null);
    files.push(("profile.json".into(), pretty(&mask(&profile))));
    for (app, state) in controller.apps.iter().zip(&controller.states) {
        let mut lines: Vec<String> =
            state.output.newest_first(0).take(OUTPUT_LINES).map(|line| mask_text(&line.text)).collect();
        lines.reverse();
        files.push((format!("output/{}.log", app.key), (lines.join("\n") + "\n").into_bytes()));
    }
    let log: Vec<String> =
        controller.log.iter().map(|entry| format!("{} {}", entry.time, mask_text(&entry.text))).collect();
    files.push(("lazyrunner.log".into(), (log.join("\n") + "\n").into_bytes()));
    let about = about(ui, controller, size);
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let path = controller.state_folder().join(FOLDER).join(format!("lazyrunner-diagnostics-{stamp}.zip"));
    let (send, receive) = std::sync::mpsc::channel();
    ui.replays.push(receive);
    controller.add_log("Writing the diagnostics zip…", Hue::DarkGray);
    std::thread::spawn(move || {
        let tools: Vec<String> = checks::tool_versions()
            .into_iter()
            .map(|(tool, version)| format!("  {tool}: {}", version.unwrap_or_else(|| "not found".into())))
            .collect();
        files.insert(0, ("about.txt".into(), format!("{about}\n\ntools:\n{}\n", tools.join("\n")).into_bytes()));
        let said = match zip(&path, &files) {
            Ok(()) => (Hue::Green, format!("{WRITTEN}{}{SHARE}", path.display())),
            Err(error) => (Hue::Red, format!("✗ Couldn't write the diagnostics zip: {error}")),
        };
        let _ = send.send(said);
    });
}

/// The zip a line of the log says was written.
pub fn written(text: &str) -> Option<&str> {
    text.strip_prefix(WRITTEN)?.strip_suffix(SHARE)
}

/// A click on the zip's path in the log (`link` in `Ui::links`): the zip in the file manager.
pub fn reveal(ui: &Ui, controller: &mut Controller, link: usize) {
    let Some(links::Target::File { path, .. }) = ui.links.get(link) else { return };
    let zip = std::path::PathBuf::from(path);
    let name = zip.file_name().map_or(String::new(), |name| name.to_string_lossy().into_owned());
    if !zip.exists() {
        controller.add_log(format!("{name} isn't there any more"), Hue::Yellow);
    } else if (ui.revealer)(&zip) {
        controller.add_log(format!("Showing {name} in its folder"), Hue::DarkGray);
    } else {
        controller.add_log(format!("✗ Couldn't show {name} in its folder"), Hue::Red);
    }
}

fn zip(path: &Path, files: &[(String, Vec<u8>)]) -> std::io::Result<()> {
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder)?;
    }
    let mut zip = zip::ZipWriter::new(std::fs::File::create(path)?);
    let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for (name, bytes) in files {
        zip.start_file(name.as_str(), options).map_err(std::io::Error::other)?;
        zip.write_all(bytes)?;
    }
    zip.finish().map_err(std::io::Error::other)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_are_masked_in_settings_profiles_and_lines() {
        let line =
            "Server=sql;Database=Shop;User Id=sa;Password=hunter2;  https://bob:pa55@feed.example/  Bearer abc.def";
        assert_eq!(
            mask_text(line),
            "Server=sql;Database=Shop;User Id=sa;Password=***;  https://bob:***@feed.example/  Bearer ***"
        );
        let settings = json!({ "githubToken": "ghx", "editor": "code", "env": { "API_KEY": "k", "Db": "Pwd=x;" } });
        assert_eq!(
            mask(&settings),
            json!({ "githubToken": "***", "editor": "code", "env": { "API_KEY": "***", "Db": "Pwd=***;" } })
        );
    }
}
