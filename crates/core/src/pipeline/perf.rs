//! Structured performance metrics for debug logging and reporting.
//!
//! Enable with: `RUST_LOG=orix::perf=debug` (or `orix=debug`).
//!
//! Finer-grained targets in other crates (same filter):
//! - `store_import` / `hash_walk_ms` / `write_lock_ms` — CAS import per package
//! - `link_graph` / `import_files_ms` / `virtual_deps_ms` — linker sub-phases
//! - `link_package` — packages slower than 200ms
//! - `prune_layout` — stale layout cleanup before link
//!
//! ## Performance Report Fields (P0)
//!
//! The `PerfReport` struct captures all metrics required for installation profiling:
//!
//! | Field | Description |
//! |-------|-------------|
//! | `workspace_count` | Number of workspace importers |
//! | `direct_dependency_count` | Total direct dependencies (root + workspace) |
//! | `resolved_package_count` | Total packages in dependency graph |
//! | `metadata_requests` | Registry packument request count |
//! | `metadata_cache_hits` | Packument cache hit count |
//! | `tarball_downloads` | Actual tarball downloads |
//! | `tarball_cache_hits` | Tarball cache hits |
//! | `store_imports` | Store import count |
//! | `store_package_hits` | Store package hits |
//! | `linked_packages` | Packages linked to node_modules |
//! | `reused_links` | Layout links reused |
//! | `resolve_ms` | Resolution phase duration |
//! | `fetch_ms` | Fetch phase duration |
//! | `extract_ms` | Extraction phase duration |
//! | `import_ms` | Store import phase duration |
//! | `link_ms` | Link phase duration |
//! | `scripts_ms` | Lifecycle scripts duration |
//! | `total_ms` | Total install duration |

use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::reporter::InstallPhase;
use orix_fetcher::FetchReport;
use orix_linker::LinkReport;

/// Tracing target for performance metrics (filter with `orix::perf=debug`).
pub const PERF_TARGET: &str = "orix::perf";

/// Throughput helper: items per second from count and duration in milliseconds.
pub fn rate_per_sec(count: u64, duration_ms: u64) -> f64 {
    if duration_ms == 0 {
        0.0
    } else {
        count as f64 * 1000.0 / duration_ms as f64
    }
}

/// Percentage of total wall time spent in a phase (0–100).
pub fn phase_share_pct(phase_ms: u64, total_ms: u64) -> f64 {
    if total_ms == 0 {
        0.0
    } else {
        phase_ms as f64 * 100.0 / total_ms as f64
    }
}

/// Slow package record for P7 debugging performance issues.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct SlowPackage {
    /// Package identifier.
    pub package: String,
    /// Phase where slowness occurred.
    pub phase: String,
    /// Duration in milliseconds.
    pub duration_ms: u64,
}

/// Complete performance report for an install operation.
///
/// This struct captures all metrics needed to profile installation performance
/// and identify bottlenecks in the install pipeline.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct PerfReport {
    /// Number of workspace importers.
    pub workspace_count: usize,
    /// Total direct dependencies across all importers.
    pub direct_dependency_count: usize,
    /// Total packages in the resolved dependency graph.
    pub resolved_package_count: usize,

    /// Registry metadata request count.
    pub metadata_requests: u64,
    /// Packument cache hits.
    pub metadata_cache_hits: u64,

    /// Actual tarball downloads.
    pub tarball_downloads: u64,
    /// Tarball cache hits.
    pub tarball_cache_hits: u64,

    /// Store package imports.
    pub store_imports: u64,
    /// Store package hits (packages already in store).
    pub store_package_hits: u64,

    /// Packages linked to node_modules.
    pub linked_packages: u64,
    /// Layout links reused (skipped relink).
    pub reused_links: u64,

    /// Resolution phase duration in milliseconds.
    pub resolve_ms: u64,
    /// Fetch phase duration in milliseconds.
    pub fetch_ms: u64,
    /// Extraction phase duration in milliseconds.
    pub extract_ms: u64,
    /// Store import phase duration in milliseconds.
    pub import_ms: u64,
    /// Link phase duration in milliseconds.
    pub link_ms: u64,
    /// Lifecycle scripts duration in milliseconds.
    pub scripts_ms: u64,
    /// Total install duration in milliseconds.
    pub total_ms: u64,

    /// Whether install used fast path (lockfile matched).
    pub fast_path: bool,

    /// Phase that took the longest (for quick identification).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slowest_phase: Option<InstallPhase>,
    /// Percentage of total time spent in slowest phase.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slowest_phase_pct: Option<f64>,

    /// P7: Slow packages for debugging performance issues.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub slow_packages: Vec<SlowPackage>,
}

