//! Plain text reporter for CI and non-TTY environments.

use std::io::{self, Write};

use orix_core::reporter::{InstallEvent, InstallPhase, LockfileStatus};

use crate::styles::{ColorState, Style};

/// Reporter that emits one-line-per-event text output.
/// Suitable for CI logs and non-interactive terminals.
pub struct PlainReporter {
    writer: io::Stderr,
    color_state: ColorState,
}

impl PlainReporter {
    /// Create a new plain reporter writing to stderr.
    pub fn new(color_state: ColorState) -> Self {
        Self {
            writer: io::stderr(),
            color_state,
        }
    }

    fn styled_write(&mut self, text: &str, style: Style) -> io::Result<()> {
        let colored = style.paint(text, self.color_state);
        writeln!(self.writer, "{}", colored)
    }

    fn write_muted(&mut self, text: &str) -> io::Result<()> {
        self.styled_write(text, Style::Muted)
    }

    /// Process an install event.
    pub fn on_event(&mut self, event: InstallEvent) -> io::Result<()> {
        match event {
            InstallEvent::Started { command } => {
                self.styled_write(&command, Style::Bold)?;
            }

            InstallEvent::RegistrySelected { url, .. } => {
                let label = Style::Muted.paint("registry:", self.color_state);
                let value = Style::Registry.paint(&url, self.color_state);
                writeln!(self.writer, "{} {}", label, value)?;
            }

            InstallEvent::DirectPackages { count, .. } => {
                let label = Style::Muted.paint("packages:", self.color_state);
                let value = Style::Success.paint(&count.to_string(), self.color_state);
                writeln!(self.writer, "{} {} direct", label, value)?;
            }

            InstallEvent::PhaseStarted { phase } => {
                let idx = phase_index(phase);
                let label = phase_label(phase);
                let idx_str = Style::Muted.paint(&format!("[{}]", idx), self.color_state);
                let label_str = Style::PhaseRunning.paint(label, self.color_state);
                writeln!(self.writer, "{} {}", idx_str, label_str)?;
            }

            InstallEvent::Resolved {
                direct,
                total,
                added,
                removed,
            } => {
                let label = Style::Muted.paint("resolved:", self.color_state);
                let direct_str = Style::Success.paint(&format!("+{}", direct), self.color_state);
                let total_str = Style::Success.paint(&format!("+{}", total), self.color_state);
                let diff = Self::format_diff(added, removed, self.color_state);
                writeln!(
                    self.writer,
                    "{} {} direct, {} total {}",
                    label, direct_str, total_str, diff
                )?;
            }

            InstallEvent::ResolveProgress { done, total, .. } => {
                let label = Style::Muted.paint("resolving packages:", self.color_state);
                let done_str = Style::Bold.paint(&format!("{}/{}", done, total), self.color_state);
                writeln!(self.writer, "{} {}", label, done_str)?;
            }

            InstallEvent::FetchProgress { done, total, .. } => {
                let label = Style::Muted.paint("fetching packages:", self.color_state);
                let done_str = Style::Bold.paint(&format!("{}/{}", done, total), self.color_state);
                writeln!(self.writer, "{} {}", label, done_str)?;
            }

            InstallEvent::PackageFetched {
                name,
                version,
                cached,
            } => {
                let cached_str = if cached {
                    Style::Muted.paint(" (cached)", self.color_state)
                } else {
                    String::new()
                };

                let name_str = Style::PackageName.paint(&name, self.color_state);
                let version_str = version
                    .map(|v| Style::PackageVersion.paint(&format!("@{}", v), self.color_state))
                    .unwrap_or_default();

                if cached_str.is_empty() && version_str.is_empty() {
                    writeln!(
                        self.writer,
                        "  {} {}",
                        Style::Checkmark.paint("\u{2713}", self.color_state),
                        name_str
                    )?;
                } else {
                    writeln!(
                        self.writer,
                        "  {} {}{}{}",
                        Style::Checkmark.paint("\u{2713}", self.color_state),
                        name_str,
                        version_str,
                        cached_str
                    )?;
                }
            }

            InstallEvent::PhaseFinished { phase } => {
                let label = phase_label(phase);
                self.styled_write(&format!("finished {}", label), Style::Muted)?;
            }

            InstallEvent::Lockfile { status } => match status {
                LockfileStatus::Unchanged => {
                    let icon = Style::Checkmark.paint("\u{2713}", self.color_state);
                    let label = Style::Muted.paint("lockfile unchanged", self.color_state);
                    writeln!(self.writer, "{} {}", icon, label)?;
                }
                LockfileStatus::Written => {
                    let icon = Style::Checkmark.paint("\u{2713}", self.color_state);
                    let label = Style::Success.paint("lockfile written", self.color_state);
                    writeln!(self.writer, "{} {}", icon, label)?;
                }
                LockfileStatus::Skipped => {
                    self.write_muted("- lockfile skipped")?;
                }
            },

            InstallEvent::Finished {
                installed,
                duration,
            } => {
                let label = Style::Muted.paint("done:", self.color_state);
                let count =
                    Style::Success.paint(&format!("{} packages", installed), self.color_state);
                let timing = Style::Duration
                    .paint(&format!("{:.2}s", duration.as_secs_f64()), self.color_state);
                writeln!(self.writer, "{} {} in {}", label, count, timing)?;
            }

            InstallEvent::Failed {
                phase,
                message,
                hint,
                ..
            } => {
                let icon = Style::Cross.paint("\u{2717}", self.color_state);
                if let Some(phase) = phase {
                    let phase_str = phase_label(phase);
                    let label = Style::PhaseFailed.paint(phase_str, self.color_state);
                    let msg = Style::Error.paint(&message, self.color_state);
                    writeln!(self.writer, "{} failed in {}: {}", icon, label, msg)?;
                } else {
                    let msg = Style::Error.paint(&message, self.color_state);
                    writeln!(self.writer, "{} {}", icon, msg)?;
                }

                if let Some(hint) = hint {
                    let hint_label = Style::Warning.paint("hint:", self.color_state);
                    let hint_text = Style::Warning.paint(&hint, self.color_state);
                    writeln!(self.writer, "{} {}", hint_label, hint_text)?;
                }
            }

            InstallEvent::ScriptsPhaseStarted { event } => {
                let label = Style::Muted.paint("[scripts]", self.color_state);
                let text = Style::PhaseRunning
                    .paint(&format!("starting lifecycle: {}", event), self.color_state);
                writeln!(self.writer, "{} {}", label, text)?;
            }

            InstallEvent::ScriptFinished {
                name,
                duration_ms,
                exit_code,
            } => {
                let label = Style::Muted.paint("[scripts]", self.color_state);
                let name_str = Style::PackageName.paint(&name, self.color_state);
                let timing = Style::Duration.paint(&format!("{}ms", duration_ms), self.color_state);
                let code_str = exit_code.map_or("?".to_string(), |c| c.to_string());
                let exit_str = if exit_code == Some(0) {
                    Style::Success.paint(&code_str, self.color_state)
                } else {
                    Style::Error.paint(&code_str, self.color_state)
                };
                writeln!(
                    self.writer,
                    "{} finished {} ({}ms, exit {})",
                    label, name_str, timing, exit_str
                )?;
            }

            InstallEvent::ScriptsPhaseSkipped { reason } => {
                let label = Style::Muted.paint("[scripts]", self.color_state);
                let reason_str = Style::Warning.paint(&reason, self.color_state);
                writeln!(self.writer, "{} skipped: {}", label, reason_str)?;
            }
        }

        self.writer.flush()
    }

