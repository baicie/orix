//! Linker implementation.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tracing::{debug, trace, warn};
use walkdir::WalkDir;

use orix_domain::{ConstraintKind, DependencyGraph, PackageId, PackageName, VersionConstraint};
use orix_store::Store;

use super::{LayoutReport, LinkReport};

const VIRTUAL_STORE_DIR: &str = ".orix";
const METADATA_FILE: &str = "metadata.json";

/// Marker written to node_modules/.orix/metadata.json after a successful link.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct LinkerMarker {
    /// Hash of the dependency graph at link time.
    pub graph_hash: String,
    /// Version of orix that performed the link.
    pub orix_version: String,
    /// Number of packages linked.
    pub package_count: usize,
}

/// The linker creates the Orix virtual node_modules structure using hardlinks and symlinks.
pub struct Linker {
    store: Store,
    node_modules: PathBuf,
}

impl Linker {
    /// Create a new linker.
    pub fn new(store: Store, node_modules: PathBuf) -> Self {
        Self {
            store,
            node_modules,
        }
    }

    /// Path to the marker file.
    fn marker_path(&self) -> PathBuf {
        self.node_modules
            .join(VIRTUAL_STORE_DIR)
            .join(METADATA_FILE)
    }

    /// Read the linker marker, if it exists.
    fn read_marker(&self) -> Option<LinkerMarker> {
        let path = self.marker_path();
        let content = fs::read_to_string(&path).ok()?;
        serde_json::from_str(&content).ok()
    }

    /// Check whether the current layout marker matches the given graph hash.
    /// Returns true if the marker exists and the graph hash matches.
    pub fn is_layout_valid(&self, graph_hash: &str) -> bool {
        match self.read_marker() {
            Some(marker) => marker.graph_hash == graph_hash && self.bin_shims_are_valid(),
            None => false,
        }
    }