#[allow(dead_code)]
impl PerfReport {
    /// Create a new empty performance report.
    #[allow(dead_code)]
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a performance report from install metrics.
    #[allow(clippy::too_many_arguments, dead_code)]
    pub fn from_install(
        workspace_count: usize,
        direct_dependency_count: usize,
        resolved_package_count: usize,
        metadata_requests: u64,
        metadata_cache_hits: u64,
        fetch_report: &FetchReport,
        link_report: &LinkReport,
        resolve_ms: Option<u64>,
        fetch_ms: Option<u64>,
        link_ms: Option<u64>,
        scripts_ms: Option<u64>,
        total_ms: u64,
        fast_path: bool,
    ) -> Self {
        let resolve_ms = resolve_ms.unwrap_or(0);
        let fetch_ms = fetch_ms.unwrap_or(0);
        let link_ms = link_ms.unwrap_or(0);
        let scripts_ms = scripts_ms.unwrap_or(0);

        // Calculate extract_ms and import_ms from fetch_report if available
        let extract_ms = 0; // TODO: Extract from fetch_report when available
        let import_ms = 0; // TODO: Extract from fetch_report when available

        let store_imports = fetch_report.success as u64;
        let store_package_hits = fetch_report.cached as u64;
        let tarball_downloads = fetch_report.success as u64;
        let tarball_cache_hits = fetch_report.cached as u64;
        let linked_packages =
            link_report.hardlinked_files + link_report.copied_files + link_report.symlinks_created;
        let reused_links = if link_report.skipped.is_some() {
            resolved_package_count as u64
        } else {
            0
        };

        // Find slowest phase
        let (slowest_phase, slowest_phase_pct) =
            Self::find_slowest_phase(resolve_ms, fetch_ms, link_ms, scripts_ms, total_ms);

        Self {
            workspace_count,
            direct_dependency_count,
            resolved_package_count,
            metadata_requests,
            metadata_cache_hits,
            tarball_downloads,
            tarball_cache_hits,
            store_imports,
            store_package_hits,
            linked_packages,
            reused_links,
            resolve_ms,
            fetch_ms,
            extract_ms,
            import_ms,
            link_ms,
            scripts_ms,
            total_ms,
            fast_path,
            slowest_phase,
            slowest_phase_pct,
            slow_packages: Vec::new(),
        }
    }

    /// Find the slowest phase and its percentage of total time.
    #[allow(dead_code)]
    fn find_slowest_phase(
        resolve_ms: u64,
        fetch_ms: u64,
        link_ms: u64,
        scripts_ms: u64,
        total_ms: u64,
    ) -> (Option<InstallPhase>, Option<f64>) {
        let phases = [
            (InstallPhase::Resolve, resolve_ms),
            (InstallPhase::Fetch, fetch_ms),
            (InstallPhase::Link, link_ms),
            (InstallPhase::Scripts, scripts_ms),
        ];

        let mut slowest = (None, 0u64);
        for (phase, ms) in phases {
            if ms > slowest.1 {
                slowest = (Some(phase), ms);
            }
        }

        let pct = if total_ms > 0 {
            Some(phase_share_pct(slowest.1, total_ms))
        } else {
            None
        };

        (slowest.0, pct)
    }

    /// Log the performance report as structured debug output.
    #[allow(dead_code)]
    pub fn log(&self) {
        debug!(
            target: PERF_TARGET,
            workspace_count = self.workspace_count,
            direct_dependency_count = self.direct_dependency_count,
            resolved_package_count = self.resolved_package_count,
            metadata_requests = self.metadata_requests,
            metadata_cache_hits = self.metadata_cache_hits,
            tarball_downloads = self.tarball_downloads,
            tarball_cache_hits = self.tarball_cache_hits,
            store_imports = self.store_imports,
            store_package_hits = self.store_package_hits,
            linked_packages = self.linked_packages,
            reused_links = self.reused_links,
            resolve_ms = self.resolve_ms,
            fetch_ms = self.fetch_ms,
            extract_ms = self.extract_ms,
            import_ms = self.import_ms,
            link_ms = self.link_ms,
            scripts_ms = self.scripts_ms,
            total_ms = self.total_ms,
            fast_path = self.fast_path,
            slowest_phase = ?self.slowest_phase,
            slowest_phase_pct = ?self.slowest_phase_pct,
            "install performance report"
        );
    }

    /// Return a summary string for quick terminal output.
    #[allow(dead_code)]
    pub fn summary(&self) -> String {
        format!(
            "Install: {} packages in {}ms ({} via fast path)",
            self.resolved_package_count,
            self.total_ms,
            if self.fast_path { "cached" } else { "fresh" }
        )
    }

