//! orix CLI entry point.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};
use tokio::sync::mpsc;
use tracing_subscriber::{fmt, EnvFilter};

use orix_core::{
    add, cache_clean_with_overrides, cache_path_with_overrides, deploy, export_pnpm_lockfile,
    import_pnpm_lockfile, install, pipeline, remove, store_path_with_overrides,
    store_prune_with_overrides, store_verify_with_overrides, ConfigOverrides, DeployOpts,
    InstallOpts, Manifest, ScriptRunner, Workspace,
};

use crate::styles::{ColorState, Style};

mod errors;
mod reporter;
mod styles;

#[derive(Parser)]
#[command(name = "orix")]
#[command(
    version,
    about = "Fast, disk-space efficient package manager written in Rust"
)]
struct Cli {
    #[arg(long, global = true, env = "ORIX_REGISTRY")]
    registry: Option<String>,

    #[arg(long, global = true, default_value = "warn", env = "ORIX_LOG")]
    log: String,

    #[arg(long, short = 'C', default_value = ".", env = "ORIX_DIR")]
    dir: PathBuf,

    #[arg(long, global = true, env = "ORIX_STORE", value_name = "DIR")]
    store_dir: Option<PathBuf>,

    #[arg(long, global = true, env = "ORIX_CACHE", value_name = "DIR")]
    cache_dir: Option<PathBuf>,

    #[arg(long, global = true, default_value = "auto")]
    color: ColorChoice,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    #[command(alias = "i")]
    Install(InstallArgs),
    Add(AddArgs),
    Remove(RemoveArgs),
    #[command(name = "run")]
    Run(RunArgs),
    #[command(subcommand)]
    Store(StoreCommand),
    #[command(subcommand)]
    Cache(CacheCommand),
    #[command(name = "store-prune", hide = true)]
    StorePrune {
        #[arg(long)]
        dry_run: bool,
    },
    #[command(name = "store-path", hide = true)]
    StorePath,
    #[command(name = "store-verify", hide = true)]
    StoreVerify,
    Import(ImportArgs),
    Export(ExportArgs),
    Deploy(DeployArgs),
}

#[derive(Subcommand)]
enum StoreCommand {
    Path,
    Prune {
        #[arg(long)]
        dry_run: bool,
    },
    Verify,
}

#[derive(Subcommand)]
enum CacheCommand {
    Path,
    Clean,
}

#[derive(Args)]
struct InstallArgs {
    /// Fail if the lockfile is missing or out of date.
    #[arg(long)]
    frozen_lockfile: bool,
    /// Use only locally cached packages.
    #[arg(long)]
    offline: bool,
    /// Re-fetch packages regardless of cache.
    #[arg(long)]
    force: bool,
    /// Skip lifecycle scripts.
    #[arg(long)]
    ignore_scripts: bool,
    /// Number of concurrent package fetches.
    #[arg(long, default_value = "10")]
    concurrency: usize,
    /// Save named packages as dev dependencies.
    #[arg(short = 'D')]
    dev: bool,
    /// Save named packages as optional dependencies.
    #[arg(short = 'O')]
    optional: bool,
    /// Package names or specs to add before installing.
    #[arg(trailing_var_arg = true)]
    packages: Vec<String>,
}

#[derive(Args)]
struct AddArgs {
    #[arg(short = 'D')]
    dev: bool,
    #[arg(short = 'O')]
    optional: bool,
    #[arg(trailing_var_arg = true)]
    packages: Vec<String>,
}

#[derive(Args)]
struct RemoveArgs {
    #[arg(trailing_var_arg = true)]
    packages: Vec<String>,
}

