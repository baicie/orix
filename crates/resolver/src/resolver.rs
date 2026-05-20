//! Dependency resolution engine.

#![deny(clippy::unwrap_used)]

use std::collections::{BTreeMap, HashSet, VecDeque};
use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::sync::{mpsc, OwnedSemaphorePermit, Semaphore};

use orix_domain::{
    ConstraintKind, DependencyGraph, PackageId, PackageName, ResolvedPackage, Version,
    VersionConstraint,
};
use orix_manifest::Manifest;
use orix_registry::{PackageMetadata, Packument, RegistryClient};
use orix_workspace::{Workspace, WorkspaceSpec};
use url::Url;

/// Default concurrency for packument resolution.
const DEFAULT_RESOLVE_CONCURRENCY: usize = 10;

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

        orix_domain::ConstraintKind::AnyRange(ranges) => {
            let mut candidates: Vec<_> = packument
                .versions
                .keys()
                .filter_map(|v| Version::parse(v).ok())
                .filter(|v| ranges.iter().any(|range| range.matches(v)))
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

        orix_domain::ConstraintKind::Patch(spec) => Ok(spec.package_version.clone()),

        orix_domain::ConstraintKind::Catalog(_) => {
            anyhow::bail!(
                "catalog reference '{}' was not expanded — workspace catalog not available",
                constraint.raw
            );
        }

        orix_domain::ConstraintKind::Alias { constraint, .. } => {
            select_version_impl(packument, constraint)
        }
    }
}

/// Result of resolving a single task.
struct ResolveTaskResult {
    pkg_id: PackageId,
    /// The raw constraint string used for this resolution, for memo key.
    constraint: String,
    deps: Vec<(PackageName, String)>,
    opt_deps: Vec<(PackageName, String)>,
    peer_deps: Vec<(PackageName, String)>,
    tarball: String,
    integrity: String,
    engines: Option<String>,
    os: Vec<String>,
    cpu: Vec<String>,
    patch: Option<orix_domain::PatchSpec>,
}

/// Concurrently resolve a batch of packages using a bounded work queue.
///
/// Uses `tokio::task::JoinSet` to manage concurrent tasks without `drain_filter`.
async fn resolve_batch_concurrent(
    registry: RegistryClient,
    state: &mut ResolverState,
    concurrency: usize,
    progress_tx: Option<&mpsc::Sender<ResolveProgressEvent>>,
    initial_tasks: Vec<(PackageName, VersionConstraint)>,
) -> Result<()> {
    let semaphore = Arc::new(Semaphore::new(concurrency));
    let mut pending: VecDeque<(PackageName, VersionConstraint)> = VecDeque::from(initial_tasks);

    state.discovered = pending.len();

    let mut tasks: tokio::task::JoinSet<Result<ResolveTaskResult>> = tokio::task::JoinSet::new();

    // Spawn initial batch up to concurrency limit.
    while tasks.len() < concurrency {
        let Some((name, constraint)) = pending.pop_front() else {
            break;
        };

        let key = (name.clone(), constraint.raw.clone());
        if state.memo.contains_key(&key) || state.in_flight.contains(&key) {
            continue;
        }
        state.in_flight.insert(key.clone());

        spawn_resolve_task(
            &semaphore,
            &mut tasks,
            registry.clone(),
            name,
            constraint,
            key.1.clone(),
        );
    }

    // Main event loop: collect completed tasks and spawn new ones.
    while let Some(result) = tasks.join_next().await {
        match result {
            Ok(Ok(task_result)) => {
                // Write to memo so repeated constraints on the same package are deduplicated.
                let key = (
                    task_result.pkg_id.name.clone(),
                    task_result.constraint.clone(),
                );
                state.memo.insert(key.clone(), task_result.pkg_id.clone());
                state.in_flight.remove(&key);

                let mut new_deps = Vec::new();

                let resolved = ResolvedPackage {
                    id: task_result.pkg_id.clone(),
                    integrity: task_result.integrity.clone(),
                    tarball: task_result.tarball.clone(),
                    dependencies: task_result.deps.clone(),
                    dev_dependencies: Vec::new(),
                    optional_dependencies: task_result.opt_deps.clone(),
                    peer_dependencies: task_result.peer_deps.clone(),
                    engines: task_result.engines.clone(),
                    os: task_result.os.clone(),
                    cpu: task_result.cpu.clone(),
                    depnodes: task_result
                        .deps
                        .iter()
                        .chain(task_result.opt_deps.iter())
                        .map(|(n, _)| n.to_string())
                        .chain(task_result.peer_deps.iter().map(|(n, _)| n.to_string()))
                        .collect(),
                    patch: task_result.patch.clone(),
                };

                state.graph.insert(resolved);
                state.resolved += 1;

                for (name, raw) in task_result
                    .deps
                    .iter()
                    .chain(task_result.opt_deps.iter())
                    .chain(task_result.peer_deps.iter())
                {
                    let dep_key = (name.clone(), raw.clone());
                    if !state.memo.contains_key(&dep_key) && !state.in_flight.contains(&dep_key) {
                        state.discovered += 1;
                        new_deps.push(dep_key);
                    }
                }

                if let Some(tx) = progress_tx {
                    let _ = tx.try_send(ResolveProgressEvent {
                        id: task_result.pkg_id.clone(),
                        discovered: state.discovered,
                        resolved: state.resolved,
                    });
                }

                for (name, raw) in new_deps {
                    if let Ok(constraint) = VersionConstraint::parse(&raw) {
                        pending.push_back((name, constraint));
                    }
                }
            }
            Ok(Err(e)) => return Err(e),
            Err(_) => {
                anyhow::bail!("resolution task panicked");
            }
        }

        // Spawn new tasks while we have capacity.
        while tasks.len() < concurrency {
            let Some((name, constraint)) = pending.pop_front() else {
                break;
            };

            let key = (name.clone(), constraint.raw.clone());
            if state.memo.contains_key(&key) || state.in_flight.contains(&key) {
                continue;
            }
            state.in_flight.insert(key.clone());

            spawn_resolve_task(
                &semaphore,
                &mut tasks,
                registry.clone(),
                name,
                constraint,
                key.1.clone(),
            );
        }
    }

    Ok(())
}

