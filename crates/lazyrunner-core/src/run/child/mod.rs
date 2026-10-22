//! Child processes as whole trees: an app's server (dotnet run and the API it starts, npm and node, ...) or a build,
//! with its output turned into events, stopped all at once, and never outliving lazyrunner.
//!
//! - Windows: each child runs in a job object that kills everything in it when it is closed, which happens when
//!   lazyrunner stops it, quits, or dies however it dies.
//! - Linux and macOS: each child leads a process group of its own; stopping it kills the group. A small guard
//!   process (`lazyrunner __guard`) knows the groups and kills them when lazyrunner goes away without doing so.
//!
//! A handover (Windows): one lazyrunner lets go of a tree without stopping it ([`Child::release`]) and the next
//! one, which inherits the handles, takes it over ([`Child::adopt`]); the app never notices. A taken-over tree can
//! be handed over again, as often as lazyrunner reopens.

use std::ffi::OsString;
use std::io;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
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
    /// Variables set for it on top of lazyrunner's own.
    pub env: Vec<(OsString, OsString)>,
}

/// A running process tree. Dropping it kills the tree.
pub struct Child {
    pub pid: u32,
    tree: Tree,
    task: JoinHandle<()>,
    /// Its process and output pipes, kept for a handover.
    #[cfg(windows)]
    kept: windows::Kept,
}

/// A tree's handles as the next lazyrunner inherits them (their values are the same in both processes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Handles {
    pub pid: u32,
    pub job: u64,
    pub process: u64,
    pub stdout: Option<u64>,
    pub stderr: Option<u64>,
}

impl Handles {
    /// Every handle value in it.
    pub fn values(&self) -> Vec<u64> {
        [Some(self.job), Some(self.process), self.stdout, self.stderr]
            .into_iter()
            .flatten()
            .filter(|&v| v != 0)
            .collect()
    }
}

/// A tree let go by this lazyrunner: its handles, inheritable, open until the next lazyrunner has started (then
/// dropping this closes this process's copies; the tree lives on in the next one's).
pub struct Released {
    pub handles: Handles,
    #[cfg(windows)]
    _open: Vec<std::os::windows::io::OwnedHandle>,
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
            .envs(spec.env.iter().map(|(name, value)| (name, value)))
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
        #[cfg(windows)]
        {
            windows::start(child, pid, tree, tag, events)
        }
        #[cfg(unix)]
        {
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
    }

    /// Kills the whole tree now.
    pub fn kill(&self) {
        self.tree.kill();
    }

    /// Whether the process (not necessarily its children) has exited.
    pub fn has_exited(&self) -> bool {
        self.task.is_finished()
    }

    /// Lets go of the tree without stopping it, for the next lazyrunner to take over: inheritable copies of its
    /// handles, and this lazyrunner stops reading its output (a partial last line goes out as it is).
    #[cfg(windows)]
    pub fn release(self) -> io::Result<Released> {
        windows::release(self)
    }

    #[cfg(not(windows))]
    pub fn release(self) -> io::Result<Released> {
        Err(unsupported())
    }

    /// Takes over a tree another lazyrunner released and this one inherited: reads its output, waits for its exit
    /// (both as events tagged `tag`), and owns its job (dropping the Child kills the tree, as for one it started).
    /// The handles are checked first (the process is `pid` and in the job, the outputs are pipes); when that fails,
    /// they are left alone. Past that check this owns them, whatever it returns: on an error they are closed, which
    /// stops the tree.
    ///
    /// # Safety
    /// The values must be handles this process inherited for this purpose, owned by nothing else in it.
    #[cfg(windows)]
    pub unsafe fn adopt(handles: Handles, tag: Tag, events: UnboundedSender<Event>) -> io::Result<Self> {
        // SAFETY: passed on from this function's contract.
        unsafe { windows::adopt(handles, tag, events) }
    }

    /// # Safety
    /// As on Windows; nothing is taken over here.
    #[cfg(not(windows))]
    pub unsafe fn adopt(_handles: Handles, _tag: Tag, _events: UnboundedSender<Event>) -> io::Result<Self> {
        Err(unsupported())
    }
}

#[cfg(not(windows))]
fn unsupported() -> io::Error {
    io::Error::other("a handover isn't supported on this system yet")
}

/// Ends a handed-over tree this lazyrunner won't take over (its app changed or is gone): kills it when `kill`, and
/// closes the handles - after checking them as `Child::adopt` does (they are left alone if that fails).
///
/// # Safety
/// As `Child::adopt`.
pub unsafe fn discard(handles: &Handles, kill: bool) {
    #[cfg(windows)]
    // SAFETY: passed on from this function's contract.
    unsafe {
        windows::discard(handles, kill);
    }
    #[cfg(not(windows))]
    let _ = (handles, kill);
}

/// Whether handed-over handles are what the handover says (the checks `Child::adopt` makes first).
pub fn handles_match(handles: &Handles) -> bool {
    #[cfg(windows)]
    {
        windows::check(handles).is_ok()
    }
    #[cfg(not(windows))]
    {
        let _ = handles;
        false
    }
}

/// Closes inherited handle values a handover file that couldn't be read was meant to explain: the jobs close, so
/// their trees stop (rather than run on with nobody reading their output).
///
/// # Safety
/// The values must be handles this process inherited for a handover, owned by nothing else in it.
pub unsafe fn close_inherited(values: &[u64]) {
    #[cfg(windows)]
    for &value in values {
        // SAFETY: passed on from this function's contract.
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(value as usize as windows_sys::Win32::Foundation::HANDLE);
        }
    }
    #[cfg(not(windows))]
    let _ = values;
}

impl Drop for Child {
    fn drop(&mut self) {
        if !self.tree.released {
            self.tree.kill();
        }
    }
}

#[cfg(unix)]
async fn forward(pipe: impl tokio::io::AsyncRead + Unpin, tag: Tag, events: UnboundedSender<Event>) {
    use tokio::io::AsyncBufReadExt;
    let mut reader = tokio::io::BufReader::with_capacity(READ_BUFFER, pipe);
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
struct Tree {
    job: windows_sys::Win32::Foundation::HANDLE,
    /// Handed over: the job lives on in the next lazyrunner, so dropping this mustn't kill it.
    released: bool,
}

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
            Ok(Self { job, released: false })
        }
    }

    fn kill(&self) {
        // SAFETY: the job handle is open until drop.
        unsafe {
            windows_sys::Win32::System::JobObjects::TerminateJobObject(self.job, 1);
        }
    }
}

#[cfg(windows)]
impl Drop for Tree {
    fn drop(&mut self) {
        // SAFETY: closes the handle this Tree owns, once; closing the last handle also kills what is left in the job
        // (a released tree's job has another handle, in the next lazyrunner).
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.job);
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

// Output, process waits and handovers on Windows.
#[cfg(windows)]
mod windows;

// ---------------------------------------------------------------- Linux and macOS: a process group per tree

#[cfg(unix)]
struct Tree {
    group: i32,
    guard: Option<Arc<Guard>>,
    /// Never set: Unix has no handover yet.
    released: bool,
}

#[cfg(unix)]
impl Tree {
    fn adopt(child: &tokio::process::Child, guard: Option<&Arc<Guard>>) -> io::Result<Self> {
        let group = child.id().ok_or_else(|| io::Error::other("the process has no id"))? as i32;
        if let Some(guard) = guard {
            guard.send(&format!("+{group}"));
        }
        Ok(Self { group, guard: guard.cloned(), released: false })
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
mod tests;
