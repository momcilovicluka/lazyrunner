//! Runs an app as a child process and turns what happens to it into events: started, a line of output, exited.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Stdio;

use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc::Sender;
use tokio::task::JoinHandle;

const READ_BUFFER: usize = 64 * 1024;
/// Longer lines are split: one app printing a huge line (minified JSON, a progress bar without newlines) can't eat
/// memory.
pub const MAX_LINE: usize = 16 * 1024;

#[derive(Debug)]
pub enum AppEvent {
    Started { app: usize, pid: Option<u32> },
    Line { app: usize, text: String, stderr: bool },
    Exited { app: usize, code: Option<i32> },
    Failed { app: usize, error: String },
}

#[derive(Clone, Debug)]
pub struct Launch {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub dir: Option<PathBuf>,
}

/// A running app. Its events go through a bounded channel: when lazyrunner falls behind, the app's pipe fills and the
/// app waits, instead of lazyrunner's memory growing. Dropping it (or `stop`) kills the process; P2 replaces this with whole-tree kills.
pub struct AppProcess {
    task: JoinHandle<()>,
}

impl AppProcess {
    pub fn spawn(app: usize, launch: &Launch, events: Sender<AppEvent>) -> Self {
        let mut command = Command::new(&launch.program);
        command
            .args(&launch.args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(dir) = &launch.dir {
            command.current_dir(dir);
        }
        let task = tokio::spawn(async move {
            let mut child = match command.spawn() {
                Ok(child) => child,
                Err(error) => {
                    let _ = events.send(AppEvent::Failed { app, error: error.to_string() }).await;
                    return;
                }
            };
            let _ = events.send(AppEvent::Started { app, pid: child.id() }).await;
            let stdout = child.stdout.take().map(|pipe| tokio::spawn(forward(app, pipe, false, events.clone())));
            let stderr = child.stderr.take().map(|pipe| tokio::spawn(forward(app, pipe, true, events.clone())));
            let status = child.wait().await;
            // Let the readers drain what the app printed before it exited, so "exited" comes last.
            for reader in [stdout, stderr].into_iter().flatten() {
                let _ = reader.await;
            }
            let event = match status {
                Ok(status) => AppEvent::Exited { app, code: status.code() },
                Err(error) => AppEvent::Failed { app, error: error.to_string() },
            };
            let _ = events.send(event).await;
        });
        Self { task }
    }

    pub fn stop(&self) {
        self.task.abort();
    }
}

impl Drop for AppProcess {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn forward(app: usize, pipe: impl AsyncRead + Unpin, stderr: bool, events: Sender<AppEvent>) {
    let mut reader = BufReader::with_capacity(READ_BUFFER, pipe);
    let mut line = Vec::with_capacity(256);
    loop {
        let chunk = match reader.fill_buf().await {
            Ok([]) | Err(_) => break,
            Ok(chunk) => chunk,
        };
        let newline = chunk.iter().position(|&byte| byte == b'\n');
        let take = newline.map_or(chunk.len(), |at| at + 1).min(MAX_LINE - line.len());
        line.extend_from_slice(&chunk[..take]);
        reader.consume(take);
        let complete = newline.is_some_and(|at| at < take) || line.len() >= MAX_LINE;
        if complete && !send_line(app, &mut line, stderr, &events).await {
            return;
        }
    }
    if !line.is_empty() {
        send_line(app, &mut line, stderr, &events).await;
    }
}

async fn send_line(app: usize, line: &mut Vec<u8>, stderr: bool, events: &Sender<AppEvent>) -> bool {
    let text = String::from_utf8_lossy(line).into_owned();
    line.clear();
    events.send(AppEvent::Line { app, text, stderr }).await.is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    fn shell(script: &str) -> Launch {
        if cfg!(windows) {
            Launch { program: "cmd".into(), args: vec!["/C".into(), script.into()], dir: None }
        } else {
            Launch { program: "sh".into(), args: vec!["-c".into(), script.into()], dir: None }
        }
    }

    #[tokio::test]
    async fn reports_start_every_line_and_the_exit_code_last() {
        let (tx, mut rx) = mpsc::channel(64);
        let _process = AppProcess::spawn(3, &shell("echo one&& echo two&& exit 4"), tx);
        let mut lines = Vec::new();
        let mut started = false;
        while let Some(event) = rx.recv().await {
            match event {
                AppEvent::Started { app, pid } => started = app == 3 && pid.is_some(),
                AppEvent::Line { text, .. } => lines.push(text.trim().to_string()),
                AppEvent::Exited { code, .. } => {
                    assert_eq!(code, Some(4));
                    break;
                }
                AppEvent::Failed { error, .. } => panic!("{error}"),
            }
        }
        assert!(started);
        assert_eq!(lines, ["one", "two"]);
    }

    #[tokio::test]
    async fn huge_lines_are_split_and_a_last_line_without_newline_still_arrives() {
        use tokio::io::AsyncWriteExt;
        let (tx, mut rx) = mpsc::channel(64);
        let (mut writer, reader) = tokio::io::duplex(1024);
        let reading = tokio::spawn(forward(0, reader, false, tx));
        writer.write_all(&vec![b'x'; MAX_LINE + 10]).await.unwrap();
        writer.write_all(b"\ntail").await.unwrap();
        drop(writer);
        reading.await.unwrap();
        let mut lengths = Vec::new();
        while let Ok(AppEvent::Line { text, .. }) = rx.try_recv() {
            lengths.push(text.len());
        }
        assert_eq!(lengths, [MAX_LINE, 11, 4]);
    }

    #[tokio::test]
    async fn a_missing_program_is_a_failure_not_a_panic() {
        let (tx, mut rx) = mpsc::channel(64);
        let launch = Launch { program: "lazyrunner-no-such-program".into(), args: vec![], dir: None };
        let _process = AppProcess::spawn(0, &launch, tx);
        assert!(matches!(rx.recv().await, Some(AppEvent::Failed { app: 0, .. })));
    }
}
