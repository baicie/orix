//! Workspace discovery and management.

#![deny(clippy::unwrap_used)]

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::WorkspaceSpec;
use orix_manifest::Manifest;
use std::collections::HashMap;

type WorkspaceDiscoveryResult =
    Result<Option<(Vec<WorkspacePackage>, Catalog, HashMap<String, Catalog>)>>;

/// A catalog reference parsed from package.json.
/// Examples: "catalog:", "catalog:react19"
#[derive(Debug, Clone)]
pub struct CatalogSpec {
    /// The catalog name (None = default catalog).
    pub catalog_name: Option<String>,
}

impl CatalogSpec {
    /// Parse a catalog: protocol specifier.
    ///
    /// Supports:
    /// - `catalog:` — references the default catalog
    /// - `catalog:name` — references a named catalog (e.g., `catalog:react19`)
    pub fn parse(spec: &str) -> Option<Self> {
        let spec = spec.trim();
        if let Some(after) = spec.strip_prefix("catalog:") {
            if after.is_empty() {
                return Some(Self { catalog_name: None });
            }
            return Some(Self {
                catalog_name: Some(after.to_string()),
            });
        }
        None
    }
}

/// A resolved catalog entry: maps package names to their version constraints.
pub type Catalog = HashMap<String, String>;

/// A discovered workspace package.
#[derive(Debug, Clone)]
pub struct WorkspacePackage {
    /// Path relative to workspace root.
    pub relative_path: PathBuf,
    /// Absolute path to the package.
    pub abs_path: PathBuf,
    /// Parsed package.json.
    pub manifest: Manifest,
}

/// The full workspace.
#[derive(Debug, Clone)]
pub struct Workspace {
    /// Workspace root directory.
    pub root: PathBuf,
    /// All discovered packages.
    pub packages: Vec<WorkspacePackage>,
    /// Lockfile path for the workspace.
    pub lockfile_path: PathBuf,
    /// The default catalog (package name -> version constraint).
    pub catalog: Catalog,
    /// Named catalogs (catalog name -> package name -> version constraint).
    pub catalogs: HashMap<String, Catalog>,
}

impl Default for Workspace {
    fn default() -> Self {
        Self {
            root: PathBuf::new(),
            packages: Vec::new(),
            lockfile_path: PathBuf::new(),
            catalog: Catalog::new(),
            catalogs: HashMap::new(),
        }
    }
}

/// A pnpm-workspace.yaml file.
#[derive(Debug, Deserialize)]
struct WorkspaceFile {
    packages: Vec<String>,
    /// The default catalog (simplified catalog entry).
    #[serde(default)]
    catalog: Option<Catalog>,
    /// Named catalogs.
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

