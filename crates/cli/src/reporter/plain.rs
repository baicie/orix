//! Plain text reporter for CI and non-TTY environments.

use std::io::{self, Write};
use std::time::{Duration, Instant};

use orix_core::reporter::{InstallEvent, InstallPhase, LockfileStatus};

/// Reporter that emits one-line-per-event text output.
/// Suitable for CI logs and non-interactive terminals.
pub struct PlainReporter {
    writer: io::Stderr,
    resolve_progress: ProgressThrottle,
    fetch_progress: ProgressThrottle,
    link_progress: ProgressThrottle,
}

impl PlainReporter {
    /// Create a new plain reporter writing to stderr.
    pub fn new() -> Self {
        Self {
            writer: io::stderr(),
            resolve_progress: ProgressThrottle::default(),
            fetch_progress: ProgressThrottle::default(),
            link_progress: ProgressThrottle::default(),
        }
    }

    /// Process an install event.
    pub fn on_event(&mut self, event: InstallEvent) -> io::Result<()> {
        match event {
            InstallEvent::Started { command } => {
                writeln!(self.writer, "{command}")?;
            }

            InstallEvent::RegistrySelected { url, .. } => {
                writeln!(self.writer, "registry: {url}")?;
            }

            InstallEvent::DirectPackages { count, .. } => {
                writeln!(self.writer, "packages: {count} direct")?;
            }

            InstallEvent::PhaseStarted { phase } => {
                writeln!(
                    self.writer,
                    "[{}] {}",
                    phase_index(phase),
                    phase_label(phase)
                )?;
            }

            InstallEvent::Resolved {
                direct,
                total,
                added,
                removed,
            } => {
                writeln!(
                    self.writer,
                    "resolved: +{} direct, +{} total (+{} -{})",
                    direct, total, added, removed
                )?;
            }

            InstallEvent::ResolveProgress { done, total, .. } => {
                if self.resolve_progress.should_emit(done, total) {
                    writeln!(self.writer, "resolving packages: {done}/{total}")?;
                }
            }

            InstallEvent::FetchProgress { done, total, .. } => {
                if self.fetch_progress.should_emit(done, total) {
                    writeln!(self.writer, "fetching packages: {done}/{total}")?;
                }
            }

            InstallEvent::LinkProgress { done, total, .. } => {
                if self.link_progress.should_emit(done, total) {
                    writeln!(self.writer, "linking packages: {done}/{total}")?;
                }
            }

            InstallEvent::PackageFetched { .. } => {}

            InstallEvent::PhaseFinished { phase } => {
                writeln!(self.writer, "finished {}", phase_label(phase))?;
            }

            InstallEvent::Lockfile { status } => match status {
                LockfileStatus::Unchanged => {
                    writeln!(self.writer, "lockfile unchanged")?;
                }
                LockfileStatus::Written => {
                    writeln!(self.writer, "lockfile written")?;
                }
                LockfileStatus::Skipped => {
                    writeln!(self.writer, "lockfile skipped")?;
                }
            },

            InstallEvent::Finished {
                installed,
                duration,
            } => {
                writeln!(
                    self.writer,
                    "done: {} packages in {:.2}s",
                    installed,
                    duration.as_secs_f64()
                )?;
            }

            InstallEvent::Failed {
                phase,
                message,
                hint,
                ..
            } => {
                if let Some(phase) = phase {
                    writeln!(self.writer, "failed in {}: {}", phase, message)?;
                } else {
                    writeln!(self.writer, "failed: {message}")?;
                }

                if let Some(hint) = hint {
                    writeln!(self.writer, "hint: {hint}")?;
                }
            }

            InstallEvent::ScriptsPhaseStarted { event } => {
                writeln!(self.writer, "[scripts] starting lifecycle: {event}")?;
            }

            InstallEvent::ScriptFinished {
                name,
                duration_ms,
                exit_code,
            } => {
                let code_str = exit_code.map_or("?".to_string(), |c| c.to_string());
                writeln!(
                    self.writer,
                    "[scripts] finished {name} ({duration_ms}ms, exit {code_str})"
                )?;
            }

            InstallEvent::ScriptsPhaseSkipped { reason } => {
                writeln!(self.writer, "[scripts] skipped: {reason}")?;
            }
        }

        self.writer.flush()
    }

    /// Output the final summary frame (called when the reporter is dropped or finished).
    #[allow(dead_code)]
    pub fn done(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Default for PlainReporter {
    fn default() -> Self {
        Self::new()
    }
}

struct ProgressThrottle {
    last_done: usize,
    last_emit: Option<Instant>,
    step: usize,
    min_interval: Duration,
}

impl Default for ProgressThrottle {
    fn default() -> Self {
        Self {
            last_done: 0,
            last_emit: None,
            step: 100,
            min_interval: Duration::from_secs(1),
        }
    }
}

impl ProgressThrottle {
    fn should_emit(&mut self, done: usize, total: usize) -> bool {
        let now = Instant::now();
        let is_first = self.last_emit.is_none();
        let is_complete = total > 0 && done >= total;
        let crossed_step = done >= self.last_done.saturating_add(self.step);
        let interval_elapsed = self
            .last_emit
            .map(|last| now.duration_since(last) >= self.min_interval)
            .unwrap_or(true);

        if is_first || is_complete || crossed_step || interval_elapsed {
            self.last_done = done;
            self.last_emit = Some(now);
            true
        } else {
            false
        }
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
    fn progress_throttle_emits_first_steps_and_completion() {
        let mut throttle = ProgressThrottle::default();

        assert!(throttle.should_emit(0, 1000));
        assert!(!throttle.should_emit(1, 1000));
        assert!(throttle.should_emit(100, 1000));
        assert!(!throttle.should_emit(150, 1000));
        assert!(throttle.should_emit(1000, 1000));
    }
}
