//! Dependency resolution engine.

#![deny(clippy::unwrap_used)]

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use tokio::sync::mpsc;

use orix_domain::{
    ConstraintKind, DependencyGraph, PackageId, PackageName, ResolvedPackage, Version,
    VersionConstraint,
};
use orix_manifest::Manifest;
use orix_registry::{Packument, RegistryClient};
use orix_workspace::{Workspace, WorkspaceSpec};
use url::Url;

/// An optional dependency that was skipped due to platform mismatch.
#[derive(Debug, Clone)]
pub struct ResolveProgressEvent {
    /// Resolved package id.
    pub id: PackageId,
    /// Index of this package in the resolution order (1-based).
    pub index: usize,
    /// Total number of packages seen so far (running estimate of upper bound).
    pub total: usize,
}

/// An optional dependency that was skipped due to platform mismatch.
#[derive(Debug, Clone)]
pub struct SkippedOptionalDep {
    /// The name of the skipped optional dependency.
    pub name: PackageName,
    /// Reason why the dependency was skipped.
    pub reason: String,
}

/// Select the best version for a package from a packument.
fn select_version_impl(packument: &Packument, constraint: &VersionConstraint) -> Result<Version> {
    match &constraint.kind {
        orix_domain::ConstraintKind::Exact(v) => Ok(v.clone()),

        orix_domain::ConstraintKind::Range(range) => {
            let mut candidates: Vec<_> = packument
                .versions
                .keys()
                .filter_map(|v| Version::parse(v).ok())
                .filter(|v| range.matches(v))
                .collect();
            candidates.sort();
            candidates
                .pop()
                .with_context(|| format!("no version satisfies {}", constraint.raw))
        }

        orix_domain::ConstraintKind::Latest => packument
            .dist_tags
            .get("latest")
            .and_then(|v| Version::parse(v).ok())
            .with_context(|| "no dist-tags.latest found in packument"),

        orix_domain::ConstraintKind::Tag(tag) => packument
            .dist_tags
            .get(tag)
            .and_then(|v| Version::parse(v).ok())
            .with_context(|| format!("tag '{}' not found in packument", tag)),

        orix_domain::ConstraintKind::Patch(spec) => {
            // The patch spec includes the exact version already.
            Ok(spec.package_version.clone())
        }

        orix_domain::ConstraintKind::Catalog(_) => {
            // Catalog expansion should happen before version selection.
            // If we reach here, the catalog was not expanded.
            anyhow::bail!(
                "catalog reference '{}' was not expanded — workspace catalog not available",
                constraint.raw
            );
        }
    }
}

/// Resolve a single peer dependency against the current resolution context.
///
/// Returns `None` if the peer cannot be resolved — the caller decides whether
/// to emit a diagnostic based on whether the peer is optional.
fn resolve_peer_dep(
    peer_name: &PackageName,
    peer_range: &str,
    memo: &BTreeMap<(PackageName, String), PackageId>,
    registry: &mut RegistryClient,
) -> Option<PackageId> {
    let constraint = VersionConstraint::parse(peer_range).ok()?;
    let key = (peer_name.clone(), constraint.raw.clone());

    // Check if already resolved in this batch.
    if let Some(id) = memo.get(&key) {
        return Some(id.clone());
    }

    // Synchronous registry lookup for peer resolution.
    // This adds a network hop per unique unresolved peer, which is acceptable
    // since peers are typically already in the memo or are well-known packages.
    let packument = registry.fetch_packument_sync(peer_name).ok()?;
    let version = select_version_impl(&packument, &constraint).ok()?;
    let metadata = packument.versions.get(&version.to_string())?;
    let tarball = metadata.dist.tarball.clone();
    let integrity = metadata
        .dist
        .integrity
        .clone()
        .or(metadata.dist.shasum.clone())
        .unwrap_or_default();
    let deps: Vec<(PackageName, String)> = metadata
        .dependencies
        .iter()
        .map(|(k, v)| (PackageName::from(k.as_str()), v.clone()))
        .collect();
    let dev_deps: Vec<(PackageName, String)> = metadata
        .dev_dependencies
        .iter()
        .map(|(k, v)| (PackageName::from(k.as_str()), v.clone()))
        .collect();
    let opt_deps: Vec<(PackageName, String)> = metadata
        .optional_dependencies
        .iter()
        .map(|(k, v)| (PackageName::from(k.as_str()), v.clone()))
        .collect();
    let peer_deps: Vec<(PackageName, String)> = metadata
        .peer_dependencies
        .iter()
        .map(|(k, v)| (PackageName::from(k.as_str()), v.clone()))
        .collect();
    let depnodes: Vec<String> = deps
        .iter()
        .chain(opt_deps.iter())
        .map(|(n, _)| n.to_string())
        .chain(peer_deps.iter().map(|(n, _)| n.to_string()))
        .collect();
    let resolved = ResolvedPackage {
        id: PackageId::new(peer_name.clone(), version.clone()),
        integrity,
        tarball,
        dependencies: deps,
        dev_dependencies: dev_deps,
        optional_dependencies: opt_deps,
        peer_dependencies: peer_deps,
        engines: metadata.engines.as_ref().and_then(|e| e.node.clone()),
        os: metadata.os.clone(),
        cpu: metadata.cpu.clone(),
        depnodes,
        patch: None,
    };
    Some(resolved.id.clone())
}

