//! Content-addressable package cache.

mod store;

pub use store::{Store, StoreError};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// SHA-256 hash of file content.
pub fn sha256(content: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content);
    let result = hasher.finalize();
    hex::encode(result)
}

/// The integrity metadata stored for each package in the store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntegrityMeta {
    /// Package name.
    pub name: String,
    /// Package version.
    pub version: String,
    /// Content integrity hash.
    pub integrity: String,
    /// Files in the package with their hashes.
    pub files: Vec<(String, String)>,
    /// Transitive dependency node keys that this package declares.
    /// Used by the linker to know which symlinks to create inside the package node_modules.
    #[serde(default)]
    pub depnodes: Vec<String>,
}

/// Report from a store prune operation.
#[derive(Debug)]
pub struct PruneReport {
    /// Number of packages removed.
    pub packages_removed: usize,
    /// Number of files removed.
    pub files_removed: usize,
    /// Number of bytes reclaimed.
    pub bytes_reclaimed: u64,
}
