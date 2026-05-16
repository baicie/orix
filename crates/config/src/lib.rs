//! Configuration loading from defaults, .npmrc files, and environment variables.

#![deny(clippy::unwrap_used, clippy::field_reassign_with_default)]

use std::env;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use url::Url;

pub use rpnpm_domain::PackageName;

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
    /// Run lifecycle scripts (default: false, MVP skips all scripts).
    pub ignore_scripts: bool,
    /// Color output.
    pub color: ColorChoice,
    /// Project root.
    pub project_root: PathBuf,
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

impl Config {
    /// Load configuration by merging defaults, .npmrc files, and environment variables.
    pub fn load(project_root: &Path) -> Result<Self> {
        let project_root = project_root
            .canonicalize()
            .unwrap_or_else(|_| project_root.to_path_buf());

        let mut config = Config {
            #[allow(clippy::expect_used)]
            registry: Url::parse("https://registry.npmjs.org/")
                .expect("default registry URL is always valid"),
            store_dir: dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".rpnpm/store/v1"),
            cache_dir: dirs::cache_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("rpnpm/tarballs"),
            auth_token: None,
            concurrency: 10,
            fetch_timeout_secs: 30,
            fetch_retries: 3,
            ignore_scripts: true,
            color: ColorChoice::Auto,
            project_root,
        };

        if let Some(home) = dirs::home_dir() {
            let global_rc = home.join(".npmrc");
            if global_rc.exists() {
                config.merge_file(&global_rc)?;
            }
        }

        let project_rc = config.project_root.join(".npmrc");
        if project_rc.exists() {
            config.merge_file(&project_rc)?;
        }

        config.merge_env();

        Ok(config)
    }

    fn merge_file(&mut self, path: &Path) -> Result<()> {
        let source = std::fs::read_to_string(path)?;
        for line in source.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((key, value)) = line.split_once('=') {
                self.set(key.trim(), value.trim());
            }
        }
        Ok(())
    }

    fn merge_env(&mut self) {
        if let Ok(v) = env::var("RPNPM_REGISTRY") {
            if let Ok(u) = Url::parse(&v) {
                self.registry = u;
            }
        }
        if let Ok(v) = env::var("RPNPM_STORE") {
            self.store_dir = PathBuf::from(v);
        }
        if let Ok(v) = env::var("RPNPM_CACHE") {
            self.cache_dir = PathBuf::from(v);
        }
        if let Ok(v) = env::var("RPNPM_CONCURRENCY") {
            self.concurrency = v.parse().unwrap_or(self.concurrency);
        }
        if let Ok(v) = env::var("RPNPM_IGNORE_SCRIPTS") {
            self.ignore_scripts = v == "true" || v == "1";
        }
    }

    fn set(&mut self, key: &str, value: &str) {
        match key {
            "registry" => {
                if let Ok(u) = Url::parse(value) {
                    self.registry = u;
                }
            }
            "store-dir" => {
                self.store_dir = PathBuf::from(
                    value.replace(
                        '~',
                        &dirs::home_dir()
                            .map(|p| p.display().to_string())
                            .unwrap_or_default(),
                    ),
                )
            }
            "cache-dir" => {
                self.cache_dir = PathBuf::from(
                    value.replace(
                        '~',
                        &dirs::home_dir()
                            .map(|p| p.display().to_string())
                            .unwrap_or_default(),
                    ),
                )
            }
            "fetch-retries" => self.fetch_retries = value.parse().unwrap_or(self.fetch_retries),
            "fetch-timeout" => {
                self.fetch_timeout_secs = value.parse().unwrap_or(self.fetch_timeout_secs)
            }
            "concurrency" => self.concurrency = value.parse().unwrap_or(self.concurrency),
            "ignore-scripts" => self.ignore_scripts = value == "true" || value == "1",
            "color" => {
                self.color = match value {
                    "always" => ColorChoice::Always,
                    "never" => ColorChoice::Never,
                    _ => ColorChoice::Auto,
                };
            }
            k if k.starts_with("_authToken") || k.ends_with("/:_authToken") => {
                self.auth_token = Some(value.to_string());
            }
            _ => {}
        }
    }

    /// Path to the lockfile for this project.
    pub fn lockfile_path(&self) -> PathBuf {
        self.project_root.join("rpnpm-lock.yaml")
    }

    /// Path to the node_modules directory for this project.
    pub fn node_modules_dir(&self) -> PathBuf {
        self.project_root.join("node_modules")
    }
}
