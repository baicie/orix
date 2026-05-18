//! Install pipeline orchestration.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tracing::{debug, info, info_span};

use orix_config::{Config, ConfigOverrides};
use orix_fetcher::{FetchEvent, Fetcher, TarballCache};
use orix_linker::{LinkReport, Linker};
use orix_lockfile::{resolve_from_lockfile_packages, Lockfile};
use orix_manifest::Manifest;
use orix_resolver::Resolver;
use orix_store::Store;
use orix_workspace::{detect_workspace_cycles, Workspace};

use crate::reporter::{InstallEvent, InstallPhase, LockfileStatus};

/// Options for the install command.
#[derive(Debug, Clone, Default)]
pub struct InstallOpts {
    /// Registry URL override from CLI.
    pub registry: Option<String>,
    /// Global store directory override from CLI.
    pub store_dir: Option<PathBuf>,
    /// Local tarball cache directory override from CLI.
    pub cache_dir: Option<PathBuf>,
    /// Require a lockfile and fail if it doesn't match package.json.
    pub frozen_lockfile: bool,
    /// Only use locally cached packages.
    pub offline: bool,
    /// Re-fetch all packages regardless of cache.
    pub force: bool,
    /// Skip running lifecycle scripts.
    pub ignore_scripts: bool,
    /// Number of concurrent download tasks.
    pub concurrency: usize,
    /// Channel to send progress events to the CLI renderer.
    #[doc(hidden)]
    pub progress_tx: Option<mpsc::Sender<InstallEvent>>,
}

/// Report from an install operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallReport {
    /// Registry URL used for resolution and downloads.
    pub registry: String,
    /// Number of direct dependencies declared by the root manifest.
    pub direct_dependencies: usize,
    /// Number of packages added.
    pub packages_added: usize,
    /// Fetch operation report.
    pub fetch_report: orix_fetcher::FetchReport,
    /// Link operation report.
    pub link_report: LinkReport,
    /// Lockfile diff (if computed).
    pub lockfile_diff: Option<LockfileDiffReport>,
    /// Whether the lockfile changed during this install.
    pub lockfile_changed: bool,
    /// Wall-clock time in seconds.
    pub duration_secs: f64,
}

/// Report from a remove operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoveReport {
    /// Names of removed packages.
    pub removed_packages: Vec<String>,
    /// Install report for the updated graph.
    pub install_report: InstallReport,
}

/// Report from cleaning the tarball cache.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheCleanReport {
    /// Cache directory that was cleaned.
    pub path: PathBuf,
    /// Whether the cache directory existed before cleaning.
    pub existed: bool,
    /// Best-effort number of bytes removed.
    pub bytes_reclaimed: u64,
}

/// Summary of lockfile changes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockfileDiffReport {
    /// Packages added since the previous lockfile.
    pub added: Vec<String>,
    /// Packages removed since the previous lockfile.
    pub removed: Vec<String>,
    /// Packages changed since the previous lockfile.
    pub changed: Vec<String>,
    /// Importers whose specifiers changed.
    pub importers_changed: Vec<String>,
}

fn send_event(tx: &Option<mpsc::Sender<InstallEvent>>, event: InstallEvent) {
    if let Some(sender) = tx {
        let _ = sender.try_send(event);
    }
}

