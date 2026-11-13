//! An app shared through a tunnel (#102): devtunnel or ngrok run beside it on its port, under lazyrunner's guard like
//! every process it starts. Its public URL is read from what the tool prints; the tunnel stops when it's stopped, when
//! the app has been down a little while (a restart keeps it), and with lazyrunner.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use super::Controller;
use crate::output::Hue;
use crate::run::Tag;
use crate::run::child::Child;
use crate::run::tunnel;

/// An app down this long takes its tunnel with it (a restart's gap doesn't).
const DOWN_GRACE: Duration = Duration::from_secs(10);
/// What the tool prints before its URL, at most this many lines in the log.
const MOST_LINES: usize = 6;

/// A tunnel to an app: the tool, its public URL once it printed it, whether anyone may open it, and its process.
pub struct Tunnel {
    pub tool: &'static str,
    pub url: Option<String>,
    pub anonymous: bool,
    job: u64,
    said: usize,
    down_since: Option<Instant>,
    _child: Child,
}

pub(super) type Tunnels = HashMap<String, Tunnel>;

impl Controller {
    /// App `key`'s tunnel, while it's shared.
    pub fn tunnel(&self, key: &str) -> Option<&Tunnel> {
        self.tunnels.get(key)
    }

    /// Shares app `key` through a tunnel (`anonymous`: anyone with the link may open it), or stops sharing it.
    pub fn share(&mut self, key: &str, anonymous: bool) {
        let name = self.app(key).map(|app| app.name.clone()).unwrap_or_default();
        if self.tunnels.remove(key).is_some() {
            self.add_log(format!("⇄ Stopped sharing {name}"), Hue::Cyan);
            return;
        }
        let Some(port) = self.port(key).filter(|_| self.is_running(key)) else {
            self.add_log(format!("{name} has to run, on a port, to be shared."), Hue::DarkGray);
            return;
        };
        let Some((tool, program)) = tunnel::tool(&self.config) else {
            self.add_log(format!("Sharing needs devtunnel or ngrok: {}", tunnel::INSTALL), Hue::Yellow);
            return;
        };
        self.next_job += 1;
        let job = self.next_job;
        let spec = tunnel::spec(tool, program, port, anonymous, &self.root);
        let tag = Tag::Tunnel { key: key.to_string(), job };
        match Child::spawn(&spec, tag, self.events.clone(), self.guard.as_ref()) {
            Ok(child) => {
                let who = if anonymous || tunnel::always_public(tool) { "anyone with the link" } else { "your login" };
                self.add_log(format!("⇄ Sharing {name} (:{port}) through {tool} - {who}…"), Hue::Cyan);
                let tunnel = Tunnel { tool, url: None, anonymous, job, said: 0, down_since: None, _child: child };
                self.tunnels.insert(key.to_string(), tunnel);
            }
            Err(error) => self.add_log(format!("✗ {tool} didn't start: {error}"), Hue::Red),
        }
    }

    /// A line the tool printed: its URL once it comes; until then what it says (a login it needs, say).
    pub(super) fn tunnel_output(&mut self, key: &str, job: u64, line: &str) {
        let name = self.app(key).map(|app| app.name.clone()).unwrap_or_default();
        let Some(tunnel) = self.tunnels.get_mut(key).filter(|tunnel| tunnel.job == job) else { return };
        if tunnel.url.is_some() {
            return;
        }
        if let Some(url) = tunnel::url_in(line) {
            tunnel.url = Some(url.clone());
            self.add_log(format!("⇄ {name} is shared at {url}"), Hue::Green);
            return;
        }
        let text = crate::output::strip_ansi(line).trim().to_string();
        if !text.is_empty() && tunnel.said < MOST_LINES {
            tunnel.said += 1;
            let tool = tunnel.tool;
            self.add_log(format!("⇄ {tool}: {text}"), Hue::DarkGray);
        }
    }

    /// The tool stopped by itself: the tunnel is gone.
    pub(super) fn tunnel_exited(&mut self, key: &str, job: u64, code: Option<i32>) {
        if !self.tunnels.get(key).is_some_and(|tunnel| tunnel.job == job) {
            return;
        }
        self.tunnels.remove(key);
        let name = self.app(key).map(|app| app.name.clone()).unwrap_or_default();
        let code = code.map_or_else(|| "?".to_string(), |code| code.to_string());
        self.add_log(format!("⇄ {name}'s tunnel stopped (code {code})"), Hue::Yellow);
    }

    /// An app that has been down a while stops being shared.
    pub(super) fn tick_tunnels(&mut self, now: Instant) {
        let running: Vec<String> = self.tunnels.keys().filter(|key| self.is_running(key)).cloned().collect();
        let mut gone = Vec::new();
        for (key, tunnel) in &mut self.tunnels {
            if running.contains(key) {
                tunnel.down_since = None;
            } else if now.saturating_duration_since(*tunnel.down_since.get_or_insert(now)) >= DOWN_GRACE {
                gone.push(key.clone());
            }
        }
        for key in gone {
            self.tunnels.remove(&key);
            let name = self.app(&key).map(|app| app.name.clone()).unwrap_or_default();
            self.add_log(format!("⇄ Stopped sharing {name}: it stopped"), Hue::Cyan);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::run::app::Status;
    use crate::testing::TempDir;

    #[tokio::test]
    async fn sharing_needs_the_app_running_on_its_port_and_a_tool_to_share_with() {
        let temp = TempDir::new("tunnels");
        let config = serde_json::json!({ "tunnelTool": "no-such-tool" });
        let (mut controller, _rx) = super::super::tests::controller(&temp, config);
        let key = controller.apps[0].key.clone();
        controller.share(&key, false);
        assert!(controller.log.back().unwrap().text.ends_with("has to run, on a port, to be shared."));
        controller.states[0].status = Status::Running;
        controller.share(&key, true);
        assert!(controller.log.back().unwrap().text.starts_with("Sharing needs devtunnel or ngrok"));
        assert!(controller.tunnel(&key).is_none());
    }
}
