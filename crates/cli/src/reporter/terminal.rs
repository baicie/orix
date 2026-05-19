//! Terminal control using crossterm for in-place UI updates.

use std::io::{self, IsTerminal, Write};

use crossterm::{
    cursor::{Hide, MoveDown, MoveToColumn, MoveUp, Show},
    queue,
    terminal::{self, Clear, ClearType},
};

/// Count the number of visual rows a frame takes given a terminal width.
/// Accounts for unicode characters that may occupy multiple columns.
/// Also strips ANSI escape sequences before computing width.
pub fn visual_row_count(frame: &str, columns: usize) -> usize {
    if columns == 0 {
        return 1;
    }

    // Strip ANSI before counting to avoid escape sequences inflating the count.
    let plain = super::strip_ansi(frame);

    let rows = plain
        .lines()
        .map(|line| {
            let width = unicode_width::UnicodeWidthStr::width(line);
            width.div_ceil(columns).max(1)
        })
        .sum::<usize>();

    rows.max(1)
}

/// A terminal that can redraw frames in-place, hiding the cursor during updates.
pub struct LiveTerminal<W: Write> {
    writer: W,
    /// Number of rows rendered in the last frame.
    last_rows: usize,
    /// Whether the cursor has been hidden.
    hidden_cursor: bool,
}

impl<W: Write> LiveTerminal<W> {
    /// Create a new live terminal writer.
    pub fn new(writer: W) -> Self {
        Self {
            writer,
            last_rows: 0,
            hidden_cursor: false,
        }
    }

    /// Render a frame, clearing the previous one in-place.
    pub fn render(&mut self, frame: &str) -> io::Result<()> {
        self.hide_cursor_once()?;
        let columns = terminal_width();
        self.clear_previous(columns)?;

        write!(self.writer, "{frame}")?;
        self.writer.flush()?;

        self.last_rows = visual_row_count(frame, columns);

        Ok(())
    }

    /// Restore the cursor after the final frame has already been rendered.
    pub fn finish(mut self, _frame: &str) -> io::Result<()> {
        queue!(self.writer, Show)?;
        self.writer.flush()?;
        self.hidden_cursor = false;
        Ok(())
    }

    fn hide_cursor_once(&mut self) -> io::Result<()> {
        if !self.hidden_cursor {
            queue!(self.writer, Hide)?;
            self.hidden_cursor = true;
        }
        Ok(())
    }

    fn clear_previous(&mut self, columns: usize) -> io::Result<()> {
        if self.last_rows == 0 || columns == 0 {
            return Ok(());
        }

        queue!(self.writer, MoveUp(self.last_rows as u16), MoveToColumn(0))?;

        for row in 0..self.last_rows {
            queue!(self.writer, Clear(ClearType::CurrentLine))?;

            if row + 1 < self.last_rows {
                queue!(self.writer, MoveDown(1), MoveToColumn(0))?;
            }
        }

        queue!(
            self.writer,
            MoveUp(self.last_rows.saturating_sub(1) as u16),
            MoveToColumn(0)
        )?;
        self.writer.flush()?;

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

    #[test]
    fn test_visual_row_count_single_line() {
        assert_eq!(visual_row_count("hello", 80), 1);
        assert_eq!(visual_row_count("hello", 3), 2);
    }

    #[test]
    fn test_visual_row_count_multiline() {
        assert_eq!(visual_row_count("hello\nworld", 80), 2);
    }

    #[test]
    fn test_visual_row_count_counts_empty_lines() {
        assert_eq!(visual_row_count("hello\n\nworld", 80), 3);
    }

    #[test]
    fn test_visual_row_count_zero_width() {
        assert_eq!(visual_row_count("hello", 0), 1);
    }

    #[test]
    fn test_visual_row_count_strips_ansi() {
        let colored = "\x1b[32mgreen\x1b[0m and plain";
        assert_eq!(visual_row_count(colored, 80), 1);
    }

    #[test]
    fn test_render_clears_previous_frame_from_top() -> io::Result<()> {
        let mut terminal = LiveTerminal::new(Vec::new());

        terminal.render("orix install\nPackages: +4 direct, +50 total\n")?;
        terminal.render("orix install\nDone\n")?;

        let output = String::from_utf8(terminal.writer.clone())
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
        assert!(
            output.contains(&format!(
                "{}{}{}",
                MoveUp(2),
                MoveToColumn(0),
                Clear(ClearType::CurrentLine)
            )),
            "second render should move back to the previous frame before clearing; output={output:?}"
        );

        Ok(())
    }
}
