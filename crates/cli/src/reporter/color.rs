//! Semantic color theme for the install progress UI.
//!
//! This module provides a `Theme` abstraction that translates semantic intent
//! (success, failure, running, etc.) into ANSI escape sequences.
//! All ANSI details are isolated here so the frame renderer stays clean.

use std::borrow::Cow;

/// Whether colors are enabled for the current output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ColorMode {
    /// Automatically enable colors when stderr is a TTY and NO_COLOR is not set.
    #[default]
    Auto,
    /// Always enable colors (useful for screenshots or manual piping).
    Always,
    /// Never enable colors.
    Never,
}

/// A theme that applies semantic styling to text.
#[derive(Debug, Clone)]
pub struct Theme {
    enabled: bool,
}

impl Theme {
    /// Create a new theme with the given color mode.
    pub fn new(mode: ColorMode, is_terminal: bool, no_color: bool) -> Self {
        let enabled = match mode {
            ColorMode::Always => true,
            ColorMode::Never => false,
            ColorMode::Auto => is_terminal && !no_color,
        };
        Self { enabled }
    }

    /// Returns whether ANSI colors are active.
    #[inline]
    #[allow(dead_code)]
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Title text (bold).
    #[inline]
    pub fn title<'a>(&self, s: &'a str) -> Cow<'a, str> {
        if self.enabled {
            Cow::Owned(crossterm::style::Stylize::bold(s).to_string())
        } else {
            Cow::Borrowed(s)
        }
    }

    /// Dim separator line.
    #[inline]
    pub fn dim<'a>(&self, s: &'a str) -> Cow<'a, str> {
        if self.enabled {
            Cow::Owned(crossterm::style::Stylize::dim(s).to_string())
        } else {
            Cow::Borrowed(s)
        }
    }

    /// Dim label text (e.g., "Packages:", "Registry:").
    #[inline]
    pub fn label<'a>(&self, s: &'a str) -> Cow<'a, str> {
        if self.enabled {
            Cow::Owned(crossterm::style::Stylize::dim(s).to_string())
        } else {
            Cow::Borrowed(s)
        }
    }

    /// Added count (green).
    #[inline]
    pub fn added<'a>(&self, s: &'a str) -> Cow<'a, str> {
        if self.enabled {
            Cow::Owned(crossterm::style::Stylize::green(s).to_string())
        } else {
            Cow::Borrowed(s)
        }
    }

    /// Removed count (dim when zero, red/yellow when non-zero).
    #[inline]
    pub fn removed<'a>(&self, s: &'a str) -> Cow<'a, str> {
        if !self.enabled {
            return Cow::Borrowed(s);
        }
        if s == "0" || s == "-0" {
            Cow::Owned(crossterm::style::Stylize::dim(s).to_string())
        } else {
            Cow::Owned(crossterm::style::Stylize::red(s).to_string())
        }
    }

    /// Success symbol and text (green).
    #[inline]
    pub fn success<'a>(&self, s: &'a str) -> Cow<'a, str> {
        if self.enabled {
            Cow::Owned(crossterm::style::Stylize::green(s).to_string())
        } else {
            Cow::Borrowed(s)
        }
    }

    /// Running symbol and text (cyan).
    #[inline]
    pub fn running<'a>(&self, s: &'a str) -> Cow<'a, str> {
        if self.enabled {
            Cow::Owned(crossterm::style::Stylize::cyan(s).to_string())
        } else {
            Cow::Borrowed(s)
        }
    }

    /// Pending symbol and text (dim).
    #[inline]
    pub fn pending<'a>(&self, s: &'a str) -> Cow<'a, str> {
        if self.enabled {
            Cow::Owned(crossterm::style::Stylize::dim(s).to_string())
        } else {
            Cow::Borrowed(s)
        }
    }

    /// Failed symbol and text (red).
    #[inline]
    pub fn failed<'a>(&self, s: &'a str) -> Cow<'a, str> {
        if self.enabled {
            Cow::Owned(crossterm::style::Stylize::red(s).to_string())
        } else {
            Cow::Borrowed(s)
        }
    }

    /// URL text (cyan).
    #[inline]
    pub fn url<'a>(&self, s: &'a str) -> Cow<'a, str> {
        if self.enabled {
            Cow::Owned(crossterm::style::Stylize::cyan(s).to_string())
        } else {
            Cow::Borrowed(s)
        }
    }

    /// Error title (bold red).
    #[inline]
    pub fn error_title<'a>(&self, s: &'a str) -> Cow<'a, str> {
        if self.enabled {
            Cow::Owned(
                crossterm::style::Stylize::red(crossterm::style::Stylize::bold(s)).to_string(),
            )
        } else {
            Cow::Borrowed(s)
        }
    }

    /// Hint title (yellow).
    #[inline]
    pub fn hint_title<'a>(&self, s: &'a str) -> Cow<'a, str> {
        if self.enabled {
            Cow::Owned(crossterm::style::Stylize::yellow(s).to_string())
        } else {
            Cow::Borrowed(s)
        }
    }

    /// Done summary line (bold green).
    #[inline]
    pub fn done<'a>(&self, s: &'a str) -> Cow<'a, str> {
        if self.enabled {
            Cow::Owned(
                crossterm::style::Stylize::green(crossterm::style::Stylize::bold(s)).to_string(),
            )
        } else {
            Cow::Borrowed(s)
        }
    }

    /// Lockfile written (green).
    #[inline]
    pub fn lockfile_written<'a>(&self, s: &'a str) -> Cow<'a, str> {
        if self.enabled {
            Cow::Owned(crossterm::style::Stylize::green(s).to_string())
        } else {
            Cow::Borrowed(s)
        }
    }

    /// Lockfile unchanged (dim green).
    #[inline]
    pub fn lockfile_unchanged<'a>(&self, s: &'a str) -> Cow<'a, str> {
        if self.enabled {
            Cow::Owned(
                crossterm::style::Stylize::green(crossterm::style::Stylize::dim(s)).to_string(),
            )
        } else {
            Cow::Borrowed(s)
        }
    }

    /// Returns a theme that produces plain text with no ANSI codes.
    #[inline]
    pub fn plain() -> Self {
        Self { enabled: false }
    }

    /// Returns a theme that always produces ANSI colors.
    #[inline]
    #[allow(dead_code)]
    pub fn always_color() -> Self {
        Self { enabled: true }
    }
}

