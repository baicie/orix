//! Install/add command helpers.

use crate::reporter::ColorMode;
use anyhow::Result;
use tokio::sync::mpsc;

use orix_core::{add, install, pipeline, InstallOpts};

pub(crate) async fn run_install(
    project_root: &std::path::Path,
    opts: &InstallOpts,
    color_mode: ColorMode,
    no_progress: bool,
) -> Result<()> {
    let run = run_with_progress(
        opts.clone(),
        color_mode,
        no_progress,
        |install_opts| async move { install(project_root, &install_opts).await },
    )
    .await?;
    if !run.rendered_summary {
        print_summary(&run.report);
    }
    Ok(())
}

pub(crate) async fn run_add(
    project_root: &std::path::Path,
    packages: &[String],
    dep_type: pipeline::DepType,
    opts: &InstallOpts,
    color_mode: ColorMode,
    no_progress: bool,
) -> Result<InstallRun> {
    run_with_progress(
        opts.clone(),
        color_mode,
        no_progress,
        |install_opts| async move { add(project_root, packages, dep_type, &install_opts).await },
    )
    .await
}

pub(crate) struct InstallRun {
    pub(crate) report: orix_core::InstallReport,
    pub(crate) rendered_summary: bool,
}

pub(crate) async fn run_with_progress<F, Fut>(
    mut opts: InstallOpts,
    color_mode: ColorMode,
    no_progress: bool,
    operation: F,
) -> Result<InstallRun>
where
    F: FnOnce(InstallOpts) -> Fut,
    Fut: std::future::Future<Output = Result<orix_core::InstallReport>>,
{
    let (tx, mut rx) = mpsc::channel(8192);
    opts.progress_tx = Some(tx.clone());

    let reporter = tokio::spawn(async move {
        let mut reporter = crate::reporter::Reporter::auto(no_progress, color_mode);

        while let Some(event) = rx.recv().await {
            if let Err(e) = reporter.on_event(event) {
                tracing::warn!(error = %e, "reporter failed to render event");
            }
        }
    });

    let result = operation(opts).await;

    if let Err(error) = &result {
        tracing::error!(error = ?error, "install operation failed");
    }

    drop(tx);
    let _ = reporter.await;

    result.map(|report| InstallRun {
        report,
        rendered_summary: true,
    })
}

pub(crate) fn print_summary(report: &orix_core::InstallReport) {
    print_install_header();
    println!(
        "Packages: +{} direct, +{} total",
        report.direct_dependencies, report.packages_added
    );
    println!("Registry: {}", report.registry);
    println!();
    println!("Resolved dependencies");
    println!(
        "Fetched packages {}/{}",
        report.fetch_report.success, report.packages_added
    );
    println!("Linked dependencies");
    if report.lockfile_changed {
        println!("Updated lockfile");
    } else {
        println!("Lockfile unchanged");
    }
    println!();
    println!("Done in {:.2}s", report.duration_secs);
}

pub(crate) fn print_install_header() {
    println!("orix install");
    println!("----------------------------------------");
    println!();
}
