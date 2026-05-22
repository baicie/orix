//! Pipeline submodule.

use super::prelude::*;
/// Options for the install command.
#[derive(Debug, Clone, Default)]
pub struct InstallOpts {
    /// Registry URL override from CLI.
    pub registry: Option<String>,
    /// Global store directory override from CLI.
    pub store_dir: Option<PathBuf>,
    /// Local tarball cache directory override from CLI.
    pub cache_dir: Option<PathBuf>,
    /// Require a lockfile and fail if it doesn't match package.json.
    pub frozen_lockfile: bool,
    /// Only use locally cached packages.
    pub offline: bool,
    /// Re-fetch all packages regardless of cache.
    pub force: bool,
    /// Skip running lifecycle scripts.
    pub ignore_scripts: bool,
    /// Number of concurrent download tasks.
    pub concurrency: usize,
    /// Channel to send progress events to the CLI renderer.
    #[doc(hidden)]
    pub progress_tx: Option<mpsc::Sender<InstallEvent>>,
}

/// Report from an install operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallReport {
    /// Registry URL used for resolution and downloads.
    pub registry: String,
    /// Number of direct dependencies declared by the root manifest.
    pub direct_dependencies: usize,
    /// Number of packages added.
    pub packages_added: usize,
    /// Fetch operation report.
    pub fetch_report: orix_fetcher::FetchReport,
    /// Link operation report.
    pub link_report: LinkReport,
    /// Lockfile diff (if computed).
    pub lockfile_diff: Option<LockfileDiffReport>,
    /// Whether the lockfile changed during this install.
    pub lockfile_changed: bool,
    /// Wall-clock time in seconds.
    pub duration_secs: f64,
    /// Resolve phase duration in milliseconds (None if skipped via fast path).
    pub resolve_ms: Option<u64>,
    /// Fetch phase duration in milliseconds.
    pub fetch_ms: Option<u64>,
    /// Link phase duration in milliseconds.
    pub link_ms: Option<u64>,
    /// Lockfile phase duration in milliseconds.
    pub lockfile_ms: Option<u64>,
}

/// Report from a remove operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoveReport {
    /// Names of removed packages.
    pub removed_packages: Vec<String>,
    /// Install report for the updated graph.
    pub install_report: InstallReport,
}

/// Report from cleaning the tarball cache.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheCleanReport {
    /// Cache directory that was cleaned.
    pub path: PathBuf,
    /// Whether the cache directory existed before cleaning.
    pub existed: bool,
    /// Best-effort number of bytes removed.
    pub bytes_reclaimed: u64,
}

/// Summary of lockfile changes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockfileDiffReport {
    /// Packages added since the previous lockfile.
    pub added: Vec<String>,
    /// Packages removed since the previous lockfile.
    pub removed: Vec<String>,
    /// Packages changed since the previous lockfile.
    pub changed: Vec<String>,
    /// Importers whose specifiers changed.
    pub importers_changed: Vec<String>,
}

pub(crate) fn emit_link_progress(
    tx: &Option<mpsc::Sender<InstallEvent>>,
    done: usize,
    total: usize,
    package: Option<String>,
) {
    send_event(
        tx,
        InstallEvent::LinkProgress {
            done,
            total,
            package,
        },
    );
}

pub(crate) fn send_event(tx: &Option<mpsc::Sender<InstallEvent>>, event: InstallEvent) {
    tracing::trace!(event = ?event, "emit install event");

    if let Some(sender) = tx {
        if let Err(err) = sender.try_send(event) {
            tracing::debug!(error = ?err, "failed to send install progress event");
        }
    }
}

/// Send a link failure event and return it as an `anyhow::Error`.
pub(crate) fn link_error(tx: &Option<mpsc::Sender<InstallEvent>>, msg: String) -> anyhow::Error {
    tracing::error!(error = %msg, "link failed");
    send_event(
        tx,
        InstallEvent::Failed {
            phase: Some(InstallPhase::Link),
            message: msg,
            hint: Some("Check file permissions and disk space.".to_string()),
        },
    );
    anyhow::anyhow!("link failed")
}

/// Log a one-line hint when a heavy link pass may trigger post-install AV scanning on Windows.
pub(crate) fn emit_windows_link_performance_hint(
    config: &Config,
    link_report: &orix_linker::LinkReport,
) {
    #[cfg(windows)]
    {
        if link_report.skipped.is_some() {
            return;
        }
        let touched =
            link_report.hardlinked_files + link_report.copied_files + link_report.symlinks_created;
        if touched < 500 {
            return;
        }
        info!(
            store = %config.store_dir.display(),
            node_modules = %config.node_modules_dir().display(),
            "Windows: if the system feels sluggish after linking, add the store path and project \
             node_modules to Windows Defender exclusions (or keep them on the same NTFS volume)"
        );
    }
    let _ = (config, link_report);
}

pub(crate) fn fetch_failure_hint(failures: &[String]) -> String {
    let joined = failures.join("\n");

    if joined.contains("extract tarball") || joined.contains("unpack") {
        return "The tarball cache may be corrupted. Try `orix cache clean` or rerun with `--force`.".to_string();
    }

    if joined.contains("integrity mismatch") {
        return "The downloaded tarball does not match registry integrity. Try `orix cache clean` and reinstall.".to_string();
    }

    "Check network connection, registry, proxy, or retry with `--force`.".to_string()
}
