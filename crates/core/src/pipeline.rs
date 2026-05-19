//! Install pipeline orchestration.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tracing::{debug, info, info_span, trace};

use orix_config::{Config, ConfigOverrides};
use orix_fetcher::{FetchEvent, Fetcher, TarballCache};
use orix_linker::{LinkReport, Linker};
use orix_lockfile::{resolve_from_lockfile_packages, Lockfile, PnpmLockfile};
use orix_manifest::Manifest;
use orix_resolver::Resolver;
use orix_store::Store;
use orix_workspace::{detect_workspace_cycles, Workspace};

use crate::reporter::{InstallEvent, InstallPhase, LockfileStatus};
use crate::script::{LifecycleEvent, ScriptRunner};

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
    /// Resolve phase duration in milliseconds (None if skipped via fast path).
    pub resolve_ms: Option<u64>,
    /// Fetch phase duration in milliseconds.
    pub fetch_ms: Option<u64>,
    /// Link phase duration in milliseconds.
    pub link_ms: Option<u64>,
    /// Lockfile phase duration in milliseconds.
    pub lockfile_ms: Option<u64>,
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
    tracing::trace!(event = ?event, "emit install event");

    if let Some(sender) = tx {
        if let Err(err) = sender.try_send(event) {
            tracing::debug!(error = ?err, "failed to send install progress event");
        }
    }
}

/// Send a link failure event and return it as an `anyhow::Error`.
fn link_error(tx: &Option<mpsc::Sender<InstallEvent>>, msg: String) -> anyhow::Error {
    tracing::error!(error = %msg, "link failed");
    send_event(
        tx,
        InstallEvent::Failed {
            phase: Some(InstallPhase::Link),
            message: msg,
            hint: Some("Check file permissions and disk space.".to_string()),
        },
    );
    anyhow::anyhow!("link failed")
}

/// Run a single lifecycle event for the project root, sending progress events.
async fn run_project_lifecycle(
    event: LifecycleEvent,
    manifest: &Manifest,
    config: &Config,
    project_root: &Path,
    progress_tx: &Option<mpsc::Sender<InstallEvent>>,
) {
    send_event(
        progress_tx,
        InstallEvent::ScriptsPhaseStarted {
            event: event.script_name().to_string(),
        },
    );

    let runner = ScriptRunner::new(
        config.clone(),
        manifest.clone(),
        project_root.to_path_buf(),
        None,
    );

    let result = runner
        .run_lifecycle(
            event,
            &orix_domain::PackageId::new(
                orix_domain::PackageName::from(""),
                #[allow(clippy::expect_used, clippy::unwrap_used)]
                orix_domain::Version::parse("0.0.0")
                    .expect("hardcoded semver 0.0.0 should always parse"),
            ),
        )
        .await;

    match result {
        Ok(()) => {
            send_event(
                progress_tx,
                InstallEvent::ScriptFinished {
                    name: event.script_name().to_string(),
                    duration_ms: 0,
                    exit_code: Some(0),
                },
            );
        }
        Err(crate::script::ScriptError::Disabled) => {
            send_event(
                progress_tx,
                InstallEvent::ScriptsPhaseSkipped {
                    reason: "scripts disabled by --ignore-scripts".to_string(),
                },
            );
        }
        Err(crate::script::ScriptError::MissingScript(..)) => {
            // Script not defined — skip silently.
        }
        Err(crate::script::ScriptError::Failed { name, code }) => {
            send_event(
                progress_tx,
                InstallEvent::ScriptFinished {
                    name,
                    duration_ms: 0,
                    exit_code: code,
                },
            );
        }
        Err(crate::script::ScriptError::Terminated { name }) => {
            send_event(
                progress_tx,
                InstallEvent::ScriptFinished {
                    name,
                    duration_ms: 0,
                    exit_code: None,
                },
            );
        }
        Err(crate::script::ScriptError::Spawn { name, .. }) => {
            send_event(
                progress_tx,
                InstallEvent::ScriptFinished {
                    name,
                    duration_ms: 0,
                    exit_code: Some(-1),
                },
            );
        }
    }
}

