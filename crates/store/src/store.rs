//! CAS store implementation.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use std::sync::Arc;

use anyhow::{Context, Result};
use parking_lot::RwLock;
use tracing::{debug, warn};
use walkdir::WalkDir;

use orix_domain::PackageId;

use super::{sha256, IntegrityMeta, PruneReport, VerifyReport};

pub const STORE_VERSION: &str = "v1";

/// The content-addressable store.
/// Uses a `RwLock` to allow concurrent reads while serializing writes.
pub struct Store {
    root: PathBuf,
    files_root: PathBuf,
    packages_root: PathBuf,
    /// Guards file I/O operations. Allows concurrent reads; exclusive access for writes.
    /// Shared via `Arc` so that cloned `Store` instances share the same lock.
    io_guard: Arc<RwLock<()>>,
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
        let root = root.join(STORE_VERSION);
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
    fn file_path(&self, hash: &str) -> PathBuf {
        let (prefix, rest) = hash.split_at(2);
        self.files_root.join(prefix).join(rest)
    }

    /// Path for a package entry.
    fn package_path(&self, pkg_id: &PackageId) -> PathBuf {
        self.packages_root.join(pkg_id.key())
    }

    /// Check if a package is already in the store.
    pub fn contains(&self, pkg_id: &PackageId) -> bool {
        self.package_path(pkg_id).join("integrity.json").exists()
    }