/// Check whether the `NO_COLOR` environment variable is set.
#[inline]
pub fn no_color_env() -> bool {
    std::env::var("NO_COLOR").is_ok()
}

/// Strip ANSI escape sequences from a string.
pub fn strip_ansi(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut in_escape = false;
    let mut chars = s.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            in_escape = true;
            // Skip everything until we hit a letter that terminates the sequence
            while let Some(&c) = chars.peek() {
                chars.next();
                if c.is_ascii_alphabetic() {
                    in_escape = false;
                    break;
                }
            }
        } else if !in_escape {
            result.push(ch);
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_plain_produces_no_ansi() {
        let theme = Theme::plain();
        let s = theme.success("✓ Done");
        assert!(!s.starts_with('\x1b'));
    }

    #[test]
    fn theme_always_color_produces_ansi() {
        let theme = Theme::always_color();
        let s = theme.success("✓ Done");
        assert!(s.starts_with('\x1b'));
    }

    #[test]
    fn strip_ansi_removes_escape_sequences() {
        let colored = "\x1b[32m✓ Done\x1b[0m";
        let stripped = strip_ansi(colored);
        assert_eq!(stripped, "✓ Done");
    }

    #[test]
    fn strip_ansi_passthrough_plain_text() {
        let plain = "no color here";
        assert_eq!(strip_ansi(plain), plain);
    }

    #[test]
    fn removed_zero_is_dim() {
        let theme = Theme::always_color();
        let s = theme.removed("0");
        assert!(s.starts_with('\x1b'));
    }

    #[test]
    fn removed_non_zero_is_red() {
        let theme = Theme::always_color();
        let s = theme.removed("3");
        assert!(s.starts_with('\x1b'));
    }
}