    /// Output the final summary frame (called when the reporter is dropped or finished).
    #[allow(dead_code)]
    pub fn done(&mut self) -> io::Result<()> {
        Ok(())
    }

    fn format_diff(added: usize, removed: usize, color_state: ColorState) -> String {
        if added > 0 && removed > 0 {
            format!(
                "({}+ {})",
                Style::DiffAdded.paint(&format!("+{}", added), color_state),
                Style::DiffRemoved.paint(&format!("-{}", removed), color_state)
            )
        } else if added > 0 {
            format!(
                "({})",
                Style::DiffAdded.paint(&format!("+{}", added), color_state)
            )
        } else if removed > 0 {
            format!(
                "({})",
                Style::DiffRemoved.paint(&format!("-{}", removed), color_state)
            )
        } else {
            String::new()
        }
    }
}

impl Default for PlainReporter {
    fn default() -> Self {
        Self::new(ColorState::Disabled)
    }
}

fn phase_index(phase: InstallPhase) -> usize {
    match phase {
        InstallPhase::Resolve => 1,
        InstallPhase::Fetch => 2,
        InstallPhase::Link => 3,
        InstallPhase::Lockfile => 4,
        InstallPhase::Scripts => 5,
    }
}

fn phase_label(phase: InstallPhase) -> &'static str {
    match phase {
        InstallPhase::Resolve => "resolving dependencies",
        InstallPhase::Fetch => "fetching packages",
        InstallPhase::Link => "linking dependencies",
        InstallPhase::Lockfile => "writing lockfile",
        InstallPhase::Scripts => "running lifecycle scripts",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_phase_index() {
        assert_eq!(phase_index(InstallPhase::Resolve), 1);
        assert_eq!(phase_index(InstallPhase::Fetch), 2);
        assert_eq!(phase_index(InstallPhase::Link), 3);
        assert_eq!(phase_index(InstallPhase::Lockfile), 4);
        assert_eq!(phase_index(InstallPhase::Scripts), 5);
    }

    #[test]
    fn test_phase_label() {
        assert_eq!(phase_label(InstallPhase::Resolve), "resolving dependencies");
        assert_eq!(phase_label(InstallPhase::Fetch), "fetching packages");
        assert_eq!(phase_label(InstallPhase::Link), "linking dependencies");
        assert_eq!(phase_label(InstallPhase::Lockfile), "writing lockfile");
        assert_eq!(
            phase_label(InstallPhase::Scripts),
            "running lifecycle scripts"
        );
    }

    #[test]
    fn test_format_diff() {
        let cs = ColorState::Disabled;
        assert!(PlainReporter::format_diff(5, 0, cs).contains("+5"));
        assert!(PlainReporter::format_diff(0, 3, cs).contains("-3"));
        assert!(PlainReporter::format_diff(5, 3, cs).contains("+5"));
        assert!(PlainReporter::format_diff(0, 0, cs).is_empty());
    }
}
