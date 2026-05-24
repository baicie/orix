//! Shared utility helpers.

use std::io;
use std::path::{Component, Path, PathBuf};

/// Normalizes a display name.
#[must_use]
pub fn normalize_name(input: &str) -> String {
    let trimmed = input.trim();

    if trimmed.is_empty() {
        "world".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Normalize a path for lockfile serialization by using `/` separators.
#[must_use]
pub fn normalize_path_for_lockfile(path: &Path) -> String {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(part) => part.to_str().map(ToOwned::to_owned),
            Component::CurDir => Some(".".to_string()),
            Component::ParentDir => Some("..".to_string()),
            Component::RootDir | Component::Prefix(_) => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// Compute a relative path from one directory to another path.
#[must_use]
pub fn relative_path(from_dir: &Path, to_path: &Path) -> PathBuf {
    let from_components = normal_components(from_dir);
    let to_components = normal_components(to_path);
    let common_len = from_components
        .iter()
        .zip(to_components.iter())
        .take_while(|(a, b)| a == b)
        .count();

    let mut result = PathBuf::new();
    for _ in common_len..from_components.len() {
        result.push("..");
    }
    for component in &to_components[common_len..] {
        result.push(component);
    }

    if result.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        result
    }
}

/// Ensure the parent directory for a path exists.
pub fn ensure_parent_dir(path: &Path) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    Ok(())
}

/// Atomically write bytes to a path using a temporary sibling file then rename.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    ensure_parent_dir(path)?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("orix-file");
    let tmp = path.with_file_name(format!(".{}.{}.tmp", file_name, std::process::id()));
    if let Err(error) = std::fs::write(&tmp, bytes).and_then(|()| std::fs::rename(&tmp, path)) {
        let _ = std::fs::remove_file(&tmp);
        return Err(error);
    }
    Ok(())
}

fn normal_components(path: &Path) -> Vec<String> {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(part) => part.to_str().map(ToOwned::to_owned),
            Component::CurDir => None,
            Component::ParentDir => Some("..".to_string()),
            Component::RootDir | Component::Prefix(_) => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_name_falls_back_to_world() {
        assert_eq!(normalize_name("  "), "world");
    }

    #[test]
    fn normalize_path_for_lockfile_uses_forward_slashes() {
        let path = Path::new("packages").join("app").join("package.json");

        assert_eq!(
            normalize_path_for_lockfile(&path),
            "packages/app/package.json"
        );
    }

    #[test]
    fn relative_path_moves_between_sibling_directories() {
        let from = Path::new("node_modules").join(".orix").join("a");
        let to = Path::new("node_modules")
            .join(".orix")
            .join("b")
            .join("pkg");

        assert_eq!(relative_path(&from, &to), PathBuf::from("../b/pkg"));
    }

    #[test]
    fn atomic_write_creates_parent_directory() -> io::Result<()> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("nested").join("file.txt");

        atomic_write(&path, b"hello")?;

        assert_eq!(std::fs::read(&path)?, b"hello");
        Ok(())
    }
}