/// Top-level install orchestration.
pub async fn install(project_root: &Path, opts: &InstallOpts) -> Result<InstallReport> {
    let _span = info_span!("install", root = %project_root.display()).entered();
    let start = Instant::now();

    debug!(
        frozen_lockfile = opts.frozen_lockfile,
        offline = opts.offline,
        force = opts.force,
        ignore_scripts = opts.ignore_scripts,
        concurrency = opts.concurrency,
        registry_override = opts.registry.as_deref().unwrap_or("<config>"),
        store_override = %opts
            .store_dir
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "<config>".to_string()),
        cache_override = %opts
            .cache_dir
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "<config>".to_string()),
        "install options"
    );

    let config = Config::load_with_overrides(
        project_root,
        &ConfigOverrides {
            registry: opts.registry.clone(),
            store_dir: opts.store_dir.clone(),
            cache_dir: opts.cache_dir.clone(),
            ignore_scripts: Some(opts.ignore_scripts),
            allow_scripts: None,
        },
    )
    .with_context(|| "failed to load configuration")?;

    trace!(
        registry = %config.registry,
        store_dir = %config.store_dir.display(),
        cache_dir = %config.cache_dir.display(),
        node_modules_dir = %config.node_modules_dir().display(),
        lockfile_path = %config.lockfile_path().display(),
        authenticated = config.auth_token.is_some(),
        "configuration resolved"
    );

    if opts.frozen_lockfile && !config.lockfile_path().exists() {
        anyhow::bail!(
            "No lockfile found at {}. Run orix install without --frozen-lockfile first.",
            config.lockfile_path().display()
        );
    }

    let manifest = Manifest::read(&project_root.join("package.json"))
        .with_context(|| "failed to read package.json")?;
    let direct_dependency_count = manifest.dependencies.len() + manifest.dev_dependencies.len();

    trace!(
        dependencies = manifest.dependencies.len(),
        dev_dependencies = manifest.dev_dependencies.len(),
        optional_dependencies = manifest.optional_dependencies.len(),
        "manifest loaded"
    );

    // Phase 1: Run preinstall lifecycle (before resolution)
    run_project_lifecycle(
        LifecycleEvent::Preinstall,
        &manifest,
        &config,
        project_root,
        &opts.progress_tx,
    )
    .await;

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

    // Phase duration tracking
    let mut resolve_instant: Option<Instant> = None;
    let mut lockfile_ms: Option<u64> = None;

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
                let fetcher =
                    Fetcher::new(tarball_cache, store.clone(), project_root.to_path_buf())
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
                use std::collections::HashSet;
                let direct_deps: HashSet<String> = manifest
                    .dependencies
                    .keys()
                    .chain(manifest.dev_dependencies.keys())
                    .cloned()
                    .collect();
                let graph_hash = graph.graph_hash();
                let link_report = if linker.is_layout_valid(&graph_hash)
                    && linker
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
                        skipped: Some("layout valid".to_string()),
                    }
                } else {
                    let t2 = Instant::now();
                    linker
                        .unlink()
                        .with_context(|| "failed to clean old node_modules")?;
                    let report = linker.link_graph(
                        &graph,
                        &direct_deps,
                        workspace.as_ref(),
                        &graph.graph_hash(),
                    );
                    match report {
                        Ok(r) => {
                            debug!(target: "orix", "link (unlink+link_graph): {:?}", t2.elapsed());
                            r
                        }
                        Err(e) => return Err(link_error(&opts.progress_tx, e.to_string())),
                    }
                };

                if let Some(ref ws) = workspace {
                    for ws_pkg in &ws.packages {
                        let nm_dir = ws_pkg.abs_path.join("node_modules");
                        let pkg_linker = Linker::new(store.clone(), nm_dir.clone());

                        use std::collections::HashSet;
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
                            if let Err(e) = pkg_linker.unlink() {
                                return Err(link_error(
                                    &opts.progress_tx,
                                    format!(
                                        "failed to clean old node_modules for {}: {}",
                                        ws_pkg.manifest.name.as_deref().unwrap_or("?"),
                                        e
                                    ),
                                ));
                            }
                            let report = pkg_linker.link_graph(
                                &graph,
                                &pkg_deps,
                                workspace.as_ref(),
                                &graph.graph_hash(),
                            );
                            if let Err(e) = report {
                                return Err(link_error(
                                    &opts.progress_tx,
                                    format!(
                                        "failed to link packages for {}: {}",
                                        ws_pkg.manifest.name.as_deref().unwrap_or("?"),
                                        e
                                    ),
                                ));
                            }
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
                    resolve_ms: None,
                    fetch_ms: None,
                    link_ms: None,
                    lockfile_ms: None,
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
        resolve_instant = Some(Instant::now());
        send_event(
            &opts.progress_tx,
            InstallEvent::PhaseStarted {
                phase: InstallPhase::Resolve,
            },
        );

        let (resolve_progress_tx, mut resolve_progress_rx) =
            mpsc::channel::<orix_resolver::ResolveProgressEvent>(4096);
        let install_progress_tx = opts.progress_tx.clone();
        let resolve_progress_forwarder = tokio::spawn(async move {
            while let Some(event) = resolve_progress_rx.recv().await {
                send_event(
                    &install_progress_tx,
                    InstallEvent::ResolveProgress {
                        done: event.resolved,
                        total: event.discovered,
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
            .with_concurrency(config.concurrency)
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
            .with_concurrency(config.concurrency)
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

    let resolve_ms: Option<u64> = resolve_instant.map(|i| i.elapsed().as_millis() as u64);

    trace!(
        packages = graph.len(),
        resolve_ms = resolve_ms,
        "dependency graph resolved"
    );

    let store = Store::open(config.store_dir.clone()).with_context(|| "failed to open store")?;
    let tarball_cache = TarballCache::new(config.cache_dir.clone());
    let fetcher = Fetcher::new(tarball_cache, store.clone(), project_root.to_path_buf())
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
    let fetch_instant = Instant::now();

    let total_to_fetch = graph.len();
    send_event(
        &opts.progress_tx,
        InstallEvent::FetchProgress {
            done: 0,
            total: total_to_fetch,
            package: None,
        },
    );

    let (fetch_progress_tx, mut fetch_progress_rx) = mpsc::channel(8192);
    let install_progress_tx = opts.progress_tx.clone();
    let fetch_total = total_to_fetch;
    let fetch_progress_forwarder = tokio::spawn(async move {
        let mut fetched_count: usize = 0;
        while let Some(event) = fetch_progress_rx.recv().await {
            match event {
                FetchEvent::PackageFetched(package) => {
                    fetched_count += 1;
                    send_event(
                        &install_progress_tx,
                        InstallEvent::FetchProgress {
                            done: fetched_count,
                            total: fetch_total,
                            package: None,
                        },
                    );
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
    let fetch_ms: Option<u64> = Some(fetch_instant.elapsed().as_millis() as u64);

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
    let link_instant = Instant::now();
    use std::collections::HashSet;
    let direct_deps: HashSet<String> = manifest
        .dependencies
        .keys()
        .chain(manifest.dev_dependencies.keys())
        .cloned()
        .collect();

    let graph_hash = graph.graph_hash();
    let linker = Linker::new(store.clone(), config.node_modules_dir());
    let layout_is_valid = linker.is_layout_valid(&graph_hash)
        && linker
            .validate_layout(&direct_deps)
            .with_context(|| "failed to validate existing node_modules layout")?
            .is_ok();
    let link_report = if layout_is_valid {
        LinkReport {
            hardlinked_files: 0,
            copied_files: 0,
            symlinks_created: 0,
            bytes_saved: 0,
            skipped: Some("node_modules layout already valid".to_string()),
        }
    } else {
        if let Err(e) = linker.unlink() {
            return Err(link_error(
                &opts.progress_tx,
                format!("failed to clean old node_modules: {e}"),
            ));
        }

        let link_report = linker.link_graph(&graph, &direct_deps, workspace.as_ref(), &graph_hash);
        match link_report {
            Ok(r) => r,
            Err(e) => return Err(link_error(&opts.progress_tx, e.to_string())),
        }
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
            let layout_is_valid = pkg_linker.is_layout_valid(&graph_hash)
                && pkg_linker
                    .validate_layout(&pkg_deps)
                    .with_context(|| {
                        format!(
                            "failed to validate existing node_modules layout for {}",
                            ws_pkg.manifest.name.as_deref().unwrap_or("?")
                        )
                    })?
                    .is_ok();
            if layout_is_valid {
                continue;
            }

            if let Err(e) = pkg_linker.unlink() {
                return Err(link_error(
                    &opts.progress_tx,
                    format!(
                        "failed to clean old node_modules for {}: {}",
                        ws_pkg.manifest.name.as_deref().unwrap_or("?"),
                        e
                    ),
                ));
            }

            if let Err(e) =
                pkg_linker.link_graph(&graph, &pkg_deps, workspace.as_ref(), &graph_hash)
            {
                return Err(link_error(
                    &opts.progress_tx,
                    format!(
                        "failed to link packages for {}: {}",
                        ws_pkg.manifest.name.as_deref().unwrap_or("?"),
                        e
                    ),
                ));
            }
        }
    }

    let link_ms: Option<u64> = Some(link_instant.elapsed().as_millis() as u64);

    trace!(
        hardlinked_files = link_report.hardlinked_files,
        copied_files = link_report.copied_files,
        symlinks_created = link_report.symlinks_created,
        bytes_saved = link_report.bytes_saved,
        link_ms = link_ms,
        "linked dependencies"
    );

    send_event(
        &opts.progress_tx,
        InstallEvent::PhaseFinished {
            phase: InstallPhase::Link,
        },
    );

    // Phase: Run install lifecycle (after link, before lockfile write)
    run_project_lifecycle(
        LifecycleEvent::Install,
        &manifest,
        &config,
        project_root,
        &opts.progress_tx,
    )
    .await;

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
        let lockfile_instant = Instant::now();
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
        lockfile_ms = Some(lockfile_instant.elapsed().as_millis() as u64);
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

    // Phase: Run postinstall and prepare lifecycle (after lockfile, before final validation)
    run_project_lifecycle(
        LifecycleEvent::Postinstall,
        &manifest,
        &config,
        project_root,
        &opts.progress_tx,
    )
    .await;

    // Initial install: also run prepare
    let is_initial_install = old_lockfile.is_none();
    if is_initial_install {
        run_project_lifecycle(
            LifecycleEvent::Prepare,
            &manifest,
            &config,
            project_root,
            &opts.progress_tx,
        )
        .await;
    }

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
        resolve_ms,
        fetch_ms,
        link_ms,
        lockfile_ms,
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

    let total_to_fetch = missing.len();
    let install_progress_tx = progress_tx.clone();
    let (fetch_progress_tx, mut fetch_progress_rx) = mpsc::channel(8192);
    let fetch_progress_forwarder = tokio::spawn(async move {
        let mut fetched_count: usize = 0;
        while let Some(event) = fetch_progress_rx.recv().await {
            match event {
                FetchEvent::PackageFetched(package) => {
                    fetched_count += 1;
                    send_event(
                        &install_progress_tx,
                        InstallEvent::FetchProgress {
                            done: fetched_count,
                            total: total_to_fetch,
                            package: None,
                        },
                    );
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
    use std::collections::HashSet;
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

/// Report from an import operation.
#[derive(Debug, Clone)]
pub struct ImportReport {
    /// Number of packages imported.
    pub packages_imported: usize,
    /// Number of warnings generated during import.
    pub warnings: usize,
}

/// Report from an export operation.
#[derive(Debug, Clone)]
pub struct ExportReport {
    /// Number of packages exported.
    pub packages_exported: usize,
}

/// Import a pnpm-lock.yaml file and convert it to orix-lock.yaml.
pub fn import_pnpm_lockfile(source_path: &Path, project_root: &Path) -> Result<ImportReport> {
    let pnpm_lockfile = PnpmLockfile::read(source_path)
        .map_err(|e| anyhow::anyhow!("failed to read pnpm-lock.yaml: {}", e))?;

    if !pnpm_lockfile.is_supported() {
        let version = pnpm_lockfile.version();
        anyhow::bail!("unsupported pnpm lockfile version: {:?}", version);
    }

    let mut warnings = 0;

    if pnpm_lockfile.importers.is_empty() {
        eprintln!(
            "warning: pnpm-lock.yaml has no importers section; the lockfile may be empty or corrupted"
        );
        warnings += 1;
    }

    let orix_lockfile = pnpm_lockfile.into_orix_lockfile();
    let packages_imported = orix_lockfile.packages.len();
    let output_path = project_root.join("orix-lock.yaml");

    orix_lockfile
        .write(&output_path)
        .with_context(|| "failed to write orix-lock.yaml")?;

    info!(
        packages = packages_imported,
        importers = orix_lockfile.importers.len(),
        "imported pnpm-lock.yaml"
    );

    Ok(ImportReport {
        packages_imported,
        warnings,
    })
}

/// Export orix-lock.yaml to pnpm-lock.yaml format.
pub fn export_pnpm_lockfile(project_root: &Path, output_path: &Path) -> Result<ExportReport> {
    let lockfile_path = project_root.join("orix-lock.yaml");
    let orix_lockfile =
        Lockfile::read(&lockfile_path).with_context(|| "failed to read orix-lock.yaml")?;

    let packages_exported = orix_lockfile.packages.len();

    // Re-serialize through orix YAML (same format, just outputs to a different path).
    let yaml = serde_yaml::to_string(&orix_lockfile).context("failed to serialize lockfile")?;
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(output_path, yaml)?;

    info!(
        packages = packages_exported,
        importers = orix_lockfile.importers.len(),
        "exported pnpm-lock.yaml"
    );

    Ok(ExportReport { packages_exported })
}

/// Deploy a package from the workspace to an output directory.
///
/// This extracts a workspace package's production-ready bundle without publishing
/// to a registry.
pub async fn deploy(
    project_root: &Path,
    filter: &str,
    output_dir: &Path,
    opts: &DeployOpts,
) -> Result<DeployReport> {
    use orix_workspace::Workspace;

    let _span = info_span!("deploy", filter = %filter, output = %output_dir.display());

    // 1. Discover workspace.
    let workspace = Workspace::discover(project_root.to_path_buf())
        .with_context(|| "failed to discover workspace")?;

    // 2. Find target package(s) by filter.
    let targets: Vec<_> = workspace
        .packages
        .iter()
        .filter(|pkg| {
            let name_match = pkg
                .manifest
                .name
                .as_deref()
                .map(|n| n == filter)
                .unwrap_or(false);
            let path_match =
                glob::glob(&project_root.join(&pkg.relative_path).display().to_string())
                    .ok()
                    .and_then(|g| g.into_iter().next())
                    .is_some();
            name_match || path_match
        })
        .collect();

    if targets.is_empty() {
        anyhow::bail!("no package found matching filter '{}' in workspace", filter);
    }
    if targets.len() > 1 {
        anyhow::bail!(
            "filter '{}' matches {} packages; specify a unique package name or path",
            filter,
            targets.len()
        );
    }
    let target = &targets[0];
    let target_manifest = &target.manifest;

    // 3. Read lockfile.
    let lockfile_path = project_root.join("orix-lock.yaml");
    let lockfile = if lockfile_path.exists() {
        Lockfile::read(&lockfile_path).with_context(|| "failed to read lockfile")?
    } else {
        Lockfile::empty()
    };

    // 4. Compute production dependency closure.
    let importer_key = target.relative_path.display().to_string();
    let mut prod_deps: Vec<String> = target_manifest.dependencies.keys().cloned().collect();

    if !opts.prod {
        prod_deps.extend(target_manifest.dev_dependencies.keys().cloned());
    }

    // Collect all packages in the closure (transitive deps).
    let mut closure: Vec<String> = Vec::new();
    for dep in &prod_deps {
        // Look up in lockfile importers.
        if let Some(importer) = lockfile.importers.get(&importer_key) {
            if let Some(resolved) = importer.dependencies.get(dep) {
                closure.push(format!("{}/{}@{}", dep, dep, resolved.version));
            }
        }
    }

    // For MVP, we handle direct dependencies. Full transitive closure would
    // require walking the lockfile graph recursively.
    let mut packages_deployed = 0;
    let mut files_copied = 0;

    // 5. Create output directory.
    std::fs::create_dir_all(output_dir).with_context(|| {
        format!(
            "failed to create output directory: {}",
            output_dir.display()
        )
    })?;

    // 6. Materialize package files.
    let target_src = target.abs_path.as_path();
    let files_field = target_manifest.files.as_slice();
    let target_files = collect_package_files(target_src, files_field);

    for file_path in &target_files {
        let rel_path = file_path.strip_prefix(target_src).unwrap_or(file_path);
        let dest = output_dir.join(rel_path);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(file_path, &dest).with_context(|| {
            format!(
                "failed to copy {} to {}",
                file_path.display(),
                dest.display()
            )
        })?;
        files_copied += 1;
    }
    packages_deployed += 1;

    // 7. Create minimal node_modules.
    let node_modules = output_dir.join("node_modules");
    std::fs::create_dir_all(&node_modules)?;

    let store_path = project_root.join(".orix-store");
    for dep_key in &closure {
        let src_pkg = store_path.join(dep_key.replace('@', "_at_").replace('/', "_sl_"));
        if src_pkg.exists() {
            let dest_link = node_modules.join(dep_key.split('@').next().unwrap_or(dep_key));
            if !dest_link.exists() {
                std::fs::hard_link(&src_pkg, &dest_link)
                    .or_else(|_| std::fs::copy(&src_pkg, &dest_link).map(|_| ()))
                    .ok();
            }
            packages_deployed += 1;
        }
    }

    // 8. Copy subset of lockfile.
    let deploy_lockfile_path = output_dir.join("orix-lock.yaml");
    let deploy_lockfile = lockfile.clone();
    let yaml =
        serde_yaml::to_string(&deploy_lockfile).context("failed to serialize deploy lockfile")?;
    std::fs::write(&deploy_lockfile_path, yaml)?;

    // 9. Copy package.json.
    let pkg_json_src = target.abs_path.join("package.json");
    let pkg_json_dest = output_dir.join("package.json");
    std::fs::copy(&pkg_json_src, &pkg_json_dest)
        .with_context(|| format!("failed to copy {}", pkg_json_src.display()))?;

    // 10. Run deploy hooks if enabled.
    if opts.hooks {
        if let Some(script) = target_manifest.scripts.get("predeploy") {
            if let Err(e) = run_hook_script(&target.abs_path, "predeploy", script).await {
                eprintln!("warning: predeploy hook failed: {}", e);
            }
        }
        if let Some(script) = target_manifest.scripts.get("postdeploy") {
            if let Err(e) = run_hook_script(&target.abs_path, "postdeploy", script).await {
                eprintln!("warning: postdeploy hook failed: {}", e);
            }
        }
    }

    info!(packages_deployed, files_copied, "deploy complete");

    Ok(DeployReport {
        packages_deployed,
        files_copied,
    })
}

/// Collect files to include in a deployed package.
///
/// If `files_field` is non-empty, only include those paths.
/// Otherwise, include all files except excluded patterns.
fn collect_package_files(pkg_dir: &Path, files_field: &[String]) -> Vec<PathBuf> {
    let mut result = Vec::new();

    if !files_field.is_empty() {
        // Whitelist mode: only include listed files.
        for pattern in files_field {
            for entry in glob::glob(&pkg_dir.join(pattern).display().to_string())
                .into_iter()
                .flatten()
                .flatten()
            {
                if entry.is_file() {
                    result.push(entry);
                }
            }
        }
        return result;
    }

    // Default: include all files except excluded patterns.
    let exclude_patterns = [
        ".git",
        "node_modules",
        ".pnpm",
        "target",
        ".DS_Store",
        "*.test.js",
        "*.spec.js",
        "test-fixtures",
        "__tests__",
        "coverage",
        ".nyc_output",
    ];

    fn walk_dir(dir: &Path, output: &mut Vec<PathBuf>, exclude: &[&str]) -> std::io::Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

            let is_excluded = exclude.iter().any(|pat| {
                if let Some(stripped) = pat.strip_prefix('*') {
                    name.ends_with(stripped)
                } else {
                    name == *pat
                }
            });

            if is_excluded {
                continue;
            }

            if path.is_dir() {
                walk_dir(&path, output, exclude)?;
            } else {
                output.push(path);
            }
        }
        Ok(())
    }

    walk_dir(pkg_dir, &mut result, &exclude_patterns).ok();
    result
}

/// Run a deploy hook script.
async fn run_hook_script(pkg_dir: &Path, name: &str, script: &str) -> anyhow::Result<()> {
    let status = tokio::process::Command::new("sh")
        .arg("-c")
        .arg(script)
        .current_dir(pkg_dir)
        .status()
        .await
        .with_context(|| format!("failed to run {} hook", name))?;

    if !status.success() {
        anyhow::bail!("{} hook exited with code {:?}", name, status.code());
    }
    Ok(())
}

/// Options for deploy operation.
#[derive(Debug, Clone)]
pub struct DeployOpts {
    /// Only include production dependencies.
    pub prod: bool,
    /// Use frozen lockfile.
    pub frozen_lockfile: bool,
    /// Run deploy hooks.
    pub hooks: bool,
}

/// Report from a deploy operation.
#[derive(Debug, Clone)]
pub struct DeployReport {
    /// Number of packages included in the deployment.
    pub packages_deployed: usize,
    /// Number of files copied.
    pub files_copied: usize,
}
