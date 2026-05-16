//! Install pipeline orchestration.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
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
use orix_config::ConfigOverrides;

/// Options for the install command.
#[derive(Debug, Clone, Default)]
pub struct InstallOpts {
    /// Registry URL override from CLI.
    pub registry: Option<String>,
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
    /// Packages changed since the previous lockfile.
    pub changed: Vec<String>,
    /// Importers whose specifiers changed.
    pub importers_changed: Vec<String>,
}

/// Top-level install orchestration.
pub async fn install(project_root: &Path, opts: &InstallOpts) -> Result<InstallReport> {
    let _span = info_span!("install", root = %project_root.display());
    let start = Instant::now();

    let config = Config::load_with_overrides(
        project_root,
        &ConfigOverrides {
            registry: opts.registry.clone(),
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
        // Normal mode: resolve from registry, with workspace awareness
        let mut resolver = Resolver::new(config.registry.clone());

        // When running from workspace root, collect all manifests and resolve together
        let graph = if let Some(ref ws) = workspace {
            let mut manifests: Vec<&Manifest> = vec![&manifest];
            manifests.extend(ws.packages.iter().map(|p| &p.manifest));
            info!(manifests = manifests.len(), "resolving workspace manifests together");
            resolver
                .resolve_manifests(&manifests)
                .await
                .with_context(|| "failed to resolve workspace dependencies")?
        } else {
            resolver
                .resolve_manifest(&manifest)
                .await
                .with_context(|| "failed to resolve dependencies")?
        };

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
            changed: diff.changed.clone(),
            importers_changed: diff.importers_changed.clone(),
        };

        updated_lockfile
            .write(&config.lockfile_path())
            .with_context(|| "failed to write lockfile")?;

        if !diff.added.is_empty() || !diff.removed.is_empty() || !diff.changed.is_empty() {
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

    let direct_deps: HashSet<String> = manifest
        .dependencies
        .keys()
        .chain(manifest.dev_dependencies.keys())
        .cloned()
        .collect();

    let linker = Linker::new(store, config.node_modules_dir());
    linker
        .unlink()
        .with_context(|| "failed to clean old node_modules")?;
    let link_report = linker
        .link_graph(&graph, &direct_deps)
        .with_context(|| "failed to link packages")?;

    // Link workspace local packages directly to their source directories
    // (bypass .pnpm/ for workspace:* references)
    if let Some(ref ws) = workspace {
        for pkg in &ws.packages {
            if let Some(ref name) = pkg.manifest.name {
                let local_linked = linker.link_local_package(name, &pkg.abs_path)?;
                if local_linked > 0 {
                    info!(package = %name, "linked workspace local package");
                }
            }
        }
    }

    let layout_report = linker
        .validate_layout(&direct_deps)
        .with_context(|| "failed to validate node_modules layout")?;
    if !layout_report.is_ok() {
        anyhow::bail!(
            "node_modules layout validation failed: {}",
            layout_report.broken.join("; ")
        );
    }

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

/// Return the resolved store path for this project.
pub fn store_path(project_root: &Path) -> Result<PathBuf> {
    let config = Config::load(project_root).with_context(|| "failed to load configuration")?;
    Ok(config.store_dir)
}

/// Prune packages from the store that are not referenced by this project's lockfile.
pub fn store_prune(project_root: &Path, dry_run: bool) -> Result<orix_store::PruneReport> {
    let config = Config::load(project_root).with_context(|| "failed to load configuration")?;
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
    let config = Config::load(project_root).with_context(|| "failed to load configuration")?;
    let store = Store::open(config.store_dir).with_context(|| "failed to open store")?;
    store.verify()
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

    // Prune orphaned packages from the lockfile after reinstall
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
