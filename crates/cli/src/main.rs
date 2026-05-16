//! orix CLI entry point.

use std::path::PathBuf;

use anyhow::Result;
use clap::{Args, Parser, Subcommand, ValueEnum};
use tracing_subscriber::{fmt, EnvFilter};

use orix_core::{add, pipeline, remove, store_path, store_prune, store_verify, InstallOpts};

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
        ..InstallOpts::default()
    };

    match cli.command {
        Command::Install(args) => {
            let report = pipeline::install(
                &dir,
                &InstallOpts {
                    registry: cli.registry.clone(),
                    frozen_lockfile: args.frozen_lockfile,
                    offline: args.offline,
                    force: args.force,
                    ignore_scripts: args.ignore_scripts,
                    concurrency: args.concurrency,
                },
            )
            .await?;

            println!("Packages installed: {}", report.packages_added);
            println!("Packages fetched: {}", report.fetch_report.success);
            if !report.fetch_report.failures.is_empty() {
                eprintln!("Failed packages:");
                for f in &report.fetch_report.failures {
                    eprintln!("  - {}", f);
                }
            }
            println!("Duration: {:.2}s", report.duration_secs);
        }

        Command::Add(args) => {
            let dep_type = if args.dev {
                pipeline::DepType::Dev
            } else if args.optional {
                pipeline::DepType::Optional
            } else {
                pipeline::DepType::Production
            };

            let report = add(&dir, &args.packages, dep_type, &opts).await?;
            println!(
                "Added {} packages (total installed: {})",
                args.packages.len(),
                report.packages_added
            );
        }

        Command::Remove(args) => {
            let report = remove(&dir, &args.packages, &opts).await?;
            println!("Removed packages: {:?}", report.removed_packages);
            println!(
                "Packages remaining: {}",
                report.install_report.packages_added
            );
        }

        Command::StorePath => {
            println!("{}", store_path(&dir)?.display());
        }

        Command::StorePrune { dry_run } => {
            let report = store_prune(&dir, dry_run)?;
            if dry_run {
                println!(
                    "Would remove {} packages and {} content files",
                    report.packages_removed, report.files_removed
                );
            } else {
                println!(
                    "Removed {} packages and {} content files",
                    report.packages_removed, report.files_removed
                );
            }
            println!("Bytes reclaimed: {}", report.bytes_reclaimed);
        }

        Command::StoreVerify => {
            let report = store_verify(&dir)?;
            println!("Packages checked: {}", report.packages_checked);
            println!("Files checked: {}", report.files_checked);
            if report.is_ok() {
                println!("Store verified");
            } else {
                for missing in &report.missing {
                    eprintln!("missing: {}", missing);
                }
                for corrupted in &report.corrupted {
                    eprintln!("corrupted: {}", corrupted);
                }
                anyhow::bail!("store verification failed");
            }
        }
    }

    Ok(())
}