/// Top-level install orchestration.
pub async fn install(project_root: &Path, opts: &InstallOpts) -> Result<InstallReport> {
    let _span = info_span!("install", root = %project_root.display());
    let start = Instant::now();

    let config = Config::load_with_overrides(
        project_root,
        &ConfigOverrides {
            registry: opts.registry.clone(),
            store_dir: opts.store_dir.clone(),
            cache_dir: opts.cache_dir.clone(),
        },
    )
    .with_context(|| "failed to load configuration")?;

    if opts.frozen_lockfile && !config.lockfile_path().exists() {
        anyhow::bail!(
            "No lockfile found at {}. Run orix install without --frozen-lockfile first.",
            config.lockfile_path().display()
        );
    }

    let manifest = Manifest::read(&project_root.join("package.json"))
        .with_context(|| "failed to read package.json")?;
    let direct_dependency_count = manifest.dependencies.len() + manifest.dev_dependencies.len();

    send_event(
        &opts.progress_tx,
        InstallEvent::Started {
            command: "orix install".to_string(),
        },
    );
    send_event(
        &opts.progress_tx,
        InstallEvent::RegistrySelected {
            url: config.registry.to_string(),
            authenticated: config.auth_token.is_some(),
        },
    );
    send_event(
        &opts.progress_tx,
        InstallEvent::DirectPackages {
            count: direct_dependency_count,
            names: manifest
                .dependencies
                .keys()
                .chain(manifest.dev_dependencies.keys())
                .cloned()
                .collect(),
        },
    );

    let workspace = match Workspace::discover(project_root.to_path_buf()) {
        Ok(ws) if !ws.packages.is_empty() => Some(ws),
        _ => None,
    };

    if let Some(ref ws) = workspace {
        info!(packages = ws.packages.len(), "discovered workspace");
        let cycle = detect_workspace_cycles(ws);
        if !cycle.is_empty() {
            send_event(
                &opts.progress_tx,
                InstallEvent::Failed {
                    phase: Some(InstallPhase::Resolve),
                    message: format!("circular workspace dependency: {}", cycle.join(" -> ")),
                    hint: Some("Check pnpm-workspace.yaml for circular references.".to_string()),
                },
            );
            anyhow::bail!(
                "circular workspace dependency detected: {}",
                cycle.join(" -> ")
            );
        }
    }

    let old_lockfile = if config.lockfile_path().exists() {
        Some(Lockfile::read(&config.lockfile_path()).with_context(|| "failed to read lockfile")?)
    } else {
        None
    };

    // Fast path: if lockfile exists and manifest unchanged, skip resolver/fetch entirely.
    // Only apply when network is not forced and we're not in frozen mode.
    if !opts.frozen_lockfile && !opts.force {
        if let Some(ref lf) = old_lockfile {
            if lf.validate(&manifest, ".").is_ok() {
                debug!(target: "orix", "FAST PATH triggered");
                let graph = resolve_from_lockfile_packages(&lf.packages);
                let pkg_count = graph.len();
                info!(packages = pkg_count, "resolved from lockfile (fast path)");

                send_event(
                    &opts.progress_tx,
                    InstallEvent::PhaseStarted {
                        phase: InstallPhase::Resolve,
                    },
                );
                send_event(
                    &opts.progress_tx,
                    InstallEvent::Resolved {
                        direct: direct_dependency_count,
                        total: pkg_count,
                        added: 0,
                        removed: 0,
                    },
                );
                send_event(
                    &opts.progress_tx,
                    InstallEvent::PhaseFinished {
                        phase: InstallPhase::Resolve,
                    },
                );

                // Only fetch packages missing from store.
                send_event(
                    &opts.progress_tx,
                    InstallEvent::PhaseStarted {
                        phase: InstallPhase::Fetch,
                    },
                );
                let store = Store::open(config.store_dir.clone())
                    .with_context(|| "failed to open store")?;
                let tarball_cache = TarballCache::new(config.cache_dir.clone());
                let fetcher = Fetcher::new(tarball_cache, store.clone())
                    .with_offline(opts.offline)
                    .with_force(false);
                let concurrency = if opts.concurrency == 0 {
                    config.concurrency
                } else {
                    opts.concurrency
                };
                let (graph, fetch_report) = fetch_only_missing(
                    &store,
                    &fetcher,
                    &graph,
                    concurrency,
                    opts.progress_tx.clone(),
                )
                .await
                .with_context(|| "failed to fetch missing packages")?;

                info!(
                    success = fetch_report.success,
                    failures = fetch_report.failures.len(),
                    "fetched packages"
                );

                if !fetch_report.failures.is_empty() {
                    send_event(
                        &opts.progress_tx,
                        InstallEvent::Failed {
                            phase: Some(InstallPhase::Fetch),
                            message: format!(
                                "failed to fetch packages:\n  {}",
                                fetch_report.failures.join("\n  ")
                            ),
                            hint: Some("Check network connection or try --offline.".to_string()),
                        },
                    );
                    anyhow::bail!(
                        "failed to fetch packages:\n  {}",
                        fetch_report.failures.join("\n  ")
                    );
                }

                send_event(
                    &opts.progress_tx,
                    InstallEvent::PhaseFinished {
                        phase: InstallPhase::Fetch,
                    },
                );

                send_event(
                    &opts.progress_tx,
                    InstallEvent::PhaseStarted {
                        phase: InstallPhase::Link,
                    },
                );
                let linker = Linker::new(store.clone(), config.node_modules_dir());
                let direct_deps: HashSet<String> = manifest
                    .dependencies
                    .keys()
                    .chain(manifest.dev_dependencies.keys())
                    .cloned()
                    .collect();
                let link_report = if linker
                    .validate_layout(&direct_deps)
                    .map(|r| r.is_ok())
                    .unwrap_or(false)
                {
                    debug!(target: "orix", "layout valid, skipping unlink+link");
                    LinkReport {
                        hardlinked_files: 0,
                        copied_files: 0,
                        symlinks_created: 0,
                        bytes_saved: 0,
                    }
                } else {
                    let t2 = Instant::now();
                    linker
                        .unlink()
                        .with_context(|| "failed to clean old node_modules")?;
                    let report = linker
                        .link_graph(&graph, &direct_deps, workspace.as_ref())
                        .with_context(|| "failed to link packages")?;
                    debug!(target: "orix", "link (unlink+link_graph): {:?}", t2.elapsed());
                    report
                };

                if let Some(ref ws) = workspace {
                    for ws_pkg in &ws.packages {
                        let nm_dir = ws_pkg.abs_path.join("node_modules");
                        let pkg_linker = Linker::new(store.clone(), nm_dir.clone());

                        let pkg_deps: HashSet<String> = ws_pkg
                            .manifest
                            .dependencies
                            .keys()
                            .chain(ws_pkg.manifest.dev_dependencies.keys())
                            .chain(ws_pkg.manifest.optional_dependencies.keys())
                            .cloned()
                            .collect();

                        if pkg_linker
                            .validate_layout(&pkg_deps)
                            .map(|r| r.is_ok())
                            .unwrap_or(false)
                        {
                            debug!(
                                target: "orix",
                                pkg = %ws_pkg.manifest.name.as_deref().unwrap_or("?"),
                                "workspace pkg layout valid, skipping"
                            );
                        } else {
                            pkg_linker.unlink().with_context(|| {
                                format!(
                                    "failed to clean old node_modules for {}",
                                    ws_pkg.manifest.name.as_deref().unwrap_or("?")
                                )
                            })?;
                            pkg_linker
                                .link_graph(&graph, &pkg_deps, workspace.as_ref())
                                .with_context(|| {
                                    format!(
                                        "failed to link packages for {}",
                                        ws_pkg.manifest.name.as_deref().unwrap_or("?")
                                    )
                                })?;
                        }
                    }
                }

                send_event(
                    &opts.progress_tx,
                    InstallEvent::PhaseFinished {
                        phase: InstallPhase::Link,
                    },
                );

                let layout_report = linker
                    .validate_layout(&direct_deps)
                    .with_context(|| "failed to validate node_modules layout")?;
                if !layout_report.is_ok() {
                    anyhow::bail!(
                        "node_modules layout validation failed: {}",
                        layout_report.broken.join("; ")
                    );
                }

                let base_lockfile = lf.clone();
                let updated_lockfile = base_lockfile.update(&manifest, &graph, ".");
                let diff = Lockfile::diff(&base_lockfile, &updated_lockfile);
                let lockfile_changed =
                    !diff.added.is_empty() || !diff.removed.is_empty() || !diff.changed.is_empty();

                if lockfile_changed {
                    send_event(
                        &opts.progress_tx,
                        InstallEvent::PhaseStarted {
                            phase: InstallPhase::Lockfile,
                        },
                    );
                    updated_lockfile
                        .write(&config.lockfile_path())
                        .with_context(|| "failed to write lockfile")?;
                    info!(
                        added = diff.added.len(),
                        removed = diff.removed.len(),
                        changed = diff.changed.len(),
                        "lockfile updated"
                    );
                }

                let lockfile_status = if lockfile_changed {
                    LockfileStatus::Written
                } else {
                    LockfileStatus::Unchanged
                };
                send_event(
                    &opts.progress_tx,
                    InstallEvent::Lockfile {
                        status: lockfile_status,
                    },
                );

                let duration = start.elapsed();
                info!(
                    duration_ms = duration.as_millis(),
                    "install complete (fast path)"
                );
                send_event(
                    &opts.progress_tx,
                    InstallEvent::Finished {
                        installed: graph.len(),
                        duration,
                    },
                );

                return Ok(InstallReport {
                    registry: config.registry.to_string(),
                    direct_dependencies: direct_dependency_count,
                    packages_added: graph.len(),
                    fetch_report,
                    link_report,
                    lockfile_diff: None,
                    lockfile_changed,
                    duration_secs: duration.as_secs_f64(),
                });
            }
        }
    }

    let graph = if opts.frozen_lockfile {
        if let Some(ref lf) = old_lockfile {
            lf.validate_frozen(&manifest, ".")
                .with_context(|| "frozen lockfile validation failed")?;

            let g = resolve_from_lockfile_packages(&lf.packages);
            info!(packages = g.len(), "resolved from lockfile (frozen mode)");
            g
        } else {
            send_event(
                &opts.progress_tx,
                InstallEvent::Failed {
                    phase: Some(InstallPhase::Resolve),
                    message: "frozen lockfile mode requires an existing lockfile".to_string(),
                    hint: Some("Run `orix install` without --frozen-lockfile first.".to_string()),
                },
            );
            anyhow::bail!("frozen lockfile mode requires an existing lockfile");
        }
    } else {
        send_event(
            &opts.progress_tx,
            InstallEvent::PhaseStarted {
                phase: InstallPhase::Resolve,
            },
        );

        let (resolve_progress_tx, mut resolve_progress_rx) =
            mpsc::channel::<orix_resolver::ResolveProgressEvent>(128);
        let install_progress_tx = opts.progress_tx.clone();
        let resolve_progress_forwarder = tokio::spawn(async move {
            while let Some(event) = resolve_progress_rx.recv().await {
                send_event(
                    &install_progress_tx,
                    InstallEvent::ResolveProgress {
                        done: event.index,
                        total: event.total,
                        package: Some(event.id.to_string()),
                    },
                );
            }
        });

        let graph = if let Some(ref ws) = workspace {
            let mut resolver = if let Some(ref token) = config.auth_token {
                info!(registry = %config.registry, "using authenticated registry");
                Resolver::with_auth(config.registry.clone(), token)
            } else {
                Resolver::new(config.registry.clone())
            }
            .with_progress(resolve_progress_tx);

            let mut merged = orix_domain::DependencyGraph::new();
            for pkg in std::iter::once(&manifest).chain(ws.packages.iter().map(|p| &p.manifest)) {
                let sub = resolver
                    .resolve_manifest_with_workspace(pkg, Some(ws))
                    .await
                    .with_context(|| "failed to resolve workspace dependencies")?;
                merged.merge(sub);
            }
            merged
        } else {
            let mut resolver = if let Some(ref token) = config.auth_token {
                info!(registry = %config.registry, "using authenticated registry");
                Resolver::with_auth(config.registry.clone(), token)
            } else {
                Resolver::new(config.registry.clone())
            }
            .with_progress(resolve_progress_tx);

            resolver
                .resolve_manifest(&manifest)
                .await
                .with_context(|| "failed to resolve dependencies")?
        };

        let _ = resolve_progress_forwarder.await;

        let old_count = old_lockfile
            .as_ref()
            .map(|lf| lf.packages.len())
            .unwrap_or(0);
        let added = graph.len().saturating_sub(old_count);
        let removed = old_count.saturating_sub(graph.len());

        send_event(
            &opts.progress_tx,
            InstallEvent::Resolved {
                direct: direct_dependency_count,
                total: graph.len(),
                added,
                removed,
            },
        );

        graph
    };

    let store = Store::open(config.store_dir.clone()).with_context(|| "failed to open store")?;
    let tarball_cache = TarballCache::new(config.cache_dir.clone());
    let fetcher = Fetcher::new(tarball_cache, store.clone())
        .with_offline(opts.offline)
        .with_force(opts.force);
    let concurrency = if opts.concurrency == 0 {
        config.concurrency
    } else {
        opts.concurrency
    };

    send_event(
        &opts.progress_tx,
        InstallEvent::PhaseStarted {
            phase: InstallPhase::Fetch,
        },
    );

    let total_to_fetch = graph.len();
    send_event(
        &opts.progress_tx,
        InstallEvent::FetchProgress {
            done: 0,
            total: total_to_fetch,
            package: None,
        },
    );

    let (fetch_progress_tx, mut fetch_progress_rx) = mpsc::channel(128);
    let install_progress_tx = opts.progress_tx.clone();
    let fetch_progress_forwarder = tokio::spawn(async move {
        while let Some(event) = fetch_progress_rx.recv().await {
            match event {
                FetchEvent::PackageFetched(package) => {
                    send_event(
                        &install_progress_tx,
                        InstallEvent::PackageFetched {
                            name: package,
                            version: None,
                            cached: false,
                        },
                    );
                }
                FetchEvent::PackageFailed(failure) => {
                    send_event(
                        &install_progress_tx,
                        InstallEvent::Failed {
                            phase: Some(InstallPhase::Fetch),
                            message: format!("failed to fetch package: {}", failure),
                            hint: Some("Check network connection or try --offline.".to_string()),
                        },
                    );
                }
            }
        }
    });

    let fetch_report = fetcher
        .fetch_all(&graph, concurrency, Some(fetch_progress_tx))
        .await
        .with_context(|| "failed to fetch packages")?;
    let _ = fetch_progress_forwarder.await;

    send_event(
        &opts.progress_tx,
        InstallEvent::FetchProgress {
            done: fetch_report.success,
            total: total_to_fetch,
            package: None,
        },
    );

    info!(
        success = fetch_report.success,
        failures = fetch_report.failures.len(),
        "fetched packages"
    );

    if !fetch_report.failures.is_empty() {
        send_event(
            &opts.progress_tx,
            InstallEvent::Failed {
                phase: Some(InstallPhase::Fetch),
                message: format!(
                    "failed to fetch packages:\n  {}",
                    fetch_report.failures.join("\n  ")
                ),
                hint: Some("Check network connection or try --offline.".to_string()),
            },
        );
        anyhow::bail!(
            "failed to fetch packages:\n  {}",
            fetch_report.failures.join("\n  ")
        );
    }

    send_event(
        &opts.progress_tx,
        InstallEvent::PhaseFinished {
            phase: InstallPhase::Fetch,
        },
    );

    send_event(
        &opts.progress_tx,
        InstallEvent::PhaseStarted {
            phase: InstallPhase::Link,
        },
    );
    let direct_deps: HashSet<String> = manifest
        .dependencies
        .keys()
        .chain(manifest.dev_dependencies.keys())
        .cloned()
        .collect();

    let linker = Linker::new(store.clone(), config.node_modules_dir());
    linker
        .unlink()
        .with_context(|| "failed to clean old node_modules")?;

    let link_report = linker
        .link_graph(&graph, &direct_deps, workspace.as_ref())
        .with_context(|| "failed to link packages")?;

    if let Some(ref ws) = workspace {
        for ws_pkg in &ws.packages {
            let nm_dir = ws_pkg.abs_path.join("node_modules");
            let pkg_linker = Linker::new(store.clone(), nm_dir.clone());
            pkg_linker.unlink().with_context(|| {
                format!(
                    "failed to clean old node_modules for {}",
                    ws_pkg.manifest.name.as_deref().unwrap_or("?")
                )
            })?;

            let pkg_deps: HashSet<String> = ws_pkg
                .manifest
                .dependencies
                .keys()
                .chain(ws_pkg.manifest.dev_dependencies.keys())
                .chain(ws_pkg.manifest.optional_dependencies.keys())
                .cloned()
                .collect();

            pkg_linker
                .link_graph(&graph, &pkg_deps, workspace.as_ref())
                .with_context(|| {
                    format!(
                        "failed to link packages for {}",
                        ws_pkg.manifest.name.as_deref().unwrap_or("?")
                    )
                })?;
        }
    }

    send_event(
        &opts.progress_tx,
        InstallEvent::PhaseFinished {
            phase: InstallPhase::Link,
        },
    );

    let layout_report = linker
        .validate_layout(&direct_deps)
        .with_context(|| "failed to validate node_modules layout")?;
    if !layout_report.is_ok() {
        anyhow::bail!(
            "node_modules layout validation failed: {}",
            layout_report.broken.join("; ")
        );
    }

    let mut lockfile_changed = false;
    let lockfile_diff: Option<LockfileDiffReport> = if !opts.frozen_lockfile {
        send_event(
            &opts.progress_tx,
            InstallEvent::PhaseStarted {
                phase: InstallPhase::Lockfile,
            },
        );
        let base_lockfile = old_lockfile
            .as_ref()
            .cloned()
            .unwrap_or_else(Lockfile::empty);
        let updated_lockfile = base_lockfile.update(&manifest, &graph, ".");

        let diff = Lockfile::diff(&base_lockfile, &updated_lockfile);
        let diff_report = LockfileDiffReport {
            added: diff.added.clone(),
            removed: diff.removed.clone(),
            changed: diff.changed.clone(),
            importers_changed: diff.importers_changed.clone(),
        };

        updated_lockfile
            .write(&config.lockfile_path())
            .with_context(|| "failed to write lockfile")?;

        lockfile_changed =
            !diff.added.is_empty() || !diff.removed.is_empty() || !diff.changed.is_empty();
        if lockfile_changed {
            info!(
                added = diff.added.len(),
                removed = diff.removed.len(),
                changed = diff.changed.len(),
                "lockfile updated"
            );
        } else {
            info!("lockfile unchanged");
        }

        Some(diff_report)
    } else {
        None
    };

    let lockfile_status = if !opts.frozen_lockfile {
        if lockfile_changed {
            LockfileStatus::Written
        } else {
            LockfileStatus::Unchanged
        }
    } else {
        LockfileStatus::Skipped
    };
    send_event(
        &opts.progress_tx,
        InstallEvent::Lockfile {
            status: lockfile_status,
        },
    );

    let duration = start.elapsed();
    info!(duration_ms = duration.as_millis(), "install complete");
    send_event(
        &opts.progress_tx,
        InstallEvent::Finished {
            installed: graph.len(),
            duration,
        },
    );

    Ok(InstallReport {
        registry: config.registry.to_string(),
        direct_dependencies: direct_dependency_count,
        packages_added: graph.len(),
        fetch_report,
        link_report,
        lockfile_diff,
        lockfile_changed,
        duration_secs: duration.as_secs_f64(),
    })
}

