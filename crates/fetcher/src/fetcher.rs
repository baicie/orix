//! Package tarball fetcher.
//!
//! P4 optimization: Check store before fetching to enable layered reuse:
//! - store hit -> skip tarball fetch, extract, import entirely
//! - tarball hit -> extract and import (but skip download)
//! - tarball miss -> download, extract, import

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, Semaphore};
use tracing::{debug, info_span, warn, Instrument};

use orix_domain::{check_platform_compatibility, DependencyGraph};
use orix_store::Store;

use crate::{extract_tarball, TarballCache};

/// Fetches package tarballs and imports them into the store.
pub struct Fetcher {
    cache: Arc<TarballCache>,
    store: Arc<Store>,
    /// If true, only use locally cached tarballs (fail if not found).
    offline: bool,
    /// If true, bypass cache and re-fetch all packages.
    force: bool,
    /// Project root directory for resolving relative patch file paths.
    project_root: PathBuf,
}

/// Progress events emitted while fetching packages.
#[derive(Debug, Clone)]
pub enum FetchEvent {
    /// A package was fetched, extracted, and imported into the store.
    PackageFetched(String),
    /// A package failed during fetch, extraction, or import.
    PackageFailed(String),
}

impl Fetcher {
    /// Create a new fetcher.
    pub fn new(cache: TarballCache, store: Store, project_root: PathBuf) -> Self {
        Self {
            cache: Arc::new(cache),
            store: Arc::new(store),
            offline: false,
            force: false,
            project_root,
        }
    }

    /// Configure offline mode (only use cached tarballs, skip network).
    pub fn with_offline(mut self, offline: bool) -> Self {
        self.offline = offline;
        self
    }

    /// Configure force mode (bypass cache, re-fetch all packages).
    pub fn with_force(mut self, force: bool) -> Self {
        self.force = force;
        self
    }

    /// Configure project root (for resolving patch file paths).
    pub fn with_project_root(mut self, root: PathBuf) -> Self {
        self.project_root = root;
        self
    }

    /// Returns a reference to the underlying store.
    pub fn store(&self) -> &Store {
        &self.store
    }

