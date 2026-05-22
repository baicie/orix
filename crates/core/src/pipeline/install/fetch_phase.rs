//! Fetch phase for the install pipeline.

use crate::pipeline::fetch::is_fetchable_package;
use crate::pipeline::prelude::*;
use crate::pipeline::types::{fetch_failure_hint, send_event};

/// Download packages in `graph` and return fetch metrics.
pub(crate) async fn fetch_install_graph(
    graph: &orix_domain::DependencyGraph,
    fetcher: &Fetcher,
    concurrency: usize,
    progress_tx: &Option<mpsc::Sender<InstallEvent>>,
) -> Result<(orix_fetcher::FetchReport, Option<u64>)> {
    send_event(
        progress_tx,
        InstallEvent::PhaseStarted {
            phase: InstallPhase::Fetch,
        },
    );
    let fetch_instant = Instant::now();

    let total_to_fetch = graph
        .packages()
        .filter(|pkg| is_fetchable_package(pkg))
        .count();
    send_event(
        progress_tx,
        InstallEvent::FetchProgress {
            done: 0,
            total: total_to_fetch,
            package: None,
        },
    );

    let (fetch_progress_tx, mut fetch_progress_rx) = mpsc::channel(8192);
    let install_progress_tx = progress_tx.clone();
    let fetch_total = total_to_fetch;
    let fetch_progress_forwarder = tokio::spawn(async move {
        let mut fetched_count: usize = 0;
        while let Some(event) = fetch_progress_rx.recv().await {
            match event {
                FetchEvent::PackageFetched(package) => {
                    fetched_count += 1;
                    send_event(
                        &install_progress_tx,
                        InstallEvent::FetchProgress {
                            done: fetched_count,
                            total: fetch_total,
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
        .fetch_all(graph, concurrency, Some(fetch_progress_tx))
        .await
        .with_context(|| "failed to fetch packages")?;
    let _ = fetch_progress_forwarder.await;
    let fetch_ms: Option<u64> = Some(fetch_instant.elapsed().as_millis() as u64);
    crate::pipeline::perf::log_fetch_phase(
        &fetch_report,
        fetch_ms.unwrap_or(0),
        total_to_fetch,
        concurrency,
    );

    send_event(
        progress_tx,
        InstallEvent::FetchProgress {
            done: fetch_report.success,
            total: total_to_fetch,
            package: None,
        },
    );

    info!(
        success = fetch_report.success,
        failures = fetch_report.failures.len(),
        "fetched packages"
    );

    if !fetch_report.failures.is_empty() {
        let hint = fetch_failure_hint(&fetch_report.failures);
        send_event(
            progress_tx,
            InstallEvent::Failed {
                phase: Some(InstallPhase::Fetch),
                message: format!(
                    "failed to fetch packages:\n  {}",
                    fetch_report.failures.join("\n  ")
                ),
                hint: Some(hint),
            },
        );
        anyhow::bail!(
            "failed to fetch packages:\n  {}",
            fetch_report.failures.join("\n  ")
        );
    }

    send_event(
        progress_tx,
        InstallEvent::PhaseFinished {
            phase: InstallPhase::Fetch,
        },
    );

    Ok((fetch_report, fetch_ms))
}