    #[cfg(windows)]
    fn bin_shims_are_valid(&self) -> bool {
        let bin_dir = self.node_modules.join(".bin");
        if !bin_dir.is_dir() {
            return true;
        }

        WalkDir::new(&bin_dir)
            .follow_links(false)
            .into_iter()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_type().is_file())
            .all(|entry| {
                let path = entry.path();
                if path.extension().is_some_and(|ext| ext == "cmd") {
                    return true;
                }

                path.with_extension("cmd").exists()
            })
    }

    #[cfg(not(windows))]
    fn bin_shims_are_valid(&self) -> bool {
        let bin_dir = self.node_modules.join(".bin");
        if !bin_dir.is_dir() {
            return true;
        }

        WalkDir::new(&bin_dir)
            .follow_links(false)
            .into_iter()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_type().is_symlink() || entry.file_type().is_file())
            .all(|entry| {
                let executable = fs::metadata(entry.path())
                    .map(|metadata| metadata.mode() & 0o111 != 0)
                    .unwrap_or(false);
                executable && self.bin_shim_points_into_package(entry.path())
            })
    }

    #[cfg(not(windows))]
    fn bin_shim_points_into_package(&self, shim_path: &Path) -> bool {
        let target = match fs::read_link(shim_path) {
            Ok(target) => target,
            Err(_) => return true,
        };
        let resolved = if target.is_absolute() {
            target
        } else {
            shim_path
                .parent()
                .unwrap_or_else(|| Path::new(""))
                .join(target)
        };
        let resolved = fs::canonicalize(&resolved).unwrap_or(resolved);

        let virtual_store = self.node_modules.join(VIRTUAL_STORE_DIR);
        if !path_starts_with_lexically(&resolved, &virtual_store) {
            return true;
        }

        normal_components(&resolved)
            .iter()
            .any(|part| part == "node_modules")
    }

    /// Write the linker marker after a successful link.
    fn write_marker(&self, graph_hash: &str, package_count: usize) -> Result<()> {
        let marker = LinkerMarker {
            graph_hash: graph_hash.to_string(),
            orix_version: env!("CARGO_PKG_VERSION").to_string(),
            package_count,
        };
        let json = serde_json::to_string_pretty(&marker)?;
        let path = self.marker_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, json)?;
        Ok(())
    }

    /// Build the full node_modules layout from a dependency graph.
    /// Workspace packages (tarball is empty) are linked to their local source directories.
    pub fn link_graph(
        &self,
        graph: &DependencyGraph,
        direct_deps: &std::collections::HashSet<String>,
        workspace: Option<&orix_workspace::Workspace>,
        graph_hash: &str,
    ) -> Result<LinkReport> {
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

        for pkg in graph.packages() {
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

            // Skip if package is already fully imported (package.json exists).
            let pkg_dest_dir = pkg_dir
                .parent()
                .ok_or_else(|| anyhow::anyhow!("pkg_dir has no parent: {}", pkg_dir.display()))?;
            if pkg_dest_dir.join("package.json").exists() {
                trace!(pkg = %pkg_key, "package already imported, skipping files");
            } else {
                self.import_package_files(&pkg.id, &pkg_dir, &store_files, &mut report)?;
            }

            // Import package files from the store using integrity metadata.
            // Uses integrity.files to avoid WalkDir, pre-creates all directories,
            // falls back to copy on EXDEV, and writes package.json last.
            self.import_package_files(&pkg.id, &pkg_dir, &store_files, &mut report)?;

            // Link bin executables for this package into .orix/<pkg>/bin/
            let link_global_bins = direct_name_to_key
                .get(pkg.id.name.as_str())
                .is_some_and(|direct_key| direct_key == &pkg_key);
            self.link_package_bins(&pkg_key, &store_files, link_global_bins, &mut report)?;
        }

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

                if !path_exists_or_symlink(&symlink_path) {
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

        // Create top-level symlinks for direct dependencies.
        for (direct_name, direct_key) in direct_name_to_key {
            let target = virtual_store_dir.join(direct_key).join("node_modules");
            let target = Self::package_path_in_node_modules(&target, &direct_name);
            let link = Self::package_path_in_node_modules(&self.node_modules, &direct_name);

            if !link.exists() {
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

        // Write marker after successful link
        self.write_marker(graph_hash, graph.len())?;

        debug!(
            packages = graph.len(),
            hardlinked_files = report.hardlinked_files,
            copied_files = report.copied_files,
            symlinks_created = report.symlinks_created,
            "link completed"
        );

        Ok(report)
    }

    /// Import package files from the store into a package directory.
    ///
    /// Optimizations:
    /// - Iterates `integrity.files` instead of WalkDir to avoid scanning the store directory.
    /// - Pre-creates all needed directories in one pass before linking/copying files.
    /// - Falls back to copy on EXDEV and remembers the decision per-package.
    /// - Writes `package.json` last as a completion marker.
    fn import_package_files(
        &self,
        pkg_id: &orix_domain::PackageId,
        pkg_dir: &Path,
        store_files: &Path,
        report: &mut LinkReport,
    ) -> Result<()> {
        let pkg_key = pkg_id.key();

        // Read integrity metadata to get the file list without WalkDir.
        let integrity = match self.store.get_integrity(pkg_id) {
            Ok(i) => i,
            Err(e) => {
                warn!(pkg = %pkg_key, error = %e, "failed to read integrity metadata, skipping files");
                return Ok(());
            }
        };

        // Collect and pre-create all needed directories in one pass.
        let mut dirs: HashSet<PathBuf> = HashSet::new();
        for (rel_path, _) in &integrity.files {
            if let Some(parent) = Path::new(rel_path).parent() {
                if !parent.as_os_str().is_empty() {
                    dirs.insert(parent.to_path_buf());
                }
            }
        }
        // Sort by depth so shallow dirs are created before deep ones.
        let mut dirs: Vec<_> = dirs.into_iter().collect();
        dirs.sort_by_key(|p| p.components().count());
        for dir in &dirs {
            let full = pkg_dir.join(dir);
            if let Err(e) = fs::create_dir_all(&full) {
                warn!(pkg = %pkg_key, error = %e, "failed to create dir {}", full.display());
            }
        }

        // Decide import strategy: try hardlink first, fall back to copy on EXDEV.
        // Once we decide to copy, all remaining files use copy (per-package decision).
        let mut use_copy = false;

        let mut hardlink_ok = 0u64;
        let mut copy_ok = 0u64;
        let mut hardlink_fail = 0u64;
        let mut copy_fail = 0u64;

        // Separate package.json to write it last.
        let mut normal_files: Vec<&(String, String)> = Vec::new();
        let mut package_json: Option<&(String, String)> = None;

        for entry in &integrity.files {
            if entry.0 == "package.json" {
                package_json = Some(entry);
            } else {
                normal_files.push(entry);
            }
        }

        // Import all files except package.json.
        for (rel_path, _) in normal_files {
            let src = store_files.join(rel_path);
            let dest = pkg_dir.join(rel_path);

            if !src.exists() {
                warn!(pkg = %pkg_key, missing = %src.display(), "file missing from store");
                continue;
            }

            if use_copy {
                Self::copy_with_mode(&src, &dest, &mut copy_ok, &mut copy_fail, &pkg_key)?;
            } else {
                #[allow(clippy::incompatible_msrv)]
                match fs::hard_link(&src, &dest) {
                    Ok(_) => hardlink_ok += 1,
                    Err(e) if e.kind() == io::ErrorKind::CrossesDevices => {
                        use_copy = true;
                        Self::copy_with_mode(&src, &dest, &mut copy_ok, &mut copy_fail, &pkg_key)?;
                    }
                    Err(e)
                        if e.kind() == io::ErrorKind::PermissionDenied
                            || e.kind() == io::ErrorKind::NotFound =>
                    {
                        Self::copy_with_mode(&src, &dest, &mut copy_ok, &mut copy_fail, &pkg_key)?;
                    }
                    Err(e) => {
                        hardlink_fail += 1;
                        warn!(pkg = %pkg_key, error = %e, "hard_link failed {} -> {}", src.display(), dest.display());
                    }
                }
            }
        }

        // Write package.json last as the completion marker.
        if let Some((rel_path, _)) = package_json {
            let src = store_files.join(rel_path);
            let dest = pkg_dir.join(rel_path);
            if src.exists() {
                match fs::copy(&src, &dest) {
                    Ok(_) => {
                        if use_copy {
                            copy_ok += 1;
                        } else {
                            hardlink_ok += 1;
                        }
                    }
                    Err(e) => {
                        copy_fail += 1;
                        warn!(pkg = %pkg_key, error = %e, "failed to write package.json {}", dest.display());
                    }
                }
            }
        }

        report.hardlinked_files += hardlink_ok;
        report.copied_files += copy_ok;
        trace!(pkg = %pkg_key, hardlink_ok, copy_ok, hardlink_fail, copy_fail, use_copy, "imported package files");
        if hardlink_ok == 0 && copy_ok == 0 && hardlink_fail == 0 && copy_fail == 0 {
            warn!(pkg = %pkg_key, "no files were imported");
        }

        Ok(())
    }

    /// Copy a file preserving its Unix permissions (mode bits).
    fn copy_with_mode(
        src: &Path,
        dest: &Path,
        copy_ok: &mut u64,
        _copy_fail: &mut u64,
        _pkg_key: &str,
    ) -> Result<()> {
        #[cfg(unix)]
        {
            let mode = fs::metadata(src)
                .map(|m| m.mode())
                .with_context(|| format!("failed to stat {} for permission copy", src.display()))?;
            fs::copy(src, dest).with_context(|| {
                format!("failed to copy {} -> {}", src.display(), dest.display())
            })?;
            fs::set_permissions(dest, PermissionsExt::from_mode(mode & 0o777))
                .with_context(|| format!("failed to set permissions on {}", dest.display()))?;
            *copy_ok += 1;
        }
        #[cfg(not(unix))]
        {
            if fs::copy(src, dest).is_ok() {
                *copy_ok += 1;
            } else {
                *_copy_fail += 1;
            }
        }
        Ok(())
    }

    /// Link bin executables from a package into the .orix/<pkg>/bin directory.
    /// Also creates the global .bin/ directory with shims for each bin.
    ///
    /// Windows: creates .cmd and .ps1 shims pointing to the actual bin file.
    /// Unix: creates symlinks.
    /// Scoped bin names (e.g. `@antfu/eslint-config`) are flattened to just the
    /// filename part (`eslint-config`) to avoid `@` characters in filenames.
    fn link_package_bins(
        &self,
        pkg_key: &str,
        store_files: &Path,
        link_global_bins: bool,
        report: &mut LinkReport,
    ) -> Result<()> {
        let pkg_json_path = store_files.join("package.json");
        if !pkg_json_path.exists() {
            return Ok(());
        }

        let pkg_json_content = std::fs::read_to_string(&pkg_json_path)?;
        let pkg_json: serde_json::Value =
            serde_json::from_str(&pkg_json_content).unwrap_or_default();

        let bin_value = match pkg_json.get("bin") {
            Some(v) => v,
            None => return Ok(()),
        };

        let bin_entries: Vec<(String, String)> = match bin_value {
            serde_json::Value::String(s) => {
                let pkg_name = pkg_json.get("name").and_then(|v| v.as_str()).unwrap_or("");
                vec![(pkg_name.to_string(), s.clone())]
            }
            serde_json::Value::Object(m) => m
                .iter()
                .map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string()))
                .collect(),
            _ => return Ok(()),
        };

        let pkg_name = pkg_json.get("name").and_then(|v| v.as_str()).unwrap_or("");
        if pkg_name.is_empty() {
            return Ok(());
        }

        // Package dir: .orix/<pkg>@<ver>/node_modules/<pkg>/.
        let package_store_dir = self.node_modules.join(VIRTUAL_STORE_DIR).join(pkg_key);
        let package_dir =
            Self::package_path_in_node_modules(&package_store_dir.join("node_modules"), pkg_name);
        let global_bin_dir = self.node_modules.join(".bin");

        for (bin_name, bin_path) in bin_entries {
            if bin_name.is_empty() || bin_path.is_empty() {
                continue;
            }

            // The actual bin file in the store.
            let bin_source = store_files.join(&bin_path);
            if !bin_source.exists() {
                trace!(
                    pkg = %pkg_key,
                    bin = %bin_name,
                    missing = %bin_source.display(),
                    "bin source not in store"
                );
                continue;
            }

            // Flatten scoped bin names: "@antfu/eslint-config" -> "eslint-config"
            let flat_bin_name = std::path::Path::new(&bin_name)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(&bin_name);

            // Shim bin name (also flattened).
            let shim_bin_name = flat_bin_name;

            let package_bin = package_dir.join(&bin_path);
            if !package_bin.exists() {
                trace!(
                    pkg = %pkg_key,
                    bin = %bin_name,
                    missing = %package_bin.display(),
                    "bin target not in linked package"
                );
                continue;
            }

            Self::ensure_bin_executable(&package_bin).with_context(|| {
                format!("failed to make bin executable: {}", package_bin.display())
            })?;

            if !link_global_bins {
                continue;
            }

            // Global shims: only create if the bin file was successfully placed.
            if package_bin.exists() {
                #[cfg(windows)]
                {
                    // Resolve the bin to an absolute path so the shim works from any cwd.
                    let absolute_bin = package_bin.canonicalize().with_context(|| {
                        format!(
                            "failed to resolve bin target {} for shim",
                            package_bin.display()
                        )
                    })?;

                    Self::create_windows_bin_shims(&global_bin_dir, shim_bin_name, &absolute_bin)
                        .with_context(|| {
                        format!("failed to create Windows bin shim for {}", bin_name)
                    })?;
                    report.symlinks_created += 2;
                }

                #[cfg(not(windows))]
                {
                    let shim_link = global_bin_dir.join(shim_bin_name);
                    if !path_exists_or_symlink(&shim_link) {
                        if let Some(parent) = shim_link.parent() {
                            fs::create_dir_all(parent)?;
                        }
                        let rel = relative_path(
                            shim_link.parent().unwrap_or(std::path::Path::new(".")),
                            &package_bin,
                        );
                        std::os::unix::fs::symlink(&rel, &shim_link)?;
                        report.symlinks_created += 1;
                    }
                }
            }
        }

        Ok(())
    }

    #[cfg(unix)]
    fn ensure_bin_executable(path: &Path) -> io::Result<()> {
        let metadata = fs::metadata(path)?;
        let mode = metadata.mode();
        if mode & 0o111 != 0 {
            return Ok(());
        }
        fs::set_permissions(path, PermissionsExt::from_mode((mode | 0o111) & 0o777))
    }

    #[cfg(not(unix))]
    fn ensure_bin_executable(_path: &Path) -> io::Result<()> {
        Ok(())
    }

    #[cfg(windows)]
    fn create_windows_bin_shims(
        global_bin_dir: &Path,
        shim_bin_name: &str,
        absolute_bin_path: &Path,
    ) -> Result<()> {
        fs::create_dir_all(global_bin_dir)
            .with_context(|| format!("failed to create {}", global_bin_dir.display()))?;

        let cmd_path = global_bin_dir.join(format!("{shim_bin_name}.cmd"));
        let ps1_path = global_bin_dir.join(format!("{shim_bin_name}.ps1"));

        let target = absolute_bin_path.display().to_string().replace('/', "\\");

        let cmd_content = format!(
            "@ECHO off\r\n\
SETLOCAL\r\n\
SET \"basedir=%~dp0\"\r\n\
IF EXIST \"%basedir%\\node.exe\" (\r\n\
  SET \"_prog=%basedir%\\node.exe\"\r\n\
) ELSE (\r\n\
  SET \"_prog=node\"\r\n\
)\r\n\
\"%_prog%\" \"{target}\" %*\r\n"
        );

        let ps1_target = target.replace('\\', "/");
        let ps1_content = format!(
            "$basedir = Split-Path $MyInvocation.MyCommand.Definition -Parent\n\
$exe = Join-Path $basedir 'node.exe'\n\
if (Test-Path $exe) {{\n\
  & $exe '{ps1_target}' @args\n\
}} else {{\n\
  & node '{ps1_target}' @args\n\
}}\n"
        );

        fs::write(&cmd_path, &cmd_content)
            .with_context(|| format!("failed to write {}", cmd_path.display()))?;

        fs::write(&ps1_path, &ps1_content)
            .with_context(|| format!("failed to write {}", ps1_path.display()))?;

        Ok(())
    }

    /// Create a directory link, falling back to junction on Windows when needed.
    fn create_dir_link(target: &Path, link: &Path) -> io::Result<()> {
        #[cfg(windows)]
        {
            match std::os::windows::fs::symlink_dir(target, link) {
                Ok(_) => return Ok(()),
                Err(e) => {
                    debug!(
                        target = %target.display(),
                        link = %link.display(),
                        error = %e,
                        "directory symlink failed; trying junction fallback"
                    );
                }
            }

            let absolute_target = Self::absolutize_link_target(target, link)?;
            Self::create_junction(&absolute_target, link)
        }
        #[cfg(not(windows))]
        {
            std::os::unix::fs::symlink(target, link)
        }
    }

    /// Create a file link for package binaries.
    #[cfg(not(windows))]
    #[allow(dead_code)]
    fn create_file_link(target: &Path, link: &Path) -> io::Result<()> {
        #[cfg(windows)]
        {
            let absolute_target = Self::absolutize_link_target(target, link)?;
            match fs::hard_link(&absolute_target, link) {
                Ok(_) => Ok(()),
                Err(e) => {
                    debug!(
                        target = %absolute_target.display(),
                        link = %link.display(),
                        error = %e,
                        "binary hardlink failed; copying file"
                    );
                    fs::copy(&absolute_target, link).map(|_| ())
                }
            }
        }
        #[cfg(not(windows))]
        {
            std::os::unix::fs::symlink(target, link)
        }
    }

    #[cfg(windows)]
    fn absolutize_link_target(target: &Path, link: &Path) -> io::Result<PathBuf> {
        if target.is_absolute() {
            return target.canonicalize();
        }

        let parent = link.parent().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "link path has no parent")
        })?;
        parent.join(target).canonicalize()
    }

    /// Create a Windows junction point (directory symbolic link alternative).
    /// Junctions don't require admin privileges on Windows Vista+.
    #[cfg(windows)]
    fn create_junction(target: &Path, link: &Path) -> io::Result<()> {
        use std::process::Command;

        // junction tool requires the link to not exist, and target must be absolute
        if link.exists() {
            return Ok(());
        }

        let target_str = target.display().to_string();
        let link_str = link.display().to_string();

        let output = Command::new("cmd")
            .args(["/C", "mklink", "/J", &link_str, &target_str])
            .output();

        match output {
            Ok(o) if o.status.success() => Ok(()),
            Ok(o) => Err(io::Error::other(format!(
                "failed to create junction {} -> {}: {}{}",
                link.display(),
                target.display(),
                String::from_utf8_lossy(&o.stderr),
                String::from_utf8_lossy(&o.stdout)
            ))),
            Err(e) => Err(e),
        }
    }

    /// Remove all generated links and .orix/ content for this project.
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
            }
        }

        for entry in WalkDir::new(&self.node_modules)
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

            if !resolved.exists() {
                report.broken.push(format!(
                    "broken symlink {} -> {}",
                    link_path.display(),
                    resolved.display()
                ));
            }
        }

        Ok(report)
    }

    fn package_path_in_node_modules(root: &Path, package_name: &str) -> PathBuf {
        package_name
            .split('/')
            .fold(root.to_path_buf(), |path, part| path.join(part))
    }
}

