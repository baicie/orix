//! Link phase for install.

use std::collections::HashSet;

use crate::pipeline::prelude::*;
use crate::pipeline::types::{
    emit_link_progress, emit_windows_link_performance_hint, link_error, send_event,
};

pub(crate) fn link_install_graph(
    store: &Store,
    config: &Config,
    graph: &orix_domain::DependencyGraph,
    manifest: &Manifest,
    workspace: &Option<Workspace>,
    progress_tx: &Option<mpsc::Sender<InstallEvent>>,
) -> Result<(LinkReport, Option<u64>)> {
    send_event(
        progress_tx,
        InstallEvent::PhaseStarted {
            phase: InstallPhase::Link,
        },
    );
    let link_instant = Instant::now();
    let total_packages = graph.len();
    emit_link_progress(progress_tx, 0, total_packages, None);
    let direct_deps: HashSet<String> = manifest
        .dependencies
        .keys()
        .chain(manifest.dev_dependencies.keys())
        .cloned()
        .collect();

    let graph_hash = graph.graph_hash();
    let linker = Linker::new(store.clone(), config.node_modules_dir());
    let layout_is_valid = linker.is_layout_valid(&graph_hash)
        && linker
            .validate_direct_layout(&direct_deps)
            .with_context(|| "failed to validate existing direct node_modules layout")?
            .is_ok();
    let link_report = if layout_is_valid {
        emit_link_progress(progress_tx, total_packages, total_packages, None);
        LinkReport {
            hardlinked_files: 0,
            copied_files: 0,
            symlinks_created: 0,
            bytes_saved: 0,
            skipped: Some("node_modules layout already valid".to_string()),
        }
    } else {
        let prune_started = Instant::now();
        if let Err(e) = linker.prune_stale_layout(graph, &direct_deps) {
            return Err(link_error(
                progress_tx,
                format!("failed to prune stale node_modules layout: {e}"),
            ));
        }
        let prune_ms = prune_started.elapsed().as_millis() as u64;
        debug!(
            target: crate::pipeline::perf::PERF_TARGET,
            phase = "prune_layout",
            duration_ms = prune_ms,
            packages = graph.len(),
            "prune stale layout complete"
        );

        let mut on_link_progress = |done: usize, _total: usize, name: &str| {
            emit_link_progress(progress_tx, done, total_packages, Some(name.to_string()));
        };
        let link_report = linker.link_graph(
            graph,
            &direct_deps,
            workspace.as_ref(),
            &graph_hash,
            Some(&mut on_link_progress),
        );
        match link_report {
            Ok(r) => r,
            Err(e) => return Err(link_error(progress_tx, e.to_string())),
        }
    };

    if let Some(ref ws) = workspace {
        super::workspace_link::link_workspace_packages(store, graph, ws, progress_tx)?;
    }

    let link_ms: u64 = link_instant.elapsed().as_millis() as u64;
    crate::pipeline::perf::log_link_phase(&link_report, link_ms, graph.len(), layout_is_valid);
    emit_windows_link_performance_hint(config, &link_report);

    send_event(
        progress_tx,
        InstallEvent::PhaseFinished {
            phase: InstallPhase::Link,
        },
    );

    Ok((link_report, Some(link_ms)))
}
