//! Child processes as whole trees: an app's server (dotnet run and the API it starts, npm and node, ...) or a build,
//! with its output turned into events, stopped all at once, and never outliving lazyrunner.
//!
//! - Windows: each child runs in a job object that kills everything in it when it is closed, which happens when
//!   lazyrunner stops it, quits, or dies however it dies.
//! - Linux and macOS: each child leads a process group of its own; stopping it kills the group. A small guard
//!   process (`lazyrunner __guard`) knows the groups and kills them when lazyrunner goes away without doing so.

use std::ffi::OsString;
use std::io;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::sync::mpsc::UnboundedSender;
use tokio::task::JoinHandle;

use super::{Event, Tag};

const READ_BUFFER: usize = 64 * 1024;
/// Longer lines are split, so one app can't eat memory with a line that never ends.
pub const MAX_LINE: usize = 16 * 1024;
/// How long output may keep coming after the process itself exited (a grandchild can hold the pipe open, as
/// MSBuild's worker nodes do).
const DRAIN_AFTER_EXIT: Duration = Duration::from_secs(2);

/// What to run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Spec {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub dir: PathBuf,
}

/// A running process tree. Dropping it kills the tree.
pub struct Child {
    pub pid: u32,
    tree: Tree,
    task: JoinHandle<()>,
}

impl Child {
    /// Starts `spec`; its output and its exit arrive as events tagged `tag`. Needs a tokio runtime.
    pub fn spawn(
        spec: &Spec,
        tag: Tag,
        events: UnboundedSender<Event>,
        guard: Option<&Arc<Guard>>,
    ) -> io::Result<Self> {
        let mut command = tokio::process::Command::new(&spec.program);
        command
            .args(&spec.args)
            .current_dir(&spec.dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            // Its own hidden console: lazyrunner's keys and Ctrl+C never reach it.
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        #[cfg(unix)]
        command.process_group(0);
        let mut child = command.spawn()?;
        let pid = child.id().unwrap_or_default();
        let tree = match Tree::adopt(&child, guard) {
            Ok(tree) => tree,
            Err(error) => {
                let _ = child.start_kill(); // without its tree it would be out of reach: better not run at all
                return Err(error);
            }
        };
        let stdout = child.stdout.take().map(|pipe| tokio::spawn(forward(pipe, tag.clone(), events.clone())));
        let stderr = child.stderr.take().map(|pipe| tokio::spawn(forward(pipe, tag.clone(), events.clone())));
        let task = tokio::spawn(async move {
            let status = child.wait().await;
            for reader in [stdout, stderr].into_iter().flatten() {
                let _ = tokio::time::timeout(DRAIN_AFTER_EXIT, reader).await;
            }
            let code = status.ok().and_then(|status| status.code());
            let _ = events.send(Event::Exited { tag, code });
        });
        Ok(Self { pid, tree, task })
    }

    /// Kills the whole tree now.
    pub fn kill(&self) {
        self.tree.kill();
    }

    /// Whether the process (not necessarily its children) has exited.
    pub fn has_exited(&self) -> bool {
        self.task.is_finished()
    }
}

impl Drop for Child {
    fn drop(&mut self) {
        self.tree.kill();
    }
}

async fn forward(pipe: impl AsyncRead + Unpin, tag: Tag, events: UnboundedSender<Event>) {
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
        if complete && !send(&tag, &mut line, &events) {
            return;
        }
    }
    if !line.is_empty() {
        send(&tag, &mut line, &events);
    }
}

fn send(tag: &Tag, line: &mut Vec<u8>, events: &UnboundedSender<Event>) -> bool {
    let text = String::from_utf8_lossy(line).trim_end_matches(['\r', '\n']).to_string();
    line.clear();
    events.send(Event::Output { tag: tag.clone(), line: text }).is_ok()
}

// ---------------------------------------------------------------- Windows: a job object per tree

#[cfg(windows)]
struct Tree(windows_sys::Win32::Foundation::HANDLE);

// SAFETY: a job handle is an opaque kernel handle; using it from another thread is fine.
#[cfg(windows)]
unsafe impl Send for Tree {}
#[cfg(windows)]
unsafe impl Sync for Tree {}

#[cfg(windows)]
impl Tree {
    fn adopt(child: &tokio::process::Child, _guard: Option<&Arc<Guard>>) -> io::Result<Self> {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation, SetInformationJobObject,
        };
        let process = child.raw_handle().ok_or_else(|| io::Error::other("the process has no handle"))?;
        // SAFETY: plain Win32 calls on handles this function owns (the job) or borrows for the call (the
        // process); the limits struct lives on the stack for the call and its size is passed with it.
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return Err(io::Error::last_os_error());
            }
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let size = std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32;
            let set = SetInformationJobObject(job, JobObjectExtendedLimitInformation, (&raw const limits).cast(), size);
            if set == 0 || AssignProcessToJobObject(job, process.cast()) == 0 {
                let error = io::Error::last_os_error();
                CloseHandle(job);
                return Err(error);
            }
            Ok(Self(job))
        }
    }

    fn kill(&self) {
        // SAFETY: the job handle is open until drop.
        unsafe {
            windows_sys::Win32::System::JobObjects::TerminateJobObject(self.0, 1);
        }
    }
}