/// Returns true if the path exists as a file or symlink.
#[allow(dead_code)]
fn path_exists_or_symlink(path: &Path) -> bool {
    path.exists() || fs::symlink_metadata(path).is_ok()
}

fn path_starts_with_lexically(path: &Path, prefix: &Path) -> bool {
    let path_components = normal_components(path);
    let prefix_components = normal_components(prefix);
    path_components.starts_with(&prefix_components)
}

fn select_dependency_key(
    graph: &DependencyGraph,
    dep_name: &PackageName,
    raw: &str,
) -> Option<String> {
    let constraint = VersionConstraint::parse(raw).ok()?;
    graph
        .packages()
        .filter(|pkg| pkg.id.name == *dep_name && package_matches_constraint(&pkg.id, &constraint))
        .map(|pkg| pkg.id.key())
        .last()
}

fn package_matches_constraint(pkg_id: &PackageId, constraint: &VersionConstraint) -> bool {
    match &constraint.kind {
        ConstraintKind::Exact(version) => pkg_id.version == *version,
        ConstraintKind::Range(req) => req.matches(&pkg_id.version),
        ConstraintKind::AnyRange(ranges) => ranges.iter().any(|req| req.matches(&pkg_id.version)),
        ConstraintKind::Alias { constraint, .. } => package_matches_constraint(pkg_id, constraint),
        ConstraintKind::Patch(spec) => pkg_id.version == spec.package_version,
        ConstraintKind::Latest | ConstraintKind::Tag(_) | ConstraintKind::Catalog(_) => true,
    }
}

