//! Workspace types and catalog parsing.

use std::collections::HashMap;
use std::path::PathBuf;

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
