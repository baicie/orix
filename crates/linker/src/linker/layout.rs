//! Layout operations.

use super::prelude::*;
use super::{Linker, METADATA_FILE, VIRTUAL_STORE_DIR};
use crate::linker_platform::*;
use tracing::{trace, warn};

impl Linker {
    /// Remove stale virtual-store packages and top-level links not in the current graph.
    ///
    /// Unlike [`Self::unlink`], this keeps valid packages and avoids deleting the entire
    /// `node_modules` tree (important on Windows where full removal is slow and triggers AV).
    pub fn prune_stale_layout(
        &self,
        graph: &DependencyGraph,
        direct_deps: &std::collections::HashSet<String>,
    ) -> Result<()> {
        let expected_keys: HashSet<String> = graph.packages().map(|p| p.id.key()).collect();
        let virtual_store = self.node_modules.join(VIRTUAL_STORE_DIR);

        if virtual_store.is_dir() {
            for entry in fs::read_dir(&virtual_store)? {
                let entry = entry?;
                let file_name = entry.file_name().to_string_lossy().to_string();
                if file_name == METADATA_FILE {
                    continue;
                }
                if !entry.file_type()?.is_dir() {
                    continue;
                }
                if !expected_keys.contains(&file_name) {
                    if let Err(e) = fs::remove_dir_all(entry.path()) {
                        warn!(
                            pkg_key = %file_name,
                            error = %e,
                            path = %entry.path().display(),
                            "failed to prune stale virtual-store package"
                        );
                    } else {
                        trace!(pkg_key = %file_name, "pruned stale virtual-store package");
                    }
                }
            }
        }

        if self.node_modules.is_dir() {
            for entry in fs::read_dir(&self.node_modules)? {
                let entry = entry?;
                let file_name = entry.file_name().to_string_lossy().to_string();
                if file_name == ".bin" || file_name == VIRTUAL_STORE_DIR {
                    continue;
                }

                if file_name.starts_with('@') {
                    if !entry.file_type()?.is_dir() {
                        continue;
                    }
                    for scoped in fs::read_dir(entry.path())? {
                        let scoped = scoped?;
                        let scoped_name = scoped.file_name().to_string_lossy().to_string();
                        let full_name = format!("{file_name}/{scoped_name}");
                        if direct_deps.contains(&full_name) {
                            continue;
                        }
                        if path_exists_or_symlink(&scoped.path()) {
                            if let Err(e) = remove_link_path(&scoped.path()) {
                                warn!(
                                    path = %scoped.path().display(),
                                    error = %e,
                                    "failed to prune stale scoped package link"
                                );
                            }
                        }
                    }
                    continue;
                }

                if direct_deps.contains(&file_name) {
                    continue;
                }
                if path_exists_or_symlink(&entry.path()) {
                    if let Err(e) = remove_link_path(&entry.path()) {
                        warn!(
                            path = %entry.path().display(),
                            error = %e,
                            "failed to prune stale top-level package link"
                        );
                    }
                }
            }
        }

        let bin_dir = self.node_modules.join(".bin");
        if bin_dir.is_dir() {
            if let Err(e) = fs::remove_dir_all(&bin_dir) {
                warn!(
                    path = %bin_dir.display(),
                    error = %e,
                    "failed to clear .bin before relink"
                );
            }
        }

        Ok(())
    }

    /// Remove stale top-level links and `.bin` without pruning hidden virtual-store packages.
    ///
    /// Workspace member installs use this lighter pass because stale packages under
    /// `node_modules/.orix` are not visible to Node unless a live symlink points at them.
    /// Avoiding recursive deletion keeps large workspaces from paying an O(all packages)
    /// cleanup cost for every member package.
    pub fn prune_stale_direct_links(
        &self,
        direct_deps: &std::collections::HashSet<String>,
    ) -> Result<()> {
        if self.node_modules.is_dir() {
            for entry in fs::read_dir(&self.node_modules)? {
                let entry = entry?;
                let file_name = entry.file_name().to_string_lossy().to_string();
                if file_name == ".bin" || file_name == VIRTUAL_STORE_DIR {
                    continue;
                }

                if file_name.starts_with('@') {
                    if !entry.file_type()?.is_dir() {
                        continue;
                    }
                    for scoped in fs::read_dir(entry.path())? {
                        let scoped = scoped?;
                        let scoped_name = scoped.file_name().to_string_lossy().to_string();
                        let full_name = format!("{file_name}/{scoped_name}");
                        if direct_deps.contains(&full_name) {
                            continue;
                        }
                        if path_exists_or_symlink(&scoped.path()) {
                            if let Err(e) = remove_link_path(&scoped.path()) {
                                warn!(
                                    path = %scoped.path().display(),
                                    error = %e,
                                    "failed to prune stale scoped package link"
                                );
                            }
                        }
                    }
                    continue;
                }

                if direct_deps.contains(&file_name) {
                    continue;
                }
                if path_exists_or_symlink(&entry.path()) {
                    if let Err(e) = remove_link_path(&entry.path()) {
                        warn!(
                            path = %entry.path().display(),
                            error = %e,
                            "failed to prune stale top-level package link"
                        );
                    }
                }
            }
        }

        let bin_dir = self.node_modules.join(".bin");
        if bin_dir.is_dir() {
            if let Err(e) = fs::remove_dir_all(&bin_dir) {
                warn!(
                    path = %bin_dir.display(),
                    error = %e,
                    "failed to clear .bin before relink"
                );
            }
        }

        Ok(())
    }

