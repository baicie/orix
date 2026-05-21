//! Lifecycle script execution for orix.
//!
//! Handles both:
//! - User-initiated scripts via `orix run <script>`
//! - Automatic lifecycle scripts during install (preinstall, postinstall, etc.)
//!
//! Security model: project scripts are enabled by default; dependency scripts
//! require an explicit allow-list entry via `allow-scripts` config.

/// Path separator as a string slice (matches the platform separator character).
pub(crate) const PATH_SEP: &str = if cfg!(windows) { ";" } else { ":" };

/// Virtual store directory under `node_modules` (must match `crates/linker`).
pub const VIRTUAL_STORE_DIR: &str = ".orix";

use std::path::PathBuf;
use std::process::ExitStatus;
use std::time::Duration;

use orix_domain::PackageId;

/// Lifecycle event names (npm convention).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleEvent {
    /// Runs before package files are added to the staging area.
    Preinstall,
    /// Runs after the package is installed.
    Install,
    /// Runs after the package is installed and all scripts have completed.
    Postinstall,
    /// Runs after `npm install` and before `npm publish`.
    Prepare,
    /// Runs before tarball is created.
    PrepublishOnly,
    /// Runs before tarball is packed.
    Prepack,
    /// Runs after tarball is created.
    Postpack,
    /// Runs after package is published.
    Publish,
    /// Runs after package is published.
    Postpublish,
}

impl LifecycleEvent {
    /// Return the script name used in package.json.
    pub fn script_name(&self) -> &'static str {
        match self {
            LifecycleEvent::Preinstall => "preinstall",
            LifecycleEvent::Install => "install",
            LifecycleEvent::Postinstall => "postinstall",
            LifecycleEvent::Prepare => "prepare",
            LifecycleEvent::PrepublishOnly => "prepublishOnly",
            LifecycleEvent::Prepack => "prepack",
            LifecycleEvent::Postpack => "postpack",
            LifecycleEvent::Publish => "publish",
            LifecycleEvent::Postpublish => "postpublish",
        }
    }
}

/// Kind of script execution.
#[derive(Debug, Clone)]
pub enum ScriptKind {
    /// User-initiated run: `orix run <name> [args...]`
    UserRun {
        /// Name of the script to run.
        name: String,
        /// Additional arguments passed after `--`.
        args: Vec<String>,
    },
    /// Automatic lifecycle event.
    Lifecycle {
        /// The lifecycle event that triggered execution.
        event: LifecycleEvent,
        /// Package in which the script is defined.
        package: PackageId,
    },
}

/// Output from a single script execution.
#[derive(Debug, Clone)]
pub struct ScriptOutput {
    /// Script name (e.g., "build", "prebuild").
    pub name: String,
    /// Process exit status.
    pub status: ExitStatus,
    /// Wall-clock duration.
    pub duration: Duration,
}

/// Script execution error.
#[derive(thiserror::Error, Debug)]
pub enum ScriptError {
    /// The requested script is not defined in the package's package.json.
    #[error("script `{0}` not found in {1}")]
    MissingScript(String, PathBuf),

    /// The script exited with a non-zero status code.
    #[error("script `{name}` failed with exit code {code:?}")]
    Failed {
        /// Name of the script that failed.
        name: String,
        /// Exit code returned by the process.
        code: Option<i32>,
    },

    /// The script process was killed by a signal.
    #[error("script `{name}` was terminated by signal")]
    Terminated {
        /// Name of the script that was terminated.
        name: String,
    },

    /// Script execution was skipped because of `--ignore-scripts`.
    #[error("script execution is disabled by --ignore-scripts")]
    Disabled,

    /// Failed to spawn the subprocess.
    #[error("failed to spawn script `{name}`: {source}")]
    Spawn {
        /// Name of the script that could not be spawned.
        name: String,
        /// The underlying I/O error.
        source: std::io::Error,
    },
}

/// Create a successful ExitStatus (exit code 0).
#[cfg(unix)]
pub(crate) fn success_status() -> ExitStatus {
    use std::os::unix::process::ExitStatusExt;
    std::process::ExitStatus::from_raw(0)
}

/// Create a successful ExitStatus on non-Unix platforms.
#[cfg(not(unix))]
#[allow(clippy::unwrap_used)]
pub(crate) fn success_status() -> ExitStatus {
    std::process::Command::new("cmd")
        .args(["/C", "exit /B 0"])
        .spawn()
        .unwrap()
        .wait()
        .unwrap()
}