/// Core resolution loop. Takes independent mutable references to avoid borrow conflicts.
async fn resolve_batch_impl(
    graph: &mut DependencyGraph,
    memo: &mut BTreeMap<(PackageName, String), PackageId>,
    registry: &mut RegistryClient,
    mut progress_tx: Option<&mut mpsc::Sender<ResolveProgressEvent>>,
    to_resolve: Vec<(PackageName, VersionConstraint)>,
) -> Result<()> {
    let mut pending: Vec<(PackageName, VersionConstraint)> = to_resolve;
    let mut resolved_count: usize = 0;

    while let Some((name, constraint)) = pending.pop() {
        let key = (name.clone(), constraint.raw.clone());
        if memo.contains_key(&key) {
            continue;
        }

        let packument = registry
            .fetch_packument(&name)
            .await
            .with_context(|| format!("failed to fetch packument for '{}'", name))?;

        let version = select_version_impl(&packument, &constraint)
            .with_context(|| format!("failed to select version for '{}'", name))?;

        let metadata = packument
            .versions
            .get(&version.to_string())
            .with_context(|| format!("version {} not found in packument", version))?;

        let pkg_id = PackageId::new(name.clone(), version.clone());
        let tarball = metadata.dist.tarball.clone();
        let integrity = metadata
            .dist
            .integrity
            .clone()
            .or(metadata.dist.shasum.clone())
            .unwrap_or_default();

        let deps: Vec<(PackageName, String)> = metadata
            .dependencies
            .iter()
            .map(|(k, v)| (PackageName::from(k.as_str()), v.clone()))
            .collect();
        let dev_deps: Vec<(PackageName, String)> = metadata
            .dev_dependencies
            .iter()
            .map(|(k, v)| (PackageName::from(k.as_str()), v.clone()))
            .collect();
        let opt_deps: Vec<(PackageName, String)> = metadata
            .optional_dependencies
            .iter()
            .map(|(k, v)| (PackageName::from(k.as_str()), v.clone()))
            .collect();
        let peer_deps: Vec<(PackageName, String)> = metadata
            .peer_dependencies
            .iter()
            .map(|(k, v)| (PackageName::from(k.as_str()), v.clone()))
            .collect();

        // Resolve each peer dependency against the current memo.
        for (peer_name, peer_range) in &peer_deps {
            let _ = resolve_peer_dep(peer_name, peer_range, memo, registry);
        }

        let depnodes: Vec<String> = deps
            .iter()
            .chain(opt_deps.iter())
            .map(|(n, _)| n.to_string())
            .chain(peer_deps.iter().map(|(n, _)| n.to_string()))
            .collect();

        // Extract patch spec if this is a patch: protocol dependency.
        let patch = match &constraint.kind {
            ConstraintKind::Patch(spec) => Some(spec.clone()),
            _ => None,
        };

        let resolved = ResolvedPackage {
            id: pkg_id.clone(),
            integrity,
            tarball,
            dependencies: deps.clone(),
            dev_dependencies: dev_deps,
            optional_dependencies: opt_deps.clone(),
            peer_dependencies: peer_deps,
            engines: metadata.engines.as_ref().and_then(|e| e.node.clone()),
            os: metadata.os.clone(),
            cpu: metadata.cpu.clone(),
            depnodes,
            patch,
        };

        resolved_count += 1;

        if let Some(ref mut tx) = progress_tx {
            let total = pending.len() + resolved_count;
            let _ = tx.try_send(ResolveProgressEvent {
                id: pkg_id.clone(),
                index: resolved_count,
                total,
            });
        }

        graph.insert(resolved);
        memo.insert(key, pkg_id);

        for (n, raw) in deps.iter().chain(opt_deps.iter()) {
            if let Ok(c) = VersionConstraint::parse(raw) {
                pending.push((n.clone(), c));
            }
        }
    }
    Ok(())
}

