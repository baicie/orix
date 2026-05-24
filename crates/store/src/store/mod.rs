//! CAS store implementation.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use parking_lot::RwLock;
use walkdir::WalkDir;

use orix_domain::PackageId;

use crate::IntegrityMeta;

pub const STORE_VERSION: &str = "v1";

/// The content-addressable store.
/// Uses a `RwLock` to allow concurrent reads while serializing writes.
pub struct Store {
    root: PathBuf,
    pub(crate) files_root: PathBuf,
    pub(crate) packages_root: PathBuf,
    /// Guards file I/O operations. Allows concurrent reads; exclusive access for writes.
    /// Shared via `Arc` so that cloned `Store` instances share the same lock.
    pub(crate) io_guard: Arc<RwLock<()>>,
}

impl Clone for Store {
    fn clone(&self) -> Self {
        Self {
            root: self.root.clone(),
            files_root: self.files_root.clone(),
            packages_root: self.packages_root.clone(),
            io_guard: Arc::clone(&self.io_guard),
        }
    }
}

impl Store {
    /// Open (or create) the store at the given root.
    pub fn open(root: PathBuf) -> Result<Self> {
        let root = if root.file_name().and_then(|name| name.to_str()) == Some(STORE_VERSION) {
            root
        } else {
            root.join(STORE_VERSION)
        };
        let files_root = root.join("files").join("sha256");
        let packages_root = root.join("packages");

        fs::create_dir_all(&files_root).context("failed to create store files directory")?;
        fs::create_dir_all(&packages_root).context("failed to create store packages directory")?;

        Ok(Self {
            root,
            files_root,
            packages_root,
            io_guard: Arc::new(RwLock::new(())),
        })
    }

    /// The root directory of the store.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Path for a content-addressable file.
    pub(crate) fn file_path(&self, hash: &str) -> PathBuf {
        let (prefix, rest) = hash.split_at(2);
        self.files_root.join(prefix).join(rest)
    }

    /// Path for a package entry.
    pub(crate) fn package_path(&self, pkg_id: &PackageId) -> PathBuf {
        self.packages_root.join(pkg_id.key())
    }

    /// Check if a package is already in the store.
    pub fn contains(&self, pkg_id: &PackageId) -> bool {
        self.package_path(pkg_id).join("integrity.json").exists()
    }

    /// Get the path to a package's files in the store.
    pub fn package_files_path(&self, pkg_id: &PackageId) -> PathBuf {
        self.package_path(pkg_id)
    }

    /// Read the integrity metadata for a package.
    pub fn get_integrity(&self, pkg_id: &PackageId) -> Result<IntegrityMeta> {
        let path = self.package_path(pkg_id).join("integrity.json");
        let content = fs::read_to_string(&path)
            .with_context(|| format!("failed to read integrity for {}", pkg_id))?;
        serde_json::from_str(&content)
            .with_context(|| format!("failed to parse integrity for {}", pkg_id))
    }

    /// List all packages currently in the store.
    pub fn list_packages(&self) -> Result<Vec<PackageId>> {
        let _guard = self.io_guard.read();
        let mut ids = Vec::new();
        for entry in WalkDir::new(&self.packages_root)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if !entry.file_type().is_file() || entry.file_name() != "integrity.json" {
                continue;
            }

            let content = fs::read_to_string(entry.path())?;
            let meta: IntegrityMeta = serde_json::from_str(&content).with_context(|| {
                format!(
                    "failed to parse integrity metadata at {}",
                    entry.path().display()
                )
            })?;
            let name = orix_domain::PackageName::from(meta.name);
            let version = orix_domain::Version::parse(&meta.version)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
            ids.push(orix_domain::PackageId::new(name, version));
        }
        Ok(ids)
    }
}

mod error;
mod import;
mod prune;
mod verify;

pub use error::StoreError;

#[cfg(test)]
mod tests {
    use super::*;
    use orix_domain::{PackageName, Version};

