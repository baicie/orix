//! Peer dependency context and resolver diagnostics.

use std::fmt;

use crate::name::PackageName;
use crate::package::PackageId;
use crate::version::{Version, VersionConstraint};

/// Peer context: the resolved peers visible from a package's installation point.
/// Used by the peer-aware dependency resolver to determine which instance of a
/// package to install.
#[derive(Debug, Clone, Default, Eq, PartialEq, Hash)]
pub struct PeerContext {
    /// Resolved peer packages, keyed by name.
    pub resolved: std::collections::BTreeMap<PackageName, PackageId>,
}

impl PeerContext {
    /// Returns true if no peers are present.
    pub fn is_empty(&self) -> bool {
        self.resolved.is_empty()
    }

    /// Insert a resolved peer.
    pub fn insert(&mut self, name: PackageName, id: PackageId) {
        self.resolved.insert(name, id);
    }

    /// Generate the peer suffix for lockfile keys, sorted by package name.
    /// Format: "(react@18.2.0)(lodash@4.17.21)"
    /// Empty context produces an empty string.
    pub fn key(&self) -> String {
        let mut parts: Vec<String> = self
            .resolved
            .values()
            .map(|id| format!("({})", id))
            .collect();
        parts.sort();
        parts.join("")
    }
}

/// Package instance ID: combines source identity (name + version) with the peer
/// context at the installation point. Two packages with the same source but
/// different peer environments resolve to different instance IDs.
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct PackageInstanceId {
    /// Source package identity.
    pub package: PackageId,
    /// Peer environment at the installation point.
    pub peer_context: PeerContext,
}

impl PackageInstanceId {
    /// Create a new package instance ID.
    pub fn new(package: PackageId, peer_context: PeerContext) -> Self {
        Self {
            package,
            peer_context,
        }
    }

    /// Generate the full lockfile key including peer suffix.
    /// Format: "name@ver(peer1@ver1)(peer2@ver2)"
    pub fn key(&self) -> String {
        let suffix = self.peer_context.key();
        if suffix.is_empty() {
            self.package.key()
        } else {
            format!("{}{}", self.package.key(), suffix)
        }
    }

    /// Return a version of this instance ID without peer context
    /// (for lockfile v1 compatibility).
    pub fn without_peers(&self) -> PackageInstanceId {
        PackageInstanceId {
            package: self.package.clone(),
            peer_context: PeerContext::default(),
        }
    }
}

/// Peer requirement: describes what a package declares as a peer dependency.
#[derive(Debug, Clone)]
pub struct PeerRequirement {
    /// Package that makes this requirement.
    pub requester: PackageId,
    /// Name of the required peer package.
    pub name: PackageName,
    /// Version constraint on the peer.
    pub range: VersionConstraint,
    /// Whether the peer is optional.
    pub optional: bool,
}

// ─── Resolver diagnostics ─────────────────────────────────────────────────────

/// Diagnostic messages produced during dependency resolution.
#[derive(Debug, Clone)]
pub enum ResolverDiagnostic {
    /// A required peer dependency was not found in the environment.
    MissingPeer {
        /// The package that declared the peer dependency.
        requester: PackageId,
        /// Name of the missing peer package.
        peer_name: PackageName,
        /// Version constraint that could not be satisfied.
        range: VersionConstraint,
    },
    /// An optional peer dependency was not found (informational only).
    OptionalPeerMissing {
        /// The package that declared the peer dependency.
        requester: PackageId,
        /// Name of the missing optional peer package.
        peer_name: PackageName,
        /// Version constraint that could not be satisfied.
        range: VersionConstraint,
    },
    /// A peer dependency version conflict detected.
    PeerVersionConflict {
        /// The package that declared the peer dependency.
        requester: PackageId,
        /// Name of the conflicting peer package.
        peer_name: PackageName,
        /// Version range that was requested.
        requested_range: VersionConstraint,
        /// The version that was actually found.
        found_version: Version,
    },
}

impl fmt::Display for ResolverDiagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ResolverDiagnostic::MissingPeer {
                requester,
                peer_name,
                range,
            } => {
                writeln!(f, "warning: unmet peer dependency")?;
                writeln!(f, "  {} requires {}@{}", requester, peer_name, range.raw)?;
                write!(f, "hint: install the required peer dependency")
            }
            ResolverDiagnostic::OptionalPeerMissing {
                requester,
                peer_name,
                range,
            } => {
                writeln!(f, "info: optional peer not found")?;
                write!(
                    f,
                    "  {} prefers {}@{} (optional)",
                    requester, peer_name, range.raw
                )
            }
            ResolverDiagnostic::PeerVersionConflict {
                requester,
                peer_name,
                requested_range,
                found_version,
            } => {
                writeln!(f, "warning: peer dependency version conflict")?;
                writeln!(
                    f,
                    "  {} requires {}@{}",
                    requester, peer_name, requested_range.raw
                )?;
                writeln!(f, "  found {}@{}", peer_name, found_version)?;
                write!(
                    f,
                    "hint: update {} to satisfy the range, or install a compatible {} version",
                    peer_name, requester
                )
            }
        }
    }
}
