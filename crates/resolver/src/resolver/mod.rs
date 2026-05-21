#![deny(clippy::unwrap_used)]

//! Dependency resolution engine.

use std::collections::{BTreeMap, HashSet};

use anyhow::{Context, Result};
use tokio::sync::mpsc;

use orix_domain::{
    DependencyGraph, PackageId, PackageName, ResolvedPackage, Version, VersionConstraint,
};
use orix_manifest::Manifest;
use orix_registry::{Packument, RegistryClient};
use orix_workspace::{Workspace, WorkspaceSpec};
use url::Url;

mod batch;
mod state;
mod types;
mod version;

pub use types::{ResolveProgressEvent, SkippedOptionalDep};

use batch::resolve_batch_concurrent;
use state::ResolverState;
use version::select_version_impl;

const DEFAULT_RESOLVE_CONCURRENCY: usize = 10;

/// The dependency resolution engine.
pub struct Resolver {
    registry: RegistryClient,
    memo: BTreeMap<(PackageName, String), PackageId>,
    skipped_optional: Vec<SkippedOptionalDep>,
    progress_tx: Option<mpsc::Sender<ResolveProgressEvent>>,
    resolve_concurrency: usize,
}

impl Resolver {
    /// Creates a new resolver with the given registry URL.
    pub fn new(registry_url: Url) -> Self {
        Self {
            registry: RegistryClient::new(registry_url),
            memo: Default::default(),
            skipped_optional: Vec::new(),
            progress_tx: None,
            resolve_concurrency: DEFAULT_RESOLVE_CONCURRENCY,
        }
    }

    /// Creates a new resolver with the given registry URL and authentication token.
    pub fn with_auth(registry_url: Url, token: &str) -> Self {
        Self {
            registry: RegistryClient::with_auth(registry_url, token),
            memo: Default::default(),
            skipped_optional: Vec::new(),
            progress_tx: None,
            resolve_concurrency: DEFAULT_RESOLVE_CONCURRENCY,
        }
    }

    /// Sets the maximum number of concurrent packument resolutions.
    pub fn with_concurrency(mut self, concurrency: usize) -> Self {
        self.resolve_concurrency = concurrency.max(1);
        self
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
                        VersionConstraint::parse(&resolved_version).unwrap_or_else(|_| {
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
        let mut state = ResolverState {
            graph: DependencyGraph::new(),
            memo: std::mem::take(&mut self.memo),
            in_flight: HashSet::new(),
            discovered: 0,
            resolved: 0,
        };

        resolve_batch_concurrent(
            self.registry.clone(),
            &mut state,
            self.resolve_concurrency,
            self.progress_tx.as_ref(),
            to_resolve,
        )
        .await?;

        // Merge resolved graph into caller's graph.
        for pkg in state.graph.packages() {
            graph.insert(pkg.clone());
        }

        self.memo = state.memo;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use orix_domain::{PackageId, PackageName, Version, VersionConstraint};
    use orix_registry::{Dist, PackageMetadata, Packument};

    use super::batch::collect_required_peer_deps;
    use super::*;

    fn resolver() -> anyhow::Result<Resolver> {
        Ok(Resolver::new(url::Url::parse(
            "https://registry.npmjs.org/",
        )?))
    }

    fn packument() -> Packument {
        let versions: HashMap<String, PackageMetadata> = [
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
                    peer_dependencies_meta: HashMap::new(),
                    engines: None,
                    os: Vec::new(),
                    cpu: Vec::new(),
                    dist: Some(Dist {
                        tarball: format!("https://registry.npmjs.org/demo/-/demo-{}.tgz", version),
                        integrity: Some(integ.to_string()),
                        shasum: None,
                    }),
                    optional: false,
                    deprecated: None,
                    bin: HashMap::new(),
                    directories: Default::default(),
                    has_shrinkwrap: false,
                    has_install_script: false,
                    bundle_dependencies: Vec::new(),
                    scripts: HashMap::new(),
                    funding: None,
                    repository: None,
                    homepage: None,
                    description: None,
                    license: None,
                    keywords: Vec::new(),
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
    fn select_version_returns_highest_matching_or_range() -> anyhow::Result<()> {
        let resolver = resolver()?;
        let selected = resolver
            .select_version(&packument(), &VersionConstraint::parse("^1.0.0 || ^2.0.0")?)?;
        assert_eq!(selected.to_string(), "2.0.0");
        Ok(())
    }

    #[test]
    fn select_version_supports_npm_alias_constraint() -> anyhow::Result<()> {
        let resolver = resolver()?;
        let selected =
            resolver.select_version(&packument(), &VersionConstraint::parse("npm:demo@^1.0.0")?)?;
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

    #[test]
    fn collect_required_peer_deps_skips_optional_peer_meta() {
        let metadata = PackageMetadata {
            name: "vite".to_string(),
            version: "8.0.13".to_string(),
            dependencies: HashMap::new(),
            dev_dependencies: HashMap::new(),
            optional_dependencies: HashMap::new(),
            peer_dependencies: HashMap::from([
                ("esbuild".to_string(), "^0.28.0".to_string()),
                ("rollup".to_string(), "^4.0.0".to_string()),
            ]),
            peer_dependencies_meta: HashMap::from([(
                "esbuild".to_string(),
                orix_registry::PeerDepMeta { optional: true },
            )]),
            engines: None,
            os: Vec::new(),
            cpu: Vec::new(),
            dist: Some(Dist {
                tarball: "https://registry.npmjs.org/vite/-/vite-8.0.13.tgz".to_string(),
                integrity: None,
                shasum: None,
            }),
            optional: false,
            deprecated: None,
            bin: HashMap::new(),
            directories: Default::default(),
            has_shrinkwrap: false,
            has_install_script: false,
            bundle_dependencies: Vec::new(),
            scripts: HashMap::new(),
            funding: None,
            repository: None,
            homepage: None,
            description: None,
            license: None,
            keywords: Vec::new(),
        };

        let peers = collect_required_peer_deps(&metadata);

        assert_eq!(
            peers,
            vec![(PackageName::from("rollup"), "^4.0.0".to_string())]
        );
    }

    #[tokio::test]
    async fn resolve_concurrency_is_configurable() -> anyhow::Result<()> {
        let resolver =
            Resolver::new(url::Url::parse("https://registry.npmjs.org/")?).with_concurrency(5);
        assert_eq!(resolver.resolve_concurrency, 5);
        Ok(())
    }

    #[tokio::test]
    async fn resolve_progress_event_has_discovered_and_resolved_fields() -> anyhow::Result<()> {
        let event = ResolveProgressEvent {
            id: PackageId::new(PackageName::from("react"), Version::parse("18.2.0")?),
            discovered: 10,
            resolved: 5,
        };
        assert_eq!(event.discovered, 10);
        assert_eq!(event.resolved, 5);
        Ok(())
    }
}