/// Fetch only packages not already in the store.
async fn fetch_only_missing(
    store: &Store,
    fetcher: &Fetcher,
    graph: &orix_domain::DependencyGraph,
    concurrency: usize,
    progress_tx: Option<mpsc::Sender<InstallEvent>>,
) -> Result<(orix_domain::DependencyGraph, orix_fetcher::FetchReport)> {
    let mut missing = orix_domain::DependencyGraph::new();
    for pkg in graph.packages() {
        if !store.contains(&pkg.id) {
            missing.insert(pkg.clone());
        }
    }

    if missing.is_empty() {
        debug!(target: "orix", "all {} packages already in store, skipping fetch", graph.len());
        return Ok((graph.clone(), orix_fetcher::FetchReport::default()));
    }

    debug!(target: "orix", "found {} packages in store, fetching {} missing", graph.len() - missing.len(), missing.len());

    let (fetch_progress_tx, mut fetch_progress_rx) = mpsc::channel(128);
    let install_progress_tx = progress_tx.clone();
    let fetch_progress_forwarder = tokio::spawn(async move {
        while let Some(event) = fetch_progress_rx.recv().await {
            match event {
                FetchEvent::PackageFetched(package) => {
                    send_event(
                        &install_progress_tx,
                        InstallEvent::PackageFetched {
                            name: package,
                            version: None,
                            cached: false,
                        },
                    );
                }
                FetchEvent::PackageFailed(failure) => {
                    send_event(
                        &install_progress_tx,
                        InstallEvent::Failed {
                            phase: Some(InstallPhase::Fetch),
                            message: format!("failed to fetch package: {}", failure),
                            hint: Some("Check network connection or try --offline.".to_string()),
                        },
                    );
                }
            }
        }
    });

    let total_to_fetch = missing.len();
    let fetch_report = fetcher
        .fetch_all(&missing, concurrency, Some(fetch_progress_tx))
        .await
        .with_context(|| "failed to fetch packages")?;
    let _ = fetch_progress_forwarder.await;

    send_event(
        &progress_tx,
        InstallEvent::FetchProgress {
            done: fetch_report.success,
            total: total_to_fetch,
            package: None,
        },
    );

    Ok((graph.clone(), fetch_report))
}

