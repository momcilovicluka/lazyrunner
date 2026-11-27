//! Cancelling a job that runs programs one after another in its own thread (a pull's `x`, #110): the thread watches a
//! [`Cancel`] while it runs, and sqlcmd and SqlPackage, started on that thread, tell it their process - cancelling
//! kills the one running (its whole tree), and a program about to start doesn't.

#![allow(
    clippy::missing_const_for_thread_local,
    reason = "clippy 1.99 flags `thread_local!` blocks whose statics are already `const { .. }`"
)]

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// What a cancelled program says it failed with.
pub const CANCELLED: &str = "cancelled";

thread_local! {
    /// The cancel the programs this thread starts answer to.
    static WATCHED: RefCell<Option<Arc<Cancel>>> = const { RefCell::new(None) };
}

/// A job's cancel: asked for once, it kills the program running for the job.
#[derive(Debug, Default)]
pub struct Cancel {
    asked: AtomicBool,
    running: Mutex<Option<u32>>,
}

/// The thread watches a cancel until this is dropped.
pub struct Watching;

impl Drop for Watching {
    fn drop(&mut self) {
        WATCHED.with(|watched| watched.borrow_mut().take());
    }
}

/// A program's process, known to its thread's cancel until this is dropped.
pub struct Running(Option<Arc<Cancel>>);

impl Drop for Running {
    fn drop(&mut self) {
        if let Some(cancel) = &self.0
            && let Ok(mut running) = cancel.running.lock()
        {
            *running = None;
        }
    }
}

impl Cancel {
    /// The programs this thread starts answer to it, while the guard lives.
    pub fn watch(self: &Arc<Self>) -> Watching {
        WATCHED.with(|watched| *watched.borrow_mut() = Some(Arc::clone(self)));
        Watching
    }

    /// Cancels: the program running is killed, and none starts after.
    pub fn cancel(&self) {
        self.asked.store(true, Ordering::SeqCst);
        let running = self.running.lock().ok().and_then(|running| *running);
        if let Some(pid) = running {
            kill_tree(pid);
        }
    }

    pub fn asked(&self) -> bool {
        self.asked.load(Ordering::SeqCst)
    }
}

/// Whether the thread's job was cancelled: then nothing more starts.
pub fn asked() -> bool {
    WATCHED.with(|watched| watched.borrow().as_ref().is_some_and(|cancel| cancel.asked()))
}

/// A program the thread started, by its process id: killed at once when its job is cancelled already.
pub fn running(pid: u32) -> Running {
    let cancel = WATCHED.with(|watched| watched.borrow().clone());
    if let Some(cancel) = &cancel
        && let Ok(mut running) = cancel.running.lock()
    {
        *running = Some(pid);
        drop(running);
        if cancel.asked() {
            kill_tree(pid);
        }
    }
    Running(cancel)
}

/// Kills a process and everything it started; true when it's gone.
pub fn kill_tree(pid: u32) -> bool {
    #[cfg(windows)]
    {
        let mut command = std::process::Command::new("taskkill");
        command.args(["/PID", &pid.to_string(), "/T", "/F"]);
        super::tools::own_console(&mut command).output().is_ok_and(|output| output.status.success())
    }
    #[cfg(unix)]
    {
        // SAFETY: sends a signal to a process lazyrunner started or the user chose to kill.
        unsafe { libc::kill(pid as i32, libc::SIGKILL) == 0 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_thread_answers_to_the_cancel_it_watches_and_forgets_it_after() {
        let cancel = Arc::new(Cancel::default());
        let other = Arc::clone(&cancel);
        std::thread::spawn(move || {
            let watching = other.watch();
            assert!(!asked());
            other.cancel();
            assert!(asked(), "cancelled: nothing more starts");
            drop(watching);
            assert!(!asked(), "the thread watches nothing after");
        })
        .join()
        .unwrap();
        assert!(cancel.asked() && !asked(), "another thread isn't watching it");
    }
}