fn relative_path(from_dir: &Path, to_path: &Path) -> PathBuf {
    let from_components = normal_components(from_dir);
    let to_components = normal_components(to_path);
    let common_len = from_components
        .iter()
        .zip(to_components.iter())
        .take_while(|(from, to)| from == to)
        .count();

    let mut result = PathBuf::new();
    for _ in common_len..from_components.len() {
        result.push("..");
    }
    for component in &to_components[common_len..] {
        result.push(component);
    }

    if result.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        result
    }
}

fn normal_components(path: &Path) -> Vec<String> {
    path.components()
        .filter_map(|component| match component {
            std::path::Component::Normal(part) => part.to_str().map(ToOwned::to_owned),
            std::path::Component::ParentDir => Some("..".to_string()),
            std::path::Component::CurDir
            | std::path::Component::RootDir
            | std::path::Component::Prefix(_) => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use orix_domain::{DependencyGraph, PackageId, PackageName, ResolvedPackage, Version};

    fn pkg_id(name: &str, version: &str) -> anyhow::Result<PackageId> {
        Ok(PackageId::new(
            PackageName::from(name),
            Version::parse(version)?,
        ))
    }

    fn resolved_package(
        name: &str,
        version: &str,
        dependencies: Vec<(&str, &str)>,
    ) -> anyhow::Result<ResolvedPackage> {
        resolved_package_with_optional(name, version, dependencies, Vec::new())
    }

    fn resolved_package_with_optional(
        name: &str,
        version: &str,
        dependencies: Vec<(&str, &str)>,
        optional_dependencies: Vec<(&str, &str)>,
    ) -> anyhow::Result<ResolvedPackage> {
        resolved_package_with_optional_and_peers(
            name,
            version,
            dependencies,
            optional_dependencies,
            Vec::new(),
        )
    }

    fn resolved_package_with_optional_and_peers(
        name: &str,
        version: &str,
        dependencies: Vec<(&str, &str)>,
        optional_dependencies: Vec<(&str, &str)>,
        peer_dependencies: Vec<(&str, &str)>,
    ) -> anyhow::Result<ResolvedPackage> {
        Ok(ResolvedPackage {
            id: pkg_id(name, version)?,
            integrity: String::new(),
            tarball: String::new(),
            dependencies: dependencies
                .into_iter()
                .map(|(name, version)| (PackageName::from(name), version.to_string()))
                .collect(),
            dev_dependencies: Vec::new(),
            optional_dependencies: optional_dependencies
                .into_iter()
                .map(|(name, version)| (PackageName::from(name), version.to_string()))
                .collect(),
            peer_dependencies: peer_dependencies
                .into_iter()
                .map(|(name, version)| (PackageName::from(name), version.to_string()))
                .collect(),
            engines: None,
            os: Vec::new(),
            cpu: Vec::new(),
            depnodes: Vec::new(),
            patch: None,
        })
    }

    fn write_package(root: &Path, name: &str, version: &str) -> anyhow::Result<()> {
        fs::create_dir_all(root)?;
        fs::write(
            root.join("package.json"),
            format!(r#"{{"name":"{}","version":"{}"}}"#, name, version),
        )?;
        fs::write(root.join("index.js"), "module.exports = 1;\n")?;
        Ok(())
    }

    fn import_package(
        store: &Store,
        temp_root: &Path,
        name: &str,
        version: &str,
    ) -> anyhow::Result<PackageId> {
        let source = temp_root.join(format!("{}-{}", name.replace('/', "-"), version));
        write_package(&source, name, version)?;
        let id = pkg_id(name, version)?;
        store.import_package(&id, &source, Vec::new(), None)?;
        Ok(id)
    }

    fn import_package_with_manifest(
        store: &Store,
        temp_root: &Path,
        name: &str,
        version: &str,
        manifest: &str,
    ) -> anyhow::Result<PackageId> {
        let source = temp_root.join(format!("{}-{}", name.replace('/', "-"), version));
        fs::create_dir_all(source.join("bin"))?;
        fs::write(source.join("package.json"), manifest)?;
        fs::write(source.join("index.js"), "module.exports = 1;\n")?;
        fs::write(
            source.join("bin").join("index.mjs"),
            "#!/usr/bin/env node\n",
        )?;
        let id = pkg_id(name, version)?;
        store.import_package(&id, &source, Vec::new(), None)?;
        Ok(id)
    }

    fn import_package_with_rollup_style_bin(
        store: &Store,
        temp_root: &Path,
    ) -> anyhow::Result<PackageId> {
        let source = temp_root.join("rollup-4.0.0-relative-bin");
        fs::create_dir_all(source.join("bin"))?;
        fs::create_dir_all(source.join("shared"))?;
        fs::write(
            source.join("package.json"),
            r#"{"name":"rollup","version":"4.0.0","bin":{"rollup":"./bin/rollup"}}"#,
        )?;
        fs::write(
            source.join("bin").join("rollup"),
            "#!/usr/bin/env node\nrequire('../shared/rollup.js');\n",
        )?;
        fs::write(
            source.join("shared").join("rollup.js"),
            "module.exports = 1;\n",
        )?;
        let id = pkg_id("rollup", "4.0.0")?;
        store.import_package(&id, &source, Vec::new(), None)?;
        Ok(id)
    }

    #[test]
    fn link_graph_creates_valid_layout_for_direct_and_transitive_deps() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let store = Store::open(temp.path().join("store"))?;
        import_package(&store, temp.path(), "react", "18.2.0")?;
        import_package(&store, temp.path(), "scheduler", "0.23.0")?;

        let mut graph = DependencyGraph::new();
        graph.insert(resolved_package(
            "react",
            "18.2.0",
            vec![("scheduler", "0.23.0")],
        )?);
        graph.insert(resolved_package("scheduler", "0.23.0", Vec::new())?);

        let linker = Linker::new(store, temp.path().join("node_modules"));
        let direct_deps = HashSet::from(["react".to_string()]);
        linker.link_graph(&graph, &direct_deps, None, &graph.graph_hash())?;

        let report = linker.validate_layout(&direct_deps)?;

        assert!(report.is_ok());
        assert!(temp.path().join("node_modules").join("react").exists());
        assert!(temp
            .path()
            .join("node_modules")
            .join(".orix")
            .join("react@18.2.0")
            .exists());
        assert!(!temp.path().join("node_modules").join(".pnpm").exists());
        assert!(!temp.path().join("node_modules").join("scheduler").exists());
        Ok(())
    }

    #[test]
    fn validate_layout_reports_missing_direct_dependency() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let store = Store::open(temp.path().join("store"))?;
        let linker = Linker::new(store, temp.path().join("node_modules"));
        fs::create_dir_all(temp.path().join("node_modules"))?;
        let direct_deps = HashSet::from(["react".to_string()]);

        let report = linker.validate_layout(&direct_deps)?;

        assert!(!report.is_ok());
        assert_eq!(report.broken.len(), 1);
        Ok(())
    }

    #[test]
    fn link_graph_supports_scoped_direct_dependencies() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let store = Store::open(temp.path().join("store"))?;
        import_package(&store, temp.path(), "@scope/pkg", "1.0.0")?;

        let mut graph = DependencyGraph::new();
        graph.insert(resolved_package("@scope/pkg", "1.0.0", Vec::new())?);

        let linker = Linker::new(store, temp.path().join("node_modules"));
        let direct_deps = HashSet::from(["@scope/pkg".to_string()]);
        linker.link_graph(&graph, &direct_deps, None, &graph.graph_hash())?;

        let report = linker.validate_layout(&direct_deps)?;

        assert!(report.is_ok());
        assert!(temp
            .path()
            .join("node_modules")
            .join("@scope")
            .join("pkg")
            .exists());
        Ok(())
    }

    #[test]
    fn link_graph_supports_scoped_transitive_dependencies() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let store = Store::open(temp.path().join("store"))?;
        import_package(&store, temp.path(), "@scope/parent", "1.0.0")?;
        import_package(&store, temp.path(), "@scope/child", "1.0.0")?;

        let mut graph = DependencyGraph::new();
        graph.insert(resolved_package(
            "@scope/parent",
            "1.0.0",
            vec![("@scope/child", "1.0.0")],
        )?);
        graph.insert(resolved_package("@scope/child", "1.0.0", Vec::new())?);

        let linker = Linker::new(store, temp.path().join("node_modules"));
        let direct_deps = HashSet::from(["@scope/parent".to_string()]);
        linker.link_graph(&graph, &direct_deps, None, &graph.graph_hash())?;

        let report = linker.validate_layout(&direct_deps)?;

        assert!(report.is_ok(), "{:?}", report.broken);
        assert!(!temp
            .path()
            .join("node_modules")
            .join("@scope")
            .join("child")
            .exists());
        assert!(temp
            .path()
            .join("node_modules")
            .join(".orix")
            .join("@scope")
            .join("parent@1.0.0")
            .join("node_modules")
            .join("@scope")
            .join("parent")
            .join("node_modules")
            .join("@scope")
            .join("child")
            .exists());
        let dep_link = temp
            .path()
            .join("node_modules")
            .join(".orix")
            .join("@scope")
            .join("parent@1.0.0")
            .join("node_modules")
            .join("@scope")
            .join("parent")
            .join("node_modules")
            .join("@scope")
            .join("child");
        let resolved = fs::canonicalize(&dep_link)?;
        let expected = fs::canonicalize(
            temp.path()
                .join("node_modules")
                .join(".orix")
                .join("@scope")
                .join("child@1.0.0")
                .join("node_modules")
                .join("@scope")
                .join("child"),
        )?;
        assert_eq!(resolved, expected);
        Ok(())
    }

    #[test]
    fn link_graph_links_optional_dependencies_after_all_packages_are_imported() -> anyhow::Result<()>
    {
        let temp = tempfile::tempdir()?;
        let store = Store::open(temp.path().join("store"))?;
        import_package(&store, temp.path(), "rollup", "4.0.0")?;
        import_package(&store, temp.path(), "@rollup/rollup-darwin-arm64", "4.0.0")?;

        let mut graph = DependencyGraph::new();
        graph.insert(resolved_package_with_optional(
            "rollup",
            "4.0.0",
            Vec::new(),
            vec![("@rollup/rollup-darwin-arm64", "4.0.0")],
        )?);
        graph.insert(resolved_package(
            "@rollup/rollup-darwin-arm64",
            "4.0.0",
            Vec::new(),
        )?);

        let linker = Linker::new(store, temp.path().join("node_modules"));
        let direct_deps = HashSet::from(["rollup".to_string()]);
        linker.link_graph(&graph, &direct_deps, None, &graph.graph_hash())?;

        let native_link = temp
            .path()
            .join("node_modules")
            .join(".orix")
            .join("rollup@4.0.0")
            .join("node_modules")
            .join("rollup")
            .join("node_modules")
            .join("@rollup")
            .join("rollup-darwin-arm64");
        let resolved = fs::canonicalize(&native_link)?;
        let expected = fs::canonicalize(
            temp.path()
                .join("node_modules")
                .join(".orix")
                .join("@rollup")
                .join("rollup-darwin-arm64@4.0.0")
                .join("node_modules")
                .join("@rollup")
                .join("rollup-darwin-arm64"),
        )?;

        assert_eq!(resolved, expected);
        Ok(())
    }

    #[test]
    fn link_graph_links_peer_dependencies_when_present_in_graph() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let store = Store::open(temp.path().join("store"))?;
        import_package(&store, temp.path(), "rollup-plugin-esbuild", "6.2.1")?;
        import_package(&store, temp.path(), "esbuild", "0.27.0")?;

        let mut graph = DependencyGraph::new();
        graph.insert(resolved_package_with_optional_and_peers(
            "rollup-plugin-esbuild",
            "6.2.1",
            Vec::new(),
            Vec::new(),
            vec![("esbuild", ">=0.18.0")],
        )?);
        graph.insert(resolved_package("esbuild", "0.27.0", Vec::new())?);

        let linker = Linker::new(store, temp.path().join("node_modules"));
        let direct_deps = HashSet::from(["rollup-plugin-esbuild".to_string()]);
        linker.link_graph(&graph, &direct_deps, None, &graph.graph_hash())?;

        let peer_link = temp
            .path()
            .join("node_modules")
            .join(".orix")
            .join("rollup-plugin-esbuild@6.2.1")
            .join("node_modules")
            .join("rollup-plugin-esbuild")
            .join("node_modules")
            .join("esbuild");
        let resolved = fs::canonicalize(&peer_link)?;
        let expected = fs::canonicalize(
            temp.path()
                .join("node_modules")
                .join(".orix")
                .join("esbuild@0.27.0")
                .join("node_modules")
                .join("esbuild"),
        )?;

        assert_eq!(resolved, expected);
        Ok(())
    }

    #[test]
    fn link_graph_creates_parent_dirs_for_scoped_bin_names() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let store = Store::open(temp.path().join("store"))?;
        import_package_with_manifest(
            &store,
            temp.path(),
            "@antfu/eslint-config",
            "9.0.0",
            r#"{"name":"@antfu/eslint-config","version":"9.0.0","bin":"./bin/index.mjs"}"#,
        )?;

        let mut graph = DependencyGraph::new();
        graph.insert(resolved_package(
            "@antfu/eslint-config",
            "9.0.0",
            Vec::new(),
        )?);

        let linker = Linker::new(store, temp.path().join("node_modules"));
        let direct_deps = HashSet::from(["@antfu/eslint-config".to_string()]);
        linker.link_graph(&graph, &direct_deps, None, &graph.graph_hash())?;

        // Scoped bin names are flattened to avoid @ and / in Windows filenames.
        // The shim should be eslint-config (not @antfu/eslint-config).
        let bin_dir = temp.path().join("node_modules").join(".bin");

        #[cfg(windows)]
        {
            // Windows creates .cmd and .ps1 shims with the flattened name.
            assert!(
                bin_dir.join("eslint-config.cmd").exists(),
                "flattened .cmd shim should exist"
            );
            assert!(
                bin_dir.join("eslint-config.ps1").exists(),
                "flattened .ps1 shim should exist"
            );
            // The original scoped path should NOT exist as a file.
            assert!(
                !bin_dir.join("@antfu").join("eslint-config").exists(),
                "scoped path should not exist on Windows"
            );
        }

        #[cfg(not(windows))]
        {
            // Unix also uses flattened name for consistency across platforms.
            // @antfu/eslint-config -> eslint-config
            assert!(
                bin_dir.join("eslint-config").exists(),
                "flattened bin symlink should exist on Unix"
            );
        }

        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn link_graph_makes_package_bins_executable() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let store = Store::open(temp.path().join("store"))?;
        import_package_with_manifest(
            &store,
            temp.path(),
            "rollup",
            "4.0.0",
            r#"{"name":"rollup","version":"4.0.0","bin":{"rollup":"./bin/index.mjs"}}"#,
        )?;

        let mut graph = DependencyGraph::new();
        graph.insert(resolved_package("rollup", "4.0.0", Vec::new())?);

        let linker = Linker::new(store, temp.path().join("node_modules"));
        let direct_deps = HashSet::from(["rollup".to_string()]);
        linker.link_graph(&graph, &direct_deps, None, &graph.graph_hash())?;

        let shim = temp.path().join("node_modules").join(".bin").join("rollup");
        let target_metadata = fs::metadata(&shim)?;

        assert!(
            target_metadata.mode() & 0o111 != 0,
            "bin shim target should be executable"
        );
        assert!(linker.is_layout_valid(&graph.graph_hash()));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn link_graph_keeps_bins_inside_package_for_relative_requires() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let store = Store::open(temp.path().join("store"))?;
        import_package_with_rollup_style_bin(&store, temp.path())?;

        let mut graph = DependencyGraph::new();
        graph.insert(resolved_package("rollup", "4.0.0", Vec::new())?);

        let linker = Linker::new(store, temp.path().join("node_modules"));
        let direct_deps = HashSet::from(["rollup".to_string()]);
        linker.link_graph(&graph, &direct_deps, None, &graph.graph_hash())?;

        let shim = temp.path().join("node_modules").join(".bin").join("rollup");
        let shim_target = fs::read_link(&shim)?;
        let shim_parent = shim
            .parent()
            .context("rollup shim should have a parent directory")?;
        let resolved = fs::canonicalize(shim_parent.join(shim_target))?;
        let expected = temp
            .path()
            .join("node_modules")
            .join(".orix")
            .join("rollup@4.0.0")
            .join("node_modules")
            .join("rollup")
            .join("bin")
            .join("rollup");

        assert_eq!(
            normal_components(&resolved),
            normal_components(&fs::canonicalize(expected)?)
        );
        let resolved_parent = resolved
            .parent()
            .context("resolved rollup bin should have a parent directory")?;
        assert!(resolved_parent.join("../shared/rollup.js").exists());
        assert!(!temp
            .path()
            .join("node_modules")
            .join(".orix")
            .join("rollup@4.0.0")
            .join("bin")
            .join("rollup")
            .exists());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn link_graph_prefers_direct_version_for_top_level_bins() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let store = Store::open(temp.path().join("store"))?;
        import_package_with_manifest(
            &store,
            temp.path(),
            "rollup",
            "1.32.1",
            r#"{"name":"rollup","version":"1.32.1","bin":{"rollup":"./bin/index.mjs"}}"#,
        )?;
        import_package_with_manifest(
            &store,
            temp.path(),
            "rollup",
            "4.60.4",
            r#"{"name":"rollup","version":"4.60.4","bin":{"rollup":"./bin/index.mjs"}}"#,
        )?;

        let mut graph = DependencyGraph::new();
        graph.insert(resolved_package("rollup", "1.32.1", Vec::new())?);
        graph.insert(resolved_package("rollup", "4.60.4", Vec::new())?);

        let linker = Linker::new(store, temp.path().join("node_modules"));
        let direct_deps = HashSet::from(["rollup".to_string()]);
        linker.link_graph(&graph, &direct_deps, None, &graph.graph_hash())?;

        let direct_link = temp.path().join("node_modules").join("rollup");
        let direct_expected = temp
            .path()
            .join("node_modules")
            .join(".orix")
            .join("rollup@4.60.4")
            .join("node_modules")
            .join("rollup");
        assert_eq!(
            normal_components(&fs::canonicalize(direct_link)?),
            normal_components(&fs::canonicalize(direct_expected)?)
        );

        let shim = temp.path().join("node_modules").join(".bin").join("rollup");
        let shim_target = fs::read_link(&shim)?;
        let shim_parent = shim
            .parent()
            .context("rollup shim should have a parent directory")?;
        let resolved = fs::canonicalize(shim_parent.join(shim_target))?;
        assert!(
            normal_components(&resolved)
                .iter()
                .any(|part| part == "rollup@4.60.4"),
            "rollup shim should point to direct rollup version"
        );
        Ok(())
    }

    #[test]
    fn link_graph_selects_internal_dependency_by_declared_range() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let store = Store::open(temp.path().join("store"))?;
        import_package(&store, temp.path(), "rollup-pluginutils", "2.8.2")?;
        import_package(&store, temp.path(), "estree-walker", "0.6.1")?;
        import_package(&store, temp.path(), "estree-walker", "3.0.3")?;

        let mut graph = DependencyGraph::new();
        graph.insert(resolved_package(
            "rollup-pluginutils",
            "2.8.2",
            vec![("estree-walker", "^0.6.1")],
        )?);
        graph.insert(resolved_package("estree-walker", "0.6.1", Vec::new())?);
        graph.insert(resolved_package("estree-walker", "3.0.3", Vec::new())?);

        let linker = Linker::new(store, temp.path().join("node_modules"));
        let direct_deps = HashSet::from(["rollup-pluginutils".to_string()]);
        linker.link_graph(&graph, &direct_deps, None, &graph.graph_hash())?;

        let dep_link = temp
            .path()
            .join("node_modules")
            .join(".orix")
            .join("rollup-pluginutils@2.8.2")
            .join("node_modules")
            .join("rollup-pluginutils")
            .join("node_modules")
            .join("estree-walker");
        let expected = temp
            .path()
            .join("node_modules")
            .join(".orix")
            .join("estree-walker@0.6.1")
            .join("node_modules")
            .join("estree-walker");

        assert_eq!(
            normal_components(&fs::canonicalize(dep_link)?),
            normal_components(&fs::canonicalize(expected)?)
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn layout_is_invalid_when_unix_bin_target_is_not_executable() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let store = Store::open(temp.path().join("store"))?;
        let linker = Linker::new(store, temp.path().join("node_modules"));
        let graph_hash = "same-graph";

        let bin_dir = temp.path().join("node_modules").join(".bin");
        let target_dir = temp
            .path()
            .join("node_modules")
            .join(".orix")
            .join("rollup@4.0.0")
            .join("bin");
        let target = target_dir.join("rollup");
        fs::create_dir_all(&bin_dir)?;
        fs::create_dir_all(&target_dir)?;
        fs::write(&target, "#!/usr/bin/env node\n")?;
        fs::set_permissions(&target, PermissionsExt::from_mode(0o644))?;
        std::os::unix::fs::symlink("../.orix/rollup@4.0.0/bin/rollup", bin_dir.join("rollup"))?;
        linker.write_marker(graph_hash, 1)?;

        assert!(!linker.is_layout_valid(graph_hash));

        fs::set_permissions(&target, PermissionsExt::from_mode(0o755))?;

        assert!(linker.is_layout_valid(graph_hash));
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn link_graph_creates_windows_cmd_shim_for_bins() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let store = Store::open(temp.path().join("store"))?;
        import_package_with_manifest(
            &store,
            temp.path(),
            "rollup",
            "4.0.0",
            r#"{"name":"rollup","version":"4.0.0","bin":{"rollup":"./bin/index.mjs"}}"#,
        )?;

        let mut graph = DependencyGraph::new();
        graph.insert(resolved_package("rollup", "4.0.0", Vec::new())?);

        let linker = Linker::new(store, temp.path().join("node_modules"));
        let direct_deps = HashSet::from(["rollup".to_string()]);
        linker.link_graph(&graph, &direct_deps, None, &graph.graph_hash())?;

        let shim = temp
            .path()
            .join("node_modules")
            .join(".bin")
            .join("rollup.cmd");
        let content = fs::read_to_string(&shim)?;

        assert!(shim.exists());
        // Shim uses %~dp0 to find the .bin directory at runtime.
        // Target is a relative path like ..\.orix\rollup@4.0.0\bin\rollup
        assert!(content.contains("basedir=%~dp0"));
        assert!(content.contains("node"));
        assert!(content.contains("index.mjs"));
        assert!(content.contains("%*"));
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn layout_is_invalid_when_windows_bin_shim_is_missing() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let store = Store::open(temp.path().join("store"))?;
        let linker = Linker::new(store, temp.path().join("node_modules"));
        let graph_hash = "same-graph";

        fs::create_dir_all(temp.path().join("node_modules").join(".bin"))?;
        fs::write(
            temp.path().join("node_modules").join(".bin").join("rollup"),
            "#!/usr/bin/env node\n",
        )?;
        linker.write_marker(graph_hash, 1)?;

        assert!(!linker.is_layout_valid(graph_hash));

        fs::write(
            temp.path()
                .join("node_modules")
                .join(".bin")
                .join("rollup.cmd"),
            "@ECHO off\r\n",
        )?;

        assert!(linker.is_layout_valid(graph_hash));
        Ok(())
    }

    #[test]
    fn unlink_removes_node_modules_directory() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let store = Store::open(temp.path().join("store"))?;
        let nm_dir = temp.path().join("node_modules");
        fs::create_dir_all(&nm_dir)?;
        fs::write(nm_dir.join("dummy.txt"), b"placeholder")?;

        let linker = Linker::new(store, nm_dir.clone());
        linker.unlink()?;

        assert!(!nm_dir.exists());
        Ok(())
    }

    #[test]
    fn unlink_does_not_error_when_node_modules_missing() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let store = Store::open(temp.path().join("store"))?;
        let nm_dir = temp.path().join("nonexistent_node_modules");

        let linker = Linker::new(store, nm_dir);
        linker.unlink()?; // Should succeed without error

        Ok(())
    }

    #[test]
    fn link_local_package_creates_symlink_to_source_directory() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let store = Store::open(temp.path().join("store"))?;
        let nm_dir = temp.path().join("node_modules");
        let source_dir = temp.path().join("packages").join("local-pkg");
        fs::create_dir_all(&source_dir)?;
        fs::write(
            source_dir.join("package.json"),
            r#"{"name":"local-pkg","version":"1.0.0"}"#,
        )?;

        let linker = Linker::new(store, nm_dir.clone());
        let created = linker.link_local_package("local-pkg", &source_dir)?;

        assert_eq!(created, 1);
        assert!(nm_dir.join("local-pkg").exists());
        Ok(())
    }

    #[test]
    fn link_local_package_skips_existing_symlink() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let store = Store::open(temp.path().join("store"))?;
        let nm_dir = temp.path().join("node_modules");
        fs::create_dir_all(&nm_dir)?;
        let source_dir = temp.path().join("packages").join("local-pkg");
        fs::create_dir_all(&source_dir)?;

        let linker = Linker::new(store, nm_dir.clone());
        linker.link_local_package("local-pkg", &source_dir)?;
        let created = linker.link_local_package("local-pkg", &source_dir)?;

        assert_eq!(created, 0); // Second call should not create again
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn windows_absolutizes_relative_junction_target() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let target = temp
            .path()
            .join("node_modules")
            .join(".orix")
            .join("dep@1.0.0")
            .join("node_modules")
            .join("dep");
        let link = temp
            .path()
            .join("node_modules")
            .join(".orix")
            .join("parent@1.0.0")
            .join("node_modules")
            .join("parent")
            .join("node_modules")
            .join("dep");
        fs::create_dir_all(&target)?;
        let link_parent = link
            .parent()
            .ok_or_else(|| anyhow::anyhow!("test link should have a parent"))?;
        fs::create_dir_all(link_parent)?;

        let relative = relative_path(link_parent, &target);
        let absolute = Linker::absolutize_link_target(&relative, &link)?;

        assert!(absolute.is_absolute());
        assert_eq!(absolute, target.canonicalize()?);
        Ok(())
    }
}