/// Spawn a single package resolution task.
fn spawn_resolve_task(
    semaphore: &Arc<Semaphore>,
    tasks: &mut tokio::task::JoinSet<Result<ResolveTaskResult>>,
    registry: RegistryClient,
    name: PackageName,
    constraint: VersionConstraint,
    constraint_raw: String,
) {
    let semaphore = semaphore.clone();
    tasks.spawn(async move {
        let _permit: OwnedSemaphorePermit = semaphore.acquire_owned().await?;

        let fetch_name = match &constraint.kind {
            ConstraintKind::Alias { package, .. } => package.clone(),
            _ => name.clone(),
        };

        let packument = registry
            .fetch_packument(&fetch_name)
            .await
            .with_context(|| format!("failed to fetch packument for '{}'", fetch_name))?;

        let version = select_version_impl(&packument, &constraint)
            .with_context(|| format!("failed to select version for '{}'", name))?;

        let metadata = packument
            .versions
            .get(&version.to_string())
            .with_context(|| format!("version {} not found in packument", version))?;

        let pkg_id = PackageId::new(name.clone(), version.clone());
        let dist = metadata.dist.as_ref().with_context(|| {
            format!(
                "package {}@{} has no dist info — may be unpublished or unavailable",
                name, version
            )
        })?;
        let tarball = dist.tarball.clone();
        let integrity = dist
            .integrity
            .clone()
            .or(dist.shasum.clone())
            .unwrap_or_default();

        let deps: Vec<(PackageName, String)> = metadata
            .dependencies
            .iter()
            .map(|(k, v)| (PackageName::from(k.as_str()), v.clone()))
            .collect();
        let opt_deps: Vec<(PackageName, String)> = metadata
            .optional_dependencies
            .iter()
            .map(|(k, v)| (PackageName::from(k.as_str()), v.clone()))
            .collect();
        let peer_deps = collect_required_peer_deps(metadata);

        let patch = match &constraint.kind {
            ConstraintKind::Patch(spec) => Some(spec.clone()),
            _ => None,
        };

        Ok(ResolveTaskResult {
            pkg_id,
            constraint: constraint_raw,
            deps,
            opt_deps,
            peer_deps,
            tarball,
            integrity,
            engines: metadata.engines.as_ref().and_then(|e| e.node.clone()),
            os: metadata.os.clone(),
            cpu: metadata.cpu.clone(),
            patch,
        })
    });
}

fn collect_required_peer_deps(metadata: &PackageMetadata) -> Vec<(PackageName, String)> {
    metadata
        .peer_dependencies
        .iter()
        .filter(|(name, _)| {
            !metadata
                .peer_dependencies_meta
                .get(*name)
                .is_some_and(|meta| meta.optional)
        })
        .map(|(k, v)| (PackageName::from(k.as_str()), v.clone()))
        .collect()
}

/// Mutable state used during concurrent resolution.
struct ResolverState {
    graph: DependencyGraph,
    memo: BTreeMap<(PackageName, String), PackageId>,
    in_flight: HashSet<(PackageName, String)>,
    discovered: usize,
    resolved: usize,
}

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
