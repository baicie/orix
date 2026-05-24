//! Workspace discovery from yaml and package.json.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

use orix_manifest::Manifest;

use super::types::{Catalog, CatalogSpec, Workspace, WorkspacePackage};
use crate::WorkspaceSpec;

pub(crate) type WorkspaceDiscoveryResult =
    Result<Option<(Vec<WorkspacePackage>, Catalog, HashMap<String, Catalog>)>>;

#[derive(Debug, Deserialize)]
pub(crate) struct WorkspaceFile {
    packages: Vec<String>,
    #[serde(default)]
    catalog: Option<Catalog>,
    #[serde(default)]
    catalogs: HashMap<String, Catalog>,
}

impl Workspace {
    /// Discover a workspace starting from the given root directory.
    ///
    /// Supports three configuration formats (checked in order):
    /// 1. `pnpm-workspace.yaml` — pnpm-compatible YAML with `packages: [...]`
    /// 2. `orix-workspace.yaml` — orix-specific YAML with `packages: [...]`
    /// 3. `orix.packages` in root `package.json` — JSON array of glob strings
    #[allow(clippy::manual_unwrap_or_default)]
    pub fn discover(root: PathBuf) -> Result<Self> {
        let (packages, catalog, catalogs) =
            if let Some((pkgs, cat, cats)) = Self::discover_from_pnpm_yaml(&root)? {
                (pkgs, cat, cats)
            } else if let Some((pkgs, cat, cats)) = Self::discover_from_orix_yaml(&root)? {
                (pkgs, cat, cats)
            } else if let Some(pkgs) = Self::discover_from_root_package_json(&root)? {
                (pkgs, Catalog::new(), HashMap::new())
            } else {
                (Vec::new(), Catalog::new(), HashMap::new())
            };

        Ok(Self {
            root: root.clone(),
            packages,
            lockfile_path: root.join("orix-lock.yaml"),
            catalog,
            catalogs,
        })
    }

    /// Resolve a `catalog:` or `catalog:name` specifier to its version constraint.
    ///
    /// Returns the version constraint string for the given package name,
    /// or `None` if the catalog entry is not found.
    pub fn resolve_catalog(&self, spec: &str, package_name: &str) -> Option<String> {
        let catalog_spec = CatalogSpec::parse(spec)?;

        let cat = if let Some(ref name) = catalog_spec.catalog_name {
            self.catalogs.get(name)?
        } else {
            &self.catalog
        };

        cat.get(package_name).cloned()
    }

    /// Try discovering workspace from `pnpm-workspace.yaml`.
    fn discover_from_pnpm_yaml(root: &Path) -> WorkspaceDiscoveryResult {
        let path = root.join("pnpm-workspace.yaml");
        if !path.exists() {
            return Ok(None);
        }
        let source =
            std::fs::read_to_string(&path).with_context(|| "failed to read pnpm-workspace.yaml")?;
        let workspace_file: WorkspaceFile =
            serde_yaml::from_str(&source).with_context(|| "failed to parse pnpm-workspace.yaml")?;
        let packages = Self::find_packages(root, &workspace_file.packages)?;
        let catalog = workspace_file.catalog.unwrap_or_default();
        let catalogs = workspace_file.catalogs;
        Ok(Some((packages, catalog, catalogs)))
    }

    /// Try discovering workspace from `orix-workspace.yaml`.
    fn discover_from_orix_yaml(root: &Path) -> WorkspaceDiscoveryResult {
        let path = root.join("orix-workspace.yaml");
        if !path.exists() {
            return Ok(None);
        }
        let source =
            std::fs::read_to_string(&path).with_context(|| "failed to read orix-workspace.yaml")?;
        let workspace_file: WorkspaceFile =
            serde_yaml::from_str(&source).with_context(|| "failed to parse orix-workspace.yaml")?;
        let packages = Self::find_packages(root, &workspace_file.packages)?;
        let catalog = workspace_file.catalog.unwrap_or_default();
        let catalogs = workspace_file.catalogs;
        Ok(Some((packages, catalog, catalogs)))
    }

