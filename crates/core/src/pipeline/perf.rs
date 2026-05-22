//! Structured performance metrics for debug logging.
//!
//! Enable with: `RUST_LOG=orix::perf=debug` (or `orix=debug`).
//!
//! Finer-grained targets in other crates (same filter):
//! - `store_import` / `hash_walk_ms` / `write_lock_ms` — CAS import per package
//! - `link_graph` / `import_files_ms` / `virtual_deps_ms` — linker sub-phases
//! - `link_package` — packages slower than 200ms
//! - `prune_layout` — stale layout cleanup before link

use orix_fetcher::FetchReport;
use orix_linker::LinkReport;
use tracing::debug;

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
