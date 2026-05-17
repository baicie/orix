//! orix CLI entry point.

use std::path::PathBuf;

use anyhow::Result;
use clap::{Args, Parser, Subcommand, ValueEnum};
use tracing_subscriber::{fmt, EnvFilter};

use orix_core::{
    add, cache_clean_with_overrides, cache_path_with_overrides, install, pipeline, remove,
    store_path_with_overrides, store_prune_with_overrides, store_verify_with_overrides,
    ConfigOverrides, InstallOpts,
};

mod errors;

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
    let filter = EnvFilter::try_new(level).unwrap_or_else(|_| EnvFilter::new("warn"));
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

            if let Err(e) = run_install(&dir, &install_opts).await {
                eprintln!("{}", errors::format_error(&e, &dir));
                std::process::exit(1);
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

        Command::Store(command) => match command {
            StoreCommand::Path => print_store_path(&dir, &config_overrides),
            StoreCommand::Prune { dry_run } => print_store_prune(&dir, &config_overrides, dry_run),
            StoreCommand::Verify => print_store_verify(&dir, &config_overrides),
        },

        Command::Cache(command) => match command {
            CacheCommand::Path => print_cache_path(&dir, &config_overrides),
            CacheCommand::Clean => print_cache_clean(&dir, &config_overrides),
        },

        Command::StorePath => print_store_path(&dir, &config_overrides),
        Command::StorePrune { dry_run } => print_store_prune(&dir, &config_overrides, dry_run),
        Command::StoreVerify => print_store_verify(&dir, &config_overrides),
    }

    Ok(())
}

const CHECKMARK: &str = "\u{2713}";
const CROSS: &str = "\u{2717}";
const INFO: &str = "\u{2139}";
const REMOVE: &str = "\u{2716}";

fn print_store_path(project_root: &std::path::Path, overrides: &ConfigOverrides) {
    let path = match store_path_with_overrides(project_root, overrides) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{}", errors::format_error(&e, project_root));
            std::process::exit(1);
        }
    };
    println!("{}", path.display());
}

fn print_store_prune(project_root: &std::path::Path, overrides: &ConfigOverrides, dry_run: bool) {
    let report = match store_prune_with_overrides(project_root, dry_run, overrides) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{}", errors::format_error(&e, project_root));
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

fn print_store_verify(project_root: &std::path::Path, overrides: &ConfigOverrides) {
    let report = match store_verify_with_overrides(project_root, overrides) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{}", errors::format_error(&e, project_root));
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

fn print_cache_path(project_root: &std::path::Path, overrides: &ConfigOverrides) {
    let path = match cache_path_with_overrides(project_root, overrides) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{}", errors::format_error(&e, project_root));
            std::process::exit(1);
        }
    };
    println!("{}", path.display());
}

fn print_cache_clean(project_root: &std::path::Path, overrides: &ConfigOverrides) {
    let report = match cache_clean_with_overrides(project_root, overrides) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{}", errors::format_error(&e, project_root));
            std::process::exit(1);
        }
    };

    if report.existed {
        println!(" {} Cleared cache: {}", CHECKMARK, report.path.display());
        println!(" {} Bytes reclaimed: {}", INFO, report.bytes_reclaimed);
    } else {
        println!(
            " {} Cache is already empty: {}",
            INFO,
            report.path.display()
        );
    }
}

/// Run the install command and print the final install summary.
async fn run_install(project_root: &std::path::Path, opts: &InstallOpts) -> Result<()> {
    let mut install_opts = opts.clone();
    install_opts.progress_tx = None;

    let report = install(project_root, &install_opts).await?;
    print_summary(&report);
    Ok(())
}

fn print_summary(report: &orix_core::InstallReport) {
    print_install_header();
    println!(
        "Packages: +{} direct, +{} total",
        report.direct_dependencies, report.packages_added
    );
    println!("Registry: {}", report.registry);
    println!();
    println!("{} Resolved dependencies", CHECKMARK);
    println!(
        "{} Fetched packages {}/{}",
        CHECKMARK, report.fetch_report.success, report.packages_added
    );
    println!("{} Linked dependencies", CHECKMARK);
    if report.lockfile_changed {
        println!("{} Updated lockfile", CHECKMARK);
    } else {
        println!("{} Lockfile unchanged", CHECKMARK);
    }
    println!();
    println!("Done in {:.2}s", report.duration_secs);
}

fn print_install_header() {
    println!("orix install");
    println!("----------------------------------------");
    println!();
}
