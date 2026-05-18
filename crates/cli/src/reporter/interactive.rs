//! TTY interactive reporter with dynamic frame updates.

use std::io;
use std::time::{Duration, Instant};

use super::frame::FrameRenderer;
use super::state::{InstallState, StepStatus};
use super::terminal::{terminal_width, LiveTerminal};
use crate::styles::ColorState;
use orix_core::reporter::InstallEvent;

/// Reporter that renders live-updating frames in a TTY.
pub struct InteractiveReporter {
    /// Current install state.
    state: InstallState,
    /// Terminal for in-place rendering.
    terminal: Option<LiveTerminal<io::Stderr>>,
    /// Last rendered frame string (to avoid duplicate renders).
    last_rendered_frame: String,
    /// When the last render occurred.
    last_render_at: Instant,
    /// Minimum interval between renders in milliseconds.
    min_render_interval: Duration,
    /// Color state.
    color_state: ColorState,
}

impl InteractiveReporter {
    /// Create a new interactive reporter using stderr.
    pub fn new(color_state: ColorState) -> Self {
        Self {
            state: InstallState::default(),
            terminal: Some(LiveTerminal::new(io::stderr())),
            last_rendered_frame: String::new(),
            last_render_at: Instant::now() - Duration::from_secs(1),
            min_render_interval: Duration::from_millis(33),
            color_state,
        }
    }

    /// Process an install event and re-render if needed.
    pub fn on_event(&mut self, event: InstallEvent) -> io::Result<()> {
        let was_finished = self.state.finished || self.state.failed;
        // Check event type before consuming it.
        let force_phase_started = matches!(&event, InstallEvent::PhaseStarted { .. });
        self.state.apply(event);
        let now_finished = self.state.finished || self.state.failed;

        // PhaseStarted: always render immediately so user sees "○ Resolving..." right away.
        // Finished/Failed: force re-render so the final frame (with "Done in Xs") is shown.
        let force = force_phase_started || now_finished;
        self.render(force)?;

        // On transition to terminal state, always call finish to restore the cursor.
        if !was_finished && now_finished {
            self.finish_internal()?;
        }
        Ok(())
    }

    fn render(&mut self, force: bool) -> io::Result<()> {
        let now = Instant::now();

        // Skip throttle when any phase is actively running so the user sees live progress.
        let any_running = matches!(self.state.resolve.status, StepStatus::Running)
            || matches!(self.state.fetch.status, StepStatus::Running)
            || matches!(self.state.link.status, StepStatus::Running);

        if !force
            && !any_running
            && now.duration_since(self.last_render_at) < self.min_render_interval
        {
            return Ok(());
        }

        let width = terminal_width();
        let renderer = FrameRenderer::new(width, self.color_state);
        let frame = renderer.render(&self.state);

        if !force && frame == self.last_rendered_frame {
            return Ok(());
        }

        self.last_rendered_frame = frame.clone();
        self.last_render_at = now;

        if let Some(terminal) = self.terminal.as_mut() {
            terminal.render(&frame)?;
        }

        Ok(())
    }

    /// Restore the cursor and clean up the terminal.
    fn finish_internal(&mut self) -> io::Result<()> {
        if let Some(terminal) = self.terminal.take() {
            terminal.finish(&self.last_rendered_frame)?;
        }
        Ok(())
    }
}

impl Default for InteractiveReporter {
    fn default() -> Self {
        Self::new(ColorState::Disabled)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_render_throttled() -> io::Result<()> {
        let mut reporter = InteractiveReporter::new(ColorState::Disabled);

        reporter.on_event(InstallEvent::Started {
            command: "orix install".to_string(),
        })?;

        let frame1 = reporter.last_rendered_frame.clone();
        assert!(!frame1.is_empty());

        reporter.on_event(InstallEvent::Started {
            command: "orix install".to_string(),
        })?;

        assert_eq!(frame1, reporter.last_rendered_frame);
        Ok(())
    }

    #[test]
    fn test_render_finished() -> io::Result<()> {
        let mut reporter = InteractiveReporter::new(ColorState::Disabled);

        reporter.on_event(InstallEvent::Started {
            command: "orix install".to_string(),
        })?;

        reporter.on_event(InstallEvent::Finished {
            installed: 5,
            duration: Duration::from_millis(100),
        })?;

        assert!(reporter.last_rendered_frame.contains("Done in"));
        Ok(())
    }
}
