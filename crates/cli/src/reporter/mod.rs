//! Install progress reporters.
//!
//! Architecture:
//!
//! ```txt
//! pipeline emits InstallEvent
//!   -> InstallState::apply()
//!   -> FrameRenderer::render()
//!   -> LiveTerminal::render()  (TTY interactive)
//!   -> Plain output            (CI / non-TTY)
//! ```
//!
//! Key rules:
//! - Pipeline never prints directly; all output goes through `Reporter::on_event`.
//! - Progress UI goes to stderr, not stdout.
//! - Final frame is rendered once (no intermediate flicker left behind).

mod frame;
mod interactive;
mod plain;
mod state;
mod terminal;

pub use orix_core::reporter::InstallEvent;

use std::io;

use interactive::InteractiveReporter;
use plain::PlainReporter;
use terminal::stderr_is_terminal;

/// Unified reporter enum, dispatching to the appropriate implementation.
pub enum Reporter {
    /// Live-updating TTY reporter using crossterm.
    Interactive(Box<InteractiveReporter>),
    /// One-line-per-event plain text reporter for CI.
    Plain(PlainReporter),
}

impl Reporter {
    /// Auto-select a reporter based on terminal capabilities.
    ///
    /// - TTY stderr -> `InteractiveReporter`
    /// - non-TTY or `--no-progress` -> `PlainReporter`
    pub fn auto(no_progress: bool) -> Self {
        if !no_progress && stderr_is_terminal() {
            Self::Interactive(InteractiveReporter::new().into())
        } else {
            Self::Plain(PlainReporter::new())
        }
    }

    /// Force interactive reporter (useful for testing).
    #[cfg(test)]
    #[allow(dead_code)]
    pub fn interactive() -> Self {
        Self::Interactive(InteractiveReporter::new().into())
    }

    /// Force plain reporter.
    #[cfg(test)]
    #[allow(dead_code)]
    pub fn plain() -> Self {
        Self::Plain(PlainReporter::new())
    }

    /// Dispatch an install event to the active reporter.
    pub fn on_event(&mut self, event: InstallEvent) -> io::Result<()> {
        match self {
            Reporter::Interactive(reporter) => reporter.on_event(event),
            Reporter::Plain(reporter) => reporter.on_event(event),
        }
    }

    /// Called when install completes. Flushes the final frame.
    #[allow(dead_code)]
    pub fn done(&mut self) -> io::Result<()> {
        match self {
            Reporter::Interactive(_) => Ok(()),
            Reporter::Plain(reporter) => reporter.done(),
        }
    }
}
