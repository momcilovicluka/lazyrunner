//! What lazyrunner says about itself outside its columns: the one-line summary (the panel header and the terminal
//! tab's title), the tab and taskbar progress (OSC 9;4: green, red, yellow in Windows Terminal), desktop
//! notifications, and opening links and files.

use lazyrunner_core::run::Controller;
use lazyrunner_core::run::app::Status;

use super::theme::C;

const SLOW_RESPONSE_MS: u64 = 1000;

/// The summary: what the header and the tab say, its colour, and the progress state for the tab.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Summary {
    pub text: String,
    pub color: C,
    /// OSC 9;4 state;progress: "1;100" green, "2;100" red, "4;100" yellow, "0;0" none.
    pub progress: &'static str,
}

pub fn summary(controller: &Controller) -> Summary {
    let running: Vec<usize> = (0..controller.apps.len()).filter(|&i| controller.states[i].is_running()).collect();
    let up = running.iter().filter(|&&i| controller.states[i].is_up()).count();
    let failed = controller.last_build_failed
        || controller.states.iter().any(|state| matches!(state.status, Status::Exited(_) | Status::Error(_)));
    let slow = running
        .iter()
        .filter(|&&i| {
            controller
                .last_probe(&controller.apps[i].key)
                .is_some_and(|probe| !probe.ok || probe.ms >= SLOW_RESPONSE_MS)
        })
        .count();
    let mut tools = Vec::new();
    if let Some(job) = &controller.building {
        tools.push(format!("building {}…", controller.app(&job.key).map_or("", |app| app.name.as_str())));
    }
    if controller.npm_ci.is_some() {
        tools.push("npm ci…".to_string());
    }
    let (progress, text, color) = if !tools.is_empty() {
        ("4;100", tools.join(" + "), C::Yellow)
    } else if failed {
        ("2;100", "something failed - see the log".into(), C::Red)
    } else if !running.is_empty() && up < running.len() {
        ("4;100", format!("starting ({up}/{} up)…", running.len()), C::Yellow)
    } else if slow > 0 {
        ("4;100", "up, but slow / not responding".into(), C::Yellow)
    } else if !running.is_empty() {
        ("1;100", format!("● all up ({up})"), C::Green)
    } else {
        ("0;0", "○ stopped".into(), C::DarkGray)
    };
    Summary { text, color, progress }
}

/// The escape sequences for the tab: its progress and its title ("lazyrunner · ShopLocal · ● all up (5)").
pub fn tab_sequence(controller: &Controller, summary: &Summary) -> String {
    let database = controller
        .db_label
        .as_ref()
        .map_or(String::new(), |label| format!("{} · ", label.split(" @ ").next().unwrap_or(label)));
    format!("\x1b]9;4;{}\x1b\\\x1b]0;lazyrunner · {database}{}\x07", summary.progress, summary.text)
}

/// Puts the tab back as it was: no progress.
pub const TAB_RESET: &str = "\x1b]9;4;0;0\x1b\\";

/// OSC 10/11 recolour the terminal for themes that set its colours; OSC 110/111 put the user's back.
pub fn terminal_colors(colors: Option<&(Option<String>, Option<String>)>, colored_before: bool) -> String {
    let mut sequence = if colored_before { "\x1b]110\x1b\\\x1b]111\x1b\\".to_string() } else { String::new() };
    if let Some((foreground, background)) = colors {
        if let Some(foreground) = foreground {
            sequence.push_str(&format!("\x1b]10;{foreground}\x1b\\"));
        }
        if let Some(background) = background {
            sequence.push_str(&format!("\x1b]11;{background}\x1b\\"));
        }
    }
    sequence
}

/// Shows a desktop notification in the background (never blocks, never fails loudly).
pub fn notify(title: &str, message: &str) {
    let (title, message) = (format!("lazyrunner · {title}"), message.to_string());
    std::thread::spawn(move || {
        let _ = notify_rust::Notification::new().summary(&title).body(&message).show();
    });
}

/// Opens a link or a file with what the OS uses for it.
pub fn open(target: &str) -> bool {
    let mut command = if cfg!(windows) {
        // Not `cmd /C start`: cmd would split a URL at & and expand %.
        let mut command = std::process::Command::new("rundll32");
        command.arg("url.dll,FileProtocolHandler").arg(target);
        command
    } else if cfg!(target_os = "macos") {
        let mut command = std::process::Command::new("open");
        command.arg(target);
        command
    } else {
        let mut command = std::process::Command::new("xdg-open");
        command.arg(target);
        command
    };
    command.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    match command.spawn() {
        Ok(mut child) => {
            std::thread::spawn(move || child.wait()); // no zombie
            true
        }
        Err(_) => false,
    }
}

/// The window title lazyrunner found, put back when it exits (it shows its status there meanwhile).
pub struct Title(#[cfg(windows)] Option<Vec<u16>>);

impl Title {
    #[cfg(windows)]
    pub fn save() -> Self {
        use windows_sys::Win32::System::Console::GetConsoleTitleW;
        let mut buffer = vec![0u16; 1024];
        // SAFETY: the buffer is valid for its length, which is what the call is told.
        let length = unsafe { GetConsoleTitleW(buffer.as_mut_ptr(), buffer.len() as u32) } as usize;
        Self((length > 0 && length < buffer.len()).then(|| {
            buffer.truncate(length);
            buffer.push(0);
            buffer
        }))
    }

    /// The terminal keeps it: xterm's title stack.
    #[cfg(not(windows))]
    pub fn save() -> Self {
        let _ = std::io::Write::write_all(&mut std::io::stdout(), b"\x1b[22;0t");
        Self()
    }

    #[cfg(windows)]
    pub fn restore(&self) {
        use windows_sys::Win32::System::Console::SetConsoleTitleW;
        if let Some(title) = &self.0 {
            // SAFETY: the title is NUL-terminated UTF-16 that lives through the call.
            unsafe { SetConsoleTitleW(title.as_ptr()) };
        }
    }

    #[cfg(not(windows))]
    pub fn restore(&self) {
        let _ = std::io::Write::write_all(&mut std::io::stdout(), b"\x1b[23;0t");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_colours_set_and_restore() {
        let colors = (Some("#c0caf5".to_string()), Some("#1a1b26".to_string()));
        assert_eq!(terminal_colors(Some(&colors), false), "\x1b]10;#c0caf5\x1b\\\x1b]11;#1a1b26\x1b\\");
        assert_eq!(terminal_colors(None, true), "\x1b]110\x1b\\\x1b]111\x1b\\");
    }
}