    /// Remove all generated links and `.orix/` content for this project.
    pub fn unlink(&self) -> Result<()> {
        if self.node_modules.exists() {
            fs::remove_dir_all(&self.node_modules)?;
        }
        Ok(())
    }

    /// Create a top-level symlink for a local workspace package.
    /// Links `node_modules/<pkg_name>` directly to the local source directory,
    /// bypassing the .orix/ store. Returns the number of symlinks created (0 or 1).
    pub fn link_local_package(&self, pkg_name: &str, local_source: &Path) -> Result<usize> {
        let link_path = Self::package_path_in_node_modules(&self.node_modules, pkg_name);

        if link_path.exists() {
            return Ok(0);
        }

        if let Some(parent) = link_path.parent() {
            fs::create_dir_all(parent)?;
        }

        Self::create_dir_link(local_source, &link_path).with_context(|| {
            format!(
                "failed to link local package {}: {} -> {}",
                pkg_name,
                link_path.display(),
                local_source.display()
            )
        })?;
        Ok(1)
    }

    /// Link a direct dependency to an existing package directory, then expose its bins.
    pub fn link_direct_package_from(
        &self,
        pkg_name: &str,
        package_dir: &Path,
        report: &mut LinkReport,
    ) -> Result<()> {
        let link_path = Self::package_path_in_node_modules(&self.node_modules, pkg_name);
        if Self::dir_link_needs_repair(&link_path, package_dir) {
            if path_exists_or_symlink(&link_path) {
                remove_link_path(&link_path)?;
            }
            if let Some(parent) = link_path.parent() {
                fs::create_dir_all(parent)?;
            }
            Self::create_dir_link(package_dir, &link_path)?;
            report.symlinks_created += 1;
        }

        self.link_package_dir_bins(package_dir, report)
    }

    /// Validate that direct dependencies and generated symlinks are resolvable.
    pub fn validate_layout(&self, direct_deps: &HashSet<String>) -> Result<LayoutReport> {
        let mut report = LayoutReport::default();

        if !self.node_modules.exists() {
            report
                .broken
                .push(format!("missing {}", self.node_modules.display()));
            return Ok(report);
        }

        for dep in direct_deps {
            let path = Self::package_path_in_node_modules(&self.node_modules, dep);
            if !path.exists() {
                report
                    .broken
                    .push(format!("missing direct dependency {}", path.display()));
                continue;
            }

            if !self.direct_dep_is_in_virtual_store(&path) {
                continue;
            }

            for bin_name in direct_package_bin_names(&path)? {
                for shim_path in self.expected_bin_shims(&bin_name) {
                    if !shim_path.exists() {
                        report.broken.push(format!(
                            "missing bin shim for {}: {}",
                            dep,
                            shim_path.display()
                        ));
                    }
                }
            }
        }

        let virtual_store = self.node_modules.join(VIRTUAL_STORE_DIR);
        if virtual_store.is_dir() {
            for entry in WalkDir::new(&virtual_store)
                .follow_links(false)
                .into_iter()
                .filter_map(|e| e.ok())
            {
                if !entry.file_type().is_symlink() {
                    continue;
                }

                let link_path = entry.path();
                let target = fs::read_link(link_path)?;
                let resolved = if target.is_absolute() {
                    target
                } else {
                    link_path
                        .parent()
                        .unwrap_or_else(|| Path::new(""))
                        .join(target)
                };

                if is_bare_drive_path(&resolved) || resolves_to_drive_root_only(&resolved) {
                    report.broken.push(format!(
                        "unsafe directory link {} -> {} (bare drive root)",
                        link_path.display(),
                        resolved.display()
                    ));
                } else if !resolved.exists() {
                    report.broken.push(format!(
                        "broken symlink {} -> {}",
                        link_path.display(),
                        resolved.display()
                    ));
                }
            }
        }

        Ok(report)
    }