#[derive(Args)]
struct RunArgs {
    /// Script name to run.
    script: String,
    /// Additional arguments to pass to the script.
    #[arg(trailing_var_arg = true)]
    args: Vec<String>,
    /// Do not error if the script is not defined.
    #[arg(long)]
    if_present: bool,
    /// Run the script in a specific workspace package.
    #[arg(long)]
    workspace: Option<String>,
    /// Run the script recursively across all workspace packages.
    #[arg(long, short = 'r')]
    recursive: bool,
    /// Maximum number of concurrent workspace scripts (for --recursive).
    #[arg(long, default_value = "4")]
    concurrency: usize,
}

#[derive(Args)]
struct ImportArgs {
    /// Source lockfile format. Defaults to pnpm-lock.yaml.
    #[arg(long, value_enum, default_value = "pnpm")]
    from: LockfileFormat,
    /// Input file path. Defaults to pnpm-lock.yaml in the project root.
    #[arg(default_value = "pnpm-lock.yaml")]
    path: PathBuf,
}

#[derive(Args)]
struct ExportArgs {
    /// Output format. Defaults to pnpm-lock.yaml.
    #[arg(long, value_enum, default_value = "pnpm")]
    to: LockfileFormat,
    /// Output file path. Defaults to pnpm-lock.yaml in the project root.
    #[arg(default_value = "pnpm-lock.yaml")]
    path: PathBuf,
}

#[derive(Args)]
struct DeployArgs {
    /// Package name or path glob to deploy (required).
    #[arg(short = 'F', long, required = true)]
    filter: String,
    /// Output directory for the deployed package.
    #[arg(short, long, required = true)]
    output: PathBuf,
    /// Only include production dependencies (skip devDependencies).
    #[arg(long, short = 'p')]
    prod: bool,
    /// Use frozen lockfile (no registry interaction).
    #[arg(long)]
    frozen_lockfile: bool,
    /// Run deploy hooks (predeploy, postdeploy).
    #[arg(long)]
    hooks: bool,
}

#[derive(ValueEnum, Clone, Default)]
enum LockfileFormat {
    #[default]
    Pnpm,
}

#[derive(ValueEnum, Clone, Default)]
pub(crate) enum ColorChoice {
    Always,
    Never,
    #[default]
    Auto,
}

