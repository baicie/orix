//! Registry API types.

use std::collections::HashMap;

use serde::Deserialize;

/// The full packument for a package — metadata for all versions plus dist-tags.
#[derive(Deserialize, Debug, Clone)]
pub struct Packument {
    /// Package name.
    pub name: String,
    /// Available versions keyed by version string.
    pub versions: HashMap<String, PackageMetadata>,
    /// Distribution tags (e.g., latest, next, beta).
    #[serde(default)]
    pub dist_tags: HashMap<String, String>,
}

/// Metadata for a single published version of a package.
#[derive(Deserialize, Debug, Clone)]
pub struct PackageMetadata {
    /// Package name.
    pub name: String,
    /// Version string.
    pub version: String,
    /// Regular dependencies.
    #[serde(default)]
    pub dependencies: HashMap<String, String>,
    /// Development dependencies.
    #[serde(default)]
    pub dev_dependencies: HashMap<String, String>,
    /// Optional dependencies.
    #[serde(default)]
    pub optional_dependencies: HashMap<String, String>,
    /// Peer dependencies.
    #[serde(default)]
    pub peer_dependencies: HashMap<String, String>,
    /// Engine constraints.
    #[serde(default)]
    pub engines: Option<Engines>,
    /// Supported operating systems.
    #[serde(default)]
    pub os: Vec<String>,
    /// Supported CPU architectures.
    #[serde(default)]
    pub cpu: Vec<String>,
    /// Distribution info (tarball URL, integrity, shasum).
    pub dist: Dist,
    /// Whether this version is marked optional.
    #[serde(default)]
    pub optional: bool,
}

/// Node/npm engine constraints.
#[derive(Deserialize, Debug, Clone, Default)]
pub struct Engines {
    /// Minimum Node version constraint.
    #[serde(default)]
    pub node: Option<String>,
}

/// Distribution info for a published package version.
#[derive(Deserialize, Debug, Clone, Default)]
pub struct Dist {
    /// URL to the tarball.
    pub tarball: String,
    /// Integrity hash (sha512/sha1).
    #[serde(default)]
    pub integrity: Option<String>,
    /// SHA1 shasum (legacy, superseded by integrity).
    #[serde(default)]
    pub shasum: Option<String>,
}
