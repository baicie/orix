//! Install state machine.

use std::collections::VecDeque;
use std::time::Duration;

use orix_core::reporter::{InstallEvent, InstallPhase, LockfileStatus};

/// Status of a single pipeline step.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub enum StepStatus {
    /// Not yet started.
    Pending,
    /// Currently running.
    Running,
    /// Completed successfully.
    Done,
    /// Failed.
    Failed,
    /// Skipped.
    Skipped,
}

/// Per-phase state.
#[derive(Debug, Clone)]
pub struct PhaseState {
    /// Current status.
    pub status: StepStatus,
    /// Number of completed units.
    pub done: usize,
    /// Total units.
    pub total: usize,
}

impl Default for PhaseState {
    fn default() -> Self {
        Self {
            status: StepStatus::Pending,
            done: 0,
            total: 0,
        }
    }
}

/// Aggregated install state, updated by applying events.
#[derive(Debug, Clone)]
pub struct InstallState {
    /// Command that was run.
    pub command: String,

    /// Registry URL.
    pub registry: Option<String>,
    /// Whether registry has authentication.
    pub authenticated: bool,

    /// Number of direct packages.
    pub direct_packages: usize,
    /// Total packages in resolved graph.
    pub total_packages: usize,

    /// Packages added since last install.
    pub added: usize,
    /// Packages removed since last install.
    pub removed: usize,

    /// Per-phase state.
    pub resolve: PhaseState,
    pub fetch: PhaseState,
    pub link: PhaseState,
    pub lockfile: PhaseState,
    pub scripts: PhaseState,

    /// Lockfile write status.
    pub lockfile_status: Option<LockfileStatus>,

    /// Recently completed packages (for display).
    pub recent_packages: VecDeque<String>,
    /// Maximum recent packages to keep.
    pub max_recent_packages: usize,

    /// Whether install finished.
    pub finished: bool,
    /// Whether install failed.
    pub failed: bool,
    /// Error message (if failed).
    pub error_message: Option<String>,
    /// Error hint (if failed).
    pub error_hint: Option<String>,

    /// Total duration.
    pub duration: Option<Duration>,
}

impl Default for InstallState {
    fn default() -> Self {
        Self {
            command: "orix install".to_string(),

            registry: None,
            authenticated: false,

            direct_packages: 0,
            total_packages: 0,

            added: 0,
            removed: 0,

            resolve: PhaseState::default(),
            fetch: PhaseState::default(),
            link: PhaseState::default(),
            lockfile: PhaseState::default(),
            scripts: PhaseState::default(),

            lockfile_status: None,

            recent_packages: VecDeque::new(),
            max_recent_packages: 5,

            finished: false,
            failed: false,
            error_message: None,
            error_hint: None,

            duration: None,
        }
    }
}

