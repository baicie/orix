//! Dependency graph linking.

mod bins;
mod import_files;
mod symlinks;

use std::time::Instant;

use crate::linker::prelude::*;
use crate::linker::{Linker, VIRTUAL_STORE_DIR};
use crate::linker_platform::*;
use tracing::{debug, trace};

impl Linker {
    /// Link all packages in `graph` into the project's `node_modules` layout.
    pub fn link_graph(
        &self,
        graph: &DependencyGraph,
        direct_deps: &std::collections::HashSet<String>,
        workspace: Option<&orix_workspace::Workspace>,
        graph_hash: &str,
        mut progress: crate::LinkProgressCallback<'_>,
    ) -> Result<LinkReport> {
        let started = Instant::now();
        let mut report = LinkReport {
            hardlinked_files: 0,
            copied_files: 0,
            symlinks_created: 0,
            bytes_saved: 0,
            skipped: None,
        };

        let virtual_store_dir = self.node_modules.join(VIRTUAL_STORE_DIR);
        fs::create_dir_all(&virtual_store_dir)?;

        // Build a lookup from package name -> pkg_id for quick dep resolution
        let name_to_key: HashMap<String, String> = graph
            .packages()
            .map(|p| (p.id.name.to_string(), p.id.key()))
            .collect();
        let direct_name_to_key: HashMap<String, String> = graph
            .packages()
            .filter(|p| direct_deps.contains(p.id.name.as_str()))
            .map(|p| (p.id.name.to_string(), p.id.key()))
            .collect();

        let mut import_files_ms: u64 = 0;
        let mut bins_ms: u64 = 0;
        let mut workspace_ms: u64 = 0;
        let mut packages_imported: u32 = 0;
        let mut packages_import_skipped: u32 = 0;
        let mut workspace_packages: u32 = 0;
        let mut slow_package_logs: u32 = 0;
        const SLOW_PACKAGE_MS: u64 = 200;

        let total_packages = graph.len();
        let mut packages_done = 0usize;

        for pkg in graph.packages() {
            let pkg_started = Instant::now();
            let pkg_key = pkg.id.key();

            // Workspace packages: link directly to local source instead of store.
            let is_workspace_pkg = pkg.tarball.is_empty();
            if is_workspace_pkg && workspace.is_some() {
                if let Some(ws) = workspace {
                    if let Some(local_pkg) = ws
                        .packages
                        .iter()
                        .find(|p| p.manifest.name.as_deref() == Some(&*pkg.id.name))
                    {
                        let ws_started = Instant::now();
                        let top_link = Self::package_path_in_node_modules(
                            &self.node_modules,
                            pkg.id.name.as_str(),
                        );
                        if !top_link.exists() {
                            if let Some(parent) = top_link.parent() {
                                fs::create_dir_all(parent)?;
                            }
                            Self::create_dir_link(&local_pkg.abs_path, &top_link).with_context(
                                || {
                                    format!(
                                        "failed to link workspace package {}: {} -> {}",
                                        pkg.id.name,
                                        top_link.display(),
                                        local_pkg.abs_path.display()
                                    )
                                },
                            )?;
                            report.symlinks_created += 1;
                        }
                        workspace_ms += ws_started.elapsed().as_millis() as u64;
                        workspace_packages += 1;
                        packages_done += 1;
                        if let Some(callback) = progress.as_deref_mut() {
                            callback(packages_done, total_packages, pkg.id.name.as_str());
                        }
                        continue;
                    }
                }
            }

            let pkg_dir = Self::package_path_in_node_modules(
                &virtual_store_dir.join(&pkg_key).join("node_modules"),
                pkg.id.name.as_str(),
            );

            let store_files = self.store.package_files_path(&pkg.id);
            fs::create_dir_all(&pkg_dir)?;

            let mut pkg_import_ms = 0_u64;
            if !self.is_package_import_complete(&pkg.id, &pkg_dir)? {
                let t = Instant::now();
                self.import_package_files(&pkg.id, &pkg_dir, &store_files, &mut report)?;
                pkg_import_ms = t.elapsed().as_millis() as u64;
                import_files_ms += pkg_import_ms;
                packages_imported += 1;
            } else {
                trace!(pkg = %pkg_key, "package already imported, skipping file import");
                packages_import_skipped += 1;
            }

            // Link bin executables for this package into .orix/<pkg>/bin/
            let link_global_bins = direct_name_to_key
                .get(pkg.id.name.as_str())
                .is_some_and(|direct_key| direct_key == &pkg_key);
            let bins_started = Instant::now();
            self.link_package_bins(&pkg_key, &store_files, link_global_bins, &mut report)?;
            let pkg_bins_ms = bins_started.elapsed().as_millis() as u64;
            bins_ms += pkg_bins_ms;

            let pkg_ms = pkg_started.elapsed().as_millis() as u64;
            if pkg_ms >= SLOW_PACKAGE_MS && slow_package_logs < 20 {
                slow_package_logs += 1;
                debug!(
                    target: "orix::perf",
                    phase = "link_package",
                    pkg = %pkg_key,
                    duration_ms = pkg_ms,
                    import_files_ms = pkg_import_ms,
                    bins_ms = pkg_bins_ms,
                    "slow package link"
                );
            }

            packages_done += 1;
            if let Some(callback) = progress.as_deref_mut() {
                callback(packages_done, total_packages, pkg.id.name.as_str());
            }
        }

        let virtual_deps_started = Instant::now();
        // Create package-internal dependency links after all packages have been imported.
        // This keeps optional dependencies from being skipped due to graph iteration order.
        for pkg in graph.packages() {
            let pkg_key = pkg.id.key();
            let pkg_dir = Self::package_path_in_node_modules(
                &virtual_store_dir.join(&pkg_key).join("node_modules"),
                pkg.id.name.as_str(),
            );

            for (dep_name, raw) in pkg
                .dependencies
                .iter()
                .chain(pkg.optional_dependencies.iter())
                .chain(pkg.peer_dependencies.iter())
            {
                let Some(dep_key) = select_dependency_key(graph, dep_name, raw)
                    .or_else(|| name_to_key.get(dep_name.as_str()).cloned())
                else {
                    continue;
                };
                let target = Self::package_path_in_node_modules(
                    &virtual_store_dir.join(&dep_key).join("node_modules"),
                    dep_name.as_str(),
                );
                if !target.exists() {
                    trace!(
                        pkg = %pkg_key,
                        dep = %dep_name,
                        missing = %target.display(),
                        "dependency target not in virtual store"
                    );
                    continue;
                }

                let symlink_path = Self::package_path_in_node_modules(
                    &pkg_dir.join("node_modules"),
                    dep_name.as_str(),
                );

                if Self::dir_link_needs_repair(&symlink_path, &target) {
                    if path_exists_or_symlink(&symlink_path) {
                        remove_link_path(&symlink_path).with_context(|| {
                            format!(
                                "failed to remove stale dependency link for {}: {}",
                                pkg_key,
                                symlink_path.display()
                            )
                        })?;
                    }
                    if let Some(parent) = symlink_path.parent() {
                        fs::create_dir_all(parent)?;
                        let symlink_target = relative_path(parent, &target);
                        Self::create_dir_link(&symlink_target, &symlink_path).with_context(
                            || {
                                format!(
                                    "failed to link dependency {} for {}: {} -> {}",
                                    dep_name,
                                    pkg_key,
                                    symlink_path.display(),
                                    target.display()
                                )
                            },
                        )?;
                    }
                    report.symlinks_created += 1;
                }
            }
        }
        let virtual_deps_ms = virtual_deps_started.elapsed().as_millis() as u64;

        let direct_deps_started = Instant::now();
        // Create top-level symlinks for direct dependencies.
        for (direct_name, direct_key) in direct_name_to_key {
            let target = virtual_store_dir.join(direct_key).join("node_modules");
            let target = Self::package_path_in_node_modules(&target, &direct_name);
            let link = Self::package_path_in_node_modules(&self.node_modules, &direct_name);

            if Self::dir_link_needs_repair(&link, &target) {
                if path_exists_or_symlink(&link) {
                    remove_link_path(&link).with_context(|| {
                        format!(
                            "failed to remove stale direct dependency link {}: {}",
                            direct_name,
                            link.display()
                        )
                    })?;
                }
                if let Some(parent) = link.parent() {
                    fs::create_dir_all(parent)?;
                }
                Self::create_dir_link(&target, &link).with_context(|| {
                    format!(
                        "failed to link direct dependency {}: {} -> {}",
                        direct_name,
                        link.display(),
                        target.display()
                    )
                })?;
                report.symlinks_created += 1;
            }
        }
        let direct_deps_ms = direct_deps_started.elapsed().as_millis() as u64;

        // Write marker after successful link
        self.write_marker(graph_hash, graph.len())?;

        let duration_ms = started.elapsed().as_millis() as u64;
        let files_linked = report.hardlinked_files + report.copied_files;
        let files_per_sec = if duration_ms == 0 {
            0.0
        } else {
            files_linked as f64 * 1000.0 / duration_ms as f64
        };
        debug!(
            target: "orix::perf",
            phase = "link_graph",
            duration_ms,
            packages = graph.len(),
            import_files_ms,
            bins_ms,
            virtual_deps_ms,
            direct_deps_ms,
            workspace_ms,
            workspace_packages,
            packages_imported,
            packages_import_skipped,
            slow_package_logs,
            hardlinked_files = report.hardlinked_files,
            copied_files = report.copied_files,
            symlinks_created = report.symlinks_created,
            bytes_saved = report.bytes_saved,
            files_per_sec,
            "link_graph complete"
        );

        Ok(report)
    }
}