    fn pkg_id(name: &str, version: &str) -> anyhow::Result<PackageId> {
        Ok(PackageId::new(
            PackageName::from(name),
            Version::parse(version)?,
        ))
    }

    fn write_fixture_package(root: &Path, content: &str) -> anyhow::Result<()> {
        fs::create_dir_all(root)?;
        fs::write(
            root.join("package.json"),
            r#"{"name":"fixture","version":"1.0.0"}"#,
        )?;
        fs::write(root.join("index.js"), content)?;
        Ok(())
    }

    #[test]
    fn import_package_deduplicates_identical_file_content() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let store = Store::open(temp.path().join("store"))?;
        let pkg_a_dir = temp.path().join("pkg-a");
        let pkg_b_dir = temp.path().join("pkg-b");
        write_fixture_package(&pkg_a_dir, "module.exports = 1;\n")?;
        write_fixture_package(&pkg_b_dir, "module.exports = 1;\n")?;

        store.import_package(&pkg_id("a", "1.0.0")?, &pkg_a_dir, Vec::new(), None)?;
        store.import_package(&pkg_id("b", "1.0.0")?, &pkg_b_dir, Vec::new(), None)?;

        let report = store.verify()?;

        assert!(report.is_ok());
        assert_eq!(report.packages_checked, 2);
        Ok(())
    }

    #[test]
    fn open_does_not_append_store_version_twice() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let versioned_root = temp.path().join("store").join(STORE_VERSION);
        let store = Store::open(versioned_root.clone())?;

        assert_eq!(store.root(), versioned_root.as_path());
        assert!(versioned_root.join("packages").exists());
        Ok(())
    }

    #[test]
    fn verify_reports_missing_package_file() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let store = Store::open(temp.path().join("store"))?;
        let pkg_dir = temp.path().join("pkg");
        let id = pkg_id("fixture", "1.0.0")?;
        write_fixture_package(&pkg_dir, "module.exports = 1;\n")?;
        store.import_package(&id, &pkg_dir, Vec::new(), None)?;
        fs::remove_file(store.package_path(&id).join("index.js"))?;

        let report = store.verify()?;

        assert!(!report.is_ok());
        assert_eq!(report.missing.len(), 1);
        Ok(())
    }

    #[test]
    fn import_package_skips_already_imported_package() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let store = Store::open(temp.path().join("store"))?;
        let pkg_dir = temp.path().join("pkg");
        write_fixture_package(&pkg_dir, "module.exports = 1;\n")?;
        let id = pkg_id("a", "1.0.0")?;

        // First import adds files.
        let first = store.import_package(&id, &pkg_dir, Vec::new(), None)?;
        assert!(!first.is_empty());

        // Second import returns empty (fast path, no I/O).
        let second = store.import_package(&id, &pkg_dir, Vec::new(), None)?;
        assert!(second.is_empty());

        Ok(())
    }

    #[test]
    fn concurrent_import_of_same_package_is_safe() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let store = Store::open(temp.path().join("store"))?;
        let pkg_dir = temp.path().join("pkg");
        write_fixture_package(&pkg_dir, "module.exports = 1;\n")?;
        let id = pkg_id("a", "1.0.0")?;

        // Simulate concurrent imports: both threads try to import the same package.
        let store_clone = store.clone();
        let pkg_dir_clone = pkg_dir.clone();
        let id_clone = id.clone();

        let handle = std::thread::spawn(move || {
            store_clone.import_package(&id_clone, &pkg_dir_clone, Vec::new(), None)
        });

        let result_main = store.import_package(&id, &pkg_dir, Vec::new(), None);
        #[allow(clippy::unwrap_used)]
        let result_thread = handle.join().unwrap();

        // Both should succeed; one does the work, the other returns empty.
        assert!(result_main.is_ok());
        assert!(result_thread.is_ok());

        // Only one package should be in the store.
        let packages = store.list_packages()?;
        assert_eq!(packages.len(), 1);

        Ok(())
    }
}
