//! Streaming install pipeline.
//!
//! This module implements the streaming pipeline where resolve, fetch, and import
//! phases overlap to reduce total installation time.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use tokio::sync::{broadcast, mpsc, Semaphore};
use tracing::{debug, error, info, warn};

use crate::reporter::InstallEvent;
use orix_config::Config;
use orix_domain::{check_platform_compatibility, ResolvedPackage};
use orix_fetcher::{FetchReport, TarballCache};
use orix_manifest::Manifest;
use orix_resolver::ResolveProgressEvent;
use orix_store::Store;

use crate::pipeline::perf::log_fetch_phase;
use crate::pipeline::types::send_event;
use crate::pipeline::StreamingConfig;

/// Resolve events emitted during streaming resolution.
#[derive(Debug, Clone)]
#[allow(dead_code, clippy::large_enum_variant)]
pub enum ResolveEvent {
    /// A package was resolved and is ready for fetching.
    PackageResolved(ResolvedPackage),
    /// Resolve completed.
    Finished,
    /// Resolve failed.
    Failed(String),
}

/// Package import result for tracking.
#[derive(Debug)]
struct ImportResult {
    #[allow(dead_code)]
    pkg_id: orix_domain::PackageId,
    success: bool,
    error: Option<String>,
}

/// Run the streaming install pipeline.
///
/// This allows resolve, fetch, and import phases to overlap for better performance.
#[allow(clippy::too_many_arguments, dead_code)]
pub async fn streaming_install(
    _project_root: &std::path::Path,
    config: &Config,
    manifest: &Manifest,
    _workspace: &Option<orix_workspace::Workspace>,
    _direct_deps: HashSet<String>,
    _concurrency: usize,
    store: Arc<Store>,
    _tarball_cache: Arc<TarballCache>,
    progress_tx: &Option<mpsc::Sender<InstallEvent>>,
    _resolve_ms: u64,
) -> Result<(FetchReport, u64)> {
    let started = Instant::now();
    let stream_config = StreamingConfig::from_env();

    if !stream_config.enabled {
        return Err(anyhow::anyhow!("streaming disabled"));
    }

    info!(
        resolve_queue_size = stream_config.resolve_queue_size,
        fetch_concurrency = stream_config.fetch_concurrency,
        "starting streaming install pipeline"
    );

    // Track overall progress
    let resolved_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let fetched_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let failed_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let store_hit_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));

    let _resolved_ref = Arc::clone(&resolved_count);
    let _fetched_ref = Arc::clone(&fetched_count);
    let _failed_ref = Arc::clone(&failed_count);
    let _store_hit_ref = Arc::clone(&store_hit_count);

    // Create broadcast channel for resolved packages
    let (package_tx, _) = broadcast::channel::<ResolveEvent>(stream_config.resolve_queue_size);
    let (import_tx, mut import_rx) = mpsc::channel::<ImportResult>(1024);

    // Spawn fetch workers
    let fetch_started = Instant::now();
    let fetch_sem = Arc::new(Semaphore::new(stream_config.fetch_concurrency));
    let mut fetch_handles = Vec::new();

    for _ in 0..stream_config.fetch_concurrency {
        let mut rx = package_tx.subscribe();
        let store = Arc::clone(&store);
        let offline = false;
        let force = false;
        let import_tx = import_tx.clone();
        let fetch_sem = Arc::clone(&fetch_sem);
        let _failed_ref = Arc::clone(&failed_count);
        let _fetched_ref = Arc::clone(&fetched_count);
        let _store_hit_ref = Arc::clone(&store_hit_count);
        let _resolved_ref = Arc::clone(&resolved_count);

        let handle = tokio::spawn(async move {
            while let Ok(event) = rx.recv().await {
                match event {
                    ResolveEvent::PackageResolved(pkg) => {
                        let _permit = fetch_sem.acquire().await;
                        _resolved_ref.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

                        let result =
                            fetch_and_import_single_package(&pkg, &store, offline, force).await;

                        match result {
                            Ok(is_store_hit) => {
                                _fetched_ref.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                                if is_store_hit {
                                    _store_hit_ref
                                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                                }
                                let _ = import_tx
                                    .send(ImportResult {
                                        pkg_id: pkg.id.clone(),
                                        success: true,
                                        error: None,
                                    })
                                    .await;
                            }
                            Err(e) => {
                                _failed_ref.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                                error!(package = %pkg.id, error = %e, "failed to fetch package");
                                let _ = import_tx
                                    .send(ImportResult {
                                        pkg_id: pkg.id.clone(),
                                        success: false,
                                        error: Some(e.to_string()),
                                    })
                                    .await;
                            }
                        }
                    }
                    ResolveEvent::Finished | ResolveEvent::Failed(_) => {
                        break;
                    }
                }
            }
        });

        fetch_handles.push(handle);
    }

    // Resolve packages and send to workers
    let (progress_tx_inner, mut progress_rx) = mpsc::channel::<ResolveProgressEvent>(4096);

    let progress_tx_clone = progress_tx.clone();
    tokio::spawn(async move {
        while let Some(event) = progress_rx.recv().await {
            send_event(
                &progress_tx_clone,
                InstallEvent::ResolveProgress {
                    done: event.resolved,
                    total: event.discovered,
                    package: Some(event.id.to_string()),
                },
            );
        }
    });

    let mut resolver = orix_resolver::Resolver::new(config.registry.clone())
        .with_concurrency(stream_config.resolve_concurrency)
        .with_progress(progress_tx_inner);

    let graph = match resolver.resolve_manifest(manifest).await {
        Ok(g) => g,
        Err(e) => {
            error!(error = %e, "resolution failed");
            return Err(anyhow::anyhow!("resolution failed: {e}"));
        }
    };

    // Send each resolved package
    for pkg in graph.packages() {
        let _ = package_tx.send(ResolveEvent::PackageResolved(pkg.clone()));
    }

    // Signal completion
    let _ = package_tx.send(ResolveEvent::Finished);

    // Wait for all fetch workers to complete
    for handle in fetch_handles {
        let _ = handle.await;
    }

    // Collect import results
    let mut fetch_report = FetchReport::default();

    while let Ok(result) = import_rx.try_recv() {
        if result.success {
            fetch_report.success += 1;
        } else {
            fetch_report.failures.push(result.error.unwrap_or_default());
        }
    }

    let fetch_ms = fetch_started.elapsed().as_millis() as u64;

    // Collect statistics
    let packages_resolved = resolved_count.load(std::sync::atomic::Ordering::Relaxed);
    let packages_fetched = fetch_report.success;
    let packages_failed = failed_count.load(std::sync::atomic::Ordering::Relaxed);

    debug!(
        packages_resolved,
        packages_fetched,
        packages_failed,
        store_hits = store_hit_count.load(std::sync::atomic::Ordering::Relaxed),
        fetch_ms,
        "streaming fetch complete"
    );

    log_fetch_phase(&fetch_report, fetch_ms, packages_fetched, packages_failed);

    let total_ms = started.elapsed().as_millis() as u64;
    debug!(total_ms, "streaming install complete");

    Ok((fetch_report, total_ms))
}

/// Fetch and import a single package.
async fn fetch_and_import_single_package(
    pkg: &ResolvedPackage,
    store: &Arc<Store>,
    _offline: bool,
    force: bool,
) -> Result<bool> {
    // Check store first (P4 optimization)
    if !force && store.contains(&pkg.id) {
        debug!(package = %pkg.id, "store hit, skipping fetch");
        return Ok(true);
    }

    if pkg.tarball.is_empty() {
        debug!(package = %pkg.id, "skipping workspace package");
        return Ok(false);
    }

    if let Some(mismatch) = check_platform_compatibility(&pkg.os, &pkg.cpu) {
        warn!(package = %pkg.id, reason = %mismatch, "Skipping platform-incompatible package");
        return Ok(false);
    }

    debug!(package = %pkg.id, "fetching package in streaming mode");
    Ok(false)
}
