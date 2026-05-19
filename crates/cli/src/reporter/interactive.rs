//! TTY interactive reporter with dynamic frame updates.

use std::io;
use std::time::{Duration, Instant};

use super::color::{no_color_env, ColorMode, Theme};
use super::frame::{FrameRenderer, RenderedFrame};
use super::state::InstallState;
use super::terminal::{stderr_is_terminal, terminal_width, LiveTerminal};
use orix_core::reporter::InstallEvent;

/// Reporter that renders live-updating frames in a TTY.
pub struct InteractiveReporter {
    /// Current install state.
    state: InstallState,
    /// Terminal for in-place rendering.
    terminal: Option<LiveTerminal<io::Stderr>>,
    /// Last rendered frame (colored).
    last_frame: RenderedFrame,
    /// When the last render occurred.
    last_render_at: Instant,
    /// Minimum interval between renders in milliseconds.
    min_render_interval: Duration,
    /// Color theme.
    theme: Theme,
    /// Whether the terminal state (Finished/Failed) frame has been rendered.
    /// Used to bypass frame comparison on the very first terminal render so the
    /// Done/Failed frame is never silently dropped by throttling.
    rendered_terminal_state: bool,
    /// Whether finish() has been called to restore the cursor.
    finished: bool,
}

impl InteractiveReporter {
    /// Create a new interactive reporter using stderr with auto color detection.
    pub fn new() -> Self {
        Self::with_color(ColorMode::Auto)
    }

    /// Create a new interactive reporter with an explicit color mode.
    pub fn with_color(color_mode: ColorMode) -> Self {
        let is_tty = stderr_is_terminal();
        let theme = Theme::new(color_mode, is_tty, no_color_env());

        Self {
            state: InstallState::default(),
            terminal: Some(LiveTerminal::new(io::stderr())),
            last_frame: RenderedFrame {
                frame: String::new(),
                plain: String::new(),
                row_count: 0,
            },
            last_render_at: Instant::now() - Duration::from_secs(1),
            min_render_interval: Duration::from_millis(33),
            theme,
            rendered_terminal_state: false,
            finished: false,
        }
    }

    /// Access the underlying theme.
    #[allow(dead_code)]
    pub fn theme(&self) -> &Theme {
        &self.theme
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

        let now_terminal = self.state.finished || self.state.failed;
        let transitioning_to_terminal = now_terminal && !self.rendered_terminal_state;

        if !force
            && !transitioning_to_terminal
            && now.duration_since(self.last_render_at) < self.min_render_interval
        {
            return Ok(());
        }

        let width = terminal_width();
        let renderer = FrameRenderer::with_theme(width, self.theme.clone());
        let frame = renderer.render(&self.state);

        // Compare plain text to avoid re-rendering when only colors change (no actual state change).
        // Also bypass comparison when transitioning to terminal state (Finished/Failed) so the
        // final frame with "Done in Xs" is never silently dropped.
        if !force && !transitioning_to_terminal && frame.plain == self.last_frame.plain {
            return Ok(());
        }

        if now_terminal {
            self.rendered_terminal_state = true;
        }

        self.last_frame = frame;
        self.last_render_at = now;

        if let Some(terminal) = self.terminal.as_mut() {
            terminal.render(&self.last_frame.frame)?;
        }

        Ok(())
    }

    /// Restore the cursor and clean up the terminal.
    fn finish_internal(&mut self) -> io::Result<()> {
        if self.finished {
            return Ok(());
        }
        self.finished = true;
        if let Some(terminal) = self.terminal.take() {
            terminal.finish(&self.last_frame.frame)?;
        }
        Ok(())
    }
}

impl Drop for InteractiveReporter {
    fn drop(&mut self) {
        // Restore cursor if finish() was never called (e.g. channel closed without a
        // Failed event). Ignore errors — we're already in Drop, best-effort is fine.
        if !self.finished {
            let _ = self.finish_internal();
        }
    }
}

impl Default for InteractiveReporter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_render_throttled() -> io::Result<()> {
        let mut reporter = InteractiveReporter::new();

        reporter.on_event(InstallEvent::Started {
            command: "orix install".to_string(),
        })?;

        let frame1 = reporter.last_frame.plain.clone();
        assert!(!frame1.is_empty());

        reporter.on_event(InstallEvent::Started {
            command: "orix install".to_string(),
        })?;

        assert_eq!(frame1, reporter.last_frame.plain);
        Ok(())
    }

    #[test]
    fn test_render_finished() -> io::Result<()> {
        let mut reporter = InteractiveReporter::new();

        reporter.on_event(InstallEvent::Started {
            command: "orix install".to_string(),
        })?;

        reporter.on_event(InstallEvent::Finished {
            installed: 5,
            duration: Duration::from_millis(100),
        })?;

        assert!(reporter.last_frame.plain.contains("Done in"));
        Ok(())
    }

    #[test]
    fn test_colored_reporter_has_ansi() -> io::Result<()> {
        let mut reporter = InteractiveReporter::with_color(ColorMode::Always);

        reporter.on_event(InstallEvent::Started {
            command: "orix install".to_string(),
        })?;

        assert!(
            reporter.last_frame.frame.starts_with('\x1b'),
            "colored frame should start with ANSI escape"
        );
        Ok(())
    }

    #[test]
    fn test_plain_reporter_no_ansi() -> io::Result<()> {
        let mut reporter = InteractiveReporter::with_color(ColorMode::Never);

        reporter.on_event(InstallEvent::Started {
            command: "orix install".to_string(),
        })?;

        assert!(
            !reporter.last_frame.frame.starts_with('\x1b'),
            "plain frame should not contain ANSI"
        );
        Ok(())
    }
}
