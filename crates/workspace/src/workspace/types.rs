//! Workspace types and catalog parsing.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use orix_manifest::Manifest;

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

/// A filter selector for workspace packages.
/// Used by `orix run --filter` to select which packages to target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceSelector {
    /// Match by package name (e.g., `@scope/pkg`).
    PackageName(String),
    /// Match by relative path (e.g., `./example`, `../utils`).
    RelativePath(PathBuf),
    /// Match by glob pattern (e.g., `./qiankun/*`, `./packages/*`).
    Glob(String),
}

impl WorkspaceSelector {
    /// Parse a raw selector string into a `WorkspaceSelector`.
    ///
    /// Rules:
    /// - `./path` or `../path` → `RelativePath`
    /// - Pattern with `*`, `?`, `[` → `Glob`
    /// - Otherwise → `PackageName`
    pub fn parse(raw: &str) -> Self {
        let raw = raw.trim();

        // Check for relative path patterns.
        if raw.starts_with("./") || raw.starts_with("../") {
            // Check if it contains glob characters.
            if raw.contains('*') || raw.contains('?') || raw.contains('[') {
                return Self::Glob(raw.to_string());
            }
            return Self::RelativePath(PathBuf::from(raw));
        }

        // No prefix → treat as package name.
        Self::PackageName(raw.to_string())
    }

    /// Match this selector against a workspace package.
    #[allow(unused_variables)]
    pub fn matches(&self, pkg: &WorkspacePackage, root: &Path) -> bool {
        match self {
            WorkspaceSelector::PackageName(name) => {
                pkg.manifest.name.as_deref() == Some(name.as_str())
            }
            WorkspaceSelector::RelativePath(path) => {
                // Normalize both paths: normalize slashes and remove trailing separator.
                let input_normalized = Self::normalize_path(path);
                let pkg_path = Self::normalize_path(&pkg.relative_path);
                // If input starts with "./", also try without it for matching.
                let input_stripped = input_normalized
                    .strip_prefix("./")
                    .unwrap_or(&input_normalized);
                input_normalized == pkg_path || input_stripped == pkg_path
            }
            WorkspaceSelector::Glob(pattern) => {
                let clean_pattern = pattern.trim_start_matches("./");
                let matcher = match glob::Pattern::new(clean_pattern) {
                    Ok(m) => m,
                    Err(_) => return false,
                };
                let pkg_path = pkg.relative_path.to_string_lossy().replace('\\', "/");
                matcher.matches(&pkg_path)
            }
        }
    }

    /// Normalize a path for comparison (forward slashes, no trailing separators).
    fn normalize_path(path: &Path) -> String {
        path.to_string_lossy()
            .replace('\\', "/")
            .trim_end_matches('/')
            .to_string()
    }
}

/// Filter workspace packages by selectors.
///
/// Returns all packages if no selectors are provided.
pub fn filter_workspace_packages(
    workspace: &Workspace,
    selectors: &[WorkspaceSelector],
) -> Vec<WorkspacePackage> {
    if selectors.is_empty() {
        return workspace.packages.clone();
    }

    workspace
        .packages
        .iter()
        .filter(|pkg| {
            selectors
                .iter()
                .any(|sel| sel.matches(pkg, &workspace.root))
        })
        .cloned()
        .collect()
}

impl std::fmt::Display for WorkspaceSelector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WorkspaceSelector::PackageName(name) => write!(f, "{}", name),
            WorkspaceSelector::RelativePath(path) => write!(f, "{}", path.display()),
            WorkspaceSelector::Glob(pattern) => write!(f, "{}", pattern),
        }
    }
}