/// Return the resolved store path for this project.
pub fn store_path(project_root: &Path) -> Result<PathBuf> {
    store_path_with_overrides(project_root, &ConfigOverrides::default())
}

/// Return the resolved store path for this project using explicit overrides.
pub fn store_path_with_overrides(
    project_root: &Path,
    overrides: &ConfigOverrides,
) -> Result<PathBuf> {
    let config = Config::load_with_overrides(project_root, overrides)
        .with_context(|| "failed to load configuration")?;
    Ok(config.store_dir)
}

/// Prune packages from the store that are not referenced by this project's lockfile.
pub fn store_prune(project_root: &Path, dry_run: bool) -> Result<orix_store::PruneReport> {
    store_prune_with_overrides(project_root, dry_run, &ConfigOverrides::default())
}

/// Prune packages from the configured store using explicit overrides.
pub fn store_prune_with_overrides(
    project_root: &Path,
    dry_run: bool,
    overrides: &ConfigOverrides,
) -> Result<orix_store::PruneReport> {
    let config = Config::load_with_overrides(project_root, overrides)
        .with_context(|| "failed to load configuration")?;
    let lockfile_path = config.lockfile_path();
    if !lockfile_path.exists() {
        anyhow::bail!(
            "No lockfile found at {}. Run orix install before pruning the store.",
            lockfile_path.display()
        );
    }

    let lockfile = Lockfile::read(&lockfile_path).with_context(|| "failed to read lockfile")?;
    let referenced: HashSet<_> = lockfile.package_ids()?.into_iter().collect();
    let store = Store::open(config.store_dir).with_context(|| "failed to open store")?;
    store.prune(&referenced, dry_run, true)
}