        for pattern in patterns {
            let full_pattern = root.join(pattern);

            for entry in glob::glob(&full_pattern.display().to_string())? {
                let pkg_path = entry?;
                let manifest_path = pkg_path.join("package.json");

                if !manifest_path.exists() {
                    continue;
                }

                let manifest = Manifest::read(&manifest_path)
                    .with_context(|| format!("failed to read {}", manifest_path.display()))?;
                let name = manifest.name.clone().unwrap_or_default();

                let key = (name.clone(), pkg_path.clone());
                if !seen.insert(key) {
                    anyhow::bail!(
                        "package '{}' at '{}' matches multiple workspace globs",
                        name,
                        pkg_path.display()
                    );
                }

                let relative_path = pkg_path
                    .strip_prefix(root)
                    .with_context(|| format!("path {} not under root", pkg_path.display()))?
                    .to_path_buf();

                packages.push(WorkspacePackage {
                    relative_path,
                    abs_path: pkg_path,
                    manifest,
                });
            }
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

#[allow(dead_code)]
/// Cycle detection result: a list of packages involved in a dependency cycle.
pub type CycleReport = Vec<String>;

/// Detects circular workspace dependencies using DFS with three-color marking.
///
/// Returns an empty `Vec` if no cycles exist, or the packages involved in the
/// first cycle found.
///
/// Color state: 0 = unvisited (white), 1 = in-progress (gray), 2 = done (black).
pub fn detect_workspace_cycles(workspace: &Workspace) -> Vec<String> {
    use std::collections::HashMap;

    let mut adj: HashMap<String, Vec<String>> = HashMap::new();
    for pkg in &workspace.packages {
        if let Some(ref name) = pkg.manifest.name {
            let deps: Vec<String> = pkg
                .manifest
                .dependencies
                .keys()
                .chain(pkg.manifest.dev_dependencies.keys())
                .chain(pkg.manifest.optional_dependencies.keys())
                .filter(|k| {
                    workspace
                        .packages
                        .iter()
                        .any(|p| p.manifest.name.as_ref() == Some(k))
                })
                .cloned()
                .collect();
            adj.insert(name.clone(), deps);
        }
    }

    let mut color: HashMap<String, u8> = HashMap::new();
    let mut parent: HashMap<String, String> = HashMap::new();
    let mut cycle: Vec<String> = Vec::new();

    fn dfs(
        name: &str,
        adj: &HashMap<String, Vec<String>>,
        color: &mut HashMap<String, u8>,
        parent: &mut HashMap<String, String>,
        cycle: &mut Vec<String>,
    ) -> bool {
        color.insert(name.to_string(), 1);
        if let Some(neighbors) = adj.get(name) {
            for neighbor in neighbors {
                let n_color = *color.get(neighbor).unwrap_or(&0);
                if n_color == 1 {
                    let mut cur = name.to_string();
                    cycle.clear();
                    cycle.push(cur.clone());
                    while let Some(p) = parent.get(&cur) {
                        cycle.push(p.clone());
                        cur = p.clone();
                        if p == neighbor {
                            break;
                        }
                    }
                    cycle.reverse();
                    return true;
                }
                if n_color == 0 {
                    parent.insert(neighbor.clone(), name.to_string());
                    if dfs(neighbor, adj, color, parent, cycle) {
                        return true;
                    }
                }
            }
        }
        color.insert(name.to_string(), 2);
        false
    }

    for pkg in &workspace.packages {
        if let Some(ref name) = pkg.manifest.name {
            if *color.get(name).unwrap_or(&0) == 0
                && dfs(name, &adj, &mut color, &mut parent, &mut cycle)
            {
                return cycle;
            }
        }
    }

    Vec::new()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn ws_with_pkgs(pkg_specs: Vec<(&str, Vec<&str>)>) -> Workspace {
        let packages: Vec<WorkspacePackage> = pkg_specs
            .into_iter()
            .map(|(name, deps)| {
                let manifest = orix_manifest::Manifest {
                    name: Some(name.to_string()),
                    version: Some("1.0.0".to_string()),
                    dependencies: deps
                        .into_iter()
                        .map(|d| (d.to_string(), "*".to_string()))
                        .collect(),
                    ..Default::default()
                };
                WorkspacePackage {
                    relative_path: PathBuf::from(name),
                    abs_path: PathBuf::from(name),
                    manifest,
                }
            })
            .collect();
        Workspace {
            root: PathBuf::from("."),
            packages,
            lockfile_path: PathBuf::from("orix-lock.yaml"),
            catalog: Catalog::new(),
            catalogs: HashMap::new(),
        }
    }

    #[test]
    fn detect_no_cycle_in_linear_deps() {
        let result = detect_workspace_cycles(&ws_with_pkgs(vec![
            ("pkg-a", vec!["pkg-b"]),
            ("pkg-b", vec!["pkg-c"]),
            ("pkg-c", vec![]),
        ]));
        assert!(result.is_empty(), "no cycle expected, got {:?}", result);
    }

    #[test]
    fn detect_self_cycle() {
        let result = detect_workspace_cycles(&ws_with_pkgs(vec![("pkg-a", vec!["pkg-a"])]));
        assert!(!result.is_empty(), "self-cycle should be detected");
        assert!(result.contains(&"pkg-a".to_string()));
    }

    #[test]
    fn detect_two_node_cycle() {
        let result = detect_workspace_cycles(&ws_with_pkgs(vec![
            ("pkg-a", vec!["pkg-b"]),
            ("pkg-b", vec!["pkg-a"]),
        ]));
        assert!(!result.is_empty(), "cycle should be detected");
    }

    #[test]
    fn no_false_positive_on_external_deps() {
        let result = detect_workspace_cycles(&ws_with_pkgs(vec![("pkg-a", vec!["lodash"])]));
        assert!(
            result.is_empty(),
            "external deps should not cause cycle: {:?}",
            result
        );
    }

    #[test]
    fn discover_skips_missing_workspace_files() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        std::fs::write(root.join("package.json"), "{}").unwrap();

        let ws = Workspace::discover(root).unwrap();
        assert!(ws.packages.is_empty());
    }

    #[test]
    fn discover_prefers_pnpm_yaml_over_orix_yaml() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();

        std::fs::create_dir_all(root.join("packages/pkg1")).unwrap();
        std::fs::write(
            root.join("packages/pkg1/package.json"),
            r#"{"name":"pkg1"}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("pnpm-workspace.yaml"),
            "packages:\n  - 'packages/pkg1'",
        )
        .unwrap();
        std::fs::write(
            root.join("orix-workspace.yaml"),
            "packages:\n  - 'packages/other'",
        )
        .unwrap();

        let ws = Workspace::discover(root).unwrap();
        assert_eq!(ws.packages.len(), 1);
        assert_eq!(ws.packages[0].manifest.name.as_deref(), Some("pkg1"));
    }

