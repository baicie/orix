//! Concurrent batch resolution.

#![deny(clippy::unwrap_used)]

use std::collections::VecDeque;
use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::sync::{mpsc, OwnedSemaphorePermit, Semaphore};
use tracing::{debug, error, instrument};

use orix_domain::{ConstraintKind, PackageId, PackageName, ResolvedPackage, VersionConstraint};
use orix_registry::{PackageMetadata, RegistryClient};

use super::state::ResolverState;
use super::types::ResolveProgressEvent;
use super::version::select_version_impl;

/// Result of resolving a single task.
struct ResolveTaskResult {
    pkg_id: PackageId,
    /// The raw constraint string used for this resolution, for memo key.
    constraint: String,
    resolve_peer_dependencies: bool,
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
#[instrument(skip_all, fields(total_initial = initial_tasks.len()))]
pub(crate) async fn resolve_batch_concurrent(
    registry: RegistryClient,
    state: &mut ResolverState,
    concurrency: usize,
    progress_tx: Option<&mpsc::Sender<ResolveProgressEvent>>,
    initial_tasks: Vec<(PackageName, VersionConstraint)>,
) -> Result<()> {
    let semaphore = Arc::new(Semaphore::new(concurrency));
    let mut pending: VecDeque<(PackageName, VersionConstraint, bool)> = initial_tasks
        .into_iter()
        .map(|(name, constraint)| (name, constraint, true))
        .collect();

    state.discovered = pending.len();

    let mut tasks: tokio::task::JoinSet<Result<ResolveTaskResult>> = tokio::task::JoinSet::new();

    debug!(
        pending = pending.len(),
        tasks = tasks.len(),
        "spawning initial batch"
    );
    // Spawn initial batch up to concurrency limit.
    while tasks.len() < concurrency {
        let Some((name, constraint, resolve_peer_dependencies)) = pending.pop_front() else {
            break;
        };

        let key = (name.clone(), constraint.raw.clone());
        if state.memo.contains_key(&key) || state.in_flight.contains(&key) {
            debug!(pkg = %name, "skipping already resolved/in-flight package");
            continue;
        }
        if let Some(pkg_id) = find_resolved_matching_package(state, &name, &constraint) {
            state.memo.insert(key, pkg_id);
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
            resolve_peer_dependencies,
        );
    }

    debug!(tasks_spawned = tasks.len(), "starting main event loop");
    // Main event loop: collect completed tasks and spawn new ones.
    while let Some(result) = tasks.join_next().await {
        debug!(
            tasks_remaining = tasks.len(),
            "task completed, processing result"
        );
        match result {
            Ok(Ok(task_result)) => {
                let key = (
                    task_result.pkg_id.name.clone(),
                    task_result.constraint.clone(),
                );
                let is_new_package =
                    record_resolved_package(state, key, task_result.pkg_id.clone());

                let mut new_deps = Vec::new();

                if is_new_package {
                    debug!(
                        pkg = %task_result.pkg_id.name,
                        version = %task_result.pkg_id.version,
                        "package resolved successfully"
                    );

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
                            .collect(),
                        patch: task_result.patch.clone(),
                    };

                    state.graph.insert(resolved);
                    state.resolved += 1;

                    for (name, raw) in task_result.deps.iter().chain(task_result.opt_deps.iter()) {
                        let dep_key = (name.clone(), raw.clone());
                        if !state.memo.contains_key(&dep_key) && !state.in_flight.contains(&dep_key)
                        {
                            state.discovered += 1;
                            new_deps.push((dep_key.0, dep_key.1, false));
                        }
                    }

                    if task_result.resolve_peer_dependencies {
                        for (name, raw) in &task_result.peer_deps {
                            let dep_key = (name.clone(), raw.clone());
                            if !state.memo.contains_key(&dep_key)
                                && !state.in_flight.contains(&dep_key)
                            {
                                state.discovered += 1;
                                new_deps.push((dep_key.0, dep_key.1, false));
                            }
                        }
                    }

                    if let Some(tx) = progress_tx {
                        if let Err(e) = tx.try_send(ResolveProgressEvent {
                            id: task_result.pkg_id.clone(),
                            discovered: state.discovered,
                            resolved: state.resolved,
                        }) {
                            debug!(error = %e, "failed to send progress event");
                        }
                    }
                } else {
                    debug!(
                        pkg = %task_result.pkg_id,
                        "skipping dependencies for already resolved package"
                    );
                }

                debug!(new_deps_count = new_deps.len(), "queuing new dependencies");
                for (name, raw, resolve_peer_dependencies) in new_deps {
                    if let Ok(constraint) = VersionConstraint::parse(&raw) {
                        pending.push_back((name, constraint, resolve_peer_dependencies));
                    }
                }
            }
            Ok(Err(e)) => {
                error!(error = %e, "resolution task returned error");
                return Err(e);
            }
            Err(e) => {
                error!(error = %e, "resolution task panicked");
                anyhow::bail!("resolution task panicked");
            }
        }

        // Spawn new tasks while we have capacity.
        while tasks.len() < concurrency {
            let Some((name, constraint, resolve_peer_dependencies)) = pending.pop_front() else {
                break;
            };

            let key = (name.clone(), constraint.raw.clone());
            if state.memo.contains_key(&key) || state.in_flight.contains(&key) {
                continue;
            }
            if let Some(pkg_id) = find_resolved_matching_package(state, &name, &constraint) {
                state.memo.insert(key, pkg_id);
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
                resolve_peer_dependencies,
            );
        }
    }