/// The dependency resolution engine.
pub struct Resolver {
    registry: RegistryClient,
    memo: BTreeMap<(PackageName, String), PackageId>,
    skipped_optional: Vec<SkippedOptionalDep>,
    progress_tx: Option<mpsc::Sender<ResolveProgressEvent>>,
}

impl Resolver {
    /// Creates a new resolver with the given registry URL.
    pub fn new(registry_url: Url) -> Self {
        Self {
            registry: RegistryClient::new(registry_url),
            memo: Default::default(),
            skipped_optional: Vec::new(),
            progress_tx: None,
        }
    }

    /// Creates a new resolver with the given registry URL and authentication token.
    pub fn with_auth(registry_url: Url, token: &str) -> Self {
        Self {
            registry: RegistryClient::with_auth(registry_url, token),
            memo: Default::default(),
            skipped_optional: Vec::new(),
            progress_tx: None,
        }
    }

    /// Attaches a progress channel to emit resolution progress events.
    pub fn with_progress(mut self, tx: mpsc::Sender<ResolveProgressEvent>) -> Self {
        self.progress_tx = Some(tx);
        self
    }

    /// Resolves all dependencies from a single manifest.
    pub async fn resolve_manifest(&mut self, manifest: &Manifest) -> Result<DependencyGraph> {
        let mut graph = DependencyGraph::new();
        let mut to_resolve: Vec<(PackageName, VersionConstraint)> = Vec::new();

        for (name, raw) in &manifest.dependencies {
            let constraint = VersionConstraint::parse(raw)
                .with_context(|| format!("invalid dependency constraint '{}': {}", name, raw))?;
            to_resolve.push((PackageName::from(name.as_str()), constraint));
        }
        for (name, raw) in &manifest.dev_dependencies {
            let constraint = VersionConstraint::parse(raw)
                .with_context(|| format!("invalid devDependency constraint '{}': {}", name, raw))?;
            to_resolve.push((PackageName::from(name.as_str()), constraint));
        }
        for (name, raw) in &manifest.optional_dependencies {
            let constraint = VersionConstraint::parse(raw).with_context(|| {
                format!("invalid optionalDependency constraint '{}': {}", name, raw)
            })?;
            to_resolve.push((PackageName::from(name.as_str()), constraint));
        }

        self.resolve_batch(&mut graph, to_resolve, None).await?;
        Ok(graph)
    }

    /// Resolves all dependencies from multiple manifests.
    pub async fn resolve_manifests(&mut self, manifests: &[&Manifest]) -> Result<DependencyGraph> {
        let mut graph = DependencyGraph::new();
        let mut to_resolve: Vec<(PackageName, VersionConstraint)> = Vec::new();

        for manifest in manifests {
            for (name, raw) in &manifest.dependencies {
                let constraint = VersionConstraint::parse(raw).with_context(|| {
                    format!("invalid dependency constraint '{}': {}", name, raw)
                })?;
                to_resolve.push((PackageName::from(name.as_str()), constraint));
            }
            for (name, raw) in &manifest.dev_dependencies {
                let constraint = VersionConstraint::parse(raw).with_context(|| {
                    format!("invalid devDependency constraint '{}': {}", name, raw)
                })?;
                to_resolve.push((PackageName::from(name.as_str()), constraint));
            }
            for (name, raw) in &manifest.optional_dependencies {
                let constraint = VersionConstraint::parse(raw).with_context(|| {
                    format!("invalid optionalDependency constraint '{}': {}", name, raw)
                })?;
                to_resolve.push((PackageName::from(name.as_str()), constraint));
            }
        }

        self.resolve_batch(&mut graph, to_resolve, None).await?;
        Ok(graph)
    }