    #[test]
    fn discover_prefers_orix_yaml_over_root_package_json() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();

        std::fs::create_dir_all(root.join("packages/pkg1")).unwrap();
        std::fs::write(
            root.join("packages/pkg1/package.json"),
            r#"{"name":"pkg1"}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("package.json"),
            r#"{"orix":{"packages":["packages/other"]}}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("orix-workspace.yaml"),
            "packages:\n  - 'packages/pkg1'",
        )
        .unwrap();

        let ws = Workspace::discover(root).unwrap();
        assert_eq!(ws.packages.len(), 1);
        assert_eq!(ws.packages[0].manifest.name.as_deref(), Some("pkg1"));
    }

    #[test]
    fn discover_from_orix_yaml() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();

        std::fs::create_dir_all(root.join("apps/web")).unwrap();
        std::fs::create_dir_all(root.join("libs/shared")).unwrap();
        std::fs::write(root.join("apps/web/package.json"), r#"{"name":"@org/web"}"#).unwrap();
        std::fs::write(
            root.join("libs/shared/package.json"),
            r#"{"name":"@org/shared"}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("orix-workspace.yaml"),
            "packages:\n  - 'apps/*'\n  - 'libs/*'",
        )
        .unwrap();

        let ws = Workspace::discover(root).unwrap();
        assert_eq!(ws.packages.len(), 2);
        let names: Vec<_> = ws
            .packages
            .iter()
            .filter_map(|p| p.manifest.name.clone())
            .collect();
        assert!(names.contains(&"@org/web".to_string()));
        assert!(names.contains(&"@org/shared".to_string()));
    }

    #[test]
    fn discover_from_root_package_json() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();

        std::fs::create_dir_all(root.join("packages/pkg-a")).unwrap();
        std::fs::write(
            root.join("packages/pkg-a/package.json"),
            r#"{"name":"pkg-a"}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("package.json"),
            r#"{"name":"root","orix":{"packages":["packages/*"]}}"#,
        )
        .unwrap();

        let ws = Workspace::discover(root).unwrap();
        assert_eq!(ws.packages.len(), 1);
        assert_eq!(ws.packages[0].manifest.name.as_deref(), Some("pkg-a"));
    }

    #[test]
    fn discover_ignores_non_array_orix_packages() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        std::fs::write(
            root.join("package.json"),
            r#"{"name":"root","orix":{"packages":"packages/*"}}"#,
        )
        .unwrap();

        let ws = Workspace::discover(root).unwrap();
        assert!(ws.packages.is_empty());
    }
}
