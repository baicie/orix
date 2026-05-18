//! Terminal text styling with ANSI color support.
//!
//! Architecture:
//!
//! ```txt
//! ColorChoice (CLI arg)
//!   -> ColorState::from_choice() -> ColorState
//!   -> Style (ANSI escape sequences or no-ops)
//!   -> applied to strings via .paint()
//! ```

use std::io::{self, IsTerminal};

/// ANSI escape sequence constants for crossterm colors and attributes.
mod ansi {
    // Text attributes
    pub const RESET: &str = "\x1b[0m";
    pub const BOLD: &str = "\x1b[1m";
    pub const DIM: &str = "\x1b[2m";

    // Standard colors (foreground)
    pub const RED: &str = "\x1b[31m";
    pub const GREEN: &str = "\x1b[32m";
    pub const YELLOW: &str = "\x1b[33m";
    pub const BLUE: &str = "\x1b[34m";
    pub const CYAN: &str = "\x1b[36m";
    pub const DARK_GREY: &str = "\x1b[90m";
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorState {
    Enabled,
    Disabled,
}

impl ColorState {
    /// Determine color state from the CLI --color choice and terminal type.
    ///
    /// - `Always` -> `Enabled`
    /// - `Never`  -> `Disabled`
    /// - `Auto`   -> terminal detection
    pub fn from_choice(choice: super::ColorChoice) -> Self {
        let is_tty = io::stdout().is_terminal();

        let enabled = match choice {
            super::ColorChoice::Always => true,
            super::ColorChoice::Never => false,
            super::ColorChoice::Auto => is_tty,
        };

        if enabled {
            Self::Enabled
        } else {
            Self::Disabled
        }
    }
}

/// Pre-defined style tokens used across CLI output.
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub enum Style {
    /// No styling (pass-through).
    None,
    /// Bold text.
    Bold,
    /// Dim / subdued text.
    Dim,
    /// Green color (success, checkmarks).
    Success,
    /// Red color (errors, failures).
    Error,
    /// Yellow / amber color (warnings, hints).
    Warning,
    /// Cyan color (info, package names).
    Info,
    /// Blue color (registry URLs).
    Registry,
    /// Package name (cyan bright).
    PackageName,
    /// Package version (dim blue).
    PackageVersion,
    /// Duration / timing (dim).
    Duration,
    /// Phase label (pending / running).
    PhasePending,
    /// Phase label (in progress).
    PhaseRunning,
    /// Phase label (done).
    PhaseDone,
    /// Phase label (failed).
    PhaseFailed,
    /// Diff bar: added packages (+).
    DiffAdded,
    /// Diff bar: removed packages (-).
    DiffRemoved,
    /// Header separator line.
    Header,
    /// Muted / secondary text.
    Muted,
    /// Error prefix symbol.
    ErrorPrefix,
    /// Info prefix symbol.
    InfoPrefix,
    /// Checkmark symbol.
    Checkmark,
    /// Cross symbol.
    Cross,
    /// Bullet / dash prefix.
    Bullet,
}

impl Style {
    /// Apply this style to a string, returning either ANSI-escaped or plain text.
    pub fn paint(self, text: &str, state: ColorState) -> String {
        if state == ColorState::Disabled {
            return text.to_string();
        }

        use ansi::*;

        match self {
            Self::None => text.to_string(),
            Self::Bold => format!("{BOLD}{text}{RESET}"),
            Self::Dim => format!("{DIM}{text}{RESET}"),
            Self::Success => format!("{GREEN}{text}{RESET}"),
            Self::Error => format!("{RED}{text}{RESET}"),
            Self::Warning => format!("{YELLOW}{text}{RESET}"),
            Self::Info => format!("{CYAN}{text}{RESET}"),
            Self::Registry => format!("{BLUE}{text}{RESET}"),
            Self::PackageName => format!("{BOLD}{CYAN}{text}{RESET}"),
            Self::PackageVersion => format!("{BLUE}{DIM}{text}{RESET}"),
            Self::Duration => format!("{DIM}{text}{RESET}"),
            Self::PhasePending => format!("{DARK_GREY}{text}{RESET}"),
            Self::PhaseRunning => format!("{YELLOW}{text}{RESET}"),
            Self::PhaseDone => format!("{GREEN}{text}{RESET}"),
            Self::PhaseFailed => format!("{RED}{text}{RESET}"),
            Self::DiffAdded => format!("{GREEN}{text}{RESET}"),
            Self::DiffRemoved => format!("{RED}{text}{RESET}"),
            Self::Header => format!("{BLUE}{text}{RESET}"),
            Self::Muted => format!("{DARK_GREY}{text}{RESET}"),
            Self::ErrorPrefix => format!("{RED}{BOLD}{text}{RESET}"),
            Self::InfoPrefix => format!("{CYAN}{BOLD}{text}{RESET}"),
            Self::Checkmark => format!("{GREEN}{BOLD}{text}{RESET}"),
            Self::Cross => format!("{RED}{BOLD}{text}{RESET}"),
            Self::Bullet => format!("{DARK_GREY}{text}{RESET}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_style_paint_disabled() {
        assert_eq!(Style::Success.paint("ok", ColorState::Disabled), "ok");
        assert_eq!(Style::Bold.paint("hello", ColorState::Disabled), "hello");
    }

    #[test]
    fn test_style_paint_enabled() {
        let result = Style::Success.paint("ok", ColorState::Enabled);
        assert!(result.contains("ok"));
        assert!(result.contains("\x1b["));
    }

    #[test]
    fn test_color_state_from_choice() {
        assert_eq!(
            ColorState::from_choice(crate::ColorChoice::Always),
            ColorState::Enabled
        );
        assert_eq!(
            ColorState::from_choice(crate::ColorChoice::Never),
            ColorState::Disabled
        );
        assert_eq!(
            ColorState::from_choice(crate::ColorChoice::Auto),
            ColorState::Disabled
        );
    }

    #[test]
    fn test_style_variants() {
        for style in &[
            Style::None,
            Style::Bold,
            Style::Dim,
            Style::Success,
            Style::Error,
            Style::Warning,
            Style::Info,
            Style::Registry,
            Style::PackageName,
            Style::PackageVersion,
            Style::Duration,
            Style::PhasePending,
            Style::PhaseRunning,
            Style::PhaseDone,
            Style::PhaseFailed,
            Style::DiffAdded,
            Style::DiffRemoved,
            Style::Header,
            Style::Muted,
            Style::ErrorPrefix,
            Style::InfoPrefix,
            Style::Checkmark,
            Style::Cross,
            Style::Bullet,
        ] {
            let colored = style.paint("test", ColorState::Enabled);
            let plain = style.paint("test", ColorState::Disabled);
            assert_eq!(plain, "test");
            assert!(colored.contains("test"));
        }
    }
}
