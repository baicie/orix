//! CLI argument definitions.

use std::path::PathBuf;

use clap::{ArgAction, Args, Parser, Subcommand, ValueEnum};

use crate::reporter::ColorMode;

#[derive(Parser)]
#[command(name = "orix")]
#[command(
    version = option_env!("ORIX_VERSION").unwrap_or(env!("CARGO_PKG_VERSION")),
    about = "Fast, disk-space efficient package manager written in Rust"
)]
pub struct Cli {
    #[arg(long, global = true, env = "ORIX_REGISTRY")]
    pub registry: Option<String>,

    #[arg(long, global = true, env = "ORIX_LOG", value_name = "FILTER")]
    pub log: Option<String>,

    #[arg(long, global = true, env = "ORIX_DEBUG", action = ArgAction::SetTrue)]
    pub debug: bool,

    #[arg(long, global = true, env = "ORIX_LOG_FILE", value_name = "FILE")]
    pub log_file: Option<PathBuf>,

    #[arg(long, global = true, env = "ORIX_NO_PROGRESS", action = ArgAction::SetTrue)]
    pub no_progress: bool,

    #[arg(long, short = 'C', default_value = ".", env = "ORIX_DIR")]
    pub dir: PathBuf,

    #[arg(long, global = true, env = "ORIX_STORE", value_name = "DIR")]
    pub store_dir: Option<PathBuf>,

    #[arg(long, global = true, env = "ORIX_CACHE", value_name = "DIR")]
    pub cache_dir: Option<PathBuf>,

    #[arg(long, global = true, default_value = "auto")]
    pub color: ColorChoice,

    /// Run the script in a specific workspace package.
    #[arg(long, global = true)]
    pub workspace: Option<String>,

    /// Filter workspace packages by selector.
    /// Supports: ./path, ./glob/*, package-name
    #[arg(long = "filter", global = true)]
    pub filter: Vec<String>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    #[command(alias = "i")]
    Install(InstallArgs),
    Add(AddArgs),
    Remove(RemoveArgs),
    #[command(name = "run")]
    Run(RunArgs),
    /// Implicit script execution (`orix <script> [args...]`, same as `orix run`).
    #[command(external_subcommand)]
    Script(Vec<String>),
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
    /// Remove node_modules and the lockfile.
    Prune(PruneArgs),
}

#[derive(Subcommand)]
pub enum StoreCommand {
    Path,
    Prune {
        #[arg(long)]
        dry_run: bool,
    },
    Verify,
}

#[derive(Subcommand)]
pub enum CacheCommand {
    Path,
    Clean,
}

#[derive(Args)]
pub struct InstallArgs {
    /// Fail if the lockfile is missing or out of date.
    #[arg(long)]
    pub frozen_lockfile: bool,
    /// Use only locally cached packages.
    #[arg(long)]
    pub offline: bool,
    /// Re-fetch packages regardless of cache.
    #[arg(long)]
    pub force: bool,
    /// Skip lifecycle scripts.
    #[arg(long)]
    pub ignore_scripts: bool,
    /// Number of concurrent package fetches.
    #[arg(long, default_value = "10")]
    pub concurrency: usize,
    /// Save named packages as dev dependencies.
    #[arg(short = 'D')]
    pub dev: bool,
    /// Save named packages as optional dependencies.
    #[arg(short = 'O')]
    pub optional: bool,
    /// Package names or specs to add before installing.
    #[arg(trailing_var_arg = true)]
    pub packages: Vec<String>,
}

#[derive(Args)]
pub struct AddArgs {
    #[arg(short = 'D')]
    pub dev: bool,
    #[arg(short = 'O')]
    pub optional: bool,
    #[arg(trailing_var_arg = true)]
    pub packages: Vec<String>,
}

#[derive(Args)]
pub struct RemoveArgs {
    #[arg(trailing_var_arg = true)]
    pub packages: Vec<String>,
}

#[derive(Args)]
pub struct RunArgs {
    /// Script name to run.
    pub script: String,
    /// Additional arguments to pass to the script (after the script name; `-` flags allowed).
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<String>,
    /// Do not error if the script is not defined.
    #[arg(long)]
    pub if_present: bool,
    /// Run the script in a specific workspace package.
    #[arg(long)]
    pub workspace: Option<String>,
    /// Run the script recursively across all workspace packages.
    #[arg(long, short = 'r')]
    pub recursive: bool,
    /// Run workspace scripts in parallel (requires --recursive).
    #[arg(long)]
    pub parallel: bool,
    /// Filter workspace packages by selector.
    /// Supports: ./path, ./glob/*, package-name
    #[arg(long = "filter")]
    pub filter: Vec<String>,
    /// Maximum number of concurrent workspace scripts (for --recursive).
    #[arg(long, default_value = "4")]
    pub concurrency: usize,
}

#[derive(Args)]
pub struct ImportArgs {
    /// Source lockfile format. Defaults to pnpm-lock.yaml.
    #[arg(long, value_enum, default_value = "pnpm")]
    pub from: LockfileFormat,
    /// Input file path. Defaults to pnpm-lock.yaml in the project root.
    #[arg(default_value = "pnpm-lock.yaml")]
    pub path: PathBuf,
}

#[derive(Args)]
pub struct ExportArgs {
    /// Output format. Defaults to pnpm-lock.yaml.
    #[arg(long, value_enum, default_value = "pnpm")]
    pub to: LockfileFormat,
    /// Output file path. Defaults to pnpm-lock.yaml in the project root.
    #[arg(default_value = "pnpm-lock.yaml")]
    pub path: PathBuf,
}

#[derive(Args)]
pub struct DeployArgs {
    /// Package name or path glob to deploy (required).
    #[arg(short = 'F', long, required = true)]
    pub filter: String,
    /// Output directory for the deployed package.
    #[arg(short, long, required = true)]
    pub output: PathBuf,
    /// Only include production dependencies (skip devDependencies).
    #[arg(long, short = 'p')]
    pub prod: bool,
    /// Use frozen lockfile (no registry interaction).
    #[arg(long)]
    pub frozen_lockfile: bool,
    /// Run deploy hooks (predeploy, postdeploy).
    #[arg(long)]
    pub hooks: bool,
}

/// Prune command arguments.
#[derive(Args, Debug)]
pub struct PruneArgs {
    /// Do not remove the lockfile.
    #[arg(long)]
    pub keep_lockfile: bool,
    /// Preview changes without deleting anything.
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(ValueEnum, Clone, Default)]
pub enum LockfileFormat {
    #[default]
    Pnpm,
}

#[derive(ValueEnum, Clone, Default)]
pub enum ColorChoice {
    Always,
    Never,
    #[default]
    Auto,
}

impl From<ColorChoice> for ColorMode {
    fn from(c: ColorChoice) -> Self {
        match c {
            ColorChoice::Always => ColorMode::Always,
            ColorChoice::Never => ColorMode::Never,
            ColorChoice::Auto => ColorMode::Auto,
        }
    }
}