    /// Resolve a manifest with workspace awareness.
    /// Workspace dependencies (workspace:*) are resolved from local packages.
    pub async fn resolve_manifest_with_workspace(
        &mut self,
        manifest: &Manifest,
        workspace: Option<&Workspace>,
    ) -> Result<DependencyGraph> {
        let mut graph = DependencyGraph::new();

        let all_deps: Vec<_> = manifest
            .dependencies
            .iter()
            .chain(manifest.dev_dependencies.iter())
            .chain(manifest.optional_dependencies.iter())
            .collect();

        let mut to_resolve: Vec<(PackageName, VersionConstraint)> = Vec::new();

        for (name, raw) in all_deps {
            let key = (PackageName::from(name.as_str()), raw.clone());
            if self.memo.contains_key(&key) {
                continue;
            }

            // Handle workspace:* dependencies before parsing version constraint.
            if let Some(ws) = workspace {
                let spec = WorkspaceSpec::parse(raw);
                if spec.is_workspace_spec() {
                    if let Some(local_pkg) = ws.resolve_workspace_dep(&spec, name.as_str()) {
                        let pkg_id = PackageId::new(
                            PackageName::from(name.as_str()),
                            local_pkg
                                .manifest
                                .version
                                .as_ref()
                                .and_then(|v| Version::parse(v).ok())
                                .unwrap_or_else(|| {
                                    #[allow(clippy::unwrap_used)]
                                    Version::parse("0.0.0").unwrap()
                                }),
                        );

                        let resolved = ResolvedPackage {
                            id: pkg_id.clone(),
                            integrity: String::new(),
                            tarball: String::new(),
                            dependencies: local_pkg
                                .manifest
                                .dependencies
                                .iter()
                                .map(|(k, v)| (PackageName::from(k.as_str()), v.clone()))
                                .collect(),
                            dev_dependencies: local_pkg
                                .manifest
                                .dev_dependencies
                                .iter()
                                .map(|(k, v)| (PackageName::from(k.as_str()), v.clone()))
                                .collect(),
                            optional_dependencies: local_pkg
                                .manifest
                                .optional_dependencies
                                .iter()
                                .map(|(k, v)| (PackageName::from(k.as_str()), v.clone()))
                                .collect(),
                            peer_dependencies: Vec::new(),
                            engines: local_pkg
                                .manifest
                                .engines
                                .as_ref()
                                .and_then(|e| e.node.clone()),
                            os: local_pkg.manifest.os.clone(),
                            cpu: local_pkg.manifest.cpu.clone(),
                            depnodes: Vec::new(),
                            patch: None,
                        };
                        graph.insert(resolved);
                        self.memo.insert(key, pkg_id);
                        continue;
                    }
                }
            }

            let Ok(constraint) = VersionConstraint::parse(raw) else {
                continue;
            };

            // Expand catalog: protocol references using workspace catalogs.
            let constraint = if let Some(ws) = workspace {
                if let orix_domain::ConstraintKind::Catalog(_cat_constraint) = &constraint.kind {
                    if let Some(resolved_version) = ws.resolve_catalog(raw, name.as_str()) {
                        // Replace catalog reference with the actual version constraint.
                        VersionConstraint::parse(&resolved_version).unwrap_or_else(|_| {
                            // Fallback: treat resolved version as exact version.
                            VersionConstraint {
                                raw: resolved_version.clone(),
                                kind: orix_domain::ConstraintKind::Exact(
                                    Version::parse(&resolved_version).unwrap_or_else(|_| {
                                        #[allow(clippy::unwrap_used)]
                                        Version::parse("0.0.0").unwrap()
                                    }),
                                ),
                            }
                        })
                    } else {
                        constraint
                    }
                } else {
                    constraint
                }
            } else {
                constraint
            };

            to_resolve.push((PackageName::from(name.as_str()), constraint));
        }

        self.resolve_batch(&mut graph, to_resolve, workspace)
            .await?;
        Ok(graph)
    }

