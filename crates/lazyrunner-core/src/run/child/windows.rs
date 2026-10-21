//! Output, process waits and handovers on Windows. Every child's output pipe is read the same way, spawned or
//! taken over: overlapped reads with an event of their own on a blocking thread, which a handover can stop.

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