/// Verify all packages and content-addressable files in the store.
pub fn store_verify(project_root: &Path) -> Result<orix_store::VerifyReport> {
    store_verify_with_overrides(project_root, &ConfigOverrides::default())
}

/// Verify all packages and content-addressable files in the configured store.
pub fn store_verify_with_overrides(
    project_root: &Path,
    overrides: &ConfigOverrides,
) -> Result<orix_store::VerifyReport> {
    let config = Config::load_with_overrides(project_root, overrides)
        .with_context(|| "failed to load configuration")?;
    let store = Store::open(config.store_dir).with_context(|| "failed to open store")?;
    store.verify()
}

/// Return the resolved tarball cache path for this project.
pub fn cache_path(project_root: &Path) -> Result<PathBuf> {
    cache_path_with_overrides(project_root, &ConfigOverrides::default())
}

/// Return the resolved tarball cache path for this project using explicit overrides.
pub fn cache_path_with_overrides(
    project_root: &Path,
    overrides: &ConfigOverrides,
) -> Result<PathBuf> {
    let config = Config::load_with_overrides(project_root, overrides)
        .with_context(|| "failed to load configuration")?;
    Ok(config.cache_dir)
}

/// Remove all tarballs from the configured cache directory.
pub fn cache_clean(project_root: &Path) -> Result<CacheCleanReport> {
    cache_clean_with_overrides(project_root, &ConfigOverrides::default())
}