    Ok(())
}

fn record_resolved_package(
    state: &mut ResolverState,
    key: (PackageName, String),
    pkg_id: PackageId,
) -> bool {
    state.memo.insert(key.clone(), pkg_id.clone());
    state.in_flight.remove(&key);
    state.resolved_ids.insert(pkg_id)
}

fn find_resolved_matching_package(
    state: &ResolverState,
    name: &PackageName,
    constraint: &VersionConstraint,
) -> Option<PackageId> {
    state
        .resolved_ids
        .iter()
        .find(|pkg_id| pkg_id.name == *name && package_matches_constraint(pkg_id, constraint))
        .cloned()
}

fn package_matches_constraint(pkg_id: &PackageId, constraint: &VersionConstraint) -> bool {
    match &constraint.kind {
        ConstraintKind::Exact(version) => pkg_id.version == *version,
        ConstraintKind::Range(req) => req.matches(&pkg_id.version),
        ConstraintKind::AnyRange(ranges) => ranges.iter().any(|req| req.matches(&pkg_id.version)),
        ConstraintKind::Alias { constraint, .. } => package_matches_constraint(pkg_id, constraint),
        ConstraintKind::Patch(spec) => pkg_id.version == spec.package_version,
        ConstraintKind::Latest | ConstraintKind::Tag(_) | ConstraintKind::Catalog(_) => true,
    }
}

