//! Dependency resolution engine.

#![deny(clippy::unwrap_used)]

use std::collections::BTreeMap;

use anyhow::{Context, Result};

use rpnpm_domain::{
    ConstraintKind, DependencyGraph, PackageId, PackageName, ResolvedPackage, Version,
    VersionConstraint,
};
use rpnpm_lockfile::PackageLock;
use rpnpm_manifest::Manifest;
use rpnpm_registry::{Packument, RegistryClient};
use url::Url;

/// An optional dependency that was skipped due to platform mismatch.
#[derive(Debug, Clone)]
pub struct SkippedOptionalDep {
    /// The name of the skipped optional dependency.
    pub name: PackageName,
    /// Reason why the dependency was skipped.
    pub reason: String,
}

/// The dependency resolution engine.
pub struct Resolver {
    registry: RegistryClient,
    memo: BTreeMap<(PackageName, String), PackageId>,
    /// Optional dependencies that were skipped due to platform incompatibility.
    skipped_optional: Vec<SkippedOptionalDep>,
}

impl Resolver {
    /// Create a new resolver with a registry URL.
    pub fn new(registry_url: Url) -> Self {
        Self {
            registry: RegistryClient::new(registry_url),
            memo: Default::default(),
            skipped_optional: Vec::new(),
        }
    }

    /// Create a new resolver with registry authentication.
    pub fn with_auth(registry_url: Url, token: &str) -> Self {
        Self {
            registry: RegistryClient::with_auth(registry_url, token),
            memo: Default::default(),
            skipped_optional: Vec::new(),
        }
    }

    /// Resolve all dependencies from a manifest into a dependency graph.
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

        self.resolve_batch(&mut graph, to_resolve).await?;
        Ok(graph)
    }

    /// Returns the list of optional dependencies that were skipped due to platform mismatch.
    pub fn skipped_optional_deps(&self) -> &[SkippedOptionalDep] {
        &self.skipped_optional
    }

    async fn resolve_batch(
        &mut self,
        graph: &mut DependencyGraph,
        to_resolve: Vec<(PackageName, VersionConstraint)>,
    ) -> Result<()> {
        let mut pending: Vec<(PackageName, VersionConstraint)> = to_resolve;

        while let Some((name, constraint)) = pending.pop() {
            let key = (name.clone(), constraint.raw.clone());
            if self.memo.contains_key(&key) {
                continue;
            }

            let packument = self
                .registry
                .fetch_packument(&name)
                .await
                .with_context(|| format!("failed to fetch packument for '{}'", name))?;

            let version = self
                .select_version(&packument, &constraint)
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

            // Build depnodes: transitive dependencies this package declares.
            let depnodes: Vec<String> = deps
                .iter()
                .chain(opt_deps.iter())
                .map(|(n, _)| n.to_string())
                .chain(peer_deps.iter().map(|(n, _)| n.to_string()))
                .collect();

            let resolved = ResolvedPackage {
                id: pkg_id.clone(),
                integrity: integrity.clone(),
                tarball,
                dependencies: deps.clone(),
                dev_dependencies: dev_deps,
                optional_dependencies: opt_deps.clone(),
                peer_dependencies: peer_deps,
                engines: metadata.engines.as_ref().and_then(|e| e.node.clone()),
                os: metadata.os.clone(),
                cpu: metadata.cpu.clone(),
                depnodes,
            };

            graph.insert(resolved);
            self.memo.insert(key, pkg_id);

            // Queue regular and dev deps for resolution. Optional deps are included
            // for completeness; platform mismatches are handled by skipping at fetch time.
            for (n, raw) in deps.iter().chain(opt_deps.iter()) {
                if let Ok(c) = VersionConstraint::parse(raw) {
                    pending.push((n.clone(), c));
                }
            }
        }
        Ok(())
    }

    fn select_version(
        &self,
        packument: &Packument,
        constraint: &VersionConstraint,
    ) -> Result<Version> {
        match &constraint.kind {
            ConstraintKind::Exact(v) => Ok(v.clone()),

            ConstraintKind::Range(range) => {
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

            ConstraintKind::Latest => packument
                .dist_tags
                .get("latest")
                .and_then(|v| Version::parse(v).ok())
                .with_context(|| "no dist-tags.latest found in packument"),

            ConstraintKind::Tag(tag) => packument
                .dist_tags
                .get(tag)
                .and_then(|v| Version::parse(v).ok())
                .with_context(|| format!("tag '{}' not found in packument", tag)),
        }
    }
}

/// Resolve dependencies from a lockfile packages section (frozen/install-from-lock workflow).
pub fn resolve_from_lockfile_packages(
    packages: &std::collections::BTreeMap<String, PackageLock>,
) -> DependencyGraph {
    let mut graph = DependencyGraph::new();

    for (key, pkg) in packages {
        let tarball = match pkg.resolution.as_ref().and_then(|r| r.tarball.clone()) {
            Some(t) => t,
            None => continue,
        };

        let integrity = pkg.integrity.clone().unwrap_or_default();
        let key_str = key.trim_start_matches('/');
        let (name_str, ver_str) = key_str.rsplit_once('@').unwrap_or((key_str, ""));

        let name = PackageName::from(name_str);
        let version = match Version::parse(ver_str) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let pkg_id = PackageId::new(name.clone(), version);

        let deps: Vec<(PackageName, String)> = pkg
            .dependencies
            .iter()
            .map(|(k, v)| (PackageName::from(k.as_str()), v.clone()))
            .collect();
        let opt_deps: Vec<(PackageName, String)> = pkg
            .optional_dependencies
            .iter()
            .map(|(k, v)| (PackageName::from(k.as_str()), v.clone()))
            .collect();

        let depnodes: Vec<String> = deps
            .iter()
            .chain(opt_deps.iter())
            .map(|(n, _)| n.to_string())
            .collect();

        let resolved = ResolvedPackage {
            id: pkg_id.clone(),
            integrity,
            tarball,
            dependencies: deps,
            dev_dependencies: Vec::new(),
            optional_dependencies: opt_deps,
            peer_dependencies: Vec::new(),
            engines: pkg.engines.clone(),
            os: pkg.os.clone().unwrap_or_default(),
            cpu: pkg.cpu.clone().unwrap_or_default(),
            depnodes,
        };
        graph.insert(resolved);
    }

    graph
}
