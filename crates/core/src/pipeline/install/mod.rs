//! Full install pipeline.

mod fast_path;
mod fetch_phase;
mod finish;
mod link;
mod resolve;
mod workspace_link;

pub mod streaming_pipeline;

use super::lifecycle::run_project_lifecycle;
use super::prelude::*;
use super::types::*;

use fast_path::try_install_fast_path;
use fetch_phase::fetch_install_graph;
use finish::finish_install;
use link::link_install_graph;
use orix_config::ConfigOverrides;
use resolve::resolve_install_graph;

/// Run the full install pipeline for a project root.
pub async fn install(project_root: &Path, opts: &InstallOpts) -> Result<InstallReport> {
    install_with_overrides(project_root, opts, &ConfigOverrides::default()).await
}

/// Run the full install pipeline with explicit configuration overrides.
pub async fn install_with_overrides(
    project_root: &Path,
    opts: &InstallOpts,
    overrides: &ConfigOverrides,
) -> Result<InstallReport> {
    let _span = info_span!("install", root = %project_root.display()).entered();
    let start = Instant::now();
    let setup_instant = Instant::now();

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

    let config = Config::load_with_overrides(project_root, overrides)
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
    .await?;

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

    crate::pipeline::perf::log_setup_phase(
        setup_instant.elapsed().as_millis() as u64,
        workspace.as_ref().map(|ws| ws.packages.len()),
        direct_dependency_count,
    );

    // Fast path: lockfile specifiers match package.json — reuse locked resolution.
    // Only apply when network is not forced and we're not in frozen mode.

    if let Some(ref lf) = old_lockfile {
        if let Some(report) = try_install_fast_path(
            project_root,
            opts,
            &config,
            &manifest,
            &workspace,
            lf,
            direct_dependency_count,
            start,
        )
        .await?
        {
            return Ok(report);
        }
    }

    let (graph, resolve_ms) = resolve_install_graph(
        opts,
        &config,
        &manifest,
        &workspace,
        &old_lockfile,
        direct_dependency_count,
        &opts.progress_tx,
    )
    .await?;

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

    let (fetch_report, fetch_ms) =
        fetch_install_graph(&graph, &fetcher, concurrency, &opts.progress_tx).await?;

    let (link_report, link_ms) = link_install_graph(
        &store,
        &config,
        &graph,
        &manifest,
        &workspace,
        &opts.progress_tx,
    )?;

    use std::collections::HashSet;
    let direct_deps: HashSet<String> = manifest
        .dependencies
        .keys()
        .chain(manifest.dev_dependencies.keys())
        .cloned()
        .collect();
    let linker = Linker::new(store.clone(), config.node_modules_dir());

    finish_install(
        opts,
        &config,
        &manifest,
        project_root,
        &graph,
        &linker,
        &direct_deps,
        &workspace,
        fetch_report,
        link_report,
        &old_lockfile,
        direct_dependency_count,
        start,
        resolve_ms,
        fetch_ms,
        link_ms,
        &opts.progress_tx,
    )
    .await
}

pub(super) fn workspace_importer_id(relative_path: &Path) -> String {
    let id = relative_path.to_string_lossy().replace('\\', "/");
    if id.is_empty() {
        ".".to_string()
    } else {
        id
    }
}

pub(super) fn lockfile_importer_mismatches(
    lockfile: &Lockfile,
    manifest: &Manifest,
    workspace: &Option<Workspace>,
) -> Vec<String> {
    let mut mismatches = Vec::new();

    if let Err(err) = lockfile.validate_frozen(manifest, ".") {
        mismatches.push(format!(".: {err}"));
    }

    let Some(ws) = workspace else {
        return mismatches;
    };

    for pkg in &ws.packages {
        let importer_id = workspace_importer_id(&pkg.relative_path);
        if let Err(err) = lockfile.validate_frozen(&pkg.manifest, &importer_id) {
            mismatches.push(format!("{importer_id}: {err}"));
        }
    }

    mismatches
}

pub(super) fn update_lockfile_importers(
    base_lockfile: &Lockfile,
    manifest: &Manifest,
    workspace: &Option<Workspace>,
    graph: &orix_domain::DependencyGraph,
) -> Lockfile {
    let mut lockfile = base_lockfile.update(manifest, graph, ".");
    if let Some(ws) = workspace {
        for pkg in &ws.packages {
            let importer_id = workspace_importer_id(&pkg.relative_path);
            lockfile = lockfile.update(&pkg.manifest, graph, &importer_id);
        }
    }
    lockfile
}