/// Remove all tarballs from the configured cache directory using explicit overrides.
pub fn cache_clean_with_overrides(
    project_root: &Path,
    overrides: &ConfigOverrides,
) -> Result<CacheCleanReport> {
    let path = cache_path_with_overrides(project_root, overrides)?;
    let existed = path.exists();
    let bytes_reclaimed = if existed { dir_size(&path) } else { 0 };

    if existed {
        fs::remove_dir_all(&path)
            .with_context(|| format!("failed to remove cache directory {}", path.display()))?;
    }
    fs::create_dir_all(&path)
        .with_context(|| format!("failed to create cache directory {}", path.display()))?;

    Ok(CacheCleanReport {
        path,
        existed,
        bytes_reclaimed,
    })
}

fn dir_size(path: &Path) -> u64 {
    let Ok(entries) = fs::read_dir(path) else {
        return 0;
    };

    entries
        .filter_map(Result::ok)
        .map(|entry| {
            let path = entry.path();
            match entry.metadata() {
                Ok(metadata) if metadata.is_dir() => dir_size(&path),
                Ok(metadata) if metadata.is_file() => metadata.len(),
                _ => 0,
            }
        })
        .sum()
}

/// Add one or more packages to the project.
pub async fn add(
    project_root: &Path,
    packages: &[String],
    dep_type: DepType,
    opts: &InstallOpts,
) -> Result<InstallReport> {
    let manifest_path = project_root.join("package.json");
    let mut manifest =
        Manifest::read(&manifest_path).with_context(|| "failed to read package.json")?;

    for pkg_spec in packages {
        let (name, constraint) = orix_resolver::parse_package_spec(pkg_spec)
            .with_context(|| format!("invalid package spec: {}", pkg_spec))?;

        match dep_type {
            DepType::Production => {
                manifest
                    .dependencies
                    .insert(name.to_string(), constraint.raw);
            }
            DepType::Dev => {
                manifest
                    .dev_dependencies
                    .insert(name.to_string(), constraint.raw);
            }
            DepType::Peer => {
                manifest
                    .peer_dependencies
                    .insert(name.to_string(), constraint.raw);
            }
            DepType::Optional => {
                manifest
                    .optional_dependencies
                    .insert(name.to_string(), constraint.raw);
            }
        }
    }

    manifest
        .write(&manifest_path)
        .with_context(|| "failed to write package.json")?;

    install(project_root, opts).await
}

