//! Install progress events and types.

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Installation phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum InstallPhase {
    /// Resolving dependencies from registry.
    Resolve,
    /// Fetching package tarballs.
    Fetch,
    /// Linking packages into node_modules.
    Link,
    /// Writing lockfile.
    Lockfile,
    /// Running lifecycle scripts.
    Scripts,
}

impl std::fmt::Display for InstallPhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InstallPhase::Resolve => write!(f, "resolve"),
            InstallPhase::Fetch => write!(f, "fetch"),
            InstallPhase::Link => write!(f, "link"),
            InstallPhase::Lockfile => write!(f, "lockfile"),
            InstallPhase::Scripts => write!(f, "scripts"),
        }
    }
}

/// Lockfile operation status.
#[derive(Debug, Clone)]
pub enum LockfileStatus {
    /// Lockfile was not modified.
    Unchanged,
    /// Lockfile was written or updated.
    Written,
    /// Lockfile write was skipped (e.g., frozen mode).
    Skipped,
}

/// Events emitted by the install pipeline.
///
/// Events only describe *what happened*, not *how to display* it.
#[derive(Debug, Clone)]
pub enum InstallEvent {
    /// Install command started.
    Started {
        /// Full command name (e.g., "orix install").
        command: String,
    },

    /// Registry was selected and resolved.
    RegistrySelected {
        /// Registry URL.
        url: String,
        /// Whether authentication token is configured.
        authenticated: bool,
    },

    /// Direct packages declared in package.json are known.
    DirectPackages {
        /// Number of direct packages.
        count: usize,
        /// Package names.
        names: Vec<String>,
    },

    /// A pipeline phase started.
    PhaseStarted {
        /// Which phase.
        phase: InstallPhase,
    },

    /// Dependency resolution completed.
    Resolved {
        /// Number of direct dependencies.
        direct: usize,
        /// Total packages in the resolved graph.
        total: usize,
        /// Packages added since last install.
        added: usize,
        /// Packages removed since last install.
        removed: usize,
    },

    /// Resolve progress update (emitted after each package is resolved).
    ResolveProgress {
        /// Packages resolved so far.
        done: usize,
        /// Total packages to resolve.
        total: usize,
        /// Currently resolved package name (for display).
        package: Option<String>,
    },

    /// Fetch progress update.
    FetchProgress {
        /// Packages completed.
        done: usize,
        /// Total packages to fetch.
        total: usize,
        /// Currently fetched package name (for display).
        package: Option<String>,
    },

    /// Link progress update (emitted as each package is linked into node_modules).
    LinkProgress {
        /// Packages linked so far.
        done: usize,
        /// Total packages to link.
        total: usize,
        /// Currently linked package name (for display).
        package: Option<String>,
    },

    /// A single package was fetched and imported into the store.
    PackageFetched {
        /// Package name.
        name: String,
        /// Package version (if available).
        version: Option<String>,
        /// Whether this was served from cache.
        cached: bool,
    },

    /// A pipeline phase completed.
    PhaseFinished {
        /// Which phase.
        phase: InstallPhase,
    },

    /// Lockfile operation result.
    Lockfile {
        /// Lockfile status.
        status: LockfileStatus,
    },

    /// Install finished successfully.
    Finished {
        /// Number of packages installed.
        installed: usize,
        /// Wall-clock duration.
        duration: Duration,
    },

    /// Install failed.
    Failed {
        /// Phase where failure occurred (if known).
        phase: Option<InstallPhase>,
        /// Error message.
        message: String,
        /// Optional hint for the user.
        hint: Option<String>,
    },

    /// A scripts phase (preinstall/install/postinstall) started.
    ScriptsPhaseStarted {
        /// Which lifecycle event.
        event: String,
    },

    /// A single script finished during a scripts phase.
    ScriptFinished {
        /// Script name.
        name: String,
        /// Duration in milliseconds.
        duration_ms: u64,
        /// Exit code if available.
        exit_code: Option<i32>,
    },

    /// All scripts were skipped (e.g., --ignore-scripts).
    ScriptsPhaseSkipped {
        /// Reason for skipping.
        reason: String,
    },
}
