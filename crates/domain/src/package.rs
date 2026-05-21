//! Package identity and resolved package metadata.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::name::PackageName;
use crate::version::{PatchSpec, Version};

/// Uniquely identifies a package: name + version.
#[derive(Clone, PartialEq, Eq, Hash, Debug, Ord, PartialOrd, Serialize, Deserialize)]
pub struct PackageId {
    /// Package name.
    pub name: PackageName,
    /// Package version.
    pub version: Version,
}

impl PackageId {
    /// Create a new package ID.
    pub fn new(name: PackageName, version: Version) -> Self {
        Self { name, version }
    }

    /// Returns the key used in lockfiles and the store: "name@version"
    pub fn key(&self) -> String {
        format!("{}@{}", self.name, self.version)
    }
}

impl fmt::Display for PackageId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{}", self.name, self.version)
    }
}

// ─── ResolvedPackage ───────────────────────────────────────────────────────────

/// A package resolved from the registry with all metadata needed for install.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResolvedPackage {
    /// Unique package ID (name + version).
    pub id: PackageId,
    /// Integrity string (sha512/sha1).
    pub integrity: String,
    /// URL to the tarball.
    pub tarball: String,
    /// Regular dependencies.
    pub dependencies: Vec<(PackageName, String)>,
    /// Dev dependencies.
    pub dev_dependencies: Vec<(PackageName, String)>,
    /// Optional dependencies.
    pub optional_dependencies: Vec<(PackageName, String)>,
    #[serde(default)]
    /// Peer dependencies.
    pub peer_dependencies: Vec<(PackageName, String)>,
    #[serde(default)]
    /// Engine constraints (e.g., node >= 14).
    pub engines: Option<String>,
    #[serde(default)]
    /// Supported operating systems.
    pub os: Vec<String>,
    #[serde(default)]
    /// Supported CPU architectures.
    pub cpu: Vec<String>,
    /// Transitive dependency node keys that this package declares.
    /// Format: "name@version". Used by the linker to know which symlinks to create.
    #[serde(default)]
    pub depnodes: Vec<String>,
    /// Patch applied to this package (if any, from patch: protocol).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patch: Option<PatchSpec>,
}
