//! `L`: a git tool (lazygit, or `"gitTool"`) without leaving lazyrunner. It runs in this terminal while lazyrunner
//! steps aside, its apps running on, and lazyrunner comes back when it quits; or, with `"gitBeside"` in Windows
//! Terminal, in a pane beside lazyrunner.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};
use std::time::Duration;

use lazyrunner_core::config::Toggle;
use lazyrunner_core::profile::native;
use lazyrunner_core::run::tools::{find_on_path, git, running_in};
use serde_json::Value;

/// What `"gitTool"` is when the settings don't say.
pub const DEFAULT: &str = "lazygit";

/// How long a pane opened beside takes to show its tool: `L` again meanwhile doesn't open another.
pub const BESIDE_STARTING: Duration = Duration::from_secs(3);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitTool {
    /// What the log calls it: the program's name.
    pub name: String,
    pub program: PathBuf,
    pub args: Vec<String>,
}

/// Where `L` opens it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    /// In this terminal, lazyrunner aside until it quits.
    Here,
    /// In a Windows Terminal pane beside lazyrunner.
    Beside,
}

impl GitTool {
    /// The tool the settings name, found on PATH (or at its path); why not, when it can't be run.
    pub fn from_config(config: &Value) -> Result<Self, String> {
        let command =
            config["gitTool"].as_str().map(str::trim).filter(|command| !command.is_empty()).unwrap_or(DEFAULT);
        let (first, rest) = split_program(command);
        let path = Path::new(&first);
        let program = if path.is_file() {
            Some(path.to_path_buf())
        } else if path.components().count() > 1 {
            None
        } else {
            let bare = first.strip_suffix(".exe").or_else(|| first.strip_suffix(".EXE")).unwrap_or(&first);
            find_on_path(bare)
        };
        let name = path.file_stem().map_or(first.clone(), |stem| stem.to_string_lossy().into_owned());
        let words = rest.split_whitespace().map(str::to_string);
        let Some(program) = program else {
            let hint = if first == DEFAULT { " - https://github.com/jesseduffield/lazygit" } else { "" };
            return Err(format!(
                "{first} isn't installed (or not on PATH){hint}; \"gitTool\" in lazyrunner.config.json picks another"
            ));
        };
        Ok(Self { name, program, args: words.collect() })
    }

    /// Runs it in this terminal (the caller has handed the terminal over) until it quits.
    pub fn run(&self, root: &Path) -> std::io::Result<ExitStatus> {
        Command::new(&self.program).args(&self.args).current_dir(root).status()
    }

    /// Whether it already runs in the project: in its folder, or in its repo's top folder (lazygit moves there when
    /// the project is a subfolder) - in a pane beside lazyrunner, or anywhere else.
    pub fn is_open_in(&self, root: &Path) -> bool {
        let name = self.program.file_name().map_or(self.name.clone(), |name| name.to_string_lossy().into_owned());
        let top = git(root, &["rev-parse", "--show-toplevel"]).map(|top| PathBuf::from(native(&top)));
        let mut dirs = vec![root];
        dirs.extend(top.as_deref());
        running_in(&name, &dirs)
    }

    /// Opens it in a Windows Terminal pane beside this one; returns at once.
    pub fn open_beside(&self, root: &Path) -> Result<(), String> {
        let wt = find_on_path("wt").ok_or("Windows Terminal (wt) isn't on PATH")?;
        Command::new(wt)
            .args(self.split_pane_args(root))
            .spawn()
            .map(drop)
            .map_err(|error| format!("Windows Terminal didn't open a pane - {error}"))
    }

    /// `wt` in this window: a pane on the right, in the project's folder, running the tool.
    fn split_pane_args(&self, root: &Path) -> Vec<OsString> {
        let mut args: Vec<OsString> =
            ["-w", "0", "split-pane", "--vertical", "--size", "0.45", "-d"].map(OsString::from).to_vec();
        // wt reads `;` as "next command": escaped, it stays part of the path or argument.
        let escape = |text: &str| OsString::from(text.replace(';', "\\;"));
        args.push(escape(&root.to_string_lossy()));
        args.push(escape(&self.program.to_string_lossy()));
        args.extend(self.args.iter().map(|arg| escape(arg)));
        args
    }
}

