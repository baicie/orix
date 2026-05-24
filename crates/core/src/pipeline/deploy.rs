//! Pipeline submodule.

use super::prelude::*;
/// Deploy workspace packages matching `filter` into `output_dir`.
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
