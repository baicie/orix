//! Prune command: remove node_modules and optionally the lockfile.

use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

/// Report from a `prune` operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PruneReport {
    /// Size of node_modules in bytes before deletion (best-effort).
    pub node_modules_bytes: u64,
    /// Whether the lockfile was removed.
    pub lockfile_removed: bool,
    /// Number of items removed from node_modules.
    pub items_removed: usize,
}

fn estimate_dir_size(path: &Path) -> u64 {
    walkdir::WalkDir::new(path)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter_map(|e| e.metadata().ok())
        .filter(|m| m.is_file())
        .map(|m| m.len())
        .sum()
}

fn count_dir_items(path: &Path) -> usize {
    walkdir::WalkDir::new(path)
        .into_iter()
        .filter_map(|e| e.ok())
        .count()
}

/// Remove node_modules and optionally the lockfile.
///
/// Does NOT reinstall. After pruning, run `orix install` to reinstall.
pub fn prune(project_root: &Path, keep_lockfile: bool, dry_run: bool) -> Result<PruneReport> {
    let node_modules = project_root.join("node_modules");
    let lockfile_path = project_root.join("orix-lock.yaml");

    let (node_modules_bytes, items_removed) = if node_modules.is_dir() {
        let size = estimate_dir_size(&node_modules);
        let count = count_dir_items(&node_modules);
        (size, count)
    } else {
        (0, 0)
    };

    let lockfile_exists = lockfile_path.exists();

    if dry_run {
        if items_removed > 0 {
            info!(
                node_modules_size = node_modules_bytes,
                lockfile_present = lockfile_exists,
                "prune: would remove node_modules ({} items, {} bytes) and lockfile",
                items_removed,
                node_modules_bytes,
            );
        } else {
            info!("prune: nothing to remove");
        }
        return Ok(PruneReport {
            node_modules_bytes,
            lockfile_removed: lockfile_exists && !keep_lockfile,
            items_removed,
        });
    }

    // Remove node_modules.
    if node_modules.is_dir() {
        debug!(
            path = %node_modules.display(),
            size_bytes = node_modules_bytes,
            item_count = items_removed,
            "removing node_modules"
        );
        remove_dir_all_with_retry(&node_modules, 3)?;
    }

    // Remove lockfile unless --keep-lockfile.
    let lockfile_removed = if !keep_lockfile && lockfile_path.exists() {
        debug!(path = %lockfile_path.display(), "removing lockfile");
        std::fs::remove_file(&lockfile_path)
            .with_context(|| format!("failed to remove {}", lockfile_path.display()))?;
        true
    } else {
        false
    };

    info!(
        removed_items = items_removed,
        bytes_freed = node_modules_bytes,
        lockfile_removed,
        "prune complete. Run `orix install` to reinstall."
    );

    Ok(PruneReport {
        node_modules_bytes,
        lockfile_removed,
        items_removed,
    })
}

/// Retry-capable directory removal for Windows, where files can be locked
/// briefly by antivirus or indexers.
fn remove_dir_all_with_retry(path: &Path, retries: u32) -> Result<()> {
    #[cfg(windows)]
    {
        use std::thread;
        use std::time::Duration;

        for attempt in 0..=retries {
            match std::fs::remove_dir_all(path) {
                Ok(()) => return Ok(()),
                Err(e) if attempt < retries => {
                    let delay = Duration::from_millis(500 * (1 << attempt));
                    debug!(
                        attempt = attempt + 1,
                        path = %path.display(),
                        error = %e,
                        "could not remove directory, retrying in {delay:?}"
                    );
                    thread::sleep(delay);
                }
                Err(e) => {
                    anyhow::bail!("failed to remove directory {}: {}", path.display(), e);
                }
            }
        }
    }

    #[cfg(not(windows))]
    {
        let _ = (path, retries);
        std::fs::remove_dir_all(path)
            .with_context(|| format!("failed to remove directory {}", path.display()))?;
    }

    Ok(())
}
