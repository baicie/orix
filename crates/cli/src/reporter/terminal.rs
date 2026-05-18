//! Terminal control using crossterm for in-place UI updates.

use std::io::{self, IsTerminal, Write};

use crossterm::{
    cursor::{MoveTo, Show},
    queue,
    terminal::{self, Clear, ClearType},
};

/// A terminal that can redraw frames in-place, hiding the cursor during updates.
pub struct LiveTerminal<W: Write> {
    writer: W,
    /// Whether the cursor has been hidden.
    hidden_cursor: bool,
    /// Whether this is the first frame (used to decide whether to clear screen).
    is_first_frame: bool,
}

impl<W: Write> LiveTerminal<W> {
    /// Create a new live terminal writer.
    pub fn new(writer: W) -> Self {
        Self {
            writer,
            hidden_cursor: false,
            is_first_frame: true,
        }
    }

    /// Render a frame, clearing the previous one in-place.
    pub fn render(&mut self, frame: &str) -> io::Result<()> {
        if self.is_first_frame {
            // First frame: clear entire screen for a clean slate.
            queue!(self.writer, Clear(ClearType::All))?;
            self.is_first_frame = false;
        } else {
            // Subsequent frames: move to top-left, clear from cursor to end of screen.
            queue!(self.writer, MoveTo(0, 0))?;
            queue!(self.writer, Clear(ClearType::FromCursorDown))?;
        }

        write!(self.writer, "{frame}")?;
        self.writer.flush()?;

        Ok(())
    }

    /// Render the final frame and restore the cursor.
    pub fn finish(mut self, frame: &str) -> io::Result<()> {
        queue!(self.writer, Clear(ClearType::FromCursorDown))?;
        write!(self.writer, "{frame}")?;
        self.writer.flush()?;
        queue!(self.writer, Show)?;
        self.writer.flush()?;
        self.hidden_cursor = false;
        Ok(())
    }
}

impl<W: Write> Drop for LiveTerminal<W> {
    fn drop(&mut self) {
        if self.hidden_cursor {
            let _ = queue!(self.writer, Show);
            let _ = self.writer.flush();
        }
    }
}

/// Check whether stdout is connected to a terminal.
#[allow(dead_code)]
pub fn stdout_is_terminal() -> bool {
    io::stdout().is_terminal()
}

/// Check whether stderr is connected to a terminal.
pub fn stderr_is_terminal() -> bool {
    io::stderr().is_terminal()
}

/// Get the current terminal width in columns.
pub fn terminal_width() -> usize {
    terminal::size()
        .map(|(width, _)| width as usize)
        .unwrap_or(80)
        .max(20)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_terminal_width_defaults() {
        let w = terminal_width();
        assert!(w >= 20);
    }
}