/// Dependency type for the `add` command.
#[derive(Debug, Clone, Copy)]
pub enum DepType {
    /// Production dependency (default).
    Production,
    /// Development dependency.
    Dev,
    /// Peer dependency.
    Peer,
    /// Optional dependency.
    Optional,
}

/// Remove one or more packages from the project.
pub async fn remove(
    project_root: &Path,
    packages: &[String],
    opts: &InstallOpts,
) -> Result<RemoveReport> {
    let manifest_path = project_root.join("package.json");
    let mut manifest =
        Manifest::read(&manifest_path).with_context(|| "failed to read package.json")?;

    let mut removed = Vec::new();
    for pkg_name in packages {
        if manifest.dependencies.remove(pkg_name).is_some() {
            removed.push(pkg_name.clone());
        }
        if manifest.dev_dependencies.remove(pkg_name).is_some() {
            removed.push(pkg_name.clone());
        }
        if manifest.optional_dependencies.remove(pkg_name).is_some() {
            removed.push(pkg_name.clone());
        }
    }

    manifest
        .write(&manifest_path)
        .with_context(|| "failed to write package.json")?;

    let report = install(project_root, opts).await?;

    let lockfile_path = project_root.join("orix-lock.yaml");
    if lockfile_path.exists() {
        let mut lockfile = Lockfile::read(&lockfile_path)
            .with_context(|| "failed to read lockfile after remove")?;
        let removed = lockfile.retain_only_referenced_packages();
        if removed > 0 {
            lockfile
                .write(&lockfile_path)
                .with_context(|| "failed to write cleaned lockfile")?;
        }
    }

    Ok(RemoveReport {
        removed_packages: removed,
        install_report: report,
    })
}
