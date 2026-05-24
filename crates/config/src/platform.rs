//! Platform-specific configuration helpers.

use std::env;
#[cfg(windows)]
use std::path::Component;
use std::path::{Path, PathBuf};
pub(crate) fn first_env<const N: usize>(keys: [&str; N]) -> Option<String> {
    keys.into_iter().find_map(|key| env::var(key).ok())
}

#[allow(unused_variables)]
pub(crate) fn default_store_dir(project_root: &Path) -> PathBuf {
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
