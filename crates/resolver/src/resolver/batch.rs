//! Concurrent batch resolution.

#![deny(clippy::unwrap_used)]

use std::collections::VecDeque;
use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::sync::{mpsc, OwnedSemaphorePermit, Semaphore};

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
pub(crate) async fn resolve_batch_concurrent(
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
