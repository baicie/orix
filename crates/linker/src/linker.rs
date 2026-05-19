//! Linker implementation.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use anyhow::Result;
use tracing::{debug, warn};
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
            Some(marker) => marker.graph_hash == graph_hash,
            None => false,
        }
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
                            Self::create_symlink(&local_pkg.abs_path, &top_link)?;
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

            fs::create_dir_all(&pkg_dir)?;

            let store_files = self.store.package_files_path(&pkg.id);
            debug!(pkg = %pkg_key, store = %store_files.display(), "linking package files");
            if store_files.exists() {
                let mut hardlink_ok = 0;
                let mut copy_ok = 0;
                let mut hardlink_fail = 0;
                let mut copy_fail = 0;
                for entry in WalkDir::new(&store_files)
                    .into_iter()
                    .filter_map(|e| e.ok())
                {
                    if !entry.file_type().is_file() {
                        continue;
                    }
                    let rel_path = entry.path().strip_prefix(&store_files)?;
                    let dest = pkg_dir.join(rel_path);

                    if let Some(parent) = dest.parent() {
                        if let Err(e) = fs::create_dir_all(parent) {
                            warn!(pkg = %pkg_key, "failed to create dir {}: {}", parent.display(), e);
                            continue;
                        }
                    }

                    #[allow(clippy::incompatible_msrv)]
                    match fs::hard_link(entry.path(), &dest) {
                        Ok(_) => {
                            hardlink_ok += 1;
                        }
                        Err(e)
                            if e.kind() == io::ErrorKind::PermissionDenied
                                || e.kind() == io::ErrorKind::NotFound
                                || e.kind() == io::ErrorKind::CrossesDevices =>
                        {
                            match fs::copy(entry.path(), &dest) {
                                Ok(_) => copy_ok += 1,
                                Err(e2) => {
                                    copy_fail += 1;
                                    warn!(pkg = %pkg_key, "hard_link failed and copy also failed {} -> {}: {}", entry.path().display(), dest.display(), e2);
                                }
                            }
                        }
                        Err(e) => {
                            hardlink_fail += 1;
                            warn!(pkg = %pkg_key, "hard_link failed {} -> {}: {}", entry.path().display(), dest.display(), e);
                        }
                    }
                }
                report.hardlinked_files += hardlink_ok;
                report.copied_files += copy_ok;
                debug!(pkg = %pkg_key, hardlink_ok, copy_ok, hardlink_fail, copy_fail, "link summary");
                if hardlink_ok == 0 && copy_ok == 0 && hardlink_fail == 0 && copy_fail == 0 {
                    warn!(pkg = %pkg_key, "no files found in store or no files were linked");
                }
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
                            Self::create_symlink(&symlink_target, &symlink_path)?;
                        }
                        report.symlinks_created += 1;
                    }
                }
            }

            // Link bin executables for this package into .orix/<pkg>/bin/
            self.link_package_bins(&pkg_dir, &pkg_key, &store_files, &mut report)?;
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
                Self::create_symlink(&target, &link)?;
                report.symlinks_created += 1;
            }
        }

        // Write marker after successful link
        self.write_marker(graph_hash, graph.len())?;

        Ok(report)
    }

    /// Link bin executables from a package into the .orix/<pkg>/bin directory.
    /// Also creates the global .bin/ directory with symlinks to each bin.
    fn link_package_bins(
        &self,
        pkg_dir: &Path,
        pkg_key: &str,
        store_files: &Path,
        report: &mut LinkReport,
    ) -> Result<()> {
        // Read the package.json from the store to get bin field
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

        let bin_entries = match bin_value {
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

        let package_bin_dir = self
            .node_modules
            .join(VIRTUAL_STORE_DIR)
            .join(pkg_key)
            .join("bin");
        let global_bin_dir = self.node_modules.join(".bin");

        fs::create_dir_all(&package_bin_dir)?;
        fs::create_dir_all(&global_bin_dir)?;

        for (cmd_name, bin_path) in bin_entries {
            if cmd_name.is_empty() {
                continue;
            }

            // Source: the bin file inside the package directory
            let bin_source = pkg_dir.join(&bin_path);
            // Dest in .orix/<pkg>/bin/<cmd>
            let package_bin_dest = package_bin_dir.join(&cmd_name);

            if bin_source.exists() && !package_bin_dest.exists() {
                if let Some(parent) = package_bin_dest.parent() {
                    fs::create_dir_all(parent)?;
                }
                #[allow(clippy::incompatible_msrv)]
                if let Err(e) = fs::hard_link(&bin_source, &package_bin_dest) {
                    if e.kind() == io::ErrorKind::PermissionDenied
                        || e.kind() == io::ErrorKind::NotFound
                        || e.kind() == io::ErrorKind::CrossesDevices
                    {
                        let _ = fs::copy(&bin_source, &package_bin_dest);
                    }
                }
            }

            // Global bin link: node_modules/.bin/<cmd> -> ../.orix/<pkg>/bin/<cmd>
            // Only create if the source actually exists (bin_source exists means the file
            // was either hardlinked or copied into the package directory).
            if bin_source.exists() {
                let global_bin_link = global_bin_dir.join(&cmd_name);
                if !global_bin_link.exists() {
                    let relative_target = PathBuf::from("..")
                        .join(VIRTUAL_STORE_DIR)
                        .join(pkg_key)
                        .join("bin")
                        .join(&cmd_name);
                    Self::create_symlink(&relative_target, &global_bin_link)?;
                    report.symlinks_created += 1;
                }
            }
        }

        Ok(())
    }

    /// Create a symlink, falling back to junction on Windows when needed.
    fn create_symlink(target: &Path, link: &Path) -> io::Result<()> {
        #[cfg(windows)]
        {
            // Try symlink_dir first; if it fails due to permissions, try junction
            match std::os::windows::fs::symlink_dir(target, link) {
                Ok(_) => Ok(()),
                Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
                    // Try junction as fallback (doesn't require admin on modern Windows)
                    Self::create_junction(target, link)
                }
                Err(e) => Err(e),
            }
        }
        #[cfg(not(windows))]
        {
            std::os::unix::fs::symlink(target, link)
        }
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
            Ok(o) => Err(io::Error::other(String::from_utf8_lossy(&o.stderr))),
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

        Self::create_symlink(local_source, &link_path)?;
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
}