#[cfg(windows)]
impl Drop for Tree {
    fn drop(&mut self) {
        // SAFETY: closes the handle this Tree owns, once; closing it also kills what is left in the job.
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

/// On Windows the job objects need no helper.
#[cfg(windows)]
pub struct Guard;

#[cfg(windows)]
impl Guard {
    pub fn start(_program: &std::path::Path) -> io::Result<Self> {
        Ok(Self)
    }
}

// ---------------------------------------------------------------- Linux and macOS: a process group per tree

#[cfg(unix)]
struct Tree {
    group: i32,
    guard: Option<Arc<Guard>>,
}

#[cfg(unix)]
impl Tree {
    fn adopt(child: &tokio::process::Child, guard: Option<&Arc<Guard>>) -> io::Result<Self> {
        let group = child.id().ok_or_else(|| io::Error::other("the process has no id"))? as i32;
        if let Some(guard) = guard {
            guard.send(&format!("+{group}"));
        }
        Ok(Self { group, guard: guard.cloned() })
    }

    fn kill(&self) {
        // SAFETY: killpg only sends a signal; the group is this tree's own (it was made for the child).
        unsafe {
            libc::killpg(self.group, libc::SIGKILL);
        }
    }
}

#[cfg(unix)]
impl Drop for Tree {
    fn drop(&mut self) {
        if let Some(guard) = &self.guard {
            guard.send(&format!("-{}", self.group));
        }
    }
}

/// The helper that kills the process groups when lazyrunner dies without doing so: it reads "+group" and
/// "-group" lines, and when its input closes (lazyrunner is gone), kills every group it still knows.
#[cfg(unix)]
pub struct Guard {
    input: std::sync::Mutex<std::process::ChildStdin>,
    _process: std::process::Child,
}

#[cfg(unix)]
impl Guard {
    /// Starts `program __guard` in a process group of its own (a closed terminal's hangup doesn't reach it).
    pub fn start(program: &std::path::Path) -> io::Result<Self> {
        use std::os::unix::process::CommandExt;
        let mut process = std::process::Command::new(program)
            .arg("__guard")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()?;
        let input = process.stdin.take().ok_or_else(|| io::Error::other("the guard has no input"))?;
        Ok(Self { input: std::sync::Mutex::new(input), _process: process })
    }

    fn send(&self, line: &str) {
        use std::io::Write;
        if let Ok(mut input) = self.input.lock() {
            let _ = writeln!(input, "{line}");
            let _ = input.flush();
        }
    }
}

/// The guard itself (`lazyrunner __guard`): returns once lazyrunner is gone and its groups are killed.
pub fn run_guard() {
    #[cfg(unix)]
    {
        use std::io::BufRead;
        // SAFETY: ignoring signals for this process only.
        unsafe {
            libc::signal(libc::SIGHUP, libc::SIG_IGN);
            libc::signal(libc::SIGINT, libc::SIG_IGN);
        }
        let mut groups = std::collections::HashSet::new();
        for line in io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if let Some(group) = line.strip_prefix('+').and_then(|g| g.parse::<i32>().ok()) {
                groups.insert(group);
            } else if let Some(group) = line.strip_prefix('-').and_then(|g| g.parse::<i32>().ok()) {
                groups.remove(&group);
            }
        }
        for group in groups {
            // SAFETY: signals the groups lazyrunner made and didn't stop.
            unsafe {
                libc::killpg(group, libc::SIGKILL);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    fn shell(script: &str) -> Spec {
        let dir = std::env::temp_dir();
        if cfg!(windows) {
            Spec { program: "cmd".into(), args: vec!["/C".into(), script.into()], dir }
        } else {
            Spec { program: "sh".into(), args: vec!["-c".into(), script.into()], dir }
        }
    }

    fn tag() -> Tag {
        Tag::Build { key: "x".into(), job: 1 }
    }

    #[tokio::test]
    async fn output_then_the_exit_code() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let _child = Child::spawn(&shell("echo one&& echo two&& exit 3"), tag(), tx, None).unwrap();
        let mut lines = Vec::new();
        loop {
            match rx.recv().await.unwrap() {
                Event::Output { line, .. } => lines.push(line.trim().to_string()),
                Event::Exited { code, .. } => {
                    assert_eq!(code, Some(3));
                    break;
                }
                other => panic!("{other:?}"),
            }
        }
        assert_eq!(lines, ["one", "two"]);
    }

    #[tokio::test]
    async fn kill_takes_the_grandchildren_too() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        // A shell that starts a long sleep and waits: killing the tree must end both.
        let script = if cfg!(windows) { "ping -n 60 127.0.0.1 > nul" } else { "sleep 60 & wait" };
        let child = Child::spawn(&shell(script), tag(), tx, None).unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        child.kill();
        let exited = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let Some(Event::Exited { .. }) = rx.recv().await {
                    return;
                }
            }
        })
        .await;
        assert!(exited.is_ok(), "the tree ended");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn no_grandchild_survives_the_kill() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let child = Child::spawn(&shell("sleep 60 & echo $!; wait"), tag(), tx, None).unwrap();
        let pid = loop {
            if let Some(Event::Output { line, .. }) = rx.recv().await {
                break line.trim().parse::<i32>().unwrap();
            }
        };
        child.kill();
        tokio::time::sleep(Duration::from_millis(500)).await;
        // SAFETY: signal 0 only asks whether the process exists.
        let alive = unsafe { libc::kill(pid, 0) == 0 };
        assert!(!alive, "the backgrounded sleep {pid} is gone too");
    }
}
