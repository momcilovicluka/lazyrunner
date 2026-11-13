//! Sharing a running app through a tunnel (#102): `devtunnel` (Microsoft Dev Tunnels) or `ngrok`, whichever is
//! installed (`"tunnelTool"` picks one), run beside the app on its port; its public URL read from what it prints.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::run::child::Spec;
use crate::run::tools;

/// The tools, the first found used when the settings name none.
pub const TOOLS: [&str; 2] = [DEVTUNNEL, NGROK];
const DEVTUNNEL: &str = "devtunnel";
const NGROK: &str = "ngrok";
/// The setting that picks one.
pub const SETTING: &str = "tunnelTool";
/// How to get one.
pub const INSTALL: &str = "winget install Microsoft.devtunnel, then devtunnel user login - or ngrok \
     (winget install ngrok.ngrok, then ngrok config add-authtoken <token>)";
/// The hosts a tunnel's public URL is on.
const HOSTS: [&str; 4] = [".devtunnels.ms", ".ngrok-free.app", ".ngrok.app", ".ngrok.io"];
const SCHEME: &str = "https://";

/// The tool to share with, and where it is: the one the settings name, else the first installed.
pub fn tool(config: &Value) -> Option<(&'static str, PathBuf)> {
    let wanted = config[SETTING].as_str().filter(|name| !name.is_empty());
    TOOLS
        .into_iter()
        .filter(|name| wanted.is_none_or(|wanted| wanted.eq_ignore_ascii_case(name)))
        .find_map(|name| tools::find_on_path(name).map(|path| (name, path)))
}

/// How `tool` shares `port`: devtunnel with its own login required unless `anonymous`; ngrok public, its log on
/// stdout so the URL can be read.
pub fn spec(tool: &str, program: PathBuf, port: u16, anonymous: bool, dir: &Path) -> Spec {
    let port = port.to_string();
    let mut args: Vec<&str> =
        if tool == NGROK { vec!["http", &port, "--log", "stdout"] } else { vec!["host", "-p", &port] };
    if anonymous && tool == DEVTUNNEL {
        args.push("--allow-anonymous");
    }
    Spec { program, args: args.into_iter().map(Into::into).collect(), dir: dir.to_path_buf(), env: Vec::new() }
}

/// Whether `tool` always shares publicly (ngrok), so "anonymous" changes nothing.
pub fn always_public(tool: &str) -> bool {
    tool == NGROK
}

/// The tunnel's public URL in a line it printed, if there is one.
pub fn url_in(line: &str) -> Option<String> {
    let from = line.find(SCHEME)?;
    let url: String =
        line[from..].chars().take_while(|c| !c.is_whitespace() && !matches!(c, '"' | '\'' | ',')).collect();
    let host = url[SCHEME.len()..].split(['/', ':']).next().unwrap_or_default();
    HOSTS.iter().any(|known| host.ends_with(known)).then(|| url.trim_end_matches('/').to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_public_url_is_read_from_what_either_tool_prints() {
        let devtunnel = "Connect via browser: https://k3x9-47201.euw.devtunnels.ms, https://k3x9.euw.devtunnels.ms";
        assert_eq!(url_in(devtunnel).as_deref(), Some("https://k3x9-47201.euw.devtunnels.ms"));
        let ngrok = r#"t=2026-11-13 lvl=info msg="started tunnel" url=https://ab12-cd.ngrok-free.app"#;
        assert_eq!(url_in(ngrok).as_deref(), Some("https://ab12-cd.ngrok-free.app"));
        assert_eq!(url_in("see https://learn.microsoft.com/devtunnels for help"), None, "not a tunnel's host");
        let spec = spec(DEVTUNNEL, "devtunnel".into(), 47201, true, Path::new("."));
        assert_eq!(spec.args, ["host", "-p", "47201", "--allow-anonymous"]);
    }
}
