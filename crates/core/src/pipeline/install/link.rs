//! Link phase for install.

use std::collections::HashSet;

use crate::pipeline::prelude::*;
use crate::pipeline::types::{emit_windows_link_performance_hint, link_error, send_event};

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
            .validate_layout(&direct_deps)
            .with_context(|| "failed to validate existing node_modules layout")?
            .is_ok();
    let link_report = if layout_is_valid {
        LinkReport {
            hardlinked_files: 0,
            copied_files: 0,
            symlinks_created: 0,
            bytes_saved: 0,
            skipped: Some("node_modules layout already valid".to_string()),
        }
    } else {
        if let Err(e) = linker.prune_stale_layout(&graph, &direct_deps) {
            return Err(link_error(
                progress_tx,
                format!("failed to prune stale node_modules layout: {e}"),
            ));
        }

        let link_report = linker.link_graph(&graph, &direct_deps, workspace.as_ref(), &graph_hash);
        match link_report {
            Ok(r) => r,
            Err(e) => return Err(link_error(progress_tx, e.to_string())),
        }
    };

    if let Some(ref ws) = workspace {
        super::workspace_link::link_workspace_packages(store, graph, ws, progress_tx)?;
    }

    let link_ms: Option<u64> = Some(link_instant.elapsed().as_millis() as u64);

    trace!(
        hardlinked_files = link_report.hardlinked_files,
        copied_files = link_report.copied_files,
        symlinks_created = link_report.symlinks_created,
        bytes_saved = link_report.bytes_saved,
        link_ms = link_ms,
        "linked dependencies"
    );
    emit_windows_link_performance_hint(&config, &link_report);

    send_event(
        progress_tx,
        InstallEvent::PhaseFinished {
            phase: InstallPhase::Link,
        },
    );

    Ok((link_report, link_ms))
}
