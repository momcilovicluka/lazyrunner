//! Where frames go: the real terminal, or an in-memory one for tests and headless runs (`Surface`); and the timer
//! resolution smooth animation needs on Windows (`TimerResolution`).

use std::io::Stdout;

use anyhow::{Result, anyhow};
use crossterm::terminal::{BeginSynchronizedUpdate, EndSynchronizedUpdate};
use crossterm::{execute, queue};
use ratatui::backend::{CrosstermBackend, TestBackend};
use ratatui::{Frame, Terminal};

use crate::dump;

/// Something that shows frames: the real terminal, or an in-memory one for tests and headless runs.
pub trait Surface {
    /// Draws a frame; returns it as text when `want_text`.
    fn render(&mut self, draw: &mut dyn FnMut(&mut Frame), want_text: bool) -> Result<Option<String>>;
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
}

impl Surface for Terminal<TestBackend> {
    fn render(&mut self, draw: &mut dyn FnMut(&mut Frame), want_text: bool) -> Result<Option<String>> {
        let frame = self.draw(|frame| draw(frame)).map_err(|error| anyhow!("{error}"))?;
        Ok(want_text.then(|| dump::text(frame.buffer)))
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
