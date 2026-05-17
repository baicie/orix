//! orix CLI entry point.

use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;

use anyhow::Result;
use clap::{Args, Parser, Subcommand, ValueEnum};
use tokio::sync::mpsc;
use tracing_subscriber::{fmt, EnvFilter};

use orix_core::{
    add, cache_clean_with_overrides, cache_path_with_overrides, install, pipeline, remove,
    store_path_with_overrides, store_prune_with_overrides, store_verify_with_overrides,
    ConfigOverrides, InstallEvent, InstallOpts,
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

            if args.packages.is_empty() {
                if args.dev || args.optional {
                    eprintln!(
                        "{}",
                        errors::format_error(
                            &anyhow::anyhow!(
                                "-D and -O can only be used when installing package names"
                            ),
                            &dir
                        )
                    );
                    std::process::exit(1);
                }

                if let Err(e) = run_install(&dir, &install_opts).await {
                    eprintln!("{}", errors::format_error(&e, &dir));
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
                            &dir
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

                let run = match run_add(&dir, &args.packages, dep_type, &install_opts).await {
                    Ok(r) => r,
                    Err(e) => {
                        eprintln!("{}", errors::format_error(&e, &dir));
                        std::process::exit(1);
                    }
                };
                if !run.rendered_summary {
                    print_summary(&run.report);
                }
                println!(
                    " {} Added {} packages (total installed: {})",
                    CHECKMARK,
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

            let run = match run_add(&dir, &args.packages, dep_type, &opts).await {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("{}", errors::format_error(&e, &dir));
                    std::process::exit(1);
                }
            };
            if !run.rendered_summary {
                print_summary(&run.report);
            }
            println!(
                " {} Added {} packages (total installed: {})",
                CHECKMARK,
                args.packages.len(),
                run.report.packages_added
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
const CURRENT: &str = "\u{25CF}";

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
    let run = run_with_progress(opts.clone(), |install_opts| async move {
        install(project_root, &install_opts).await
    })
    .await?;
    if !run.rendered_summary {
        print_summary(&run.report);
    }
    Ok(())
}

async fn run_add(
    project_root: &std::path::Path,
    packages: &[String],
    dep_type: pipeline::DepType,
    opts: &InstallOpts,
) -> Result<InstallRun> {
    run_with_progress(opts.clone(), |install_opts| async move {
        add(project_root, packages, dep_type, &install_opts).await
    })
    .await
}

struct InstallRun {
    report: orix_core::InstallReport,
    rendered_summary: bool,
}

async fn run_with_progress<F, Fut>(mut opts: InstallOpts, operation: F) -> Result<InstallRun>
where
    F: FnOnce(InstallOpts) -> Fut,
    Fut: std::future::Future<Output = Result<orix_core::InstallReport>>,
{
    let render_progress = io::stdout().is_terminal();
    if !render_progress {
        opts.progress_tx = None;
        let report = operation(opts).await?;
        return Ok(InstallRun {
            report,
            rendered_summary: false,
        });
    }

    let (tx, mut rx) = mpsc::channel(128);
    opts.progress_tx = Some(tx.clone());

    let reporter = tokio::spawn(async move {
        let mut progress = InstallProgress::default();
        while let Some(event) = rx.recv().await {
            progress.on_event(event);
        }
    });

    let result = operation(opts).await;
    if result.is_err() {
        let _ = tx.send(InstallEvent::Failed).await;
    }
    drop(tx);
    let _ = reporter.await;
    result.map(|report| InstallRun {
        report,
        rendered_summary: true,
    })
}

#[derive(Default)]
struct InstallProgress {
    registry: Option<String>,
    direct_dependencies: Option<usize>,
    total_packages: Option<usize>,
    resolved: bool,
    fetch_total: usize,
    fetch_seen: usize,
    fetched: bool,
    fetch_failed: bool,
    linking: bool,
    linked: bool,
    writing_lockfile: bool,
    lockfile_changed: Option<bool>,
    duration_secs: Option<f64>,
    failed: bool,
    rendered_lines: usize,
}

impl InstallProgress {
    fn on_event(&mut self, event: InstallEvent) {
        match event {
            InstallEvent::Started {
                registry,
                direct_dependencies,
            } => {
                self.registry = Some(registry);
                self.direct_dependencies = Some(direct_dependencies);
                self.render();
            }
            InstallEvent::ResolvingTotal(_) => self.render(),
            InstallEvent::ResolvingPackage(_) => {}
            InstallEvent::Resolved { total_packages } => {
                self.resolved = true;
                self.total_packages = Some(total_packages);
                self.render();
            }
            InstallEvent::FetchingTotal(total) => {
                self.fetch_total = total;
                self.fetch_seen = 0;
                self.render();
            }
            InstallEvent::FetchingPackage(_) => {
                self.fetch_seen += 1;
                self.render();
            }
            InstallEvent::FetchFailure(failure) => {
                self.fetch_failed = true;
                self.render();
                eprintln!("{} Failed to fetch {}", CROSS, failure);
            }
            InstallEvent::Fetched { success, total } => {
                self.fetch_seen = success;
                self.fetch_total = total;
                self.fetched = true;
                self.render();
            }
            InstallEvent::Linking => {
                self.linking = true;
                self.render();
            }
            InstallEvent::Linked => {
                self.linked = true;
                self.render();
            }
            InstallEvent::WritingLockfile => {
                self.writing_lockfile = true;
                self.render();
            }
            InstallEvent::LockfileDone { changed } => {
                self.lockfile_changed = Some(changed);
                self.render();
            }
            InstallEvent::Finished { duration_secs } => {
                self.duration_secs = Some(duration_secs);
                self.render();
                println!();
            }
            InstallEvent::Failed => {
                self.failed = true;
                self.render();
                println!();
            }
        }
    }

    fn render(&mut self) {
        let lines = self.lines();
        let mut stdout = io::stdout();
        if self.rendered_lines > 0 {
            let _ = write!(stdout, "\x1b[{}A", self.rendered_lines);
        }
        for line in &lines {
            let _ = writeln!(stdout, "\x1b[2K\r{}", line);
        }
        let _ = stdout.flush();
        self.rendered_lines = lines.len();
    }

    fn lines(&self) -> Vec<String> {
        let mut lines = vec![
            "orix install".to_string(),
            "----------------------------------------".to_string(),
            String::new(),
            format!(
                "Packages: +{} direct, +{} total",
                self.direct_dependencies
                    .map_or_else(|| "?".to_string(), |value| value.to_string()),
                self.total_packages
                    .map_or_else(|| "?".to_string(), |value| value.to_string())
            ),
            format!(
                "Registry: {}",
                self.registry.as_deref().unwrap_or("resolving")
            ),
            String::new(),
        ];

        lines.push(format!(
            "{} Resolved dependencies",
            if !self.resolved && self.failed {
                CROSS
            } else if self.resolved {
                CHECKMARK
            } else {
                CURRENT
            }
        ));

        if self.fetch_total > 0 || self.fetched || self.fetch_failed {
            let symbol = if self.fetch_failed || (!self.fetched && self.failed) {
                CROSS
            } else if self.fetched {
                CHECKMARK
            } else {
                CURRENT
            };
            let total = self
                .fetch_total
                .max(self.total_packages.unwrap_or_default());
            lines.push(format!(
                "{} Fetched packages {}/{}",
                symbol, self.fetch_seen, total
            ));
        }

        if self.linking || self.linked {
            lines.push(format!(
                "{} Linked dependencies",
                if !self.linked && self.failed {
                    CROSS
                } else if self.linked {
                    CHECKMARK
                } else {
                    CURRENT
                }
            ));
        }

        if let Some(changed) = self.lockfile_changed {
            if changed {
                lines.push(format!("{} Updated lockfile", CHECKMARK));
            } else {
                lines.push(format!("{} Lockfile unchanged", CHECKMARK));
            }
        } else if self.writing_lockfile {
            lines.push(format!(
                "{} Writing lockfile",
                if self.failed { CROSS } else { CURRENT }
            ));
        }

        if let Some(duration_secs) = self.duration_secs {
            lines.push(String::new());
            lines.push(format!("Done in {:.2}s", duration_secs));
        }

        lines
    }
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