impl InstallState {
    /// Apply an event to update state.
    pub fn apply(&mut self, event: InstallEvent) {
        match event {
            InstallEvent::Started { command } => {
                self.command = command;
            }

            InstallEvent::RegistrySelected { url, authenticated } => {
                self.registry = Some(url);
                self.authenticated = authenticated;
            }

            InstallEvent::DirectPackages { count, names } => {
                self.direct_packages = count;

                for name in names {
                    self.push_recent_package(name);
                }
            }

            InstallEvent::PhaseStarted { phase } => {
                self.phase_mut(phase).status = StepStatus::Running;
                if phase == InstallPhase::Link {
                    self.recent_packages.clear();
                }
            }

            InstallEvent::ResolveProgress {
                done,
                total,
                package,
            } => {
                self.resolve.status = if done >= total && total > 0 {
                    StepStatus::Done
                } else {
                    StepStatus::Running
                };
                self.resolve.done = done;
                self.resolve.total = total;

                if let Some(package) = package {
                    self.push_recent_package(package);
                }
            }

            InstallEvent::Resolved {
                direct,
                total,
                added,
                removed,
            } => {
                self.direct_packages = direct;
                self.total_packages = total;
                self.added = added;
                self.removed = removed;
                self.resolve.status = StepStatus::Done;
                self.resolve.done = total;
                self.resolve.total = self.resolve.total.max(total);
            }

            InstallEvent::FetchProgress {
                done,
                total,
                package,
            } => {
                self.fetch.status = if done >= total && total > 0 {
                    StepStatus::Done
                } else {
                    StepStatus::Running
                };

                self.fetch.done = done;
                self.fetch.total = total;

                if let Some(package) = package {
                    self.push_recent_package(package);
                }
            }

            InstallEvent::LinkProgress {
                done,
                total,
                package,
            } => {
                self.link.status = if done >= total && total > 0 {
                    StepStatus::Done
                } else {
                    StepStatus::Running
                };
                self.link.done = done;
                self.link.total = total;

                if let Some(package) = package.filter(|name| !name.is_empty()) {
                    self.push_recent_package(package);
                }
            }

            InstallEvent::PackageFetched {
                name,
                version,
                cached,
            } => {
                let label = match version {
                    Some(version) => {
                        if cached {
                            format!("{name}@{version} cached")
                        } else {
                            format!("{name}@{version}")
                        }
                    }
                    None => {
                        if cached {
                            format!("{name} cached")
                        } else {
                            name
                        }
                    }
                };

                self.push_recent_package(label);
            }

            InstallEvent::PhaseFinished { phase } => {
                self.phase_mut(phase).status = StepStatus::Done;
            }

            InstallEvent::Lockfile { status } => {
                self.lockfile_status = Some(status);
                self.lockfile.status = StepStatus::Done;
            }

            InstallEvent::Finished {
                installed: _,
                duration,
            } => {
                self.finished = true;
                self.duration = Some(duration);

                if self.resolve.status == StepStatus::Running {
                    self.resolve.status = StepStatus::Done;
                }

                if self.fetch.status == StepStatus::Running {
                    self.fetch.status = StepStatus::Done;
                }

                if self.link.status == StepStatus::Running {
                    self.link.status = StepStatus::Done;
                }

                if self.lockfile.status == StepStatus::Running {
                    self.lockfile.status = StepStatus::Done;
                }
            }

            InstallEvent::Failed {
                phase,
                message,
                hint,
            } => {
                self.failed = true;
                self.error_message = Some(message);
                self.error_hint = hint;

                if let Some(phase) = phase {
                    self.phase_mut(phase).status = StepStatus::Failed;
                }
            }

            InstallEvent::ScriptsPhaseStarted { .. } => {
                self.scripts.status = StepStatus::Running;
            }

            InstallEvent::ScriptFinished { .. } => {
                // Each script increments done count.
                self.scripts.done += 1;
            }

            InstallEvent::ScriptsPhaseSkipped { .. } => {
                self.scripts.status = StepStatus::Skipped;
            }
        }
    }

    fn phase_mut(&mut self, phase: InstallPhase) -> &mut PhaseState {
        match phase {
            InstallPhase::Resolve => &mut self.resolve,
            InstallPhase::Fetch => &mut self.fetch,
            InstallPhase::Link => &mut self.link,
            InstallPhase::Lockfile => &mut self.lockfile,
            InstallPhase::Scripts => &mut self.scripts,
        }
    }

