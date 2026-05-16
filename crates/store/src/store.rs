//! CAS store implementation.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use walkdir::WalkDir;

use orix_domain::PackageId;

use super::{sha256, IntegrityMeta, PruneReport};

pub const STORE_VERSION: &str = "v1";

/// The content-addressable store.
#[derive(Debug, Clone)]
pub struct Store {
    root: PathBuf,
    files_root: PathBuf,
    packages_root: PathBuf,
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
        let dest = self.package_path(pkg_id);
        fs::create_dir_all(&dest).context("failed to create package directory")?;

        let mut new_files = HashSet::new();

        for entry in WalkDir::new(source_dir).into_iter().filter_map(|e| e.ok()) {
            if !entry.file_type().is_file() {
                continue;
            }
            let rel_path = entry
                .path()
                .strip_prefix(source_dir)
                .with_context(|| format!("path {} not under source_dir", entry.path().display()))?;
            let rel_str = rel_path.display().to_string().replace('\\', "/");

            let content = fs::read(entry.path())?;
            let hash = sha256(&content);
            let content_path = self.file_path(&hash);

            let is_new = !content_path.exists();
            if is_new {
                if let Some(parent) = content_path.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::copy(entry.path(), &content_path)?;
            }

            let dest_file = dest.join(&rel_str);
            if let Some(parent) = dest_file.parent() {
                fs::create_dir_all(parent)?;
            }

            if let Err(e) = fs::hard_link(&content_path, &dest_file) {
                if e.kind() == io::ErrorKind::NotFound
                    || e.kind() == io::ErrorKind::PermissionDenied
                {
                    fs::copy(entry.path(), &dest_file)?;
                } else {
                    return Err(e.into());
                }
            }

            if is_new {
                new_files.insert(rel_path.to_path_buf());
            }
        }

        let mut files: Vec<(String, String)> = Vec::new();
        for entry in WalkDir::new(&dest).into_iter().filter_map(|e| e.ok()) {
            if !entry.file_type().is_file() {
                continue;
            }
            let rel_path = entry
                .path()
                .strip_prefix(&dest)
                .with_context(|| format!("path {} not under dest", entry.path().display()))?;
            let rel_str = rel_path.display().to_string().replace('\\', "/");
            let content = fs::read(entry.path())?;
            let hash = sha256(&content);
            files.push((rel_str, format!("sha256:{}", hash)));
        }

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
        let mut ids = Vec::new();
        for entry in fs::read_dir(&self.packages_root)? {
            let entry = entry?;
            let name = entry.file_name().into_string().unwrap_or_default();
            if let Some((pkg_name, ver_str)) = name.rsplit_once('@') {
                let name = orix_domain::PackageName::from(pkg_name.to_lowercase());
                let version = orix_domain::Version::parse(ver_str)
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
                ids.push(orix_domain::PackageId::new(name, version));
            }
        }
        Ok(ids)
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
        let all = self.list_packages()?;
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
            // Count orphaned files without deleting
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
            if let Ok(meta) = self.get_integrity(pkg_id) {
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
            let hash = entry.file_name().to_string_lossy().into_owned();
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
        for pkg_id in self.list_packages()? {
            if let Ok(meta) = self.get_integrity(&pkg_id) {
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
                let hash = entry.file_name().to_string_lossy().into_owned();
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
}

/// Errors that can occur when operating on the store.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// A generic store error with a message.
    #[error("store error: {0}")]
    Other(String),
}
