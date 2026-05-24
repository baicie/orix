//! UI frame rendering.

mod sections;
mod steps;
mod util;

#[cfg(test)]
mod tests;

use super::color::Theme;
use super::state::InstallState;

/// A rendered frame with both colored and plain representations.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct RenderedFrame {
    /// The full frame string (may contain ANSI escape sequences).
    pub frame: String,
    /// The frame stripped of ANSI codes, used for row count calculations.
    pub plain: String,
    /// Number of visual rows the plain frame occupies.
    pub row_count: usize,
}

impl RenderedFrame {
    #[allow(dead_code)]
    fn new(frame: String, width: usize) -> Self {
        let plain = super::strip_ansi(&frame);
        let row_count = super::terminal::visual_row_count(&plain, width);
        Self {
            frame,
            plain,
            row_count,
        }
    }
}

/// Renders an `InstallState` into a printable string frame.
pub struct FrameRenderer {
    /// Terminal width for bar calculations.
    pub width: usize,
    /// Whether to show recent packages list.
    pub show_recent_packages: bool,
    /// The color theme.
    pub(super) theme: Theme,
}

impl FrameRenderer {
    #[allow(dead_code)]
    pub fn new(width: usize) -> Self {
        Self {
            width,
            show_recent_packages: true,
            theme: Theme::plain(),
        }
    }

    /// Create a renderer with an explicit theme.
    pub fn with_theme(width: usize, theme: Theme) -> Self {
        Self {
            width,
            show_recent_packages: true,
            theme,
        }
    }

    /// Render the current state into a complete frame.
    pub fn render(&self, state: &InstallState) -> RenderedFrame {
        let mut colored = String::new();
        let mut plain = String::new();

        self.push_header(&mut colored, &mut plain, state);
        self.push_summary(&mut colored, &mut plain, state);
        self.push_phases(&mut colored, &mut plain, state);

        if state.failed {
            self.push_error(&mut colored, &mut plain, state);
        } else if state.finished {
            self.push_done(&mut colored, &mut plain, state);
        }

        if !colored.ends_with('\n') {
            colored.push('\n');
        }
        if !plain.ends_with('\n') {
            plain.push('\n');
        }

        let row_count = super::terminal::visual_row_count(&plain, self.width);

        RenderedFrame {
            frame: colored,
            plain,
            row_count,
        }
    }

    /// Render the portion below the static command header.
    pub fn render_body(&self, state: &InstallState) -> RenderedFrame {
        let mut colored = String::new();
        let mut plain = String::new();

        self.push_phases(&mut colored, &mut plain, state);

        if state.failed {
            self.push_error(&mut colored, &mut plain, state);
        } else if state.finished {
            self.push_done(&mut colored, &mut plain, state);
        }

        if !colored.ends_with('\n') {
            colored.push('\n');
        }
        if !plain.ends_with('\n') {
            plain.push('\n');
        }

        let row_count = super::terminal::visual_row_count(&plain, self.width);

        RenderedFrame {
            frame: colored,
            plain,
            row_count,
        }
    }
}
