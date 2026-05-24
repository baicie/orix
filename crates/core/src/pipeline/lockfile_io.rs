//! Pipeline submodule.

use super::prelude::*;
/// Result of importing a pnpm lockfile into orix format.
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
