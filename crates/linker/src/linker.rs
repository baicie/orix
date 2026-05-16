//! Linker implementation.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use anyhow::Result;
use walkdir::WalkDir;

use orix_domain::DependencyGraph;
use orix_store::Store;

use super::LinkReport;

/// The linker creates the pnpm-style node_modules structure using hardlinks and symlinks.
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

    /// Build the full node_modules layout from a dependency graph.
    pub fn link_graph(
        &self,
        graph: &DependencyGraph,
        direct_deps: &std::collections::HashSet<String>,
    ) -> Result<LinkReport> {
        let mut report = LinkReport {
            hardlinked_files: 0,
            copied_files: 0,
            symlinks_created: 0,
            bytes_saved: 0,
        };

        let pnpm_dir = self.node_modules.join(".pnpm");
        fs::create_dir_all(&pnpm_dir)?;

        // Build a lookup from package name -> pkg_id for quick dep resolution
        let name_to_key: HashMap<String, String> = graph
            .packages()
            .map(|p| (p.id.name.to_string(), p.id.key()))
            .collect();

        for pkg in graph.packages() {
            let pkg_key = pkg.id.key();
            let pkg_dir = pnpm_dir
                .join(&pkg_key)
                .join("node_modules")
                .join(pkg.id.name.as_str());

            fs::create_dir_all(&pkg_dir)?;

            let store_files = self.store.package_files_path(&pkg.id);
            if store_files.exists() {
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
                        fs::create_dir_all(parent)?;
                    }

                    match fs::hard_link(entry.path(), &dest) {
                        Ok(_) => {
                            report.hardlinked_files += 1;
                        }
                        Err(e)
                            if e.kind() == io::ErrorKind::PermissionDenied
                                || e.kind() == io::ErrorKind::NotFound =>
                        {
                            if fs::copy(entry.path(), &dest).is_ok() {
                                report.copied_files += 1;
                            }
                        }
                        Err(_) => {}
                    }
                }
            }

            // Create symlinks for this package's declared dependencies
            for (dep_name, _) in pkg
                .dependencies
                .iter()
                .chain(pkg.optional_dependencies.iter())
            {
                if let Some(dep_key) = name_to_key.get(dep_name.as_str()) {
                    let symlink_target = PathBuf::from("..")
                        .join("..")
                        .join(dep_key)
                        .join("node_modules")
                        .join(dep_name.as_str());
                    let symlink_path = pkg_dir.join("node_modules").join(dep_name.as_str());

                    if !symlink_path.exists() {
                        if let Some(parent) = symlink_path.parent() {
                            fs::create_dir_all(parent)?;
                        }
                        Self::create_symlink(&symlink_target, &symlink_path)?;
                        report.symlinks_created += 1;
                    }
                }
            }

            // Link bin executables for this package into .pnpm/<pkg>/bin/
            self.link_package_bins(&pkg_dir, &pkg_key, &store_files, &mut report)?;
        }

        // Create top-level symlinks for direct dependencies
        for pkg in graph.packages() {
            if !direct_deps.contains(pkg.id.name.as_str()) {
                continue;
            }

            let target = pnpm_dir
                .join(pkg.id.key())
                .join("node_modules")
                .join(pkg.id.name.as_str());
            let link = self.node_modules.join(pkg.id.name.as_str());

            if !link.exists() {
                Self::create_symlink(&target, &link)?;
                report.symlinks_created += 1;
            }
        }

        Ok(report)
    }

    /// Link bin executables from a package into the .pnpm/<pkg>/bin directory.
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

        let pnpm_bin_dir = self.node_modules.join(".pnpm").join(pkg_key).join("bin");
        let global_bin_dir = self.node_modules.join(".bin");

        fs::create_dir_all(&pnpm_bin_dir)?;
        fs::create_dir_all(&global_bin_dir)?;

        for (cmd_name, bin_path) in bin_entries {
            if cmd_name.is_empty() {
                continue;
            }

            // Source: the bin file inside the package directory
            let bin_source = pkg_dir.join(&bin_path);
            // Dest in .pnpm/<pkg>/bin/<cmd>
            let pnpm_bin_dest = pnpm_bin_dir.join(&cmd_name);

            if bin_source.exists() && !pnpm_bin_dest.exists() {
                if let Some(parent) = pnpm_bin_dest.parent() {
                    fs::create_dir_all(parent)?;
                }
                if let Err(e) = fs::hard_link(&bin_source, &pnpm_bin_dest) {
                    if e.kind() == io::ErrorKind::PermissionDenied
                        || e.kind() == io::ErrorKind::NotFound
                    {
                        let _ = fs::copy(&bin_source, &pnpm_bin_dest);
                    }
                }
            }

            // Global bin link: node_modules/.bin/<cmd> -> ../.pnpm/<pkg>/bin/<cmd>
            let global_bin_link = global_bin_dir.join(&cmd_name);
            if !global_bin_link.exists() {
                let relative_target = PathBuf::from("..")
                    .join(".pnpm")
                    .join(pkg_key)
                    .join("bin")
                    .join(&cmd_name);
                Self::create_symlink(&relative_target, &global_bin_link)?;
                report.symlinks_created += 1;
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

    /// Remove all generated links and .pnpm/ content for this project.
    pub fn unlink(&self) -> Result<()> {
        if self.node_modules.exists() {
            fs::remove_dir_all(&self.node_modules)?;
        }
        Ok(())
    }
}