    /// Returns the list of optional dependencies that were skipped due to platform mismatch.
    pub fn skipped_optional_deps(&self) -> &[SkippedOptionalDep] {
        &self.skipped_optional
    }

    /// Select the best version for a package from a packument.
    #[allow(dead_code)]
    pub fn select_version(
        &self,
        packument: &Packument,
        constraint: &VersionConstraint,
    ) -> Result<Version> {
        select_version_impl(packument, constraint)
    }

    async fn resolve_batch(
        &mut self,
        graph: &mut DependencyGraph,
        to_resolve: Vec<(PackageName, VersionConstraint)>,
        _workspace: Option<&Workspace>,
    ) -> Result<()> {
        let memo = &mut self.memo;
        let registry = &mut self.registry;
        let progress_tx = self.progress_tx.as_mut();

        resolve_batch_impl(graph, memo, registry, progress_tx, to_resolve).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    use orix_registry::{Dist, PackageMetadata, Packument};

    fn resolver() -> anyhow::Result<Resolver> {
        Ok(Resolver::new(url::Url::parse(
            "https://registry.npmjs.org/",
        )?))
    }

    fn packument() -> Packument {
        let versions: std::collections::HashMap<String, PackageMetadata> = [
            ("1.0.0", "sha512-1.0.0"),
            ("1.2.0", "sha512-1.2.0"),
            ("1.3.0", "sha512-1.3.0"),
            ("2.0.0", "sha512-2.0.0"),
        ]
        .into_iter()
        .map(|(version, integ)| {
            (
                version.to_string(),
                PackageMetadata {
                    name: "demo".to_string(),
                    version: version.to_string(),
                    dependencies: HashMap::new(),
                    dev_dependencies: HashMap::new(),
                    optional_dependencies: HashMap::new(),
                    peer_dependencies: HashMap::new(),
                    engines: None,
                    os: Vec::new(),
                    cpu: Vec::new(),
                    dist: Dist {
                        tarball: format!("https://registry.npmjs.org/demo/-/demo-{}.tgz", version),
                        integrity: Some(integ.to_string()),
                        shasum: None,
                    },
                    optional: false,
                },
            )
        })
        .collect();

        Packument {
            name: "demo".to_string(),
            versions,
            dist_tags: HashMap::from([
                ("latest".to_string(), "2.0.0".to_string()),
                ("next".to_string(), "1.3.0".to_string()),
            ]),
        }
    }

    #[test]
    fn select_version_returns_exact_version() -> anyhow::Result<()> {
        let resolver = resolver()?;
        let selected =
            resolver.select_version(&packument(), &VersionConstraint::parse("1.2.0")?)?;
        assert_eq!(selected.to_string(), "1.2.0");
        Ok(())
    }

    #[test]
    fn select_version_returns_highest_matching_range() -> anyhow::Result<()> {
        let resolver = resolver()?;
        let selected =
            resolver.select_version(&packument(), &VersionConstraint::parse("^1.0.0")?)?;
        assert_eq!(selected.to_string(), "1.3.0");
        Ok(())
    }

    #[test]
    fn select_version_uses_latest_dist_tag() -> anyhow::Result<()> {
        let resolver = resolver()?;
        let selected =
            resolver.select_version(&packument(), &VersionConstraint::parse("latest")?)?;
        assert_eq!(selected.to_string(), "2.0.0");
        Ok(())
    }

    #[test]
    fn select_version_uses_named_dist_tag() -> anyhow::Result<()> {
        let resolver = resolver()?;
        let selected = resolver.select_version(&packument(), &VersionConstraint::parse("next")?)?;
        assert_eq!(selected.to_string(), "1.3.0");
        Ok(())
    }

    #[test]
    fn select_version_errors_when_range_has_no_match() -> anyhow::Result<()> {
        let resolver = resolver()?;
        let result = resolver.select_version(&packument(), &VersionConstraint::parse("^3.0.0")?);
        assert!(result.is_err());
        Ok(())
    }
}
