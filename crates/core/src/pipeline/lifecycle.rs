//! Pipeline submodule.

use super::fetch::is_fetchable_package;
use super::prelude::*;
use super::types::send_event;

pub(crate) async fn run_project_lifecycle(
    event: LifecycleEvent,
    manifest: &Manifest,
    config: &Config,
    project_root: &Path,
    progress_tx: &Option<mpsc::Sender<InstallEvent>>,
) -> Result<()> {
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
            Ok(())
        }
        Err(ScriptError::Disabled) => {
            send_event(
                progress_tx,
                InstallEvent::ScriptsPhaseSkipped {
                    reason: "scripts disabled by --ignore-scripts".to_string(),
                },
            );
            Ok(())
        }
        Err(ScriptError::MissingScript(..)) => Ok(()),
        Err(ScriptError::Failed { name, code }) => {
            send_event(
                progress_tx,
                InstallEvent::ScriptFinished {
                    name: name.clone(),
                    duration_ms: 0,
                    exit_code: code,
                },
            );
            Err(ScriptError::Failed { name, code }.into())
        }
        Err(ScriptError::Terminated { name }) => {
            send_event(
                progress_tx,
                InstallEvent::ScriptFinished {
                    name: name.clone(),
                    duration_ms: 0,
                    exit_code: None,
                },
            );
            Err(ScriptError::Terminated { name }.into())
        }
        Err(ScriptError::Spawn { name, source }) => {
            send_event(
                progress_tx,
                InstallEvent::ScriptFinished {
                    name: name.clone(),
                    duration_ms: 0,
                    exit_code: Some(-1),
                },
            );
            Err(ScriptError::Spawn { name, source }.into())
        }
    }
}

/// Run preinstall/install/postinstall for allow-listed dependency packages.
pub(crate) async fn run_dependency_lifecycles(
    graph: &orix_domain::DependencyGraph,
    config: &Config,
    project_root: &Path,
    progress_tx: &Option<mpsc::Sender<InstallEvent>>,
) -> Result<()> {
    if config.ignore_scripts {
        return Ok(());
    }

    let events = [
        LifecycleEvent::Preinstall,
        LifecycleEvent::Install,
        LifecycleEvent::Postinstall,
    ];

    for pkg_id in graph_install_order(graph) {
        let Some(pkg) = graph.get(&pkg_id) else {
            continue;
        };
        if !is_fetchable_package(pkg) {
            continue;
        }
        let pkg_name = pkg.id.name.as_str();
        if !dependency_scripts_allowed(config, pkg_name) {
            continue;
        }

        let pkg_dir = installed_package_dir(project_root, &pkg.id);
        let manifest_path = pkg_dir.join("package.json");
        if !manifest_path.exists() {
            trace!(
                pkg = %pkg.id,
                path = %manifest_path.display(),
                "skipping dependency scripts: package.json missing"
            );
            continue;
        }

        let dep_manifest = Manifest::read(&manifest_path)
            .with_context(|| format!("failed to read {}", manifest_path.display()))?;

        let runner = ScriptRunner::new(config.clone(), dep_manifest, pkg_dir, None);

        for event in events {
            send_event(
                progress_tx,
                InstallEvent::ScriptsPhaseStarted {
                    event: format!("{} {}", event.script_name(), pkg.id),
                },
            );
            runner.run_lifecycle(event, &pkg.id).await?;
            send_event(
                progress_tx,
                InstallEvent::ScriptFinished {
                    name: format!("{}:{}", pkg.id, event.script_name()),
                    duration_ms: 0,
                    exit_code: Some(0),
                },
            );
        }
    }

    Ok(())
}