    /// Fetch all packages in the dependency graph into the store.
    ///
    /// Packages with platform/os/cpu restrictions that don't match the current machine
    /// are skipped with a warning (MVP behavior — no hard failure).
    pub async fn fetch_all(
        &self,
        graph: &DependencyGraph,
        concurrency: usize,
        progress_tx: Option<mpsc::Sender<FetchEvent>>,
    ) -> Result<FetchReport> {
        let started = Instant::now();
        let sem = Arc::new(Semaphore::new(concurrency.max(1)));
        let mut handles = Vec::new();
        let mut scheduled = 0usize;

        for pkg in graph.packages() {
            // Skip workspace packages (they have no tarball and live on disk).
            if pkg.tarball.is_empty() {
                debug!(package = %pkg.id, "skipping workspace package (no tarball)");
                continue;
            }

            // Platform compatibility check: skip with warning if incompatible.
            if let Some(mismatch) = check_platform_compatibility(&pkg.os, &pkg.cpu) {
                warn!(
                    package = %pkg.id,
                    reason = %mismatch,
                    "Skipping platform-incompatible package"
                );
                continue;
            }

            // P4: Check if package already exists in store before fetching tarball.
            // This enables layered reuse: store hit -> skip fetch entirely.
            if !self.force && self.store.contains(&pkg.id) {
                debug!(package = %pkg.id, "store hit, skipping fetch");
                continue;
            }

            scheduled += 1;
            let sem = Arc::clone(&sem);
            let cache = Arc::clone(&self.cache);
            let store = Arc::clone(&self.store);
            let pkg_id = pkg.id.clone();
            let tarball_url = pkg.tarball.clone();
            let integrity = pkg.integrity.clone();
            let depnodes = pkg.depnodes.clone();
            let offline = self.offline;
            let force = self.force;
            let progress_tx = progress_tx.clone();

            let span = info_span!("fetch", package = %pkg_id);

            let handle = tokio::spawn(
                async move {
                    let _permit = sem.acquire().await?;

                    let max_extract_attempts = if offline { 1 } else { 2 };

                    for attempt in 0..max_extract_attempts {
                        let retrying_after_extract_failure = attempt > 0;

                        let tarball = match cache
                            .get_or_fetch(
                                &tarball_url,
                                &integrity,
                                offline,
                                force || retrying_after_extract_failure,
                            )
                            .await
                        {
                            Ok(t) => t,
                            Err(e) => {
                                warn!(package = %pkg_id, attempt = attempt + 1, error = %e, "failed to fetch tarball");
                                if let Some(tx) = &progress_tx {
                                    let _ = tx.try_send(FetchEvent::PackageFailed(format!("{}: {}", pkg_id, e)));
                                }
                                return Err(e).with_context(|| format!("failed to fetch tarball for {}", pkg_id));
                            }
                        };

                        let temp_dir = match tempfile::tempdir() {
                            Ok(d) => d,
                            Err(e) => {
                                return Err(e).with_context(|| format!("failed to create temp dir for {}", pkg_id));
                            }
                        };

                        debug!(package = %pkg_id, temp_dir = %temp_dir.path().display(), attempt = attempt + 1, "created temp dir");

                        match extract_tarball(&tarball, temp_dir.path()) {
                            Ok(_) => {
                                debug!(package = %pkg_id, "importing into store");

                                if let Err(e) = store.import_package(&pkg_id, temp_dir.path(), depnodes, Some(&integrity)) {
                                    let error = e.context(format!("failed to import package {} into store", pkg_id));
                                    warn!(package = %pkg_id, error = %error, "failed to import package");
                                    if let Some(tx) = &progress_tx {
                                        let _ = tx.try_send(FetchEvent::PackageFailed(format!("{}: {}", pkg_id, error)));
                                    }
                                    return Err(error);
                                }

                                debug!(package = %pkg_id, "success");
                                if let Some(tx) = &progress_tx {
                                    let _ = tx.try_send(FetchEvent::PackageFetched(pkg_id.to_string()));
                                }
                                return Ok(pkg_id);
                            }
                            Err(e) => {
                                warn!(
                                    package = %pkg_id,
                                    tarball = %tarball.display(),
                                    attempt = attempt + 1,
                                    error = %e,
                                    "failed to extract tarball"
                                );

                                if !offline && attempt + 1 < max_extract_attempts {
                                    let _ = cache.invalidate(&tarball_url).await;
                                    continue;
                                }

                                let error = e.context(format!("failed to extract tarball for {}", pkg_id));
                                if let Some(tx) = &progress_tx {
                                    let _ = tx.try_send(FetchEvent::PackageFailed(format!("{}: {}", pkg_id, error)));
                                }
                                return Err(error);
                            }
                        }
                    }

                    unreachable!("extract attempts loop should always return")
                }
                .instrument(span),
            );
            handles.push(handle);
        }

        let mut report = FetchReport::default();
        for handle in handles {
            match handle.await {
                Ok(Ok(_)) => report.success += 1,
                Ok(Err(e)) => report.failures.push(e.to_string()),
                Err(e) => report.failures.push(format!("task cancelled: {}", e)),
            }
        }

        let duration_ms = started.elapsed().as_millis() as u64;
        let packages_per_sec = if duration_ms == 0 {
            0.0
        } else {
            report.success as f64 * 1000.0 / duration_ms as f64
        };
        debug!(
            target: "orix::perf",
            phase = "fetcher",
            duration_ms,
            scheduled,
            success = report.success,
            failures = report.failures.len(),
            concurrency,
            packages_per_sec,
            "fetch_all complete"
        );

        Ok(report)
    }
}

/// Report from a fetch operation.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FetchReport {
    /// Number of packages successfully fetched (downloaded + extracted + imported).
    pub success: usize,
    /// Number of packages served from tarball cache (extracted + imported, no download).
    pub cached: usize,
    /// Number of packages already in store (completely skipped, no fetch/extract/import).
    pub store_hits: usize,
    /// Number of packages that failed.
    pub failures: Vec<String>,
}
