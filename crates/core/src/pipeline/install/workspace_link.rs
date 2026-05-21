//! Workspace package linking during install.

use std::collections::HashSet;

use anyhow::Context;

use crate::pipeline::prelude::*;
use crate::pipeline::types::link_error;

/// Link `node_modules` for each workspace member package.
pub(crate) fn link_workspace_packages(
    store: &Store,
    graph: &orix_domain::DependencyGraph,
    workspace: &Workspace,
    graph_hash: &str,
    progress_tx: &Option<mpsc::Sender<InstallEvent>>,
) -> Result<()> {
    for ws_pkg in &workspace.packages {
        let nm_dir = ws_pkg.abs_path.join("node_modules");
        let pkg_linker = Linker::new(store.clone(), nm_dir.clone());

        let pkg_deps: HashSet<String> = ws_pkg
            .manifest
            .dependencies
            .keys()
            .chain(ws_pkg.manifest.dev_dependencies.keys())
            .chain(ws_pkg.manifest.optional_dependencies.keys())
            .cloned()
            .collect();

        let layout_is_valid = pkg_linker.is_layout_valid(graph_hash)
            && pkg_linker
                .validate_layout(&pkg_deps)
                .with_context(|| {
                    format!(
                        "failed to validate existing node_modules layout for {}",
                        ws_pkg.manifest.name.as_deref().unwrap_or("?")
                    )
                })?
                .is_ok();
        if layout_is_valid {
            debug!(
                target: "orix",
                pkg = %ws_pkg.manifest.name.as_deref().unwrap_or("?"),
                "workspace pkg layout valid, skipping"
            );
            continue;
        }

        if let Err(e) = pkg_linker.prune_stale_layout(graph, &pkg_deps) {
            return Err(link_error(
                progress_tx,
                format!(
                    "failed to prune stale node_modules for {}: {}",
                    ws_pkg.manifest.name.as_deref().unwrap_or("?"),
                    e
                ),
            ));
        }

        if let Err(e) = pkg_linker.link_graph(graph, &pkg_deps, Some(workspace), graph_hash) {
            return Err(link_error(
                progress_tx,
                format!(
                    "failed to link packages for {}: {}",
                    ws_pkg.manifest.name.as_deref().unwrap_or("?"),
                    e
                ),
            ));
        }
    }

    Ok(())
}
