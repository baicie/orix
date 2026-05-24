//! Dependency resolution for install.

use crate::pipeline::prelude::*;
use crate::pipeline::types::{send_event, InstallOpts};

pub(crate) async fn resolve_install_graph(
    opts: &InstallOpts,
    config: &Config,
    manifest: &Manifest,
    workspace: &Option<Workspace>,
    old_lockfile: &Option<Lockfile>,
    direct_dependency_count: usize,
    progress_tx: &Option<mpsc::Sender<InstallEvent>>,
) -> Result<(orix_domain::DependencyGraph, Option<u64>)> {
    let mut resolve_instant: Option<Instant> = None;
    let graph = if opts.frozen_lockfile {
        if let Some(ref lf) = old_lockfile {
            lf.validate_frozen(manifest, ".")
                .with_context(|| "frozen lockfile validation failed")?;

            let g = resolve_from_lockfile_packages(&lf.packages);
            info!(packages = g.len(), "resolved from lockfile (frozen mode)");
            g
        } else {
            send_event(
                progress_tx,
                InstallEvent::Failed {
                    phase: Some(InstallPhase::Resolve),
                    message: "frozen lockfile mode requires an existing lockfile".to_string(),
                    hint: Some("Run `orix install` without --frozen-lockfile first.".to_string()),
                },
            );
            anyhow::bail!("frozen lockfile mode requires an existing lockfile");
        }
    } else {
        resolve_instant = Some(Instant::now());
        send_event(
            progress_tx,
            InstallEvent::PhaseStarted {
                phase: InstallPhase::Resolve,
            },
        );

        let (resolve_progress_tx, mut resolve_progress_rx) =
            mpsc::channel::<orix_resolver::ResolveProgressEvent>(4096);
        let install_progress_tx = progress_tx.clone();
        let resolve_progress_forwarder = tokio::spawn(async move {
            while let Some(event) = resolve_progress_rx.recv().await {
                send_event(
                    &install_progress_tx,
                    InstallEvent::ResolveProgress {
                        done: event.resolved,
                        total: event.discovered,
                        package: Some(event.id.to_string()),
                    },
                );
            }
        });

        let graph = if let Some(ref ws) = workspace {
            let mut resolver = if let Some(ref token) = config.auth_token {
                info!(registry = %config.registry, "using authenticated registry");
                Resolver::with_auth_disk_cache(
                    config.registry.clone(),
                    token,
                    config.cache_dir.clone(),
                    config.concurrency,
                )
            } else {
                Resolver::with_disk_cache(
                    config.registry.clone(),
                    config.cache_dir.clone(),
                    config.concurrency,
                )
            }
            .with_concurrency(config.concurrency)
            .with_progress(resolve_progress_tx);

            let manifests: Vec<&Manifest> = std::iter::once(manifest)
                .chain(ws.packages.iter().map(|pkg| &pkg.manifest))
                .collect();

            resolver
                .resolve_manifests_with_workspace(&manifests, Some(ws))
                .await
                .with_context(|| "failed to resolve workspace dependencies")?
        } else {
            let mut resolver = if let Some(ref token) = config.auth_token {
                info!(registry = %config.registry, "using authenticated registry");
                Resolver::with_auth_disk_cache(
                    config.registry.clone(),
                    token,
                    config.cache_dir.clone(),
                    config.concurrency,
                )
            } else {
                Resolver::with_disk_cache(
                    config.registry.clone(),
                    config.cache_dir.clone(),
                    config.concurrency,
                )
            }
            .with_concurrency(config.concurrency)
            .with_progress(resolve_progress_tx);

            resolver
                .resolve_manifest(manifest)
                .await
                .with_context(|| "failed to resolve dependencies")?
        };

        let _ = resolve_progress_forwarder.await;

        let old_count = old_lockfile
            .as_ref()
            .map(|lf| lf.packages.len())
            .unwrap_or(0);
        let added = graph.len().saturating_sub(old_count);
        let removed = old_count.saturating_sub(graph.len());

        send_event(
            progress_tx,
            InstallEvent::Resolved {
                direct: direct_dependency_count,
                total: graph.len(),
                added,
                removed,
            },
        );

        graph
    };

    let resolve_ms: Option<u64> = resolve_instant.map(|i| i.elapsed().as_millis() as u64);
    if let Some(ms) = resolve_ms {
        let old_count = old_lockfile
            .as_ref()
            .map(|lf| lf.packages.len())
            .unwrap_or(0);
        crate::pipeline::perf::log_resolve_phase(
            graph.len(),
            ms,
            direct_dependency_count,
            graph.len().saturating_sub(old_count),
            old_count.saturating_sub(graph.len()),
            false,
        );
    } else {
        crate::pipeline::perf::log_resolve_phase(
            graph.len(),
            0,
            direct_dependency_count,
            0,
            0,
            true,
        );
    }
    Ok((graph, resolve_ms))
}