    fn push_recent_package(&mut self, package: String) {
        if package.is_empty() {
            return;
        }

        if self.recent_packages.back() == Some(&package) {
            return;
        }

        self.recent_packages.push_back(package);

        while self.recent_packages.len() > self.max_recent_packages {
            self.recent_packages.pop_front();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_apply_started() {
        let mut state = InstallState::default();
        state.apply(InstallEvent::Started {
            command: "orix add lodash".to_string(),
        });
        assert_eq!(state.command, "orix add lodash");
    }

    #[test]
    fn test_apply_resolved() {
        let mut state = InstallState::default();
        state.apply(InstallEvent::Resolved {
            direct: 3,
            total: 12,
            added: 2,
            removed: 1,
        });
        assert_eq!(state.direct_packages, 3);
        assert_eq!(state.total_packages, 12);
        assert_eq!(state.added, 2);
        assert_eq!(state.removed, 1);
        assert_eq!(state.resolve.status, StepStatus::Done);
    }

    #[test]
    fn test_apply_resolved_preserves_discovered_total() {
        let mut state = InstallState::default();
        state.apply(InstallEvent::ResolveProgress {
            done: 1000,
            total: 1600,
            package: Some("last-scanned-package".to_string()),
        });
        state.apply(InstallEvent::Resolved {
            direct: 10,
            total: 1000,
            added: 1000,
            removed: 0,
        });

        assert_eq!(state.resolve.status, StepStatus::Done);
        assert_eq!(state.resolve.done, 1000);
        assert_eq!(state.resolve.total, 1600);
    }

    #[test]
    fn test_apply_link_progress() {
        let mut state = InstallState::default();
        state.apply(InstallEvent::LinkProgress {
            done: 0,
            total: 5,
            package: None,
        });
        assert_eq!(state.link.status, StepStatus::Running);
        assert_eq!(state.link.done, 0);
        assert_eq!(state.link.total, 5);

        state.apply(InstallEvent::LinkProgress {
            done: 3,
            total: 5,
            package: Some("lodash".to_string()),
        });
        assert_eq!(state.link.status, StepStatus::Running);
        assert_eq!(state.link.done, 3);
        assert_eq!(
            state.recent_packages.back().map(String::as_str),
            Some("lodash")
        );

        state.apply(InstallEvent::LinkProgress {
            done: 5,
            total: 5,
            package: Some("react".to_string()),
        });
        assert_eq!(state.link.status, StepStatus::Done);
    }

    fn test_apply_fetch_progress() {
        let mut state = InstallState::default();
        state.apply(InstallEvent::FetchProgress {
            done: 0,
            total: 10,
            package: None,
        });
        assert_eq!(state.fetch.status, StepStatus::Running);
        assert_eq!(state.fetch.done, 0);
        assert_eq!(state.fetch.total, 10);

        state.apply(InstallEvent::FetchProgress {
            done: 10,
            total: 10,
            package: Some("lodash".to_string()),
        });
        assert_eq!(state.fetch.status, StepStatus::Done);
    }

    #[test]
    fn test_apply_resolve_progress_preserves_large_counts() {
        let mut state = InstallState::default();
        state.apply(InstallEvent::ResolveProgress {
            done: 1001,
            total: 1200,
            package: Some("large-graph-package".to_string()),
        });

        assert_eq!(state.resolve.status, StepStatus::Running);
        assert_eq!(state.resolve.done, 1001);
        assert_eq!(state.resolve.total, 1200);
    }

    #[test]
    fn test_apply_failed() {
        let mut state = InstallState::default();
        state.apply(InstallEvent::Failed {
            phase: Some(InstallPhase::Fetch),
            message: "network error".to_string(),
            hint: Some("check connection".to_string()),
        });
        assert!(state.failed);
        assert_eq!(state.error_message, Some("network error".to_string()));
        assert_eq!(state.error_hint, Some("check connection".to_string()));
        assert_eq!(state.fetch.status, StepStatus::Failed);
    }

    #[test]
    fn test_push_recent_package_dedup() {
        let mut state = InstallState::default();
        state.push_recent_package("lodash".to_string());
        state.push_recent_package("lodash".to_string());
        state.push_recent_package("lodash".to_string());
        assert_eq!(state.recent_packages.len(), 1);
    }

    #[test]
    fn test_push_recent_package_max() {
        let mut state = InstallState {
            max_recent_packages: 3,
            ..Default::default()
        };
        for i in 0..5 {
            state.push_recent_package(format!("pkg-{i}"));
        }
        assert_eq!(state.recent_packages.len(), 3);
        assert_eq!(state.recent_packages.front(), Some(&"pkg-2".to_string()));
    }

    #[test]
    fn test_finished_cleans_up_running_phases() {
        let mut state = InstallState::default();
        state.resolve.status = StepStatus::Running;
        state.fetch.status = StepStatus::Running;
        state.apply(InstallEvent::Finished {
            installed: 10,
            duration: Duration::from_millis(500),
        });
        assert!(state.finished);
        assert_eq!(state.resolve.status, StepStatus::Done);
        assert_eq!(state.fetch.status, StepStatus::Done);
    }
}
