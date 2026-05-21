//! Configuration loading and merging.

use std::path::{Path, PathBuf};

use anyhow::Result;
use url::Url;

use super::platform::{default_store_dir, first_env};
use super::types::{ColorChoice, Config, ConfigOverrides};

impl Config {
    /// Load configuration by merging defaults, .npmrc files, and environment variables.
    pub fn load(project_root: &Path) -> Result<Self> {
        Self::load_with_overrides(project_root, &ConfigOverrides::default())
    }

    /// Load configuration and then apply explicit overrides such as CLI arguments.
    pub fn load_with_overrides(project_root: &Path, overrides: &ConfigOverrides) -> Result<Self> {
        let project_root = project_root
            .canonicalize()
            .unwrap_or_else(|_| project_root.to_path_buf());

        let mut config = Config {
            #[allow(clippy::expect_used)]
            registry: Url::parse("https://registry.npmjs.org/")
                .expect("default registry URL is always valid"),
            store_dir: default_store_dir(&project_root),
            cache_dir: dirs::cache_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("orix/tarballs"),
            auth_token: None,
            concurrency: 10,
            fetch_timeout_secs: 30,
            fetch_retries: 3,
            ignore_scripts: false,
            allow_scripts: Vec::new(),
            save_exact: false,
            engine_strict: false,
            color: ColorChoice::Auto,
            hoist_patterns: vec!["*".to_string()],
            side_effects_cache: true,
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
        config.merge_overrides(overrides);

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
        if let Some(v) = first_env(["ORIX_REGISTRY", "RPNPM_REGISTRY"]) {
            if let Ok(u) = Url::parse(&v) {
                self.registry = u;
            }
        }
        if let Some(v) = first_env(["ORIX_STORE", "RPNPM_STORE"]) {
            self.store_dir = PathBuf::from(v);
        }
        if let Some(v) = first_env(["ORIX_CACHE", "RPNPM_CACHE"]) {
            self.cache_dir = PathBuf::from(v);
        }
        if let Some(v) = first_env(["ORIX_CONCURRENCY", "RPNPM_CONCURRENCY"]) {
            self.concurrency = v.parse().unwrap_or(self.concurrency);
        }
        if let Some(v) = first_env(["ORIX_IGNORE_SCRIPTS", "RPNPM_IGNORE_SCRIPTS"]) {
            self.ignore_scripts = v == "true" || v == "1";
        }
        if let Some(v) = first_env(["ORIX_HOIST_PATTERNS", "RPNPM_HOIST_PATTERNS"]) {
            self.hoist_patterns = v.split_whitespace().map(String::from).collect();
        }
        if let Some(v) = first_env(["ORIX_SIDE_EFFECTS_CACHE", "RPNPM_SIDE_EFFECTS_CACHE"]) {
            self.side_effects_cache = v == "true" || v == "1";
        }
        if let Some(v) = first_env(["ORIX_ALLOW_SCRIPTS", "RPNPM_ALLOW_SCRIPTS"]) {
            self.allow_scripts = v.split(',').map(String::from).collect();
        }
    }

    fn merge_overrides(&mut self, overrides: &ConfigOverrides) {
        if let Some(registry) = &overrides.registry {
            if let Ok(url) = Url::parse(registry) {
                self.registry = url;
            }
        }
        if let Some(store_dir) = &overrides.store_dir {
            self.store_dir = store_dir.clone();
        }
        if let Some(cache_dir) = &overrides.cache_dir {
            self.cache_dir = cache_dir.clone();
        }
        if let Some(ignore_scripts) = overrides.ignore_scripts {
            self.ignore_scripts = ignore_scripts;
        }
        if let Some(allow_scripts) = &overrides.allow_scripts {
            self.allow_scripts = allow_scripts.clone();
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
            "allow-scripts" => {
                self.allow_scripts = value.split(',').map(String::from).collect();
            }
            "save-exact" => self.save_exact = value == "true" || value == "1",
            "engine-strict" => self.engine_strict = value == "true" || value == "1",
            "hoist-patterns" => {
                self.hoist_patterns = value.split_whitespace().map(String::from).collect();
            }
            "side-effects-cache" => self.side_effects_cache = value == "true" || value == "1",
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
        self.project_root.join("orix-lock.yaml")
    }

    /// Path to the node_modules directory for this project.
    pub fn node_modules_dir(&self) -> PathBuf {
        self.project_root.join("node_modules")
    }
}
