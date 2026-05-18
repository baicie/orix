//! Package tarball fetcher.

use std::sync::Arc;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, Semaphore};
use tracing::{debug, info_span, warn};

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
    pub fn new(cache: TarballCache, store: Store) -> Self {
        Self {
            cache: Arc::new(cache),
            store: Arc::new(store),
            offline: false,
            force: false,
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
        let sem = Arc::new(Semaphore::new(concurrency.max(1)));
        let mut handles = Vec::new();

        for pkg in graph.packages() {
            let span = info_span!("fetch", package = %pkg.id);

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

            let handle = tokio::spawn(async move {
                let _permit = sem.acquire().await?;
                let _guard = span.enter();
                let tarball = match cache
                    .get_or_fetch(&tarball_url, &integrity, offline, force)
                    .await
                    .with_context(|| format!("failed to fetch tarball for {}", pkg_id))
                {
                    Ok(t) => t,
                    Err(e) => {
                        warn!(package = %pkg_id, "failed to fetch tarball: {}", e);
                        if let Some(tx) = &progress_tx {
                            let _ = tx
                                .try_send(FetchEvent::PackageFailed(format!("{}: {}", pkg_id, e)));
                        }
                        return Err(e);
                    }
                };
                let temp_dir = tempfile::tempdir()?;
                debug!(package = %pkg_id, temp_dir = %temp_dir.path().display(), "created temp dir");
                if let Err(e) = extract_tarball(&tarball, temp_dir.path()) {
                    let error = e.context(format!("failed to extract tarball for {}", pkg_id));
                    warn!(package = %pkg_id, "failed to extract tarball: {}", error);
                    if let Some(tx) = &progress_tx {
                        let _ = tx
                            .try_send(FetchEvent::PackageFailed(format!("{}: {}", pkg_id, error)));
                    }
                    return Err(error);
                }
                debug!(package = %pkg_id, "importing into store");
                if let Err(e) =
                    store.import_package(&pkg_id, temp_dir.path(), depnodes, Some(&integrity))
                {
                    let error =
                        e.context(format!("failed to import package {} into store", pkg_id));
                    warn!(package = %pkg_id, "failed to import package: {}", error);
                    if let Some(tx) = &progress_tx {
                        let _ = tx
                            .try_send(FetchEvent::PackageFailed(format!("{}: {}", pkg_id, error)));
                    }
                    return Err(error);
                }
                debug!(package = %pkg_id, "success");
                if let Some(tx) = &progress_tx {
                    let _ = tx.try_send(FetchEvent::PackageFetched(pkg_id.to_string()));
                }
                Ok::<_, anyhow::Error>(pkg_id)
            });
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

        Ok(report)
    }
}

/// Report from a fetch operation.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FetchReport {
    /// Number of packages successfully fetched.
    pub success: usize,
    /// Error messages for failed packages.
    pub failures: Vec<String>,
}
