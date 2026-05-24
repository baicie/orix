//! Lockfile data types.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Lockfile format version.
pub const LOCKFILE_VERSION: i32 = 4;

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
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub packages: BTreeMap<String, PackageLock>,
    /// Logical dependency snapshots keyed by package instance ID.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub snapshots: BTreeMap<String, SnapshotLock>,
    /// SHA-256 hash of the serialized dependency graph for layout validation.
    /// When set, the linker can skip rebuild if node_modules was generated from the same graph.
    #[serde(
        rename = "orixGraphHash",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub graph_hash: Option<String>,
}

/// Dependency edges for one logical package instance.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotLock {
    /// Transitive dependencies.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub dependencies: BTreeMap<String, String>,
    /// Transitive optional dependencies.
    #[serde(
        rename = "optionalDependencies",
        default,
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub optional_dependencies: BTreeMap<String, String>,
    /// Peer dependencies.
    #[serde(
        rename = "peerDependencies",
        default,
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub peer_dependencies: BTreeMap<String, String>,
    /// Resolved peer context for future peer-aware package instances.
    #[serde(
        rename = "peerContext",
        default,
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub peer_context: BTreeMap<String, String>,
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
    #[serde(rename = "id", default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Whether this is a dev dependency.
    #[serde(rename = "dev", default, skip_serializing_if = "Option::is_none")]
    pub dev: Option<bool>,
    /// Whether this is an optional dependency.
    #[serde(rename = "optional", default, skip_serializing_if = "Option::is_none")]
    pub optional: Option<bool>,
    /// Node engine constraint.
    #[serde(rename = "engines", default, skip_serializing_if = "Option::is_none")]
    pub engines: Option<String>,
    /// Supported OS constraints.
    #[serde(rename = "os", default, skip_serializing_if = "Option::is_none")]
    pub os: Option<Vec<String>>,
    /// Supported CPU constraints.
    #[serde(rename = "cpu", default, skip_serializing_if = "Option::is_none")]
    pub cpu: Option<Vec<String>>,
    /// Transitive dependencies of this package.
    #[serde(
        rename = "dependencies",
        default,
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub dependencies: BTreeMap<String, String>,
    /// Transitive optional dependencies.
    #[serde(
        rename = "optionalDependencies",
        default,
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub optional_dependencies: BTreeMap<String, String>,
    /// Peer dependencies visible from this dependency.
    #[serde(
        rename = "peerDependencies",
        default,
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub peer_dependencies: BTreeMap<String, String>,
}

/// A resolved package entry in the lockfile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageLock {
    /// Registry package ID.
    #[serde(rename = "id", default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Local path (for workspace packages).
    #[serde(rename = "local", default, skip_serializing_if = "Option::is_none")]
    pub local: Option<String>,
    /// Integrity hash.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub integrity: Option<String>,
    /// Package name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Package version.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Resolution details.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolution: Option<PackageResolution>,
    /// Node engine constraint.
    #[serde(rename = "engines", default, skip_serializing_if = "Option::is_none")]
    pub engines: Option<String>,
    /// Supported OS constraints.
    #[serde(rename = "os", default, skip_serializing_if = "Option::is_none")]
    pub os: Option<Vec<String>>,
    /// Supported CPU constraints.
    #[serde(rename = "cpu", default, skip_serializing_if = "Option::is_none")]
    pub cpu: Option<Vec<String>>,
}

/// Resolution details for a package.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageResolution {
    /// Tarball URL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tarball: Option<String>,
    /// Integrity hash.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub integrity: Option<String>,
    /// Resolution type.
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub resolution_type: Option<String>,
    /// Local file path (for workspace packages).
    #[serde(skip_serializing_if = "Option::is_none")]
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
