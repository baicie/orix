#[cfg(test)]
mod tests {
    use std::env;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::Mutex;

    use crate::platform::default_store_dir;
    use crate::{Config, ConfigOverrides};

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
