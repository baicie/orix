//! orix CLI entry point.

use std::path::PathBuf;

use anyhow::Result;
use clap::{Args, Parser, Subcommand, ValueEnum};
use tokio::sync::mpsc;
use tracing_subscriber::{fmt, EnvFilter};

use orix_core::{
    add, install, pipeline, remove, store_path_with_overrides, store_prune_with_overrides,
    store_verify_with_overrides, ConfigOverrides, InstallEvent, InstallOpts,
};

mod errors;
mod progress;

#[derive(Parser)]
#[command(name = "orix")]
#[command(
    version,
    about = "Fast, disk-space efficient package manager written in Rust"
)]
struct Cli {
    #[arg(long, global = true, env = "ORIX_REGISTRY")]
    registry: Option<String>,

    #[arg(long, global = true, default_value = "info", env = "ORIX_LOG")]
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
    Install(InstallArgs),
    Add(AddArgs),
    Remove(RemoveArgs),
    StorePrune {
        #[arg(long)]
        dry_run: bool,
    },
    StorePath,
    StoreVerify,
}

#[derive(Args)]
struct InstallArgs {
    #[arg(long)]
    frozen_lockfile: bool,
    #[arg(long)]
    offline: bool,
    #[arg(long)]
    force: bool,
    #[arg(long)]
    ignore_scripts: bool,
    #[arg(long, default_value = "10")]
    concurrency: usize,
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

#[derive(ValueEnum, Clone, Default)]
enum ColorChoice {
    Always,
    Never,
    #[default]
    Auto,
}

fn init_tracing(level: &str) {
    let filter = EnvFilter::try_new(level).unwrap_or_else(|_| EnvFilter::new("info"));
    fmt().with_env_filter(filter).init();
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    init_tracing(&cli.log);

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

            run_install(&dir, &install_opts).await?;
        }

        Command::Add(args) => {
            let dep_type = if args.dev {
                pipeline::DepType::Dev
            } else if args.optional {
                pipeline::DepType::Optional
            } else {
                pipeline::DepType::Production
            };

            let report = match add(&dir, &args.packages, dep_type, &opts).await {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("{}", errors::format_error(&e, &dir));
                    std::process::exit(1);
                }
            };
            println!(
                " {} Added {} packages (total installed: {})",
                CHECKMARK,
                args.packages.len(),
                report.packages_added
            );
        }

        Command::Remove(args) => {
            let report = match remove(&dir, &args.packages, &opts).await {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("{}", errors::format_error(&e, &dir));
                    std::process::exit(1);
                }
            };
            println!(
                " {} Removed packages: {:?}",
                REMOVE, report.removed_packages
            );
            println!(
                " {} Packages remaining: {}",
                INFO, report.install_report.packages_added
            );
        }

        Command::StorePath => {
            let path = match store_path_with_overrides(&dir, &config_overrides) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("{}", errors::format_error(&e, &dir));
                    std::process::exit(1);
                }
            };
            println!("{}", path.display());
        }

        Command::StorePrune { dry_run } => {
            let report = match store_prune_with_overrides(&dir, dry_run, &config_overrides) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("{}", errors::format_error(&e, &dir));
                    std::process::exit(1);
                }
            };
            if dry_run {
                println!(
                    " {} Would remove {} packages and {} content files",
                    INFO, report.packages_removed, report.files_removed
                );
            } else {
                println!(
                    " {} Removed {} packages and {} content files",
                    CHECKMARK, report.packages_removed, report.files_removed
                );
            }
            println!(" {} Bytes reclaimed: {}", INFO, report.bytes_reclaimed);
        }

        Command::StoreVerify => {
            let report = match store_verify_with_overrides(&dir, &config_overrides) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("{}", errors::format_error(&e, &dir));
                    std::process::exit(1);
                }
            };
            println!(" {} Packages checked: {}", INFO, report.packages_checked);
            println!(" {} Files checked: {}", INFO, report.files_checked);
            if report.is_ok() {
                println!(" {} Store verified", CHECKMARK);
            } else {
                for missing in &report.missing {
                    eprintln!("{} missing: {}", CROSS, missing);
                }
                for corrupted in &report.corrupted {
                    eprintln!("{} corrupted: {}", CROSS, corrupted);
                }
                std::process::exit(1);
            }
        }
    }

    Ok(())
}

const CHECKMARK: &str = "\u{2713}";
const CROSS: &str = "\u{2717}";
const INFO: &str = "\u{2139}";
const REMOVE: &str = "\u{2716}";

/// Spawns a task that drains install events from the channel and renders them.
fn spawn_event_renderer(mut rx: mpsc::Receiver<InstallEvent>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut renderer = progress::ProgressRenderer::new();
        while let Some(event) = rx.recv().await {
            match event {
                InstallEvent::FetchingPackage(name) => {
                    renderer.fetched_package(name);
                }
                _ => {
                    renderer.handle(event);
                }
            }
        }
    })
}

/// Run the install command with a live progress renderer.
async fn run_install(project_root: &std::path::Path, opts: &InstallOpts) -> Result<()> {
    let (tx, rx) = mpsc::channel::<InstallEvent>(100);

    let mut install_opts = opts.clone();
    install_opts.progress_tx = Some(tx);

    // Start the event renderer task.
    let renderer_handle = spawn_event_renderer(rx);

    // Run install; the pipeline sends events through `progress_tx`.
    let report = install(project_root, &install_opts).await?;

    // Drop the sender so the channel closes and the renderer task exits cleanly.
    drop(install_opts.progress_tx);

    // Wait for the renderer to finish draining any remaining events.
    renderer_handle.await?;

    print_summary(&report);
    Ok(())
}

fn print_summary(report: &orix_core::InstallReport) {
    println!();

    if report.fetch_report.failures.is_empty() {
        println!(
            " {} Packages installed: {}",
            CHECKMARK, report.packages_added
        );
    } else {
        println!(
            " {} Packages installed: {} ({}/{} failed)",
            CHECKMARK,
            report.packages_added,
            report.fetch_report.failures.len(),
            report.fetch_report.success + report.fetch_report.failures.len()
        );
        for f in &report.fetch_report.failures {
            eprintln!("{} {}", CROSS, f);
        }
    }

    if let Some(diff) = &report.lockfile_diff {
        if !diff.added.is_empty() {
            println!(" {} Added: {}", INFO, diff.added.join(", "));
        }
        if !diff.removed.is_empty() {
            println!(" {} Removed: {}", INFO, diff.removed.join(", "));
        }
    }

    println!(" {} Duration: {:.2}s", INFO, report.duration_secs);
}
