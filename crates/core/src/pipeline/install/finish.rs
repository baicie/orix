//! Post-link validation, lockfile write, and lifecycle scripts.

use crate::pipeline::lifecycle::{run_dependency_lifecycles, run_project_lifecycle};
use crate::pipeline::prelude::*;
use crate::pipeline::types::{send_event, InstallOpts, InstallReport, LockfileDiffReport};
use crate::reporter::LockfileStatus;

pub(crate) async fn finish_install(
    opts: &InstallOpts,
    config: &Config,
    manifest: &Manifest,
    project_root: &Path,
    graph: &orix_domain::DependencyGraph,
    linker: &Linker,
    direct_deps: &std::collections::HashSet<String>,
    fetch_report: orix_fetcher::FetchReport,
    link_report: LinkReport,
    old_lockfile: &Option<Lockfile>,
    direct_dependency_count: usize,
    start: Instant,
    resolve_ms: Option<u64>,
    fetch_ms: Option<u64>,
    link_ms: Option<u64>,
    _progress_tx: &Option<mpsc::Sender<InstallEvent>>,
) -> Result<InstallReport> {
    let mut lockfile_ms: Option<u64> = None;
    run_dependency_lifecycles(&graph, &config, project_root, &opts.progress_tx).await?;

    // Phase: Run install lifecycle (after link, before lockfile write)
    run_project_lifecycle(
        LifecycleEvent::Install,
        &manifest,
        &config,
        project_root,
        &opts.progress_tx,
    )
    .await?;

    let layout_report = linker
        .validate_layout(&direct_deps)
        .with_context(|| "failed to validate node_modules layout")?;
    if !layout_report.is_ok() {
        anyhow::bail!(
            "node_modules layout validation failed: {}",
            layout_report.broken.join("; ")
        );
    }

    let mut lockfile_changed = false;
    let lockfile_diff: Option<LockfileDiffReport> = if !opts.frozen_lockfile {
        send_event(
            &opts.progress_tx,
            InstallEvent::PhaseStarted {
                phase: InstallPhase::Lockfile,
            },
        );
        let lockfile_instant = Instant::now();
        let base_lockfile = old_lockfile
            .as_ref()
            .cloned()
            .unwrap_or_else(Lockfile::empty);
        let updated_lockfile = base_lockfile.update(&manifest, &graph, ".");

        let diff = Lockfile::diff(&base_lockfile, &updated_lockfile);
        let diff_report = LockfileDiffReport {
            added: diff.added.clone(),
            removed: diff.removed.clone(),
            changed: diff.changed.clone(),
            importers_changed: diff.importers_changed.clone(),
        };

        updated_lockfile
            .write(&config.lockfile_path())
            .with_context(|| "failed to write lockfile")?;

        lockfile_changed = Lockfile::diff_has_changes(&diff);
        lockfile_ms = Some(lockfile_instant.elapsed().as_millis() as u64);
        if lockfile_changed {
            info!(
                added = diff.added.len(),
                removed = diff.removed.len(),
                changed = diff.changed.len(),
                importers_changed = diff.importers_changed.len(),
                "lockfile updated"
            );
        } else {
            info!("lockfile unchanged");
        }

        Some(diff_report)
    } else {
        None
    };

    let lockfile_status = if !opts.frozen_lockfile {
        if lockfile_changed {
            LockfileStatus::Written
        } else {
            LockfileStatus::Unchanged
        }
    } else {
        LockfileStatus::Skipped
    };
    send_event(
        &opts.progress_tx,
        InstallEvent::Lockfile {
            status: lockfile_status,
        },
    );

    run_project_lifecycle(
        LifecycleEvent::Postinstall,
        &manifest,
        &config,
        project_root,
        &opts.progress_tx,
    )
    .await?;

    if manifest
        .script(LifecycleEvent::Prepare.script_name())
        .is_some()
    {
        run_project_lifecycle(
            LifecycleEvent::Prepare,
            &manifest,
            &config,
            project_root,
            &opts.progress_tx,
        )
        .await?;
    }

    let duration = start.elapsed();
    info!(duration_ms = duration.as_millis(), "install complete");
    send_event(
        &opts.progress_tx,
        InstallEvent::Finished {
            installed: graph.len(),
            duration,
        },
    );

    Ok(InstallReport {
        registry: config.registry.to_string(),
        direct_dependencies: direct_dependency_count,
        packages_added: graph.len(),
        fetch_report,
        link_report,
        lockfile_diff,
        lockfile_changed,
        duration_secs: duration.as_secs_f64(),
        resolve_ms,
        fetch_ms,
        link_ms,
        lockfile_ms,
    })
}
