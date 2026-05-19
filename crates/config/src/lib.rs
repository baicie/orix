//! Configuration loading from defaults, .npmrc files, and environment variables.

#![deny(clippy::unwrap_used, clippy::field_reassign_with_default)]

use std::env;
#[cfg(windows)]
use std::path::Component;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use url::Url;

pub use orix_domain::PackageName;

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

fn first_env<const N: usize>(keys: [&str; N]) -> Option<String> {
    keys.into_iter().find_map(|key| env::var(key).ok())
}

fn default_store_dir(project_root: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        if let Some(root) = volume_root(project_root) {
            return root.join(".orix").join("store");
        }
    }

    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".orix")
        .join("store")
}

#[cfg(windows)]
fn volume_root(path: &Path) -> Option<PathBuf> {
    let mut components = path.components();
    let Component::Prefix(prefix) = components.next()? else {
        return None;
    };

    let mut root = PathBuf::from(prefix.as_os_str());
    if matches!(components.next(), Some(Component::RootDir)) {
        root.push("\\");
    }
    Some(root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct EnvGuard {
        key: &'static str,
        previous: Option<String>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let previous = env::var(key).ok();
            env::set_var(key, value);
            Self { key, previous }
        }

        fn remove(key: &'static str) -> Self {
            let previous = env::var(key).ok();
            env::remove_var(key);
            Self { key, previous }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            if let Some(previous) = &self.previous {
                env::set_var(self.key, previous);
            } else {
                env::remove_var(self.key);
            }
        }
    }

    #[test]
    fn load_supports_rpnpm_env_aliases() -> anyhow::Result<()> {
        let _lock = ENV_LOCK
            .lock()
            .map_err(|error| anyhow::anyhow!("env lock poisoned: {}", error))?;
        let temp = tempfile::tempdir()?;
        let _orix_registry = EnvGuard::remove("ORIX_REGISTRY");
        let _orix_store = EnvGuard::remove("ORIX_STORE");
        let _registry = EnvGuard::set("RPNPM_REGISTRY", "https://registry.example.test/");
        let _store = EnvGuard::set("RPNPM_STORE", "D:/orix-store-test");

        let config = Config::load(temp.path())?;

        assert_eq!(config.registry.as_str(), "https://registry.example.test/");
        assert_eq!(config.store_dir, PathBuf::from("D:/orix-store-test"));
        Ok(())
    }

    #[test]
    fn explicit_overrides_win_over_environment() -> anyhow::Result<()> {
        let _lock = ENV_LOCK
            .lock()
            .map_err(|error| anyhow::anyhow!("env lock poisoned: {}", error))?;
        let temp = tempfile::tempdir()?;
        let _registry = EnvGuard::set("ORIX_REGISTRY", "https://env.example.test/");

        let config = Config::load_with_overrides(
            temp.path(),
            &ConfigOverrides {
                registry: Some("https://cli.example.test/".to_string()),
                store_dir: None,
                cache_dir: None,
                ignore_scripts: None,
                allow_scripts: None,
            },
        )?;

        assert_eq!(config.registry.as_str(), "https://cli.example.test/");
        Ok(())
    }

    #[test]
    fn explicit_path_overrides_win_over_environment() -> anyhow::Result<()> {
        let _lock = ENV_LOCK
            .lock()
            .map_err(|error| anyhow::anyhow!("env lock poisoned: {}", error))?;
        let temp = tempfile::tempdir()?;
        let _store = EnvGuard::set("ORIX_STORE", "C:/orix-env-store");
        let _cache = EnvGuard::set("ORIX_CACHE", "C:/orix-env-cache");

        let config = Config::load_with_overrides(
            temp.path(),
            &ConfigOverrides {
                registry: None,
                store_dir: Some(PathBuf::from("D:/orix-cli-store")),
                cache_dir: Some(PathBuf::from("D:/orix-cli-cache")),
                ignore_scripts: None,
                allow_scripts: None,
            },
        )?;

        assert_eq!(config.store_dir, PathBuf::from("D:/orix-cli-store"));
        assert_eq!(config.cache_dir, PathBuf::from("D:/orix-cli-cache"));
        Ok(())
    }

    #[test]
    fn hoist_patterns_default_to_star() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let config = Config::load(temp.path())?;
        assert_eq!(config.hoist_patterns, vec!["*"]);
        Ok(())
    }

    #[test]
    fn hoist_patterns_parsed_from_npmrc() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        fs::write(
            temp.path().join(".npmrc"),
            "hoist-patterns=@types/* *babel* *jest*",
        )?;
        let config = Config::load(temp.path())?;
        assert_eq!(config.hoist_patterns, vec!["@types/*", "*babel*", "*jest*"]);
        Ok(())
    }

    #[test]
    fn side_effects_cache_defaults_to_true() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let config = Config::load(temp.path())?;
        assert!(config.side_effects_cache);
        Ok(())
    }

    #[test]
    fn ignore_scripts_defaults_to_false() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let config = Config::load(temp.path())?;
        assert!(!config.ignore_scripts);
        Ok(())
    }

    #[test]
    fn allow_scripts_defaults_to_empty() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let config = Config::load(temp.path())?;
        assert!(config.allow_scripts.is_empty());
        Ok(())
    }

    #[test]
    fn allow_scripts_parsed_from_npmrc() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        fs::write(
            temp.path().join(".npmrc"),
            "allow-scripts=esbuild,@swc/core",
        )?;
        let config = Config::load(temp.path())?;
        assert_eq!(config.allow_scripts, vec!["esbuild", "@swc/core"]);
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn default_store_dir_uses_project_volume_on_windows() -> anyhow::Result<()> {
        let project_root = PathBuf::from(r"D:\workspace\project");

        assert_eq!(
            default_store_dir(&project_root),
            PathBuf::from(r"D:\.orix\store")
        );
        Ok(())
    }
}
