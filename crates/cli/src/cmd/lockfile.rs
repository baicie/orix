//! Lockfile import/export and deploy.

use orix_core::{
    deploy, export_pnpm_lockfile, import_pnpm_lockfile, DeployOpts, DeployReport, ExportReport,
    ImportReport,
};

pub(crate) fn run_import(
    project_root: &std::path::Path,
    source_path: &std::path::Path,
) -> anyhow::Result<ImportReport> {
    import_pnpm_lockfile(source_path, project_root)
}

pub(crate) fn run_export(
    project_root: &std::path::Path,
    output_path: &std::path::Path,
) -> anyhow::Result<ExportReport> {
    export_pnpm_lockfile(project_root, output_path)
}

pub(crate) fn run_deploy(
    project_root: &std::path::Path,
    filter: &str,
    output_dir: &std::path::Path,
    opts: &DeployOpts,
) -> anyhow::Result<DeployReport> {
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(deploy(project_root, filter, output_dir, opts))
}
