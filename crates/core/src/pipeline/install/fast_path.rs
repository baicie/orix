//! Lockfile fast path: skip resolve when specifiers are frozen.

use crate::pipeline::fetch::fetch_only_missing;
use crate::pipeline::prelude::*;
use crate::pipeline::types::{
    fetch_failure_hint, link_error, send_event, InstallOpts, InstallReport,
};
use crate::reporter::LockfileStatus;

/// Returns `Some(report)` when install completed via the lockfile fast path.
pub(crate) async fn try_install_fast_path(
    project_root: &Path,
    opts: &InstallOpts,
    config: &Config,
    manifest: &Manifest,
    workspace: &Option<Workspace>,
    old_lockfile: &Lockfile,
    direct_dependency_count: usize,
    start: Instant,
) -> Result<Option<InstallReport>> {
    if opts.frozen_lockfile || opts.force {
        return Ok(None);
    }
    if old_lockfile.validate(&manifest, ".").is_ok()
        && old_lockfile.validate_frozen(&manifest, ".").is_err()
    {
        info!("package.json dependency specifiers changed; re-resolving from registry");
    }
    if old_lockfile.validate_frozen(&manifest, ".").is_err() {
        return Ok(None);
    }

    debug!(target: "orix", "FAST PATH triggered");
    let graph = resolve_from_lockfile_packages(&old_lockfile.packages);
    let pkg_count = graph.len();
    info!(packages = pkg_count, "resolved from lockfile (fast path)");

    send_event(
        &opts.progress_tx,
        InstallEvent::PhaseStarted {
            phase: InstallPhase::Resolve,
        },
    );
    send_event(
        &opts.progress_tx,
        InstallEvent::Resolved {
            direct: direct_dependency_count,
            total: pkg_count,
            added: 0,
            removed: 0,
        },
    );
    send_event(
        &opts.progress_tx,
        InstallEvent::PhaseFinished {
            phase: InstallPhase::Resolve,
        },
    );

    // Only fetch packages missing from store.
    send_event(
        &opts.progress_tx,
        InstallEvent::PhaseStarted {
            phase: InstallPhase::Fetch,
        },
    );
    let store = Store::open(config.store_dir.clone()).with_context(|| "failed to open store")?;
    let tarball_cache = TarballCache::new(config.cache_dir.clone());
    let fetcher = Fetcher::new(tarball_cache, store.clone(), project_root.to_path_buf())
        .with_offline(opts.offline)
        .with_force(false);
    let concurrency = if opts.concurrency == 0 {
        config.concurrency
    } else {
        opts.concurrency
    };
    let (graph, fetch_report) = fetch_only_missing(
        &store,
        &fetcher,
        &graph,
        concurrency,
        opts.progress_tx.clone(),
    )
    .await
    .with_context(|| "failed to fetch missing packages")?;

    info!(
        success = fetch_report.success,
        failures = fetch_report.failures.len(),
        "fetched packages"
    );

    if !fetch_report.failures.is_empty() {
        let hint = fetch_failure_hint(&fetch_report.failures);
        send_event(
            &opts.progress_tx,
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
        &opts.progress_tx,
        InstallEvent::PhaseFinished {
            phase: InstallPhase::Fetch,
        },
    );

    send_event(
        &opts.progress_tx,
        InstallEvent::PhaseStarted {
            phase: InstallPhase::Link,
        },
    );
    let linker = Linker::new(store.clone(), config.node_modules_dir());
    use std::collections::HashSet;
    let direct_deps: HashSet<String> = manifest
        .dependencies
        .keys()
        .chain(manifest.dev_dependencies.keys())
        .cloned()
        .collect();
    let graph_hash = graph.graph_hash();
    let link_report = if linker.is_layout_valid(&graph_hash)
        && linker
            .validate_layout(&direct_deps)
            .map(|r| r.is_ok())
            .unwrap_or(false)
    {
        debug!(target: "orix", "layout valid, skipping unlink+link");
        LinkReport {
            hardlinked_files: 0,
            copied_files: 0,
            symlinks_created: 0,
            bytes_saved: 0,
            skipped: Some("layout valid".to_string()),
        }
    } else {
        let t2 = Instant::now();
        linker
            .prune_stale_layout(&graph, &direct_deps)
            .with_context(|| "failed to prune stale node_modules layout")?;
        let report = linker.link_graph(
            &graph,
            &direct_deps,
            workspace.as_ref(),
            &graph.graph_hash(),
        );
        match report {
            Ok(r) => {
                debug!(target: "orix", "link (unlink+link_graph): {:?}", t2.elapsed());
                r
            }
            Err(e) => return Err(link_error(&opts.progress_tx, e.to_string())),
        }
    };

    if let Some(ref ws) = workspace {
        super::workspace_link::link_workspace_packages(
            &store,
            &graph,
            ws,
            &graph_hash,
            &opts.progress_tx,
        )?;
    }

    send_event(
        &opts.progress_tx,
        InstallEvent::PhaseFinished {
            phase: InstallPhase::Link,
        },
    );

    let layout_report = linker
        .validate_layout(&direct_deps)
        .with_context(|| "failed to validate node_modules layout")?;
    if !layout_report.is_ok() {
        anyhow::bail!(
            "node_modules layout validation failed: {}",
            layout_report.broken.join("; ")
        );
    }

    let base_lockfile = old_lockfile.clone();
    let updated_lockfile = base_lockfile.update(&manifest, &graph, ".");
    let diff = Lockfile::diff(&base_lockfile, &updated_lockfile);
    let lockfile_changed = Lockfile::diff_has_changes(&diff);

    if lockfile_changed {
        send_event(
            &opts.progress_tx,
            InstallEvent::PhaseStarted {
                phase: InstallPhase::Lockfile,
            },
        );
        updated_lockfile
            .write(&config.lockfile_path())
            .with_context(|| "failed to write lockfile")?;
        info!(
            added = diff.added.len(),
            removed = diff.removed.len(),
            changed = diff.changed.len(),
            "lockfile updated"
        );
    }

    let lockfile_status = if lockfile_changed {
        LockfileStatus::Written
    } else {
        LockfileStatus::Unchanged
    };
    send_event(
        &opts.progress_tx,
        InstallEvent::Lockfile {
            status: lockfile_status,
        },
    );

    let duration = start.elapsed();
    info!(
        duration_ms = duration.as_millis(),
        "install complete (fast path)"
    );
    send_event(
        &opts.progress_tx,
        InstallEvent::Finished {
            installed: graph.len(),
            duration,
        },
    );

    Ok(Some(InstallReport {
        registry: config.registry.to_string(),
        direct_dependencies: direct_dependency_count,
        packages_added: graph.len(),
        fetch_report,
        link_report,
        lockfile_diff: None,
        lockfile_changed,
        duration_secs: duration.as_secs_f64(),
        resolve_ms: None,
        fetch_ms: None,
        link_ms: None,
        lockfile_ms: None,
    }))
}
