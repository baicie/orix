//! Resolver progress and skip types.

use orix_domain::{PackageId, PackageName};

/// Progress event emitted during dependency resolution.
#[derive(Debug, Clone)]
pub struct ResolveProgressEvent {
    /// Resolved package id.
    pub id: PackageId,
    /// Total number of packages discovered so far (running estimate).
    pub discovered: usize,
    /// Number of packages resolved so far.
    pub resolved: usize,
}

/// An optional dependency that was skipped due to platform mismatch.
#[derive(Debug, Clone)]
pub struct SkippedOptionalDep {
    /// The name of the skipped optional dependency.
    pub name: PackageName,
    /// Reason why the dependency was skipped.
    pub reason: String,
}
