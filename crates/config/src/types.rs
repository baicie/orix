//! Configuration types.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use url::Url;

/// Application-wide configuration.
#[derive(Clone, Debug)]
pub struct Config {
    /// Registry base URL.
    pub registry: Url,
    /// Global store directory.
    pub store_dir: PathBuf,
    /// Local tarball cache directory.
    pub cache_dir: PathBuf,
    /// HTTP auth token (optional).
    pub auth_token: Option<String>,
    /// Number of concurrent downloads.
    pub concurrency: usize,
    /// HTTP timeout in seconds.
    pub fetch_timeout_secs: u64,
    /// Number of fetch retries.
    pub fetch_retries: u32,
    /// Skip running lifecycle scripts during install.
    pub ignore_scripts: bool,
    /// Package name allowlist for running dependency lifecycle scripts.
    /// Only packages in this list will have their lifecycle scripts executed.
    /// An empty list means only project scripts are allowed.
    pub allow_scripts: Vec<String>,
    /// Save exact versions instead of caret/tilde in package.json.
    pub save_exact: bool,
    /// Fail install if engine constraints are not met.
    pub engine_strict: bool,
    /// Color output.
    pub color: ColorChoice,
    /// Glob patterns for packages hoisted to the root node_modules.
    pub hoist_patterns: Vec<String>,
    /// Whether to use the side-effects cache for lifecycle scripts.
    pub side_effects_cache: bool,
    /// Project root.
    pub project_root: PathBuf,
}

/// Explicit configuration overrides, usually produced by CLI arguments.
#[derive(Clone, Debug, Default)]
pub struct ConfigOverrides {
    /// Registry base URL override.
    pub registry: Option<String>,
    /// Global store directory override.
    pub store_dir: Option<PathBuf>,
    /// Local tarball cache directory override.
    pub cache_dir: Option<PathBuf>,
    /// Skip running lifecycle scripts.
    pub ignore_scripts: Option<bool>,
    /// Package name allowlist for running dependency lifecycle scripts.
    pub allow_scripts: Option<Vec<String>>,
}

/// Color output preference.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ColorChoice {
    /// Always use colors.
    Always,
    /// Never use colors.
    Never,
    /// Use colors when outputting to a terminal.
    #[default]
    Auto,
}