    /// Try discovering workspace from `orix.packages` field in root `package.json`.
    fn discover_from_root_package_json(root: &Path) -> Result<Option<Vec<WorkspacePackage>>> {
        let path = root.join("package.json");
        if !path.exists() {
            return Ok(None);
        }
        let source =
            std::fs::read_to_string(&path).with_context(|| "failed to read package.json")?;
        let json: serde_json::Value =
            serde_json::from_str(&source).with_context(|| "failed to parse package.json")?;

        let packages_array = match json.get("orix").and_then(|v| v.get("packages")) {
            Some(serde_json::Value::Array(arr)) => arr,
            _ => return Ok(None),
        };

        let patterns: Vec<String> = packages_array
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect();

        if patterns.is_empty() {
            return Ok(None);
        }

        let packages = Self::find_packages(root, &patterns)?;
        Ok(Some(packages))
    }

    fn find_packages(root: &Path, patterns: &[String]) -> Result<Vec<WorkspacePackage>> {
        let mut packages = Vec::new();
        let mut seen = HashSet::new();
        let include_patterns: Vec<glob::Pattern> = patterns
            .iter()
            .filter(|pattern| !pattern.starts_with('!'))
            .map(|pattern| glob::Pattern::new(pattern))
            .collect::<Result<_, _>>()?;
        let exclude_patterns: Vec<glob::Pattern> = patterns
            .iter()
            .filter_map(|pattern| pattern.strip_prefix('!'))
            .map(glob::Pattern::new)
            .collect::<Result<_, _>>()?;

        for entry in walkdir::WalkDir::new(root)
            .follow_links(false)
            .into_iter()
            .filter_entry(|entry| should_descend_workspace_entry(entry.path()))
        {
            let entry = entry?;
            if !entry.file_type().is_dir() {
                continue;
            }

            let pkg_path = entry.path();
            if pkg_path == root || path_contains_node_modules(pkg_path) {
                continue;
            }

            let relative_path = pkg_path
                .strip_prefix(root)
                .with_context(|| format!("path {} not under root", pkg_path.display()))?
                .to_path_buf();
            let relative_pattern = relative_path.to_string_lossy().replace('\\', "/");

            if !include_patterns
                .iter()
                .any(|pattern| pattern.matches(&relative_pattern))
                || exclude_patterns
                    .iter()
                    .any(|pattern| pattern.matches(&relative_pattern))
            {
                continue;
            }

            let manifest_path = pkg_path.join("package.json");

            if !manifest_path.exists() {
                continue;
            }

            // Skip packages that fail to parse - be tolerant of malformed package.json
            let manifest = match Manifest::read(&manifest_path) {
                Ok(m) => m,
                Err(e) => {
                    tracing::warn!(
                        "skipping package at {}: failed to parse package.json: {}",
                        manifest_path.display(),
                        e
                    );
                    continue;
                }
            };
            let name = manifest.name.clone().unwrap_or_default();

            let key = (name.clone(), pkg_path.to_path_buf());
            if !seen.insert(key) {
                anyhow::bail!(
                    "package '{}' at '{}' matches multiple workspace globs",
                    name,
                    pkg_path.display()
                );
            }

            packages.push(WorkspacePackage {
                relative_path,
                abs_path: pkg_path.to_path_buf(),
                manifest,
            });
        }

        packages.sort_by_key(|p| p.relative_path.clone());
        Ok(packages)
    }

    /// Resolve a workspace protocol dependency to a local PackageId.
    ///
    /// For `workspace:*` (name=None, path=empty), the dependency `name` is used to find the package.
    pub fn resolve_workspace_dep(
        &self,
        spec: &WorkspaceSpec,
        dep_name: &str,
    ) -> Option<WorkspacePackage> {
        let name_to_find = spec.name.as_deref().unwrap_or(dep_name);
        self.packages
            .iter()
            .find(|p| p.manifest.name.as_deref() == Some(name_to_find))
            .cloned()
    }

    /// Find a workspace package by its package name.
    ///
    /// Returns `None` if no package with that name exists in the workspace.
    pub fn find_package_by_name(&self, name: &str) -> Option<&WorkspacePackage> {
        self.packages
            .iter()
            .find(|p| p.manifest.name.as_deref() == Some(name))
    }
}

fn path_contains_node_modules(path: &Path) -> bool {
    path.components()
        .any(|component| component.as_os_str() == "node_modules")
}

fn should_descend_workspace_entry(path: &Path) -> bool {
    !path.components().any(|component| {
        let name = component.as_os_str();
        name == "node_modules" || name == ".git" || name == "target"
    })
}
