//! Package tarball fetcher.

use std::sync::Arc;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use tokio::sync::Semaphore;

use orix_domain::DependencyGraph;
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

    /// Fetch all packages in the dependency graph into the store.
    pub async fn fetch_all(
        &self,
        graph: &DependencyGraph,
        concurrency: usize,
    ) -> Result<FetchReport> {
        let sem = Arc::new(Semaphore::new(concurrency.max(1)));
        let mut handles = Vec::new();

        for pkg in graph.packages() {
            let sem = Arc::clone(&sem);
            let cache = Arc::clone(&self.cache);
            let store = Arc::clone(&self.store);
            let pkg_id = pkg.id.clone();
            let tarball_url = pkg.tarball.clone();
            let integrity = pkg.integrity.clone();
            let depnodes = pkg.depnodes.clone();
            let offline = self.offline;
            let force = self.force;

            let handle = tokio::spawn(async move {
                let _permit = sem.acquire().await?;
                let tarball = cache
                    .get_or_fetch(&tarball_url, &integrity, offline, force)
                    .await?;
                let temp_dir = tempfile::tempdir()?;
                extract_tarball(&tarball, temp_dir.path())?;
                store.import_package(&pkg_id, temp_dir.path(), depnodes, Some(&integrity))?;
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
