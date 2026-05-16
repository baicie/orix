//! Progress reporting and spinners for CLI operations.

#![allow(dead_code)]

use std::sync::Arc;

use tokio::sync::RwLock;

/// Accumulated install progress state shared across all report lines.
#[derive(Debug, Clone, Default)]
pub struct InstallProgress {
    pub phase: InstallPhase,
    pub packages_done: u32,
    pub packages_total: u32,
    pub fetching: Option<String>,
    pub linking: bool,
    pub failures: Vec<String>,
}

impl InstallProgress {
    pub fn new(total: u32) -> Self {
        Self {
            packages_total: total,
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InstallPhase {
    #[default]
    Resolving,
    Fetching,
    Linking,
    WritingLockfile,
    Done,
}

impl std::fmt::Display for InstallPhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InstallPhase::Resolving => write!(f, "Resolving dependencies"),
            InstallPhase::Fetching => write!(f, "Fetching packages"),
            InstallPhase::Linking => write!(f, "Linking packages"),
            InstallPhase::WritingLockfile => write!(f, "Writing lockfile"),
            InstallPhase::Done => write!(f, "Done"),
        }
    }
}

/// A shared, atomic install progress reporter.
#[derive(Default)]
pub struct ProgressReporter {
    state: Arc<RwLock<InstallProgress>>,
}

impl ProgressReporter {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn set_phase(&self, phase: InstallPhase) {
        let mut guard = self.state.write().await;
        guard.phase = phase;
    }

    pub async fn set_total(&self, total: u32) {
        let mut guard = self.state.write().await;
        guard.packages_total = total;
    }

    pub async fn package_done(&self) {
        let mut guard = self.state.write().await;
        guard.packages_done += 1;
    }

    pub async fn set_fetching(&self, pkg: String) {
        let mut guard = self.state.write().await;
        guard.fetching = Some(pkg);
    }

    pub async fn finish_fetching(&self) {
        let mut guard = self.state.write().await;
        guard.fetching = None;
    }

    pub async fn add_failure(&self, msg: String) {
        let mut guard = self.state.write().await;
        guard.failures.push(msg);
    }

    pub async fn snapshot(&self) -> InstallProgress {
        self.state.read().await.clone()
    }
}
