//! TTY interactive reporter with dynamic frame updates.

use std::io;
use std::time::{Duration, Instant};

use super::frame::FrameRenderer;
use super::state::InstallState;
use super::terminal::{terminal_width, LiveTerminal};
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
}

impl InteractiveReporter {
    /// Create a new interactive reporter using stderr.
    pub fn new() -> Self {
        Self {
            state: InstallState::default(),
            terminal: Some(LiveTerminal::new(io::stderr())),
            last_rendered_frame: String::new(),
            last_render_at: Instant::now() - Duration::from_secs(1),
            min_render_interval: Duration::from_millis(33),
        }
    }

    /// Process an install event and re-render if needed.
    pub fn on_event(&mut self, event: InstallEvent) -> io::Result<()> {
        self.state.apply(event);

        let force = self.state.finished || self.state.failed;
        self.render(force)
    }

    fn render(&mut self, force: bool) -> io::Result<()> {
        let now = Instant::now();

        if !force && now.duration_since(self.last_render_at) < self.min_render_interval {
            return Ok(());
        }

        let width = terminal_width();
        let renderer = FrameRenderer::new(width);
        let frame = renderer.render(&self.state);

        if !force && frame == self.last_rendered_frame {
            return Ok(());
        }

        self.last_rendered_frame = frame.clone();
        self.last_render_at = now;

        if let Some(terminal) = self.terminal.as_mut() {
            terminal.render(&frame)?;
        }

        if force {
            if let Some(terminal) = self.terminal.take() {
                terminal.finish(&frame)?;
            }
        }

        Ok(())
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
    fn test_render_throttled() {
        let mut reporter = InteractiveReporter::new();

        reporter
            .on_event(InstallEvent::Started {
                command: "orix install".to_string(),
            })
            .expect("reporter should not fail");

        let frame1 = reporter.last_rendered_frame.clone();
        assert!(!frame1.is_empty());

        // Sending an identical Started event should not re-render (throttled).
        reporter
            .on_event(InstallEvent::Started {
                command: "orix install".to_string(),
            })
            .expect("reporter should not fail");

        // Frame should be unchanged due to throttling + identical frame dedup.
        assert_eq!(frame1, reporter.last_rendered_frame);
    }

    #[test]
    fn test_render_finished() {
        let mut reporter = InteractiveReporter::new();

        reporter
            .on_event(InstallEvent::Started {
                command: "orix install".to_string(),
            })
            .expect("reporter should not fail");

        reporter
            .on_event(InstallEvent::Finished {
                installed: 5,
                duration: Duration::from_millis(100),
            })
            .expect("reporter should not fail");

        assert!(reporter.last_rendered_frame.contains("Done in"));
    }
}
