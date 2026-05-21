//! orix CLI entry point.

use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;

mod args;
mod cmd;
mod errors;
mod logging;
mod reporter;

use args::*;
use cmd::*;
use orix_core::{
    normalize_script_args, pipeline, remove, ConfigOverrides, DeployOpts, InstallOpts,
};

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let log_handle = logging::init_logging(logging::LogConfig {
        filter: cli.log.clone(),
        debug: cli.debug,
        log_file: cli.log_file.clone(),
        color_mode: cli.color.clone().into(),
    })?;

    if cli.debug {
        if let Some(path) = log_handle.log_file() {
            eprintln!("debug log: {}", path.display());
        }
    }

    let no_progress = cli.no_progress || log_handle.console_enabled();

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
                            &dir
                        )
                    );
                    std::process::exit(1);
                }

                if let Err(e) =
                    run_install(&dir, &install_opts, cli.color.clone().into(), no_progress).await
                {
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

                let run = match run_add(
                    &dir,
                    &args.packages,
                    dep_type,
                    &install_opts,
                    cli.color.clone().into(),
                    no_progress,
                )
                .await
                {
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

            let run = match run_add(
                &dir,
                &args.packages,
                dep_type,
                &opts,
                cli.color.clone().into(),
                no_progress,
            )
            .await
            {
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

        Command::Run(args) => {
            if let Err(e) = run_script(&dir, &args).await {
                eprintln!("{}", errors::format_error(&e, &dir));
                std::process::exit(1);
            }
        }

        Command::Script(mut parts) => {
            let script = match parts.first() {
                Some(s) => s.clone(),
                None => {
                    eprintln!(
                        "{}",
                        errors::format_error(&anyhow::anyhow!("a script name is required"), &dir)
                    );
                    std::process::exit(1);
                }
            };
            parts.remove(0);
            let args = RunArgs {
                script,
                args: normalize_script_args(parts),
                if_present: false,
                workspace: None,
                recursive: false,
                concurrency: 4,
            };
            if let Err(e) = run_script(&dir, &args).await {
                eprintln!("{}", errors::format_error(&e, &dir));
                std::process::exit(1);
            }
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
                        CHECKMARK,
                        report.packages_imported,
                        input_path.display()
                    );
                    if report.warnings > 0 {
                        println!(" {} {} warnings (see above)", INFO, report.warnings);
                    }
                }
                Err(e) => {
                    eprintln!("{}", errors::format_error(&e, &dir));
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
                        CHECKMARK,
                        report.packages_exported,
                        output_path.display()
                    );
                }
                Err(e) => {
                    eprintln!("{}", errors::format_error(&e, &dir));
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
                        CHECKMARK, report.packages_deployed, report.files_copied
                    );
                }
                Err(e) => {
                    eprintln!("{}", errors::format_error(&e, &dir));
                    std::process::exit(1);
                }
            }
        }
    }

    Ok(())
}