    /// Calculate metadata cache hit rate (0.0 to 1.0).
    #[allow(dead_code)]
    pub fn metadata_hit_rate(&self) -> f64 {
        let total = self.metadata_requests + self.metadata_cache_hits;
        if total == 0 {
            0.0
        } else {
            self.metadata_cache_hits as f64 / total as f64
        }
    }

    /// Calculate store hit rate (0.0 to 1.0).
    #[allow(dead_code)]
    pub fn store_hit_rate(&self) -> f64 {
        let total = self.store_imports + self.store_package_hits;
        if total == 0 {
            0.0
        } else {
            self.store_package_hits as f64 / total as f64
        }
    }
}

/// Accumulator for tracking performance metrics during install.
#[derive(Debug, Default)]
#[allow(dead_code)]
pub struct PerfAccumulator {
    pub metadata_requests: u64,
    pub metadata_cache_hits: u64,
    pub tarball_downloads: u64,
    pub tarball_cache_hits: u64,
    pub store_imports: u64,
    pub store_package_hits: u64,
    pub scripts_ms: u64,
}

#[allow(dead_code)]
impl PerfAccumulator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn into_report(self, builder: PerfReportBuilder) -> PerfReport {
        builder
            .metadata_requests(self.metadata_requests)
            .metadata_cache_hits(self.metadata_cache_hits)
            .tarball_downloads(self.tarball_downloads)
            .tarball_cache_hits(self.tarball_cache_hits)
            .store_imports(self.store_imports)
            .store_package_hits(self.store_package_hits)
            .scripts_ms(self.scripts_ms)
            .build()
    }
}

/// Builder pattern for constructing PerfReport.
#[derive(Debug, Default)]
#[allow(dead_code)]
pub struct PerfReportBuilder {
    workspace_count: usize,
    direct_dependency_count: usize,
    resolved_package_count: usize,
    metadata_requests: u64,
    metadata_cache_hits: u64,
    tarball_downloads: u64,
    tarball_cache_hits: u64,
    store_imports: u64,
    store_package_hits: u64,
    linked_packages: u64,
    reused_links: u64,
    resolve_ms: Option<u64>,
    fetch_ms: Option<u64>,
    extract_ms: Option<u64>,
    import_ms: Option<u64>,
    link_ms: Option<u64>,
    scripts_ms: Option<u64>,
    total_ms: Option<u64>,
    fast_path: bool,
}

#[allow(dead_code)]
impl PerfReportBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn workspace_count(mut self, count: usize) -> Self {
        self.workspace_count = count;
        self
    }

    pub fn direct_dependency_count(mut self, count: usize) -> Self {
        self.direct_dependency_count = count;
        self
    }

    pub fn resolved_package_count(mut self, count: usize) -> Self {
        self.resolved_package_count = count;
        self
    }

    pub fn metadata_requests(mut self, count: u64) -> Self {
        self.metadata_requests = count;
        self
    }

    pub fn metadata_cache_hits(mut self, count: u64) -> Self {
        self.metadata_cache_hits = count;
        self
    }

    pub fn tarball_downloads(mut self, count: u64) -> Self {
        self.tarball_downloads = count;
        self
    }

    pub fn tarball_cache_hits(mut self, count: u64) -> Self {
        self.tarball_cache_hits = count;
        self
    }

    pub fn store_imports(mut self, count: u64) -> Self {
        self.store_imports = count;
        self
    }

    pub fn store_package_hits(mut self, count: u64) -> Self {
        self.store_package_hits = count;
        self
    }

    pub fn linked_packages(mut self, count: u64) -> Self {
        self.linked_packages = count;
        self
    }

    pub fn reused_links(mut self, count: u64) -> Self {
        self.reused_links = count;
        self
    }

    pub fn resolve_ms(mut self, ms: u64) -> Self {
        self.resolve_ms = Some(ms);
        self
    }

    pub fn fetch_ms(mut self, ms: u64) -> Self {
        self.fetch_ms = Some(ms);
        self
    }

    pub fn extract_ms(mut self, ms: u64) -> Self {
        self.extract_ms = Some(ms);
        self
    }

    pub fn import_ms(mut self, ms: u64) -> Self {
        self.import_ms = Some(ms);
        self
    }

    pub fn link_ms(mut self, ms: u64) -> Self {
        self.link_ms = Some(ms);
        self
    }

    pub fn scripts_ms(mut self, ms: u64) -> Self {
        self.scripts_ms = Some(ms);
        self
    }

    pub fn total_ms(mut self, ms: u64) -> Self {
        self.total_ms = Some(ms);
        self
    }

    pub fn fast_path(mut self, enabled: bool) -> Self {
        self.fast_path = enabled;
        self
    }

    pub fn build(self) -> PerfReport {
        PerfReport::from_install(
            self.workspace_count,
            self.direct_dependency_count,
            self.resolved_package_count,
            self.metadata_requests,
            self.metadata_cache_hits,
            &FetchReport {
                success: self.store_imports as usize,
                cached: self.store_package_hits as usize,
                store_hits: 0,
                failures: Vec::new(),
            },
            &LinkReport {
                hardlinked_files: self.linked_packages,
                copied_files: 0,
                symlinks_created: 0,
                bytes_saved: 0,
                skipped: if self.reused_links > 0 {
                    Some("reused".to_string())
                } else {
                    None
                },
            },
            self.resolve_ms,
            self.fetch_ms,
            self.link_ms,
            self.scripts_ms,
            self.total_ms.unwrap_or(0),
            self.fast_path,
        )
    }
}

