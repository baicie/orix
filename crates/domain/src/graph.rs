//! Dependency graph.

use crate::package::{PackageId, ResolvedPackage};
use crate::peer::ResolverDiagnostic;

/// The complete resolved dependency graph for one importer.
#[derive(Clone, Debug, Default)]
pub struct DependencyGraph {
    inner: std::collections::BTreeMap<PackageId, ResolvedPackage>,
    /// Diagnostic messages collected during resolution.
    pub diagnostics: Vec<ResolverDiagnostic>,
}

impl DependencyGraph {
    /// Create an empty dependency graph.
    pub fn new() -> Self {
        Self {
            inner: Default::default(),
            diagnostics: Vec::new(),
        }
    }

    /// Insert a resolved package into the graph.
    pub fn insert(&mut self, pkg: ResolvedPackage) {
        self.inner.insert(pkg.id.clone(), pkg);
    }

    /// Look up a package by its ID.
    pub fn get(&self, id: &PackageId) -> Option<&ResolvedPackage> {
        self.inner.get(id)
    }

    /// Check whether a package ID exists in the graph.
    pub fn contains(&self, id: &PackageId) -> bool {
        self.inner.contains_key(id)
    }

    /// Iterate over all resolved packages.
    pub fn packages(&self) -> impl Iterator<Item = &ResolvedPackage> {
        self.inner.values()
    }

    /// Iterate over all package IDs.
    pub fn package_ids(&self) -> impl Iterator<Item = &PackageId> {
        self.inner.keys()
    }

    /// Merge another graph into this one (packages with the same ID are deduplicated by ID).
    pub fn merge(&mut self, other: DependencyGraph) {
        self.inner.extend(other.inner);
    }

    /// Number of packages in the graph.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// True when the graph has no packages.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Compute a stable SHA-256 hash of the dependency graph.
    /// Used by the linker fast path to detect whether node_modules layout is still valid.
    pub fn graph_hash(&self) -> String {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        for pkg in self.inner.values() {
            hasher.update(pkg.id.key().as_bytes());
        }
        hex::encode(hasher.finalize())
    }
}
