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

/// Output, process waits and handovers on Windows. Every child's output pipe is read the same way, spawned or
/// taken over: overlapped reads with an event of their own on a blocking thread, which a handover can stop.
#[cfg(windows)]
mod windows {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, IntoRawHandle, OwnedHandle, RawHandle};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Instant;

    use tokio::sync::oneshot;
    use windows_sys::Win32::Foundation::{
        CloseHandle, DUPLICATE_SAME_ACCESS, DuplicateHandle, ERROR_BROKEN_PIPE, ERROR_HANDLE_EOF, ERROR_IO_PENDING,
        GetLastError, HANDLE, HANDLE_FLAG_INHERIT, SetHandleInformation, WAIT_OBJECT_0,
    };
    use windows_sys::Win32::Storage::FileSystem::{FILE_TYPE_PIPE, GetFileType, ReadFile};
    use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
    use windows_sys::Win32::System::JobObjects::{IsProcessInJob, TerminateJobObject};
    use windows_sys::Win32::System::Threading::{
        CreateEventW, GetCurrentProcess, GetExitCodeProcess, GetProcessId, INFINITE, SetEvent, WaitForMultipleObjects,
        WaitForSingleObject,
    };

    use super::*;

    /// How long a handover waits for this lazyrunner's readers to stop.
    const STOP_READERS: Duration = Duration::from_secs(1);
    const STOP_POLL: Duration = Duration::from_millis(5);
    /// A process that is still running reports this "exit code".
    const STILL_ACTIVE: u32 = 259;
    const POLL_EXIT: Duration = Duration::from_millis(500);

    /// What a Child keeps for a handover: its own copy of the process handle, its output pipes and their readers,
    /// and (taken over) the event that ends the wait for its exit without an exit event.
    pub(super) struct Kept {
        process: Arc<OwnedHandle>,
        pipes: Vec<Pipe>,
        cancel: Option<Arc<OwnedHandle>>,
    }

    struct Pipe {
        handle: Arc<OwnedHandle>,
        stop: Arc<AtomicBool>,
        finished: Arc<AtomicBool>,
    }

    /// Reads the output and waits for the exit of a child just spawned.
    pub(super) fn start(
        mut child: tokio::process::Child,
        pid: u32,
        tree: Tree,
        tag: Tag,
        events: UnboundedSender<Event>,
    ) -> io::Result<Child> {
        let raw = child.raw_handle().ok_or_else(|| io::Error::other("the process has no handle"))?;
        let process = Arc::new(duplicate(raw as usize, false)?);
        let outputs = [
            child.stdout.take().map(|pipe| pipe.into_owned_handle()),
            child.stderr.take().map(|pipe| pipe.into_owned_handle()),
        ];
        let mut pipes = Vec::new();
        let mut done = Vec::new();
        for output in outputs.into_iter().flatten() {
            let (pipe, finished) = read(Arc::new(output?), event()?, tag.clone(), events.clone());
            pipes.push(pipe);
            done.push(finished);
        }
        let task = tokio::spawn(async move {
            let status = child.wait().await;
            drain(done).await;
            let code = status.ok().and_then(|status| status.code());
            let _ = events.send(Event::Exited { tag, code });
        });
        Ok(Child { pid, tree, task, kept: Kept { process, pipes, cancel: None } })
    }

    /// Reads `handle` on a blocking thread; the receiver hears when the reader is done.
    fn read(
        handle: Arc<OwnedHandle>,
        event: OwnedHandle,
        tag: Tag,
        events: UnboundedSender<Event>,
    ) -> (Pipe, oneshot::Receiver<()>) {
        let stop = Arc::new(AtomicBool::new(false));
        let finished = Arc::new(AtomicBool::new(false));
        let reader = OverlappedPipe { pipe: handle.clone(), event, stop: stop.clone() };
        let (done, heard) = oneshot::channel();
        let flag = finished.clone();
        tokio::task::spawn_blocking(move || {
            forward_blocking(reader, &tag, &events);
            flag.store(true, Ordering::SeqCst);
            let _ = done.send(());
        });
        (Pipe { handle, stop, finished }, heard)
    }

    async fn drain(done: Vec<oneshot::Receiver<()>>) {
        for reader in done {
            let _ = tokio::time::timeout(DRAIN_AFTER_EXIT, reader).await;
        }
    }

    pub(super) fn release(mut child: Child) -> io::Result<Released> {
        if child.task.is_finished() {
            return Err(io::Error::other("it has exited"));
        }
        let job = duplicate(child.tree.job as usize, true)?;
        let process = duplicate(child.kept.process.as_raw_handle() as usize, true)?;
        let pipes: Vec<OwnedHandle> = child
            .kept
            .pipes
            .iter()
            .map(|pipe| duplicate(pipe.handle.as_raw_handle() as usize, true))
            .collect::<io::Result<_>>()?;
        // Stop reading here (the next lazyrunner reads from now on), so no read of this one takes its output.
        for pipe in &child.kept.pipes {
            pipe.stop.store(true, Ordering::SeqCst);
        }
        let deadline = Instant::now() + STOP_READERS;
        for pipe in &child.kept.pipes {
            while !pipe.finished.load(Ordering::SeqCst) && Instant::now() < deadline {
                // SAFETY: cancels this process's I/O on a handle it holds (the Arc keeps it open).
                unsafe { CancelIoEx(pipe.handle.as_raw_handle(), std::ptr::null()) };
                std::thread::sleep(STOP_POLL);
            }
        }
        if let Some(cancel) = &child.kept.cancel {
            // SAFETY: signals this Child's own event; its waiter ends without an exit event.
            unsafe { SetEvent(cancel.as_raw_handle()) };
        }
        child.task.abort();
        child.tree.released = true; // dropping this Child now only closes its own copies
        let value = |handle: &OwnedHandle| handle.as_raw_handle() as usize as u64;
        let handles = Handles {
            pid: child.pid,
            job: value(&job),
            process: value(&process),
            stdout: pipes.first().map(value),
            stderr: pipes.get(1).map(value),
        };
        let mut open = vec![job, process];
        open.extend(pipes);
        Ok(Released { handles, _open: open })
    }

    pub(super) unsafe fn adopt(handles: Handles, tag: Tag, events: UnboundedSender<Event>) -> io::Result<Child> {
        check(&handles)?;
        let outputs: Vec<u64> = [handles.stdout, handles.stderr].into_iter().flatten().collect();
        let pipe_events = outputs.iter().map(|_| event()).collect::<io::Result<Vec<_>>>()?;
        let cancel = Arc::new(event()?);
        for value in handles.values() {
            // SAFETY: inherited handles this process owns: from now on not passed on to the processes it starts.
            unsafe { SetHandleInformation(value as usize as HANDLE, HANDLE_FLAG_INHERIT, 0) };
        }
        // SAFETY: the caller's contract: these are this process's inherited handles, owned by nothing else.
        let owned = |value: u64| unsafe { OwnedHandle::from_raw_handle(value as usize as RawHandle) };
        let tree = Tree { job: owned(handles.job).into_raw_handle(), released: false };
        let process = Arc::new(owned(handles.process));
        let mut pipes = Vec::new();
        let mut done = Vec::new();
        for (value, event) in outputs.into_iter().zip(pipe_events) {
            let (pipe, finished) = read(Arc::new(owned(value)), event, tag.clone(), events.clone());
            pipes.push(pipe);
            done.push(finished);
        }
        let (waited, signal) = (process.clone(), cancel.clone());
        let task = tokio::spawn(async move {
            let exit = tokio::task::spawn_blocking(move || wait_or_cancel(&waited, &signal)).await;
            if let Ok(Some(code)) = exit {
                drain(done).await;
                let _ = events.send(Event::Exited { tag, code });
            }
        });
        Ok(Child { pid: handles.pid, tree, task, kept: Kept { process, pipes, cancel: Some(cancel) } })
    }

    pub(super) unsafe fn discard(handles: &Handles, kill: bool) {
        if check(handles).is_err() {
            return;
        }
        // SAFETY: the caller's contract: this process's inherited handles, owned by nothing else; checked above.
        unsafe {
            if kill {
                TerminateJobObject(handles.job as usize as HANDLE, 1);
            }
            for value in handles.values() {
                CloseHandle(value as usize as HANDLE);
            }
        }
    }

    /// That the handles are what the handover says: the process is the one with that id and inside the job, and the
    /// outputs are pipes.
    pub(super) fn check(handles: &Handles) -> io::Result<()> {
        let (job, process) = (handles.job as usize as HANDLE, handles.process as usize as HANDLE);
        // SAFETY: queries only; a wrong value makes them fail, which is the answer.
        unsafe {
            if handles.job == 0 || handles.process == 0 || GetProcessId(process) != handles.pid {
                return Err(io::Error::other("its process isn't the one handed over"));
            }
            let mut inside = 0;
            if IsProcessInJob(process, job, &mut inside) == 0 || inside == 0 {
                return Err(io::Error::other("its process isn't in its job"));
            }
            for pipe in [handles.stdout, handles.stderr].into_iter().flatten() {
                if GetFileType(pipe as usize as HANDLE) != FILE_TYPE_PIPE {
                    return Err(io::Error::other("its output isn't a pipe"));
                }
            }
        }
        Ok(())
    }

    /// Waits for the process to end (its exit code) or for `cancel` (a handover: `None`).
    fn wait_or_cancel(process: &OwnedHandle, cancel: &OwnedHandle) -> Option<Option<i32>> {
        let objects = [process.as_raw_handle(), cancel.as_raw_handle()];
        // SAFETY: waits on two handles this function borrows for the call.
        let woke = unsafe { WaitForMultipleObjects(2, objects.as_ptr(), 0, INFINITE) };
        if woke == WAIT_OBJECT_0 + 1 {
            return None;
        }
        loop {
            let mut code: u32 = 0;
            // SAFETY: reads the exit code of a process handle this function borrows.
            let read = unsafe { GetExitCodeProcess(process.as_raw_handle(), &mut code) };
            if read == 0 {
                return Some(None);
            }
            if code != STILL_ACTIVE || woke == WAIT_OBJECT_0 {
                return Some(Some(code as i32));
            }
            // The wait itself failed: poll, until the process ends or a handover cancels.
            std::thread::sleep(POLL_EXIT);
            // SAFETY: checks the cancel event without waiting.
            if unsafe { WaitForSingleObject(cancel.as_raw_handle(), 0) } == WAIT_OBJECT_0 {
                return None;
            }
        }
    }

    /// A copy of `handle` in this process; `inherit`: so the next lazyrunner gets it.
    fn duplicate(handle: usize, inherit: bool) -> io::Result<OwnedHandle> {
        if handle == 0 {
            return Err(io::Error::other("no handle to hand over"));
        }
        let mut copy: HANDLE = std::ptr::null_mut();
        // SAFETY: duplicates a handle this process holds into this same process; `copy` receives the new handle,
        // which the OwnedHandle below then owns.
        let done = unsafe {
            let me = GetCurrentProcess();
            DuplicateHandle(me, handle as HANDLE, me, &mut copy, 0, i32::from(inherit), DUPLICATE_SAME_ACCESS)
        };
        if done == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `copy` is a fresh handle owned by nobody else.
        Ok(unsafe { OwnedHandle::from_raw_handle(copy) })
    }

    /// A new unnamed manual-reset event.
    fn event() -> io::Result<OwnedHandle> {
        // SAFETY: creates an event, owned by the OwnedHandle below.
        let event = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
        if event.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `event` was just created and nothing else owns it.
        Ok(unsafe { OwnedHandle::from_raw_handle(event) })
    }

    /// A child's output pipe, opened for overlapped I/O (as std opens the parent's end): read with an OVERLAPPED and
    /// an event of its own, so a read completes on its own event, and ends when `stop` is set (a handover).
    struct OverlappedPipe {
        pipe: Arc<OwnedHandle>,
        event: OwnedHandle,
        stop: Arc<AtomicBool>,
    }

    impl io::Read for OverlappedPipe {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.stop.load(Ordering::SeqCst) {
                return Ok(0);
            }
            let pipe = self.pipe.as_raw_handle();
            let len = u32::try_from(buf.len()).unwrap_or(u32::MAX);
            let mut read: u32 = 0;
            // SAFETY: `overlapped` and `buf` live until the read is over (GetOverlappedResult waits for it), and the
            // handles are this reader's own.
            let done = unsafe {
                let mut overlapped: OVERLAPPED = std::mem::zeroed();
                overlapped.hEvent = self.event.as_raw_handle();
                if ReadFile(pipe, buf.as_mut_ptr(), len, &mut read, &mut overlapped) != 0 {
                    true
                } else if GetLastError() == ERROR_IO_PENDING {
                    GetOverlappedResult(pipe, &overlapped, &mut read, 1) != 0
                } else {
                    false
                }
            };
            if done {
                return Ok(read as usize);
            }
            // SAFETY: reads this thread's last error.
            match unsafe { GetLastError() } {
                ERROR_BROKEN_PIPE | ERROR_HANDLE_EOF => Ok(0), // the app closed its end: no more output
                _ => Err(io::Error::last_os_error()),          // a handover cancelled it, or a real failure
            }
        }
    }

    /// The lines of a pipe as events, on a blocking thread (as `forward` does on Unix).
    fn forward_blocking(pipe: impl io::Read, tag: &Tag, events: &UnboundedSender<Event>) {
        use std::io::BufRead;
        let mut reader = io::BufReader::with_capacity(READ_BUFFER, pipe);
        let mut line = Vec::with_capacity(256);
        loop {
            let chunk = match reader.fill_buf() {
                Ok([]) | Err(_) => break,
                Ok(chunk) => chunk,
            };
            let newline = chunk.iter().position(|&byte| byte == b'\n');
            let take = newline.map_or(chunk.len(), |at| at + 1).min(MAX_LINE - line.len());
            line.extend_from_slice(&chunk[..take]);
            reader.consume(take);
            let complete = newline.is_some_and(|at| at < take) || line.len() >= MAX_LINE;
            if complete && !send(tag, &mut line, events) {
                return;
            }
        }
        if !line.is_empty() {
            send(tag, &mut line, events);
        }
    }
}

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

    /// Waits for a line tagged `wanted`.
    #[cfg(windows)]
    async fn line_from(rx: &mut mpsc::UnboundedReceiver<Event>, wanted: &Tag) -> String {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                match rx.recv().await {
                    Some(Event::Output { tag, line }) if tag == *wanted => return line,
                    Some(_) => {}
                    None => panic!("the events ended"),
                }
            }
        })
        .await
        .expect("output arrives")
    }

    /// Whether a process with this id is running.
    #[cfg(windows)]
    fn alive(pid: u32) -> bool {
        let filter = format!("PID eq {pid}");
        let out = std::process::Command::new("tasklist").args(["/FI", &filter, "/NH"]).output().unwrap();
        String::from_utf8_lossy(&out.stdout).contains(&pid.to_string())
    }

    /// Handovers in one process: released, the tree keeps running; taken over, its output and exit arrive under the
    /// new tag - and a taken-over tree can be handed over again. Dropping the last owner kills it.
    #[cfg(windows)]
    #[tokio::test]
    async fn a_tree_can_be_handed_over_again_and_again() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let script = "for /L %i in (1,1,600) do @(echo tick %i & ping -n 2 127.0.0.1 > nul)";
        let mut child = Child::spawn(&shell(script), tag(), tx.clone(), None).unwrap();
        let pid = child.pid;
        line_from(&mut rx, &tag()).await;
        for generation in [7, 8] {
            let released = child.release().unwrap();
            let next = Tag::App { key: "y".into(), generation };
            // SAFETY: in a real handover these copies are the next lazyrunner's; here adopt takes them over, so
            // `released` mustn't close them too.
            child = unsafe { Child::adopt(released.handles, next.clone(), tx.clone()) }.unwrap();
            std::mem::forget(released);
            assert_eq!(child.pid, pid, "the same process");
            assert!(line_from(&mut rx, &next).await.contains("tick"), "output keeps arriving");
        }
        drop(child);
        let exited = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let Some(Event::Exited { tag: Tag::App { generation: 8, .. }, .. }) = rx.recv().await {
                    return;
                }
            }
        })
        .await;
        assert!(exited.is_ok(), "dropping the last owner killed it");
    }

    /// Handles that don't match what the handover says are refused and left alone; ending a tree that isn't taken
    /// over (`discard`) kills it.
    #[cfg(windows)]
    #[tokio::test]
    async fn wrong_handles_are_refused_and_a_discarded_tree_ends() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let child = Child::spawn(&shell("ping -n 30 127.0.0.1 > nul"), tag(), tx.clone(), None).unwrap();
        let pid = child.pid;
        let released = child.release().unwrap();
        let wrong = Handles { pid: pid + 1, ..released.handles };
        // SAFETY: the check fails (another process id) before anything is taken over.
        assert!(unsafe { Child::adopt(wrong, tag(), tx) }.is_err(), "a process with another id is refused");
        assert!(alive(pid), "refusing left the tree running");
        // SAFETY: discard closes the copies `released` holds, so `released` mustn't close them too.
        unsafe { discard(&released.handles, true) };
        std::mem::forget(released);
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while alive(pid) && std::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(!alive(pid), "discarding killed the tree");
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
