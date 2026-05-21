//! Mutable resolver state during concurrent resolution.

use std::collections::{BTreeMap, HashSet};

use orix_domain::{DependencyGraph, PackageId, PackageName};

/// Mutable state used during concurrent resolution.
pub(crate) struct ResolverState {
    pub(crate) graph: DependencyGraph,
    pub(crate) memo: BTreeMap<(PackageName, String), PackageId>,
    pub(crate) in_flight: HashSet<(PackageName, String)>,
    pub(crate) discovered: usize,
    pub(crate) resolved: usize,
}