    /// True when `link` is missing or points at a different / unsafe target than `expected`.
    pub(crate) fn dir_link_needs_repair(link: &Path, expected: &Path) -> bool {
        if !path_exists_or_symlink(link) {
            return true;
        }

        #[cfg(windows)]
        {
            let Ok(raw_target) = fs::read_link(link) else {
                return true;
            };
            let resolved = if raw_target.is_absolute() {
                raw_target
            } else {
                link.parent()
                    .unwrap_or_else(|| Path::new(""))
                    .join(&raw_target)
            };

            if is_bare_drive_path(&resolved) || resolves_to_drive_root_only(&resolved) {
                return true;
            }

            let Ok(expected_canon) = expected.canonicalize() else {
                return true;
            };
            let Ok(link_canon) = resolved.canonicalize() else {
                return true;
            };
            link_canon != expected_canon
        }

        #[cfg(not(windows))]
        {
            let _ = (link, expected);
            false
        }
    }

    /// True when every file listed in store integrity metadata exists under `pkg_dir`.
    pub(crate) fn is_package_import_complete(
        &self,
        pkg_id: &orix_domain::PackageId,
        pkg_dir: &Path,
    ) -> Result<bool> {
        if !pkg_dir.join("package.json").exists() {
            return Ok(false);
        }

        let integrity = match self.store.get_integrity(pkg_id) {
            Ok(i) => i,
            Err(_) => return Ok(false),
        };

        Ok(integrity
            .files
            .iter()
            .all(|(rel_path, _)| pkg_dir.join(rel_path).exists()))
    }

    /// Whether `src` and `dest` live on the same filesystem volume (hardlink-safe).
    pub(crate) fn same_volume(src: &Path, dest: &Path) -> bool {
        #[cfg(unix)]
        {
            let (Ok(src_meta), Ok(dest_meta)) = (fs::metadata(src), fs::metadata(dest)) else {
                return false;
            };
            src_meta.dev() == dest_meta.dev()
        }

        #[cfg(windows)]
        {
            let src_root = src.canonicalize().ok();
            let dest_root = dest.parent().and_then(|p| p.canonicalize().ok());
            match (
                src_root.as_ref().and_then(|p| volume_root(p)),
                dest_root.as_ref().and_then(|p| volume_root(p)),
            ) {
                (Some(a), Some(b)) => a == b,
                _ => false,
            }
        }

        #[cfg(not(any(unix, windows)))]
        {
            let _ = (src, dest);
            false
        }
    }

    /// Return the filesystem path for a package name under a node_modules root.
    pub fn package_path_in_node_modules(root: &Path, package_name: &str) -> PathBuf {
        package_name
            .split('/')
            .fold(root.to_path_buf(), |path, part| path.join(part))
    }

    fn direct_dep_is_in_virtual_store(&self, path: &Path) -> bool {
        let virtual_store = self.node_modules.join(VIRTUAL_STORE_DIR);
        let Ok(resolved) = path.canonicalize() else {
            return false;
        };
        let Ok(resolved_virtual_store) = virtual_store.canonicalize() else {
            return false;
        };

        normal_components(&resolved).starts_with(&normal_components(&resolved_virtual_store))
    }

    fn expected_bin_shims(&self, bin_name: &str) -> Vec<PathBuf> {
        let flat_name = std::path::Path::new(bin_name)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(bin_name);
        let bin_dir = self.node_modules.join(".bin");

        #[cfg(windows)]
        {
            vec![
                bin_dir.join(format!("{flat_name}.cmd")),
                bin_dir.join(format!("{flat_name}.ps1")),
            ]
        }

        #[cfg(not(windows))]
        {
            vec![bin_dir.join(flat_name)]
        }
    }
}

fn direct_package_bin_names(package_dir: &Path) -> Result<Vec<String>> {
    let pkg_json_path = package_dir.join("package.json");
    if !pkg_json_path.exists() {
        return Ok(Vec::new());
    }

    let content = fs::read_to_string(&pkg_json_path)
        .with_context(|| format!("failed to read {}", pkg_json_path.display()))?;
    let pkg_json: serde_json::Value = serde_json::from_str(&content)
        .with_context(|| format!("failed to parse {}", pkg_json_path.display()))?;
    let Some(bin_value) = pkg_json.get("bin") else {
        return Ok(Vec::new());
    };

    let names = match bin_value {
        serde_json::Value::String(_) => pkg_json
            .get("name")
            .and_then(|name| name.as_str())
            .map(|name| vec![name.to_string()])
            .unwrap_or_default(),
        serde_json::Value::Object(entries) => entries.keys().cloned().collect(),
        _ => Vec::new(),
    };

    Ok(names)
}