pub(crate) fn log_resolve_phase(
    packages: usize,
    duration_ms: u64,
    direct: usize,
    added: usize,
    removed: usize,
    frozen: bool,
) {
    debug!(
        target: PERF_TARGET,
        phase = "resolve",
        duration_ms,
        packages,
        direct,
        added,
        removed,
        frozen,
        packages_per_sec = rate_per_sec(packages as u64, duration_ms),
        "phase complete"
    );
}

pub(crate) fn log_fetch_phase(
    report: &FetchReport,
    duration_ms: u64,
    scheduled: usize,
    concurrency: usize,
) {
    let success = report.success as u64;
    debug!(
        target: PERF_TARGET,
        phase = "fetch",
        duration_ms,
        scheduled,
        success = report.success,
        failures = report.failures.len(),
        concurrency,
        packages_per_sec = rate_per_sec(success, duration_ms),
        "phase complete"
    );
}

pub(crate) fn log_link_phase(
    report: &LinkReport,
    duration_ms: u64,
    packages: usize,
    layout_cached: bool,
) {
    let files_linked = report.hardlinked_files + report.copied_files;
    debug!(
        target: PERF_TARGET,
        phase = "link",
        duration_ms,
        packages,
        layout_cached,
        hardlinked_files = report.hardlinked_files,
        copied_files = report.copied_files,
        symlinks_created = report.symlinks_created,
        bytes_saved = report.bytes_saved,
        files_per_sec = rate_per_sec(files_linked, duration_ms),
        skipped = report.skipped.as_deref().unwrap_or(""),
        "phase complete"
    );
}

pub(crate) fn log_lockfile_phase(
    duration_ms: u64,
    changed: bool,
    added: usize,
    removed: usize,
    changed_count: usize,
) {
    debug!(
        target: PERF_TARGET,
        phase = "lockfile",
        duration_ms,
        changed,
        added,
        removed,
        changed_count,
        "phase complete"
    );
}

pub(crate) fn log_setup_phase(
    duration_ms: u64,
    workspace_packages: Option<usize>,
    direct_deps: usize,
) {
    debug!(
        target: PERF_TARGET,
        phase = "setup",
        duration_ms,
        workspace_packages = workspace_packages.unwrap_or(0),
        has_workspace = workspace_packages.is_some(),
        direct_deps,
        "phase complete"
    );
}

/// End-of-install summary with per-phase timings and share of total wall time.
#[allow(clippy::too_many_arguments)]
pub(crate) fn log_install_summary(
    total_ms: u64,
    packages: usize,
    resolve_ms: Option<u64>,
    fetch_ms: Option<u64>,
    link_ms: Option<u64>,
    lockfile_ms: Option<u64>,
    fetch_report: &FetchReport,
    link_report: &LinkReport,
    fast_path: bool,
) {
    let resolve = resolve_ms.unwrap_or(0);
    let fetch = fetch_ms.unwrap_or(0);
    let link = link_ms.unwrap_or(0);
    let lockfile = lockfile_ms.unwrap_or(0);
    let accounted = resolve + fetch + link + lockfile;
    let unaccounted_ms = total_ms.saturating_sub(accounted);

    debug!(
        target: PERF_TARGET,
        phase = "install",
        total_ms,
        packages,
        fast_path,
        resolve_ms = resolve_ms.unwrap_or(0),
        fetch_ms = fetch_ms.unwrap_or(0),
        link_ms = link_ms.unwrap_or(0),
        lockfile_ms = lockfile_ms.unwrap_or(0),
        unaccounted_ms,
        resolve_pct = phase_share_pct(resolve, total_ms),
        fetch_pct = phase_share_pct(fetch, total_ms),
        link_pct = phase_share_pct(link, total_ms),
        lockfile_pct = phase_share_pct(lockfile, total_ms),
        fetch_success = fetch_report.success,
        fetch_failures = fetch_report.failures.len(),
        link_hardlinked_files = link_report.hardlinked_files,
        link_copied_files = link_report.copied_files,
        link_symlinks = link_report.symlinks_created,
        link_bytes_saved = link_report.bytes_saved,
        "install complete"
    );
}
