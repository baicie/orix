//! Install pipeline orchestration.

use std::collections::HashSet;
use std::path::Path;
use std::time::Instant;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tracing::{info, info_span};

use orix_fetcher::{Fetcher, TarballCache};
use orix_linker::{LinkReport, Linker};
use orix_lockfile::Lockfile;
use orix_manifest::Manifest;
use orix_resolver::{resolve_from_lockfile_packages, Resolver};
use orix_store::Store;
use orix_workspace::Workspace;

pub use orix_config::Config;

/// Options for the install command.
#[derive(Debug, Clone, Default)]
pub struct InstallOpts {
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
}

/// Report from an install operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallReport {
    /// Number of packages added.
    pub packages_added: usize,
    /// Fetch operation report.
    pub fetch_report: orix_fetcher::FetchReport,
    /// Link operation report.
    pub link_report: LinkReport,
    /// Lockfile diff (if computed).
    pub lockfile_diff: Option<LockfileDiffReport>,
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

/// Summary of lockfile changes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockfileDiffReport {
    /// Packages added since the previous lockfile.
    pub added: Vec<String>,
    /// Packages removed since the previous lockfile.
    pub removed: Vec<String>,
}

/// Top-level install orchestration.
pub async fn install(project_root: &Path, opts: &InstallOpts) -> Result<InstallReport> {
    let _span = info_span!("install", root = %project_root.display());
    let start = Instant::now();

    let config = Config::load(project_root).with_context(|| "failed to load configuration")?;

    if opts.frozen_lockfile && !config.lockfile_path().exists() {
        anyhow::bail!(
            "No lockfile found at {}. Run orix install without --frozen-lockfile first.",
            config.lockfile_path().display()
        );
    }

    let manifest = Manifest::read(&project_root.join("package.json"))
        .with_context(|| "failed to read package.json")?;

    let workspace = match Workspace::discover(project_root.to_path_buf()) {
        Ok(ws) if !ws.packages.is_empty() => Some(ws),
        _ => None,
    };

    if let Some(ref ws) = workspace {
        info!(packages = ws.packages.len(), "discovered workspace");
    }

    let old_lockfile = if config.lockfile_path().exists() {
        Some(Lockfile::read(&config.lockfile_path()).with_context(|| "failed to read lockfile")?)
    } else {
        None
    };

    // Resolve dependency graph
    let graph = if opts.frozen_lockfile {
        // Frozen lockfile mode: verify lockfile matches manifest, use lockfile packages directly
        if let Some(ref lf) = old_lockfile {
            lf.validate_frozen(&manifest, ".")
                .with_context(|| "frozen lockfile validation failed")?;

            let g = resolve_from_lockfile_packages(&lf.packages);
            info!(packages = g.len(), "resolved from lockfile (frozen mode)");
            g
        } else {
            anyhow::bail!("frozen lockfile mode requires an existing lockfile");
        }
    } else {
        // Normal mode: resolve from registry
        let mut resolver = Resolver::new(config.registry.clone());

        if let Some(ref ws) = workspace {
            let _ = ws;
        }

        resolver
            .resolve_manifest(&manifest)
            .await
            .with_context(|| "failed to resolve dependencies")?
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

    let fetch_report = fetcher
        .fetch_all(&graph, concurrency)
        .await
        .with_context(|| "failed to fetch packages")?;

    info!(
        success = fetch_report.success,
        failures = fetch_report.failures.len(),
        "fetched packages"
    );

    // Write lockfile (unless frozen)
    let lockfile_diff: Option<LockfileDiffReport> = if !opts.frozen_lockfile {
        let base_lockfile = old_lockfile
            .as_ref()
            .cloned()
            .unwrap_or_else(Lockfile::empty);
        let updated_lockfile = base_lockfile.update(&manifest, &graph, ".");

        // Compute diff before writing so we have the old vs new comparison
        let diff = Lockfile::diff(&base_lockfile, &updated_lockfile);
        let diff_report = LockfileDiffReport {
            added: diff.added.clone(),
            removed: diff.removed.clone(),
        };

        updated_lockfile
            .write(&config.lockfile_path())
            .with_context(|| "failed to write lockfile")?;

        if !diff.added.is_empty() || !diff.removed.is_empty() {
            info!(
                added = diff.added.len(),
                removed = diff.removed.len(),
                "lockfile updated"
            );
        } else {
            info!("lockfile unchanged");
        }

        Some(diff_report)
    } else {
        None
    };

    let direct_deps: HashSet<String> = manifest
        .dependencies
        .keys()
        .chain(manifest.dev_dependencies.keys())
        .cloned()
        .collect();

    let linker = Linker::new(store, config.node_modules_dir());
    let link_report = linker
        .link_graph(&graph, &direct_deps)
        .with_context(|| "failed to link packages")?;

    let duration = start.elapsed();
    info!(duration_ms = duration.as_millis(), "install complete");

    Ok(InstallReport {
        packages_added: graph.len(),
        fetch_report,
        link_report,
        lockfile_diff,
        duration_secs: duration.as_secs_f64(),
    })
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

    Ok(RemoveReport {
        removed_packages: removed,
        install_report: report,
    })
}