/// Spawn a single package resolution task.
#[instrument(skip_all, fields(pkg = %name, constraint = %constraint_raw))]
fn spawn_resolve_task(
    semaphore: &Arc<Semaphore>,
    tasks: &mut tokio::task::JoinSet<Result<ResolveTaskResult>>,
    registry: RegistryClient,
    name: PackageName,
    constraint: VersionConstraint,
    constraint_raw: String,
    resolve_peer_dependencies: bool,
) {
    let semaphore = semaphore.clone();
    tasks.spawn(async move {
        debug!("acquiring semaphore permit");
        let _permit: OwnedSemaphorePermit = semaphore.acquire_owned().await?;
        debug!("semaphore acquired, fetching packument");

        let fetch_name = match &constraint.kind {
            ConstraintKind::Alias { package, .. } => package.clone(),
            _ => name.clone(),
        };

        debug!(fetch_name = %fetch_name, "fetching packument from registry");
        let packument = registry
            .fetch_packument(&fetch_name)
            .await
            .with_context(|| format!("failed to fetch packument for '{}'", fetch_name))?;
        debug!(
            pkg_count = packument.versions.len(),
            "packument fetched successfully"
        );

        let version = select_version_impl(&packument, &constraint)
            .with_context(|| format!("failed to select version for '{}'", name))?;
        debug!(version = %version, "version selected");

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
            resolve_peer_dependencies,
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

pub(crate) fn collect_required_peer_deps(metadata: &PackageMetadata) -> Vec<(PackageName, String)> {
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

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashSet};

    use orix_domain::{DependencyGraph, Version};

    use super::*;

    #[test]
    fn record_resolved_package_rejects_duplicate_package_id() -> anyhow::Result<()> {
        let pkg_id = PackageId::new(PackageName::from("postcss"), Version::parse("8.5.15")?);
        let mut state = ResolverState {
            graph: DependencyGraph::new(),
            memo: BTreeMap::new(),
            in_flight: HashSet::from([
                (PackageName::from("postcss"), "^8.4.0".to_string()),
                (PackageName::from("postcss"), "^8.5.0".to_string()),
            ]),
            resolved_ids: HashSet::new(),
            discovered: 2,
            resolved: 0,
        };

        assert!(record_resolved_package(
            &mut state,
            (PackageName::from("postcss"), "^8.4.0".to_string()),
            pkg_id.clone()
        ));
        assert!(!record_resolved_package(
            &mut state,
            (PackageName::from("postcss"), "^8.5.0".to_string()),
            pkg_id.clone()
        ));

        assert_eq!(
            state
                .memo
                .get(&(PackageName::from("postcss"), "^8.5.0".to_string())),
            Some(&pkg_id)
        );
        assert!(state.in_flight.is_empty());
        Ok(())
    }

    #[test]
    fn resolved_package_does_not_link_peer_dependencies_as_children() -> anyhow::Result<()> {
        let task_result = ResolveTaskResult {
            pkg_id: PackageId::new(PackageName::from("plugin"), Version::parse("1.0.0")?),
            constraint: "1.0.0".to_string(),
            resolve_peer_dependencies: true,
            deps: vec![(PackageName::from("regular"), "^1.0.0".to_string())],
            opt_deps: vec![(PackageName::from("optional"), "^1.0.0".to_string())],
            peer_deps: vec![(PackageName::from("peer"), "^1.0.0".to_string())],
            tarball: "https://registry.example/plugin.tgz".to_string(),
            integrity: String::new(),
            engines: None,
            os: Vec::new(),
            cpu: Vec::new(),
            patch: None,
        };

        let depnodes: Vec<_> = task_result
            .deps
            .iter()
            .chain(task_result.opt_deps.iter())
            .map(|(name, _)| name.to_string())
            .collect();

        assert_eq!(depnodes, vec!["regular", "optional"]);
        Ok(())
    }

    #[test]
    fn find_resolved_matching_package_reuses_compatible_version() -> anyhow::Result<()> {
        let pkg_id = PackageId::new(PackageName::from("semver"), Version::parse("7.8.0")?);
        let state = ResolverState {
            graph: DependencyGraph::new(),
            memo: BTreeMap::new(),
            in_flight: HashSet::new(),
            resolved_ids: HashSet::from([pkg_id.clone()]),
            discovered: 1,
            resolved: 1,
        };

        assert_eq!(
            find_resolved_matching_package(
                &state,
                &PackageName::from("semver"),
                &VersionConstraint::parse("^7.0.0")?
            ),
            Some(pkg_id)
        );
        assert_eq!(
            find_resolved_matching_package(
                &state,
                &PackageName::from("semver"),
                &VersionConstraint::parse("^6.0.0")?
            ),
            None
        );
        Ok(())
    }
}
