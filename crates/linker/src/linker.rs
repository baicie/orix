//! Linker implementation.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tracing::{debug, trace, warn};
use walkdir::WalkDir;

use orix_domain::DependencyGraph;
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
        true
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

            // Create symlinks for this package's declared dependencies
            for (dep_name, _) in pkg
                .dependencies
                .iter()
                .chain(pkg.optional_dependencies.iter())
            {
                if let Some(dep_key) = name_to_key.get(dep_name.as_str()) {
                    let symlink_path = Self::package_path_in_node_modules(
                        &pkg_dir.join("node_modules"),
                        dep_name.as_str(),
                    );
                    let target = Self::package_path_in_node_modules(
                        &virtual_store_dir.join(dep_key).join("node_modules"),
                        dep_name.as_str(),
                    );

                    if !symlink_path.exists() {
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

            // Import package files from the store using integrity metadata.
            // Uses integrity.files to avoid WalkDir, pre-creates all directories,
            // falls back to copy on EXDEV, and writes package.json last.
            self.import_package_files(&pkg.id, &pkg_dir, &store_files, &mut report)?;

            // Link bin executables for this package into .orix/<pkg>/bin/
            self.link_package_bins(&pkg_key, &store_files, &mut report)?;
        }

        // Create top-level symlinks for direct dependencies
        for pkg in graph.packages() {
            if !direct_deps.contains(pkg.id.name.as_str()) {
                continue;
            }

            let target = virtual_store_dir.join(pkg.id.key()).join("node_modules");
            let target = Self::package_path_in_node_modules(&target, pkg.id.name.as_str());
            let link = Self::package_path_in_node_modules(&self.node_modules, pkg.id.name.as_str());

            if !link.exists() {
                if let Some(parent) = link.parent() {
                    fs::create_dir_all(parent)?;
                }
                Self::create_dir_link(&target, &link).with_context(|| {
                    format!(
                        "failed to link direct dependency {}: {} -> {}",
                        pkg.id.name,
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

        let mut hardlink_ok = 0;
        let mut copy_ok = 0;
        let mut hardlink_fail = 0;
        let mut copy_fail = 0;

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
                match fs::copy(&src, &dest) {
                    Ok(_) => copy_ok += 1,
                    Err(e) => {
                        copy_fail += 1;
                        warn!(pkg = %pkg_key, error = %e, "failed to copy {} -> {}", src.display(), dest.display());
                    }
                }
            } else {
                #[allow(clippy::incompatible_msrv)]
                match fs::hard_link(&src, &dest) {
                    Ok(_) => hardlink_ok += 1,
                    Err(e) if e.kind() == io::ErrorKind::CrossesDevices => {
                        // Cross-device: fall back to copy and remember the decision.
                        use_copy = true;
                        match fs::copy(&src, &dest) {
                            Ok(_) => copy_ok += 1,
                            Err(e2) => {
                                copy_fail += 1;
                                warn!(pkg = %pkg_key, error = %e2, "copy failed after EXDEV {} -> {}", src.display(), dest.display());
                            }
                        }
                    }
                    Err(e)
                        if e.kind() == io::ErrorKind::PermissionDenied
                            || e.kind() == io::ErrorKind::NotFound =>
                    {
                        match fs::copy(&src, &dest) {
                            Ok(_) => copy_ok += 1,
                            Err(e2) => {
                                copy_fail += 1;
                                warn!(pkg = %pkg_key, error = %e2, "hard_link failed and copy also failed {} -> {}", src.display(), dest.display());
                            }
                        }
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

        // Flat package dir: .orix/pkg@ver/ (parent of the package-name subdirectory).
        let package_store_dir = self.node_modules.join(VIRTUAL_STORE_DIR).join(pkg_key);
        let flat_pkg_dir = package_store_dir.parent().unwrap_or(&package_store_dir);
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

            // Destination in flat package dir: .orix/pkg@ver/bin/<flat_name>
            let flat_bin_dest = flat_pkg_dir.join("bin").join(flat_bin_name);
            let flat_bin_dest_parent = flat_bin_dest.parent().unwrap_or(&flat_bin_dest);

            fs::create_dir_all(flat_bin_dest_parent)?;

            // Only hard-link/copy the bin file if the destination doesn't exist yet.
            if !flat_bin_dest.exists() {
                #[allow(clippy::incompatible_msrv)]
                if let Err(e) = fs::hard_link(&bin_source, &flat_bin_dest) {
                    if e.kind() == io::ErrorKind::PermissionDenied
                        || e.kind() == io::ErrorKind::NotFound
                        || e.kind() == io::ErrorKind::CrossesDevices
                    {
                        fs::copy(&bin_source, &flat_bin_dest).with_context(|| {
                            format!(
                                "failed to copy bin {} -> {}",
                                bin_source.display(),
                                flat_bin_dest.display()
                            )
                        })?;
                    } else {
                        return Err(e).with_context(|| {
                            format!(
                                "failed to hard-link bin {} -> {}",
                                bin_source.display(),
                                flat_bin_dest.display()
                            )
                        });
                    }
                }
            }

            // Global shims: only create if the bin file was successfully placed.
            if flat_bin_dest.exists() {
                #[cfg(windows)]
                {
                    // Resolve the bin to an absolute path so the shim works from any cwd.
                    let absolute_bin = bin_source.canonicalize().with_context(|| {
                        format!(
                            "failed to resolve bin source {} for shim",
                            bin_source.display()
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
                            &flat_bin_dest,
                        );
                        std::os::unix::fs::symlink(&rel, &shim_link)?;
                        report.symlinks_created += 1;
                    }
                }
            }
        }

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
        Ok(ResolvedPackage {
            id: pkg_id(name, version)?,
            integrity: String::new(),
            tarball: String::new(),
            dependencies: dependencies
                .into_iter()
                .map(|(name, version)| (PackageName::from(name), version.to_string()))
                .collect(),
            dev_dependencies: Vec::new(),
            optional_dependencies: Vec::new(),
            peer_dependencies: Vec::new(),
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
            // Unix can keep the scoped name.
            assert!(
                bin_dir.join("@antfu").join("eslint-config").exists(),
                "scoped bin symlink should exist on Unix"
            );
        }

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