fn init_tracing(level: &str) {
    let filter = EnvFilter::try_new(level).unwrap_or_else(|_| EnvFilter::new("warn"));
    fmt().with_env_filter(filter).init();
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    init_tracing(&cli.log);

    let color_state = ColorState::from_choice(cli.color.clone());

    #[allow(clippy::cmp_owned)]
    let dir = if cli.dir == PathBuf::from(".") {
        std::env::current_dir()?
    } else {
        cli.dir.canonicalize().unwrap_or(cli.dir)
    };

    let opts = InstallOpts {
        registry: cli.registry.clone(),
        store_dir: cli.store_dir.clone(),
        cache_dir: cli.cache_dir.clone(),
        ..InstallOpts::default()
    };
    let config_overrides = ConfigOverrides {
        registry: cli.registry.clone(),
        store_dir: cli.store_dir.clone(),
        cache_dir: cli.cache_dir.clone(),
        ignore_scripts: None,
        allow_scripts: None,
    };

    match cli.command {
        Command::Install(args) => {
            let install_opts = InstallOpts {
                registry: cli.registry.clone(),
                store_dir: cli.store_dir.clone(),
                cache_dir: cli.cache_dir.clone(),
                frozen_lockfile: args.frozen_lockfile,
                offline: args.offline,
                force: args.force,
                ignore_scripts: args.ignore_scripts,
                concurrency: args.concurrency,
                progress_tx: None,
            };

            if args.packages.is_empty() {
                if args.dev || args.optional {
                    eprintln!(
                        "{}",
                        errors::format_error(
                            &anyhow::anyhow!(
                                "-D and -O can only be used when installing package names"
                            ),
                            &dir,
                            color_state
                        )
                    );
                    std::process::exit(1);
                }

                if let Err(e) = run_install(&dir, &install_opts, color_state).await {
                    eprintln!("{}", errors::format_error(&e, &dir, color_state));
                    std::process::exit(1);
                }
            } else {
                if args.frozen_lockfile {
                    eprintln!(
                        "{}",
                        errors::format_error(
                            &anyhow::anyhow!(
                                "--frozen-lockfile cannot be used when installing package names"
                            ),
                            &dir,
                            color_state
                        )
                    );
                    std::process::exit(1);
                }

                let dep_type = if args.dev {
                    pipeline::DepType::Dev
                } else if args.optional {
                    pipeline::DepType::Optional
                } else {
                    pipeline::DepType::Production
                };

                let run = match run_add(&dir, &args.packages, dep_type, &install_opts, color_state)
                    .await
                {
                    Ok(r) => r,
                    Err(e) => {
                        eprintln!("{}", errors::format_error(&e, &dir, color_state));
                        std::process::exit(1);
                    }
                };
                if !run.rendered_summary {
                    print_summary(&run.report, color_state);
                }
                println!(
                    " {} Added {} packages (total installed: {})",
                    Style::Checkmark.paint(CHECKMARK, color_state),
                    args.packages.len(),
                    run.report.packages_added
                );
            }
        }

        Command::Add(args) => {
            let dep_type = if args.dev {
                pipeline::DepType::Dev
            } else if args.optional {
                pipeline::DepType::Optional
            } else {
                pipeline::DepType::Production
            };

            let run = match run_add(&dir, &args.packages, dep_type, &opts, color_state).await {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("{}", errors::format_error(&e, &dir, color_state));
                    std::process::exit(1);
                }
            };
            if !run.rendered_summary {
                print_summary(&run.report, color_state);
            }
            println!(
                " {} Added {} packages (total installed: {})",
                Style::Checkmark.paint(CHECKMARK, color_state),
                args.packages.len(),
                run.report.packages_added
            );
        }

        Command::Remove(args) => {
            let report = match remove(&dir, &args.packages, &opts).await {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("{}", errors::format_error(&e, &dir, color_state));
                    std::process::exit(1);
                }
            };
            println!(
                " {} Removed packages: {:?}",
                Style::Cross.paint(REMOVE, color_state),
                report.removed_packages
            );
            println!(
                " {} Packages remaining: {}",
                Style::InfoPrefix.paint(INFO, color_state),
                report.install_report.packages_added
            );
        }

        Command::Run(args) => {
            if let Err(e) = run_script(&dir, &args, color_state).await {
                eprintln!("{}", errors::format_error(&e, &dir, color_state));
                std::process::exit(1);
            }
        }

        Command::Store(command) => match command {
            StoreCommand::Path => print_store_path(&dir, &config_overrides, color_state),
            StoreCommand::Prune { dry_run } => {
                print_store_prune(&dir, &config_overrides, dry_run, color_state)
            }
            StoreCommand::Verify => print_store_verify(&dir, &config_overrides, color_state),
        },

        Command::Cache(command) => match command {
            CacheCommand::Path => print_cache_path(&dir, &config_overrides, color_state),
            CacheCommand::Clean => print_cache_clean(&dir, &config_overrides, color_state),
        },

        Command::StorePath => print_store_path(&dir, &config_overrides, color_state),
        Command::StorePrune { dry_run } => {
            print_store_prune(&dir, &config_overrides, dry_run, color_state)
        }
        Command::StoreVerify => print_store_verify(&dir, &config_overrides, color_state),

        Command::Import(args) => {
            let input_path = if args.path.is_relative() {
                dir.join(&args.path)
            } else {
                args.path.clone()
            };
            match run_import(&dir, &input_path) {
                Ok(report) => {
                    println!(
                        " {} Imported {} packages from {}",
                        Style::Checkmark.paint(CHECKMARK, color_state),
                        report.packages_imported,
                        Style::Registry.paint(&input_path.display().to_string(), color_state)
                    );
                    if report.warnings > 0 {
                        println!(
                            " {} {} warnings (see above)",
                            Style::Warning.paint(INFO, color_state),
                            report.warnings
                        );
                    }
                }
                Err(e) => {
                    eprintln!("{}", errors::format_error(&e, &dir, color_state));
                    std::process::exit(1);
                }
            }
        }

        Command::Export(args) => {
            let output_path = if args.path.is_relative() {
                dir.join(&args.path)
            } else {
                args.path.clone()
            };
            match run_export(&dir, &output_path) {
                Ok(report) => {
                    println!(
                        " {} Exported {} packages to {}",
                        Style::Checkmark.paint(CHECKMARK, color_state),
                        report.packages_exported,
                        Style::Registry.paint(&output_path.display().to_string(), color_state)
                    );
                }
                Err(e) => {
                    eprintln!("{}", errors::format_error(&e, &dir, color_state));
                    std::process::exit(1);
                }
            }
        }
        Command::Deploy(args) => {
            let output_path = if args.output.is_relative() {
                dir.join(&args.output)
            } else {
                args.output.clone()
            };
            let opts = DeployOpts {
                prod: args.prod,
                frozen_lockfile: args.frozen_lockfile,
                hooks: args.hooks,
            };
            match run_deploy(&dir, &args.filter, &output_path, &opts) {
                Ok(report) => {
                    println!(
                        " {} Deployed {} packages ({} files)",
                        Style::Checkmark.paint(CHECKMARK, color_state),
                        report.packages_deployed,
                        report.files_copied
                    );
                }
                Err(e) => {
                    eprintln!("{}", errors::format_error(&e, &dir, color_state));
                    std::process::exit(1);
                }
            }
        }
    }

    Ok(())
}

