//! Lockfile data types.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Lockfile format version.
pub const LOCKFILE_VERSION: i32 = 3;

/// The lockfile root — mirrors pnpm's orix-lock.yaml structure.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lockfile {
    /// Lockfile version number.
    #[serde(rename = "lockfileVersion")]
    pub version: i32,
    /// Whether to save remote cache URLs.
    #[serde(rename = "saveRemoteCacheURLs", default)]
    pub save_remote_cache_urls: bool,
    /// Per-importer dependency resolutions.
    pub importers: BTreeMap<String, ImporterLock>,
    /// Resolved packages keyed by package ID.
    pub packages: BTreeMap<String, PackageLock>,
    /// SHA-256 hash of the serialized dependency graph for layout validation.
    /// When set, the linker can skip rebuild if node_modules was generated from the same graph.
    #[serde(
        rename = "orixGraphHash",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub graph_hash: Option<String>,
}

/// Dependency resolutions for one importer (root or workspace package).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImporterLock {
    /// Resolved production dependencies.
    #[serde(default)]
    pub dependencies: BTreeMap<String, ResolvedDep>,
    /// Resolved development dependencies.
    #[serde(rename = "devDependencies", default)]
    pub dev_dependencies: BTreeMap<String, ResolvedDep>,
    /// Resolved optional dependencies.
    #[serde(rename = "optionalDependencies", default)]
    pub optional_dependencies: BTreeMap<String, ResolvedDep>,
    /// Original dependency specifiers (for diffing).
    #[serde(default)]
    pub specifiers: BTreeMap<String, String>,
}

/// A single resolved dependency entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedDep {
    /// Resolved version string.
    pub version: String,
    /// Original specifier from package.json.
    #[serde(rename = "specifier", default)]
    pub specifier: String,
    /// Registry package ID.
    #[serde(rename = "id", default)]
    pub id: Option<String>,
    /// Whether this is a dev dependency.
    #[serde(rename = "dev", default)]
    pub dev: Option<bool>,
    /// Whether this is an optional dependency.
    #[serde(rename = "optional", default)]
    pub optional: Option<bool>,
    /// Node engine constraint.
    #[serde(rename = "engines", default)]
    pub engines: Option<String>,
    /// Supported OS constraints.
    #[serde(rename = "os", default)]
    pub os: Option<Vec<String>>,
    /// Supported CPU constraints.
    #[serde(rename = "cpu", default)]
    pub cpu: Option<Vec<String>>,
    /// Transitive dependencies of this package.
    #[serde(rename = "dependencies", default)]
    pub dependencies: BTreeMap<String, String>,
    /// Transitive optional dependencies.
    #[serde(rename = "optionalDependencies", default)]
    pub optional_dependencies: BTreeMap<String, String>,
    /// Peer dependencies visible from this dependency.
    #[serde(rename = "peerDependencies", default)]
    pub peer_dependencies: BTreeMap<String, String>,
}

/// A resolved package entry in the lockfile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageLock {
    /// Registry package ID.
    #[serde(rename = "id", default)]
    pub id: Option<String>,
    /// Local path (for workspace packages).
    #[serde(rename = "local", default)]
    pub local: Option<String>,
    /// Integrity hash.
    pub integrity: Option<String>,
    /// Package name.
    pub name: Option<String>,
    /// Package version.
    pub version: Option<String>,
    /// Resolution details.
    pub resolution: Option<PackageResolution>,
    /// Transitive dependencies.
    #[serde(default)]
    pub dependencies: BTreeMap<String, String>,
    /// Transitive optional dependencies.
    #[serde(rename = "optionalDependencies", default)]
    pub optional_dependencies: BTreeMap<String, String>,
    /// Peer dependencies.
    #[serde(rename = "peerDependencies", default)]
    pub peer_dependencies: BTreeMap<String, String>,
    /// Node engine constraint.
    #[serde(rename = "engines", default)]
    pub engines: Option<String>,
    /// Supported OS constraints.
    #[serde(rename = "os", default)]
    pub os: Option<Vec<String>>,
    /// Supported CPU constraints.
    #[serde(rename = "cpu", default)]
    pub cpu: Option<Vec<String>>,
}

/// Resolution details for a package.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageResolution {
    /// Tarball URL.
    pub tarball: Option<String>,
    /// Integrity hash.
    pub integrity: Option<String>,
    /// Resolution type.
    #[serde(rename = "type", default)]
    pub resolution_type: Option<String>,
    /// Local file path (for workspace packages).
    pub path: Option<String>,
}

/// The diff between two lockfile states.
#[derive(Debug, Default)]
pub struct LockfileDiff {
    /// Packages added since the old lockfile.
    pub added: Vec<String>,
    /// Packages removed since the old lockfile.
    pub removed: Vec<String>,
    /// Packages whose lockfile entry changed while keeping the same package key.
    pub changed: Vec<String>,
    /// Importers whose specifiers changed.
    pub importers_changed: Vec<String>,
}
