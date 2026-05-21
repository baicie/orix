//! Pipeline submodule.

use super::prelude::*;
use super::types::CacheCleanReport;
/// Resolve the global CAS store path for a project.
pub fn store_path(project_root: &Path) -> Result<PathBuf> {
    store_path_with_overrides(project_root, &ConfigOverrides::default())
}

/// Return the resolved store path for this project using explicit overrides.
pub fn store_path_with_overrides(
    project_root: &Path,
    overrides: &ConfigOverrides,
) -> Result<PathBuf> {
    let config = Config::load_with_overrides(project_root, overrides)
        .with_context(|| "failed to load configuration")?;
    Ok(config.store_dir)
}

/// Prune packages from the store that are not referenced by this project's lockfile.
pub fn store_prune(project_root: &Path, dry_run: bool) -> Result<orix_store::PruneReport> {
    store_prune_with_overrides(project_root, dry_run, &ConfigOverrides::default())
}

/// Prune packages from the configured store using explicit overrides.
pub fn store_prune_with_overrides(
    project_root: &Path,
    dry_run: bool,
    overrides: &ConfigOverrides,
) -> Result<orix_store::PruneReport> {
    let config = Config::load_with_overrides(project_root, overrides)
        .with_context(|| "failed to load configuration")?;
    let lockfile_path = config.lockfile_path();
    if !lockfile_path.exists() {
        anyhow::bail!(
            "No lockfile found at {}. Run orix install before pruning the store.",
            lockfile_path.display()
        );
    }

    let lockfile = Lockfile::read(&lockfile_path).with_context(|| "failed to read lockfile")?;
    use std::collections::HashSet;
    let referenced: HashSet<_> = lockfile.package_ids()?.into_iter().collect();
    let store = Store::open(config.store_dir).with_context(|| "failed to open store")?;
    store.prune(&referenced, dry_run, true)
}

/// Verify all packages and content-addressable files in the store.
pub fn store_verify(project_root: &Path) -> Result<orix_store::VerifyReport> {
    store_verify_with_overrides(project_root, &ConfigOverrides::default())
}

/// Verify all packages and content-addressable files in the configured store.
pub fn store_verify_with_overrides(
    project_root: &Path,
    overrides: &ConfigOverrides,
) -> Result<orix_store::VerifyReport> {
    let config = Config::load_with_overrides(project_root, overrides)
        .with_context(|| "failed to load configuration")?;
    let store = Store::open(config.store_dir).with_context(|| "failed to open store")?;
    store.verify()
}

/// Return the resolved tarball cache path for this project.
pub fn cache_path(project_root: &Path) -> Result<PathBuf> {
    cache_path_with_overrides(project_root, &ConfigOverrides::default())
}

/// Return the resolved tarball cache path for this project using explicit overrides.
pub fn cache_path_with_overrides(
    project_root: &Path,
    overrides: &ConfigOverrides,
) -> Result<PathBuf> {
    let config = Config::load_with_overrides(project_root, overrides)
        .with_context(|| "failed to load configuration")?;
    Ok(config.cache_dir)
}

/// Remove all tarballs from the configured cache directory.
pub fn cache_clean(project_root: &Path) -> Result<CacheCleanReport> {
    cache_clean_with_overrides(project_root, &ConfigOverrides::default())
}

/// Remove all tarballs from the configured cache directory using explicit overrides.
pub fn cache_clean_with_overrides(
    project_root: &Path,
    overrides: &ConfigOverrides,
) -> Result<CacheCleanReport> {
    let path = cache_path_with_overrides(project_root, overrides)?;
    let existed = path.exists();
    let bytes_reclaimed = if existed { dir_size(&path) } else { 0 };

    if existed {
        fs::remove_dir_all(&path)
            .with_context(|| format!("failed to remove cache directory {}", path.display()))?;
    }
    fs::create_dir_all(&path)
        .with_context(|| format!("failed to create cache directory {}", path.display()))?;

    Ok(CacheCleanReport {
        path,
        existed,
        bytes_reclaimed,
    })
}

pub(crate) fn dir_size(path: &Path) -> u64 {
    let Ok(entries) = fs::read_dir(path) else {
        return 0;
    };

    entries
        .filter_map(Result::ok)
        .map(|entry| {
            let path = entry.path();
            match entry.metadata() {
                Ok(metadata) if metadata.is_dir() => dir_size(&path),
                Ok(metadata) if metadata.is_file() => metadata.len(),
                _ => 0,
            }
        })
        .sum()
}