const CHECKMARK: &str = "\u{2713}";
const CROSS: &str = "\u{2717}";
const INFO: &str = "\u{2139}";
const REMOVE: &str = "\u{2716}";

fn print_store_path(
    project_root: &std::path::Path,
    overrides: &ConfigOverrides,
    color_state: ColorState,
) {
    let path = match store_path_with_overrides(project_root, overrides) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{}", errors::format_error(&e, project_root, color_state));
            std::process::exit(1);
        }
    };
    println!("{}", path.display());
}

fn print_store_prune(
    project_root: &std::path::Path,
    overrides: &ConfigOverrides,
    dry_run: bool,
    color_state: ColorState,
) {
    let report = match store_prune_with_overrides(project_root, dry_run, overrides) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{}", errors::format_error(&e, project_root, color_state));
            std::process::exit(1);
        }
    };
    if dry_run {
        println!(
            " {} Would remove {} packages and {} content files",
            Style::InfoPrefix.paint(INFO, color_state),
            Style::Success.paint(&report.packages_removed.to_string(), color_state),
            report.files_removed
        );
    } else {
        println!(
            " {} Removed {} packages and {} content files",
            Style::Checkmark.paint(CHECKMARK, color_state),
            Style::Success.paint(&report.packages_removed.to_string(), color_state),
            report.files_removed
        );
    }
    println!(
        " {} Bytes reclaimed: {}",
        Style::InfoPrefix.paint(INFO, color_state),
        report.bytes_reclaimed
    );
}

fn print_store_verify(
    project_root: &std::path::Path,
    overrides: &ConfigOverrides,
    color_state: ColorState,
) {
    let report = match store_verify_with_overrides(project_root, overrides) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{}", errors::format_error(&e, project_root, color_state));
            std::process::exit(1);
        }
    };
    println!(
        " {} Packages checked: {}",
        Style::InfoPrefix.paint(INFO, color_state),
        report.packages_checked
    );
    println!(
        " {} Files checked: {}",
        Style::InfoPrefix.paint(INFO, color_state),
        report.files_checked
    );
    if report.is_ok() {
        println!(
            " {} Store verified",
            Style::Checkmark.paint(CHECKMARK, color_state)
        );
    } else {
        for missing in &report.missing {
            eprintln!(
                "{} missing: {}",
                Style::Cross.paint(CROSS, color_state),
                missing
            );
        }
        for corrupted in &report.corrupted {
            eprintln!(
                "{} corrupted: {}",
                Style::Cross.paint(CROSS, color_state),
                corrupted
            );
        }
        std::process::exit(1);
    }
}