    /// Import an extracted package directory into the store.
    /// Returns the set of files that were newly added.
    ///
    /// Lock strategy (方案 A from design):
    /// - **Outside lock**: walk source dir, read files, compute hashes, prepare index.
    ///   This work is per-package and can run concurrently across packages.
    /// - **Inside lock**: create missing CAS files, hardlink package files, write integrity.json.
    ///   This is the minimal critical section to prevent duplicate work and race conditions.
    ///
    /// - `pkg_id`: the package identity
    /// - `source_dir`: directory containing the extracted tarball contents
    /// - `depnodes`: transitive dependency keys that this package declares
    /// - `top_integrity`: the overall package integrity hash (optional)
    pub fn import_package(
        &self,
        pkg_id: &PackageId,
        source_dir: &Path,
        depnodes: Vec<String>,
        top_integrity: Option<&str>,
    ) -> Result<HashSet<PathBuf>> {
        // Fast path: if already imported, skip all I/O.
        let already_exists = self.contains(pkg_id);
        debug!(pkg = %pkg_id, already_exists, source_dir = %source_dir.display(), "import_package called");
        if already_exists {
            return Ok(HashSet::new());
        }

        // ── Phase 1: Compute hashes and prepare file index (outside lock) ──────────
        // This can run concurrently for different packages without contention.
        #[derive(Debug)]
        struct FileEntry {
            rel_path: PathBuf,
            content: Vec<u8>,
            hash: String,
            dest_path: PathBuf,
            content_path: PathBuf,
        }

        let mut file_index: Vec<FileEntry> = Vec::new();
        let mut errors = Vec::new();

        for entry in WalkDir::new(source_dir).into_iter().filter_map(|e| e.ok()) {
            if !entry.file_type().is_file() {
                continue;
            }
            let rel_path = match entry.path().strip_prefix(source_dir) {
                Ok(p) => p.to_path_buf(),
                Err(_) => continue,
            };
            let rel_str = rel_path.display().to_string().replace('\\', "/");

            let content = match fs::read(entry.path()) {
                Ok(c) => c,
                Err(e) => {
                    errors.push(format!("failed to read {}: {}", entry.path().display(), e));
                    continue;
                }
            };
            let hash = sha256(&content);
            let dest_path = self.package_path(pkg_id).join(&rel_str);
            let content_path = self.file_path(&hash);

            file_index.push(FileEntry {
                rel_path,
                content,
                hash,
                dest_path,
                content_path,
            });
        }

        if file_index.is_empty() {
            warn!(pkg = %pkg_id, "no files found in source directory, skipping import");
            return Ok(HashSet::new());
        }

        // ── Phase 2: Write operations under lock ──────────────────────────────────
        let _guard = self.io_guard.write();

        // Re-check after acquiring lock (another thread may have imported it).
        if self.contains(pkg_id) {
            debug!(pkg = %pkg_id, "package already in store, skipping");
            return Ok(HashSet::new());
        }

        debug!(pkg = %pkg_id, source_dir = %source_dir.display(), "importing package into store");

        let dest = self.package_path(pkg_id);
        fs::create_dir_all(&dest).context("failed to create package directory")?;

        let mut new_files = HashSet::new();

        for entry in &file_index {
            // Create missing CAS files
            if !entry.content_path.exists() {
                if let Some(parent) = entry.content_path.parent() {
                    if let Err(e) = fs::create_dir_all(parent) {
                        errors.push(format!("failed to create dir {}: {}", parent.display(), e));
                    } else if let Err(e) = fs::write(&entry.content_path, &entry.content) {
                        errors.push(format!(
                            "failed to write CAS file {}: {}",
                            entry.content_path.display(),
                            e
                        ));
                    }
                }
            }

            // Hardlink package file from CAS
            if let Some(parent) = entry.dest_path.parent() {
                if let Err(e) = fs::create_dir_all(parent) {
                    errors.push(format!(
                        "failed to create dest dir {}: {}",
                        parent.display(),
                        e
                    ));
                } else {
                    #[allow(clippy::incompatible_msrv)]
                    match fs::hard_link(&entry.content_path, &entry.dest_path) {
                        Ok(_) => {}
                        Err(e)
                            if e.kind() == io::ErrorKind::NotFound
                                || e.kind() == io::ErrorKind::PermissionDenied
                                || e.kind() == io::ErrorKind::CrossesDevices =>
                        {
                            if let Err(e2) = fs::copy(&entry.content_path, &entry.dest_path) {
                                errors.push(format!(
                                    "hard_link/copy failed {} -> {}: {}",
                                    entry.content_path.display(),
                                    entry.dest_path.display(),
                                    e2
                                ));
                            }
                        }
                        Err(e) => {
                            errors.push(format!(
                                "hard_link failed {} -> {}: {}",
                                entry.content_path.display(),
                                entry.dest_path.display(),
                                e
                            ));
                        }
                    }
                }
            }

            new_files.insert(entry.rel_path.clone());
        }

        for err in &errors {
            warn!(pkg = %pkg_id, "{}", err);
        }

        debug!(pkg = %pkg_id, files = file_index.len(), new_files = new_files.len(), "imported package files");

        // Build integrity metadata from the file index
        let files: Vec<(String, String)> = file_index
            .into_iter()
            .map(|e| {
                (
                    e.rel_path.display().to_string().replace('\\', "/"),
                    format!("sha256:{}", e.hash),
                )
            })
            .collect();

        let integrity = IntegrityMeta {
            name: pkg_id.name.to_string(),
            version: pkg_id.version.to_string(),
            integrity: top_integrity.unwrap_or("").to_string(),
            files,
            depnodes,
        };

        let integrity_path = dest.join("integrity.json");
        let json = serde_json::to_string_pretty(&integrity)?;
        let tmp_path = integrity_path.with_extension("tmp");
        fs::write(&tmp_path, &json)?;
        fs::rename(&tmp_path, &integrity_path).context("failed to write integrity.json")?;

        Ok(new_files)
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

    /// Verify that every package entry and content-addressable file matches integrity metadata.
    pub fn verify(&self) -> Result<VerifyReport> {
        let _guard = self.io_guard.read();
        let mut report = VerifyReport::default();

        for pkg_id in self.list_packages_unchecked()? {
            report.packages_checked += 1;
            let package_path = self.package_path(&pkg_id);
            let meta = match self.get_integrity_unchecked(&pkg_id) {
                Ok(meta) => meta,
                Err(error) => {
                    report.corrupted.push(format!("{}: {}", pkg_id, error));
                    continue;
                }
            };

            for (rel_path, hash) in meta.files {
                report.files_checked += 1;
                let expected_hash = hash.trim_start_matches("sha256:");
                let package_file = package_path.join(&rel_path);
                if !package_file.exists() {
                    report
                        .missing
                        .push(format!("{}: missing package file {}", pkg_id, rel_path));
                    continue;
                }

                let content = fs::read(&package_file)?;
                let actual_hash = sha256(&content);
                if actual_hash != expected_hash {
                    report.corrupted.push(format!(
                        "{}: package file {} hash mismatch",
                        pkg_id, rel_path
                    ));
                }

                let content_file = self.file_path(expected_hash);
                if !content_file.exists() {
                    report.missing.push(format!(
                        "{}: missing content file sha256:{}",
                        pkg_id, expected_hash
                    ));
                    continue;
                }

                let content_file_hash = sha256(&fs::read(&content_file)?);
                if content_file_hash != expected_hash {
                    report.corrupted.push(format!(
                        "{}: content file sha256:{} hash mismatch",
                        pkg_id, expected_hash
                    ));
                }
            }
        }

        Ok(report)
    }

    /// List all packages without acquiring the I/O lock.
    /// Caller must hold the lock.
    fn list_packages_unchecked(&self) -> Result<Vec<PackageId>> {
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

    /// Read integrity metadata without acquiring the I/O lock.
    /// Caller must hold the lock.
    fn get_integrity_unchecked(&self, pkg_id: &PackageId) -> Result<IntegrityMeta> {
        let path = self.package_path(pkg_id).join("integrity.json");
        let content = fs::read_to_string(&path)
            .with_context(|| format!("failed to read integrity for {}", pkg_id))?;
        serde_json::from_str(&content)
            .with_context(|| format!("failed to parse integrity for {}", pkg_id))
    }

    /// Prune unreferenced packages from the store.
    /// If `prune_orphaned_files` is true, also removes content-addressable files
    /// that are no longer referenced by any package.
    pub fn prune(
        &self,
        referenced: &HashSet<PackageId>,
        dry_run: bool,
        prune_orphaned_files: bool,
    ) -> Result<PruneReport> {
        let _guard = self.io_guard.write();
        let all = self.list_packages_unchecked()?;
        let referenced_set: HashSet<_> = referenced.iter().map(PackageId::key).collect();

        let mut report = PruneReport {
            packages_removed: 0,
            files_removed: 0,
            bytes_reclaimed: 0,
        };

        for pkg_id in all {
            if !referenced_set.contains(&pkg_id.key()) {
                let path = self.package_path(&pkg_id);
                if path.exists() {
                    if dry_run {
                        report.packages_removed += 1;
                    } else {
                        let size = Self::dir_size(&path);
                        fs::remove_dir_all(&path)?;
                        report.packages_removed += 1;
                        report.bytes_reclaimed += size;
                    }
                }
            }
        }

        if prune_orphaned_files && !dry_run {
            let (files_count, bytes) = self.prune_orphaned_content_files(referenced)?;
            report.files_removed = files_count;
            report.bytes_reclaimed += bytes;
        } else if prune_orphaned_files && dry_run {
            if let Ok(orphaned) = self.count_orphaned_content_files() {
                report.files_removed = orphaned;
            }
        }

        Ok(report)
    }

    /// Remove content-addressable files that are no longer referenced by any package.
    fn prune_orphaned_content_files(
        &self,
        referenced: &HashSet<PackageId>,
    ) -> Result<(usize, u64)> {
        let mut referenced_files = HashSet::new();
        for pkg_id in referenced {
            if let Ok(meta) = self.get_integrity_unchecked(pkg_id) {
                for (_, hash) in &meta.files {
                    let clean = hash.trim_start_matches("sha256:");
                    referenced_files.insert(clean.to_string());
                }
            }
        }

        let mut removed_count = 0;
        let mut reclaimed_bytes = 0u64;

        for entry in WalkDir::new(&self.files_root)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if !entry.file_type().is_file() {
                continue;
            }
            let Some(hash) = self.content_hash_for_entry(entry.path()) else {
                continue;
            };
            if !referenced_files.contains(&hash) {
                let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                fs::remove_file(entry.path())?;
                removed_count += 1;
                reclaimed_bytes += size;
            }
        }

        Ok((removed_count, reclaimed_bytes))
    }

    /// Count orphaned content-addressable files.
    fn count_orphaned_content_files(&self) -> Result<usize> {
        let mut referenced_files = HashSet::new();
        for pkg_id in self.list_packages_unchecked()? {
            if let Ok(meta) = self.get_integrity_unchecked(&pkg_id) {
                for (_, hash) in &meta.files {
                    let clean = hash.trim_start_matches("sha256:");
                    referenced_files.insert(clean.to_string());
                }
            }
        }

        let mut count = 0;
        for entry in WalkDir::new(&self.files_root)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if entry.file_type().is_file() {
                let Some(hash) = self.content_hash_for_entry(entry.path()) else {
                    continue;
                };
                if !referenced_files.contains(&hash) {
                    count += 1;
                }
            }
        }

        Ok(count)
    }

    fn dir_size(path: &Path) -> u64 {
        WalkDir::new(path)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter_map(|e| e.metadata().ok())
            .filter(|m| m.is_file())
            .map(|m| m.len())
            .sum()
    }

    fn content_hash_for_entry(&self, path: &Path) -> Option<String> {
        let rel = path.strip_prefix(&self.files_root).ok()?;
        let mut components = rel.components();
        let prefix = components.next()?.as_os_str().to_str()?;
        let rest = components.next()?.as_os_str().to_str()?;
        if components.next().is_some() {
            return None;
        }
        Some(format!("{}{}", prefix, rest))
    }
}

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

/// Errors that can occur when operating on the store.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// A generic store error with a message.
    #[error("store error: {0}")]
    Other(String),
}
