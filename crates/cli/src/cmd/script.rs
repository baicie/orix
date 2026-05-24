//! Script execution helpers.

use std::sync::Arc;

use crate::args::RunArgs;
use anyhow::Context;
use tokio::sync::Semaphore;

use orix_core::{
    filter_workspace_packages, normalize_script_args, Manifest, ScriptRunner, Workspace,
    WorkspacePackage, WorkspaceSelector,
};

use super::{CHECKMARK, CROSS};

pub(crate) async fn run_script(
    project_root: &std::path::Path,
    args: &RunArgs,
    workspace_arg: Option<String>,
    filter_arg: Vec<String>,
) -> anyhow::Result<()> {
    let config = orix_core::Config::load(project_root)?;
    let workspace = match Workspace::discover(project_root.to_path_buf()) {
        Ok(ws) => Some(ws),
        Err(e) => {
            tracing::warn!("failed to discover workspace: {}", e);
            None
        }
    };
    let script_args = normalize_script_args(args.args.clone());

    // Merge: RunArgs fields take precedence, then global args
    let workspace_arg = args.workspace.clone().or(workspace_arg);
    let filter_arg = if args.filter.is_empty() {
        filter_arg
    } else {
        args.filter.clone()
    };

    if args.recursive {
        let ws = workspace.as_ref().context("no workspace found")?;

        // Parse filter selectors.
        let selectors: Vec<WorkspaceSelector> = filter_arg
            .iter()
            .map(|s| WorkspaceSelector::parse(s))
            .collect();

        // Filter packages based on selectors.
        let packages = filter_workspace_packages(ws, &selectors);

        if packages.is_empty() {
            if !filter_arg.is_empty() {
                anyhow::bail!("no packages match the filter: {}", filter_arg.join(", "));
            }
            return Ok(());
        }

        if args.parallel {
            run_parallel(
                ws,
                &packages,
                &config,
                &args.script,
                script_args,
                args.concurrency,
            )
            .await?;
        } else {
            run_serial(ws, &packages, &config, &args.script, script_args).await?;
        }
    } else if let Some(ref ws_pkg) = workspace_arg {
        let ws = workspace.as_ref().context("no workspace found")?;
        let manifest = find_workspace_manifest(ws, ws_pkg)?;
        let runner = ScriptRunner::new(config, manifest, project_root.to_path_buf(), workspace);
        let output = runner
            .run_in_workspace(ws_pkg, &args.script, script_args, args.if_present)
            .await?;
        if !output.status.success() {
            std::process::exit(output.status.code().unwrap_or(-1));
        }
    } else if !filter_arg.is_empty() {
        // --filter without --recursive: run in the first matching package
        let ws = workspace.as_ref().context("no workspace found")?;
        let selectors: Vec<WorkspaceSelector> = filter_arg
            .iter()
            .map(|s| WorkspaceSelector::parse(s))
            .collect();
        let packages = filter_workspace_packages(ws, &selectors);

        if packages.is_empty() {
            anyhow::bail!("no packages match the filter: {}", filter_arg.join(", "));
        }

        if packages.len() > 1 {
            anyhow::bail!(
                "filter matched {} packages, use --recursive to run in all packages: {}",
                packages.len(),
                packages
                    .iter()
                    .filter_map(|p| p.manifest.name.clone())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }

        let pkg = &packages[0];
        let manifest = pkg.manifest.clone();
        let runner = ScriptRunner::new(config, manifest, pkg.abs_path.clone(), Some(ws.clone()));
        let outputs = runner
            .run_script(&args.script, script_args, args.if_present)
            .await?;

        let all_success = outputs.iter().all(|o| o.status.success());
        if let Some(last) = outputs.last() {
            if !last.status.success() {
                std::process::exit(last.status.code().unwrap_or(-1));
            }
        }

        if !all_success {
            anyhow::bail!("one or more scripts in the lifecycle chain failed");
        }
    } else {
        let manifest = Manifest::read(&project_root.join("package.json"))
            .with_context(|| "failed to read package.json")?;
        let runner = ScriptRunner::new(config, manifest, project_root.to_path_buf(), workspace);
        let outputs = runner
            .run_script(&args.script, script_args, args.if_present)
            .await?;

        let all_success = outputs.iter().all(|o| o.status.success());
        if let Some(last) = outputs.last() {
            if !last.status.success() {
                std::process::exit(last.status.code().unwrap_or(-1));
            }
        }

        if !all_success {
            anyhow::bail!("one or more scripts in the lifecycle chain failed");
        }
    }

    Ok(())
}

/// Run scripts serially in topological order.
async fn run_serial(
    ws: &Workspace,
    packages: &[WorkspacePackage],
    config: &orix_core::Config,
    script: &str,
    args: Vec<String>,
) -> anyhow::Result<()> {
    let sorted = topological_sort(packages);

    let mut failed = false;
    for pkg in sorted {
        let pkg_name = pkg.manifest.name.clone().unwrap_or_default();

        if pkg.manifest.script(script).is_none() {
            println!(" - {} (no script)", pkg_name);
            continue;
        }

        let runner = ScriptRunner::new(
            config.clone(),
            pkg.manifest.clone(),
            pkg.abs_path.clone(),
            Some(ws.clone()),
        );

        match runner.run_script(script, args.clone(), true).await {
            Ok(outputs) => {
                if let Some(last) = outputs.last() {
                    println!(
                        " {} {} (exit {})",
                        CHECKMARK,
                        pkg_name,
                        last.status.code().unwrap_or(-1)
                    );
                }
            }
            Err(orix_core::ScriptError::MissingScript(..)) => {
                println!(" - {} (no script)", pkg_name);
            }
            Err(orix_core::ScriptError::Disabled) => {
                println!(" - {} (scripts disabled)", pkg_name);
            }
            Err(e) => {
                eprintln!(" {} {}: {}", CROSS, pkg_name, e);
                failed = true;
            }
        }
    }

    if failed {
        anyhow::bail!("one or more scripts failed");
    }
    Ok(())
}

/// Run scripts in parallel with controlled concurrency.
async fn run_parallel(
    ws: &Workspace,
    packages: &[WorkspacePackage],
    config: &orix_core::Config,
    script: &str,
    args: Vec<String>,
    concurrency: usize,
) -> anyhow::Result<()> {
    let semaphore = Arc::new(Semaphore::new(concurrency.max(1)));
    let mut handles = Vec::new();

    for pkg in packages {
        if pkg.manifest.script(script).is_none() {
            continue;
        }

        let permit = semaphore.clone().acquire_owned().await?;
        let pkg = pkg.clone();
        let ws = ws.clone();
        let config = config.clone();
        let script = script.to_string();
        let args = args.clone();

        let handle = tokio::spawn(async move {
            let _permit = permit;
            let runner =
                ScriptRunner::new(config, pkg.manifest.clone(), pkg.abs_path.clone(), Some(ws));
            let pkg_name = pkg.manifest.name.clone().unwrap_or_default();
            let result = runner.run_script(&script, args, true).await;
            (pkg_name, result)
        });

        handles.push(handle);
    }

    let mut failed = false;
    for handle in handles {
        let join_result = handle.await;
        let (pkg_name, result) = match join_result {
            Ok((name, res)) => (name, res),
            Err(e) => {
                eprintln!(" {} task join error: {}", CROSS, e);
                failed = true;
                continue;
            }
        };

        match result {
            Ok(outputs) => {
                if let Some(last) = outputs.last() {
                    println!(
                        " {} {} (exit {})",
                        CHECKMARK,
                        pkg_name,
                        last.status.code().unwrap_or(-1)
                    );
                }
            }
            Err(orix_core::ScriptError::MissingScript(..)) => {
                println!(" - {} (no script)", pkg_name);
            }
            Err(orix_core::ScriptError::Disabled) => {
                println!(" - {} (scripts disabled)", pkg_name);
            }
            Err(e) => {
                eprintln!(" {} {}: {}", CROSS, pkg_name, e);
                failed = true;
            }
        }
    }

    if failed {
        anyhow::bail!("one or more scripts failed");
    }
    Ok(())
}

/// Simple topological sort of workspace packages based on dependency order.
fn topological_sort<'a>(packages: &'a [WorkspacePackage]) -> Vec<&'a WorkspacePackage> {
    use std::collections::{HashMap, HashSet};

    let pkg_names: HashSet<_> = packages
        .iter()
        .filter_map(|p| p.manifest.name.clone())
        .collect();

    // Build dependency graph: package -> packages it depends on.
    let mut deps: HashMap<&str, Vec<&str>> = HashMap::new();
    for pkg in packages {
        if let Some(ref name) = pkg.manifest.name {
            let local_deps: Vec<&str> = pkg
                .manifest
                .dependencies
                .keys()
                .filter(|d| pkg_names.contains(d.as_str()))
                .map(|d| d.as_str())
                .collect();
            deps.insert(name.as_str(), local_deps);
        }
    }

    // Compute in-degrees based on packages in our filtered set.
    let mut in_degree: HashMap<&str, usize> = HashMap::new();
    for pkg in packages {
        if let Some(ref name) = pkg.manifest.name {
            in_degree.entry(name.as_str()).or_insert(0);
        }
    }
    for deps_list in deps.values() {
        for &dep in deps_list {
            if pkg_names.contains(dep) {
                *in_degree.entry(dep).or_insert(0) += 1;
            }
        }
    }

    // Kahn's algorithm for topological sort.
    let mut queue: Vec<_> = in_degree
        .iter()
        .filter(|(_, &d)| d == 0)
        .map(|(k, _)| *k)
        .collect();
    queue.sort();

    let mut sorted = Vec::new();
    while let Some(name) = queue.pop() {
        if let Some(pkg) = packages
            .iter()
            .find(|p| p.manifest.name.as_deref() == Some(name))
        {
            sorted.push(pkg);
        }

        if let Some(deps_list) = deps.get(name) {
            for &dep in deps_list {
                if let Some(d) = in_degree.get_mut(dep) {
                    *d = d.saturating_sub(1);
                    if *d == 0 {
                        queue.push(dep);
                        queue.sort();
                    }
                }
            }
        }
    }

    // Add any remaining packages not in the dependency graph.
    let sorted_names: HashSet<_> = sorted
        .iter()
        .filter_map(|p| p.manifest.name.clone())
        .collect();
    for pkg in packages {
        if let Some(ref name) = pkg.manifest.name {
            if !sorted_names.contains(name) {
                sorted.push(pkg);
            }
        }
    }

    sorted
}

/// Find the manifest for a workspace package by name or path.
fn find_workspace_manifest(
    ws: &Workspace,
    selector: &str,
) -> anyhow::Result<orix_manifest::Manifest> {
    use orix_workspace::WorkspaceSelector;

    let parsed = WorkspaceSelector::parse(selector);
    let packages = orix_core::filter_workspace_packages(ws, &[parsed.clone()]);

    if packages.is_empty() {
        anyhow::bail!("workspace package not found: {}", selector);
    }

    if packages.len() > 1 {
        anyhow::bail!(
            "selector matched {} packages: {}",
            packages.len(),
            packages
                .iter()
                .filter_map(|p| p.manifest.name.clone())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    Ok(packages[0].manifest.clone())
}