fn print_cache_path(
    project_root: &std::path::Path,
    overrides: &ConfigOverrides,
    color_state: ColorState,
) {
    let path = match cache_path_with_overrides(project_root, overrides) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{}", errors::format_error(&e, project_root, color_state));
            std::process::exit(1);
        }
    };
    println!("{}", path.display());
}

fn print_cache_clean(
    project_root: &std::path::Path,
    overrides: &ConfigOverrides,
    color_state: ColorState,
) {
    let report = match cache_clean_with_overrides(project_root, overrides) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{}", errors::format_error(&e, project_root, color_state));
            std::process::exit(1);
        }
    };

    if report.existed {
        println!(
            " {} Cleared cache: {}",
            Style::Checkmark.paint(CHECKMARK, color_state),
            Style::Registry.paint(&report.path.display().to_string(), color_state)
        );
        println!(
            " {} Bytes reclaimed: {}",
            Style::InfoPrefix.paint(INFO, color_state),
            report.bytes_reclaimed
        );
    } else {
        println!(
            " {} Cache is already empty: {}",
            Style::InfoPrefix.paint(INFO, color_state),
            Style::Registry.paint(&report.path.display().to_string(), color_state)
        );
    }
}

/// Run the install command and print the final install summary.
async fn run_install(
    project_root: &std::path::Path,
    opts: &InstallOpts,
    color_state: ColorState,
) -> Result<()> {
    let run = run_with_progress(
        opts.clone(),
        |install_opts| async move { install(project_root, &install_opts).await },
        color_state,
    )
    .await?;
    if !run.rendered_summary {
        print_summary(&run.report, color_state);
    }
    Ok(())
}

async fn run_add(
    project_root: &std::path::Path,
    packages: &[String],
    dep_type: pipeline::DepType,
    opts: &InstallOpts,
    color_state: ColorState,
) -> Result<InstallRun> {
    run_with_progress(
        opts.clone(),
        |install_opts| async move { add(project_root, packages, dep_type, &install_opts).await },
        color_state,
    )
    .await
}

struct InstallRun {
    report: orix_core::InstallReport,
    rendered_summary: bool,
}

async fn run_with_progress<F, Fut>(
    mut opts: InstallOpts,
    operation: F,
    color_state: ColorState,
) -> Result<InstallRun>
where
    F: FnOnce(InstallOpts) -> Fut,
    Fut: std::future::Future<Output = Result<orix_core::InstallReport>>,
{
    let (tx, mut rx) = mpsc::channel(128);
    opts.progress_tx = Some(tx.clone());

    let reporter = tokio::spawn(async move {
        let mut reporter = reporter::Reporter::auto(false, color_state);
        while let Some(event) = rx.recv().await {
            if let Err(e) = reporter.on_event(event) {
                eprintln!("reporter error: {}", e);
            }
        }
    });

    let result = operation(opts).await;
    drop(tx);
    let _ = reporter.await;
    result.map(|report| InstallRun {
        report,
        rendered_summary: true,
    })
}

