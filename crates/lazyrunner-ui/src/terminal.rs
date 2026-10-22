//! Where frames go: the real terminal, or an in-memory one for tests and headless runs (`Surface`); and the timer
//! resolution smooth animation needs on Windows (`TimerResolution`).

use std::io::{self, Stdout};
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Result, anyhow};
use crossterm::cursor::{Hide, Show};
use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use crossterm::terminal::{BeginSynchronizedUpdate, EndSynchronizedUpdate, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::{execute, queue};
use ratatui::backend::{CrosstermBackend, TestBackend};
use ratatui::{Frame, Terminal};

use crate::dump;

/// Something that shows frames: the real terminal, or an in-memory one for tests and headless runs.
pub trait Surface {
    /// Draws a frame; returns it as text when `want_text`.
    fn render(&mut self, draw: &mut dyn FnMut(&mut Frame), want_text: bool) -> Result<Option<String>>;

    /// Hands the terminal to another program (the git tool): its normal screen and input back.
    fn hand_over(&mut self) -> Result<()> {
        Ok(())
    }

    /// Takes it back once that program has quit, and draws everything again.
    fn take_back(&mut self) -> Result<()> {
        Ok(())
    }

    /// Sets the terminal up again (Ctrl+R): raw mode, the alternate screen, no cursor, and a full repaint.
    fn reset(&mut self) -> Result<()> {
        Ok(())
    }
}

impl Surface for Terminal<CrosstermBackend<Stdout>> {
    fn render(&mut self, draw: &mut dyn FnMut(&mut Frame), want_text: bool) -> Result<Option<String>> {
        // Synchronized output: the terminal shows the whole frame at once, never half of it.
        queue!(self.backend_mut(), BeginSynchronizedUpdate)?;
        let text = {
            let frame = self.draw(|frame| draw(frame))?;
            want_text.then(|| dump::text(frame.buffer))
        };
        execute!(self.backend_mut(), EndSynchronizedUpdate)?;
        Ok(text)
    }

    fn hand_over(&mut self) -> Result<()> {
        ignore_ctrl_c(true); // its Ctrl+C mustn't end lazyrunner and its apps
        flush_input();
        crossterm::terminal::disable_raw_mode()?;
        execute!(self.backend_mut(), LeaveAlternateScreen, Show)?;
        Ok(())
    }

    fn take_back(&mut self) -> Result<()> {
        flush_input(); // keys typed for it, not read (a second q), mustn't reach lazyrunner
        crossterm::terminal::enable_raw_mode()?;
        execute!(self.backend_mut(), EnterAlternateScreen, Hide)?;
        ignore_ctrl_c(false);
        self.clear()?;
        Ok(())
    }

    fn reset(&mut self) -> Result<()> {
        crossterm::terminal::enable_raw_mode()?;
        execute!(self.backend_mut(), EnterAlternateScreen, Hide)?;
        self.clear()?;
        Ok(())
    }
}

/// While another program has the terminal, Ctrl+C and Ctrl+Break reach every process on the console: lazyrunner
/// swallows them. A handler routine, unlike the ignore flag, isn't inherited by what lazyrunner starts meanwhile.
fn ignore_ctrl_c(ignore: bool) {
    #[cfg(windows)]
    // SAFETY: registers or removes a handler that only reads its argument.
    unsafe {
        windows_sys::Win32::System::Console::SetConsoleCtrlHandler(Some(swallow), i32::from(ignore));
    }
    #[cfg(not(windows))]
    let _ = ignore;
}

#[cfg(windows)]
unsafe extern "system" fn swallow(kind: u32) -> windows_sys::core::BOOL {
    use windows_sys::Win32::System::Console::{CTRL_BREAK_EVENT, CTRL_C_EVENT};
    i32::from(kind == CTRL_C_EVENT || kind == CTRL_BREAK_EVENT)
}

/// Drops the keys waiting in the console's input.
fn flush_input() {
    #[cfg(windows)]
    // SAFETY: the process's own standard input handle.
    unsafe {
        use windows_sys::Win32::System::Console::{FlushConsoleInputBuffer, GetStdHandle, STD_INPUT_HANDLE};
        FlushConsoleInputBuffer(GetStdHandle(STD_INPUT_HANDLE));
    }
}

impl Surface for Terminal<TestBackend> {
    fn render(&mut self, draw: &mut dyn FnMut(&mut Frame), want_text: bool) -> Result<Option<String>> {
        let frame = self.draw(|frame| draw(frame)).map_err(|error| anyhow!("{error}"))?;
        Ok(want_text.then(|| dump::text(frame.buffer)))
    }
}

/// Whether lazyrunner has the terminal's mouse (so a panic can give it back).
static MOUSE: AtomicBool = AtomicBool::new(false);

/// The terminal's mouse: lazyrunner's while the mouse setting is on (`, m`; off, the terminal selects text again),
/// and given back when lazyrunner closes, so the shell after it doesn't get mouse codes.
pub struct MouseCapture {
    /// A real terminal (headless runs have none).
    live: bool,
}

impl MouseCapture {
    pub fn new(live: bool) -> Self {
        Self { live }
    }

    pub fn set(&mut self, on: bool) {
        if !self.live || MOUSE.load(Ordering::Relaxed) == on {
            return;
        }
        let done =
            if on { execute!(io::stdout(), EnableMouseCapture) } else { execute!(io::stdout(), DisableMouseCapture) };
        if done.is_ok() {
            MOUSE.store(on, Ordering::Relaxed);
        }
    }
}

impl MouseCapture {
    /// Asks the terminal for the mouse again (or gives it back), whatever it was told before (Ctrl+R).
    pub fn again(&mut self, on: bool) {
        MOUSE.store(!on, Ordering::Relaxed);
        self.set(on);
    }
}

impl Drop for MouseCapture {
    fn drop(&mut self) {
        self.set(false);
    }
}

/// Gives the mouse back if lazyrunner has it (the panic hook).
pub fn release_mouse() {
    if MOUSE.swap(false, Ordering::Relaxed) {
        let _ = execute!(io::stdout(), DisableMouseCapture);
    }
}

/// Windows wakes sleeping threads every 15.6 ms unless a process asks for better; 60 fps needs 1 ms. Windows 11 also
/// ignores that request while the process's window is hidden or covered (power throttling) unless the process opts out,
/// which would halve the frame rate whenever another window is on top.
pub(crate) struct TimerResolution;

impl TimerResolution {
    pub(crate) fn high() -> Self {
        #[cfg(windows)]
        // SAFETY: both calls only change this process's own timer and throttling settings; the state struct lives on
        // the stack for the call and its size is passed with it; timeBeginPeriod is undone in drop.
        unsafe {
            use windows_sys::Win32::System::Threading::{
                GetCurrentProcess, PROCESS_POWER_THROTTLING_CURRENT_VERSION,
                PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION, PROCESS_POWER_THROTTLING_STATE,
                ProcessPowerThrottling, SetProcessInformation,
            };
            let state = PROCESS_POWER_THROTTLING_STATE {
                Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
                ControlMask: PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION,
                StateMask: 0, // controlled and off: always honour the timer resolution
            };
            SetProcessInformation(
                GetCurrentProcess(),
                ProcessPowerThrottling,
                (&raw const state).cast(),
                size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
            );
            windows_sys::Win32::Media::timeBeginPeriod(1);
        }
        Self
    }
}

impl Drop for TimerResolution {
    fn drop(&mut self) {
        #[cfg(windows)]
        // SAFETY: pairs the timeBeginPeriod call above.
        unsafe {
            windows_sys::Win32::Media::timeEndPeriod(1);
        }
    }
}