/// The program and the rest of a command; a quoted program may have spaces (`"C:\Program Files\x\lazygit.exe"`).
fn split_program(command: &str) -> (String, &str) {
    if let Some(quoted) = command.strip_prefix('"')
        && let Some((program, rest)) = quoted.split_once('"')
    {
        return (program.to_string(), rest);
    }
    match command.split_once(char::is_whitespace) {
        Some((program, rest)) => (program.to_string(), rest),
        None => (command.to_string(), ""),
    }
}

/// What the settings call the tool (lazygit unless `"gitTool"` names another), without looking for it.
pub fn name(config: &Value) -> String {
    let command = config["gitTool"].as_str().and_then(|command| command.split_whitespace().next()).unwrap_or(DEFAULT);
    Path::new(command).file_stem().map_or_else(|| DEFAULT.into(), |stem| stem.to_string_lossy().into_owned())
}

/// Beside when `"gitBeside"` is on and lazyrunner runs in Windows Terminal; else here.
pub fn place(config: &Value, in_windows_terminal: bool) -> Place {
    if Toggle::GitBeside.read(config) && in_windows_terminal { Place::Beside } else { Place::Here }
}

/// Whether this terminal is Windows Terminal (it sets WT_SESSION for what runs in it).
pub fn in_windows_terminal() -> bool {
    std::env::var_os("WT_SESSION").is_some()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_tool_that_isnt_installed_says_how_to_get_one() {
        let why = GitTool::from_config(&json!({ "gitTool": "no-such-git-tool-here --flag" })).unwrap_err();
        assert!(why.starts_with("no-such-git-tool-here isn't installed"), "{why}");
        assert!(why.contains("\"gitTool\""), "{why}");
    }

    #[test]
    fn the_tool_is_found_on_path_with_its_arguments() {
        let shell = if cfg!(windows) { "cmd" } else { "sh" };
        let tool = GitTool::from_config(&json!({ "gitTool": format!("{shell} -x  --y") })).unwrap();
        assert_eq!(tool.name, shell);
        assert!(tool.program.is_file());
        assert_eq!(tool.args, ["-x", "--y"]);
    }

    #[test]
    fn beside_is_a_windows_terminal_pane_on_the_right_in_the_project_folder() {
        let tool = GitTool { name: "lazygit".into(), program: "lazygit".into(), args: vec!["--debug".into()] };
        let args: Vec<String> = tool
            .split_pane_args(Path::new("/work/shop"))
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            ["-w", "0", "split-pane", "--vertical", "--size", "0.45", "-d", "/work/shop", "lazygit", "--debug"]
        );
    }

    #[test]
    fn a_quoted_program_may_have_spaces_and_semicolons_are_kept_for_wt() {
        assert_eq!(
            split_program(r#""C:\Program Files\Git Tools\lazygit.exe" --debug"#),
            (r"C:\Program Files\Git Tools\lazygit.exe".to_string(), " --debug")
        );
        assert_eq!(split_program("lazygit"), ("lazygit".to_string(), ""));
        let tool = GitTool { name: "lazygit".into(), program: "lazygit".into(), args: Vec::new() };
        let args = tool.split_pane_args(Path::new("C:/work/a;b"));
        assert_eq!(args[7], OsString::from("C:/work/a\\;b"));
    }

    #[test]
    fn beside_only_in_windows_terminal() {
        let on = json!({ "gitBeside": true });
        assert_eq!(place(&on, true), Place::Beside);
        assert_eq!(place(&on, false), Place::Here);
        assert_eq!(place(&json!({ "gitBeside": false }), true), Place::Here);
    }
}
