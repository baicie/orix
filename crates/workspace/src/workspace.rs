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
