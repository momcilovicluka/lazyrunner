//! Panics outside the UI thread (a check, the update, a tokio task, a reader) are caught by tokio or end their
//! thread, but the panic hook ratatui installs would still tear the terminal down while lazyrunner carries on
//! drawing. `keep_off_screen` leaves the UI thread's own panics to that hook and keeps the others for the panel's
//! log (`take`).

use std::panic::PanicHookInfo;
use std::sync::Mutex;

/// The background panics not yet shown in the log.
static PANICS: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Called on the UI thread, after the terminal is set up (ratatui's hook installed).
pub fn keep_off_screen() {
    let ui_thread = std::thread::current().id();
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if std::thread::current().id() == ui_thread {
            previous(info); // restores the terminal and says what happened
        } else if let Ok(mut panics) = PANICS.lock() {
            panics.push(describe(info));
        }
    }));
}

/// The background panics since the last call, for the log.
pub fn take() -> Vec<String> {
    PANICS.lock().map(|mut panics| std::mem::take(&mut *panics)).unwrap_or_default()
}

fn describe(info: &PanicHookInfo) -> String {
    let payload = info.payload();
    let message = payload
        .downcast_ref::<&str>()
        .map(|text| (*text).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "a panic".into());
    let place = info.location().map(|at| format!(" ({}:{})", at.file(), at.line())).unwrap_or_default();
    let thread = std::thread::current().name().unwrap_or("a background thread").to_string();
    format!("{thread}: {message}{place}")
}
