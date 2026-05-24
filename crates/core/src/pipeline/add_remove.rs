//! Pipeline submodule.

use super::install::install;
use super::prelude::*;
use super::types::{InstallOpts, InstallReport, RemoveReport};

/// Add dependencies to `package.json` and run install.
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