/// Run a user script via `orix run`.
async fn run_script(
    project_root: &std::path::Path,
    args: &RunArgs,
    color_state: ColorState,
) -> anyhow::Result<()> {
    let manifest = Manifest::read(&project_root.join("package.json"))
        .with_context(|| "failed to read package.json")?;
    let config = orix_core::Config::load(project_root)?;
    let workspace = Workspace::discover(project_root.to_path_buf()).ok();

    if args.recursive {
        let runner = ScriptRunner::new(config, manifest, project_root.to_path_buf(), workspace);
        let results = runner
            .run_recursive(&args.script, args.args.clone(), args.concurrency)
            .await?;

        let mut failed = false;
        for (pkg_name, result) in results {
            match result {
                Ok(output) => {
                    println!(
                        " {} {} (exit {})",
                        Style::Checkmark.paint(CHECKMARK, color_state),
                        Style::PackageName.paint(&pkg_name, color_state),
                        Style::Duration
                            .paint(&output.status.code().unwrap_or(-1).to_string(), color_state)
                    );
                }
                Err(orix_core::ScriptError::MissingScript(..)) => {
                    println!(
                        " {} {} (no script)",
                        Style::Muted.paint("-", color_state),
                        pkg_name
                    );
                }
                Err(orix_core::ScriptError::Disabled) => {
                    println!(
                        " {} {} (scripts disabled)",
                        Style::Muted.paint("-", color_state),
                        pkg_name
                    );
                }
                Err(e) => {
                    eprintln!(
                        " {} {}: {}",
                        Style::Cross.paint(CROSS, color_state),
                        Style::PackageName.paint(&pkg_name, color_state),
                        Style::Error.paint(&e.to_string(), color_state)
                    );
                    failed = true;
                }
            }
        }

        if failed {
            anyhow::bail!("one or more scripts failed");
        }
    } else if let Some(ref ws_pkg) = args.workspace {
        let runner = ScriptRunner::new(config, manifest, project_root.to_path_buf(), workspace);
        let output = runner
            .run_in_workspace(ws_pkg, &args.script, args.args.clone(), args.if_present)
            .await?;
        if !output.status.success() {
            std::process::exit(output.status.code().unwrap_or(-1));
        }
    } else {
        let runner = ScriptRunner::new(config, manifest, project_root.to_path_buf(), workspace);
        let outputs = runner
            .run_script(&args.script, args.args.clone(), args.if_present)
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

fn print_summary(report: &orix_core::InstallReport, color_state: ColorState) {
    print_install_header(color_state);
    println!(
        "Packages: +{} direct, +{} total",
        Style::Success.paint(&report.direct_dependencies.to_string(), color_state),
        Style::Success.paint(&report.packages_added.to_string(), color_state)
    );
    println!(
        "Registry: {}",
        Style::Registry.paint(&report.registry, color_state)
    );
    println!();
    println!("Resolved dependencies");
    println!(
        "Fetched packages {}/{}",
        Style::Success.paint(&report.fetch_report.success.to_string(), color_state),
        report.packages_added
    );
    println!("Linked dependencies");
    if report.lockfile_changed {
        println!("Updated lockfile");
    } else {
        println!("Lockfile unchanged");
    }
    println!();
    println!(
        "Done in {}",
        Style::Duration.paint(&format!("{:.2}s", report.duration_secs), color_state)
    );
}

fn print_install_header(color_state: ColorState) {
    let title = Style::Bold.paint("orix install", color_state);
    println!("{}", title);
    let sep = Style::Header.paint("----------------------------------------", color_state);
    println!("{}", sep);
    println!();
}

fn run_import(
    project_root: &std::path::Path,
    source_path: &std::path::Path,
) -> anyhow::Result<orix_core::ImportReport> {
    import_pnpm_lockfile(source_path, project_root)
}

fn run_export(
    project_root: &std::path::Path,
    output_path: &std::path::Path,
) -> anyhow::Result<orix_core::ExportReport> {
    export_pnpm_lockfile(project_root, output_path)
}

fn run_deploy(
    project_root: &std::path::Path,
    filter: &str,
    output_dir: &std::path::Path,
    opts: &DeployOpts,
) -> anyhow::Result<orix_core::DeployReport> {
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(deploy(project_root, filter, output_dir, opts))
}
