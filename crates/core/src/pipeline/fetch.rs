//! Pipeline submodule.

use super::prelude::*;
use super::types::send_event;

pub(crate) async fn fetch_only_missing(
    store: &Store,
    fetcher: &Fetcher,
    graph: &orix_domain::DependencyGraph,
    concurrency: usize,
    progress_tx: Option<mpsc::Sender<InstallEvent>>,
) -> Result<(orix_domain::DependencyGraph, orix_fetcher::FetchReport)> {
    let mut missing = orix_domain::DependencyGraph::new();
    for pkg in graph.packages() {
        if is_fetchable_package(pkg) && !store.contains(&pkg.id) {
            missing.insert(pkg.clone());
        }
    }
    let total_fetchable = graph
        .packages()
        .filter(|pkg| is_fetchable_package(pkg))
        .count();
    let cached = total_fetchable.saturating_sub(missing.len());

    if missing.is_empty() {
        debug!(target: "orix", "all {} fetchable packages already in store, skipping fetch", total_fetchable);
        send_event(
            &progress_tx,
            InstallEvent::FetchProgress {
                done: total_fetchable,
                total: total_fetchable,
                package: None,
            },
        );
        return Ok((graph.clone(), orix_fetcher::FetchReport::default()));
    }

    debug!(target: "orix", "found {} packages in store, fetching {} missing", cached, missing.len());

    let total_to_fetch = total_fetchable;
    send_event(
        &progress_tx,
        InstallEvent::FetchProgress {
            done: cached,
            total: total_to_fetch,
            package: None,
        },
    );

    let install_progress_tx = progress_tx.clone();
    let (fetch_progress_tx, mut fetch_progress_rx) = mpsc::channel(8192);
    let fetch_progress_forwarder = tokio::spawn(async move {
        let mut fetched_count: usize = cached;
        while let Some(event) = fetch_progress_rx.recv().await {
            match event {
                FetchEvent::PackageFetched(package) => {
                    fetched_count += 1;
                    send_event(
                        &install_progress_tx,
                        InstallEvent::FetchProgress {
                            done: fetched_count,
                            total: total_to_fetch,
                            package: None,
                        },
                    );
                    send_event(
                        &install_progress_tx,
                        InstallEvent::PackageFetched {
                            name: package,
                            version: None,
                            cached: false,
                        },
                    );
                }
                FetchEvent::PackageFailed(failure) => {
                    tracing::debug!(failure = %failure, "package fetch failed");
                }
            }
        }
    });
    let fetch_report = fetcher
        .fetch_all(&missing, concurrency, Some(fetch_progress_tx))
        .await
        .with_context(|| "failed to fetch packages")?;
    let _ = fetch_progress_forwarder.await;

    send_event(
        &progress_tx,
        InstallEvent::FetchProgress {
            done: cached + fetch_report.success,
            total: total_to_fetch,
            package: None,
        },
    );

    Ok((graph.clone(), fetch_report))
}

pub(crate) fn is_fetchable_package(pkg: &orix_domain::ResolvedPackage) -> bool {
    !pkg.tarball.is_empty()
        && orix_domain::check_platform_compatibility(&pkg.os, &pkg.cpu).is_none()
}
