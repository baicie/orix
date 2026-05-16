//! Workspace discovery and management.

#![deny(clippy::unwrap_used)]

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

use orix_manifest::Manifest;

use super::WorkspaceSpec;

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
}

/// A pnpm-workspace.yaml file.
#[derive(Debug, Deserialize)]
struct WorkspaceFile {
    packages: Vec<String>,
}

impl Workspace {
    /// Discover a workspace starting from the given root directory.
    pub fn discover(root: PathBuf) -> Result<Self> {
        let manifest_path = root.join("pnpm-workspace.yaml");
        let packages = if manifest_path.exists() {
            let source = std::fs::read_to_string(&manifest_path)
                .with_context(|| "failed to read pnpm-workspace.yaml")?;
            let workspace_file: WorkspaceFile = serde_yaml::from_str(&source)
                .with_context(|| "failed to parse pnpm-workspace.yaml")?;
            Self::find_packages(&root, &workspace_file.packages)?
        } else {
            Vec::new()
        };

        Ok(Self {
            root: root.clone(),
            packages,
            lockfile_path: root.join("orix-lock.yaml"),
        })
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
    pub fn resolve_workspace_dep(&self, spec: &WorkspaceSpec) -> Option<WorkspacePackage> {
        if let Some(ref name) = spec.name {
            self.packages
                .iter()
                .find(|p| p.manifest.name.as_ref() == Some(name))
                .cloned()
        } else {
            let abs = self.root.join(&spec.path);
            let manifest = Manifest::read(&abs.join("package.json")).ok()?;
            Some(WorkspacePackage {
                relative_path: spec.path.clone(),
                abs_path: abs,
                manifest,
            })
        }
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
}
