//! Package import into the CAS store.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result};
use tracing::{debug, warn};
use walkdir::WalkDir;

use orix_domain::PackageId;

use crate::{sha256, IntegrityMeta};

use super::Store;

impl Store {
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
        let started = Instant::now();
        // Fast path: if already imported, skip all I/O.
        let already_exists = self.contains(pkg_id);
        debug!(pkg = %pkg_id, already_exists, source_dir = %source_dir.display(), "import_package called");
        if already_exists {
            debug!(
                target: "orix::perf",
                phase = "store_import",
                pkg = %pkg_id,
                duration_ms = 0_u64,
                skipped = true,
                "import_package skipped (already in store)"
            );
            return Ok(HashSet::new());
        }

        let hash_walk_started = Instant::now();
        // ── Phase 1: Compute hashes and prepare file index (outside lock) ──────────
        // This can run concurrently for different packages without contention.
        #[derive(Debug)]
        struct FileEntry {
            rel_path: PathBuf,
            content: Vec<u8>,
            hash: String,
            dest_path: PathBuf,
            content_path: PathBuf,
            mode: u32,
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
            #[cfg(unix)]
            let mode = {
                use std::os::unix::fs::MetadataExt;
                entry.metadata().map(|m| m.mode()).unwrap_or(0o644)
            };
            #[cfg(not(unix))]
            let mode = 0o644;

            file_index.push(FileEntry {
                rel_path,
                content,
                hash,
                dest_path,
                content_path,
                mode,
            });
        }

        if file_index.is_empty() {
            warn!(pkg = %pkg_id, "no files found in source directory, skipping import");
            return Ok(HashSet::new());
        }

        let hash_walk_ms = hash_walk_started.elapsed().as_millis() as u64;
        let write_lock_started = Instant::now();
        // ── Phase 2: Write operations under lock ──────────────────────────────────
        let _guard = self.io_guard.write();

        // Re-check after acquiring lock (another thread may have imported it).
        if self.contains(pkg_id) {
            debug!(pkg = %pkg_id, "package already in store, skipping");
            debug!(
                target: "orix::perf",
                phase = "store_import",
                pkg = %pkg_id,
                duration_ms = started.elapsed().as_millis() as u64,
                hash_walk_ms,
                write_lock_ms = 0_u64,
                files = file_index.len(),
                skipped = true,
                "import_package skipped (race after lock)"
            );
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
                    } else if let Err(e) = restore_mode(&entry.content_path, entry.mode) {
                        errors.push(format!(
                            "failed to set permissions on CAS file {}: {}",
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

        let write_lock_ms = write_lock_started.elapsed().as_millis() as u64;
        let duration_ms = started.elapsed().as_millis() as u64;
        let files = file_index.len() as u64;
        let files_per_sec = if duration_ms == 0 {
            0.0
        } else {
            files as f64 * 1000.0 / duration_ms as f64
        };
        debug!(
            target: "orix::perf",
            phase = "store_import",
            pkg = %pkg_id,
            duration_ms,
            hash_walk_ms,
            write_lock_ms,
            files = file_index.len(),
            new_files = new_files.len(),
            files_per_sec,
            "import_package complete"
        );
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
}

/// Restore file permissions after a write operation.
///
/// On Unix, restores the original mode bits (e.g. +x for executables).
/// On Windows, this is a no-op since mode bits don't exist.
#[cfg(unix)]
fn restore_mode(path: &Path, mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode & 0o777))
}

#[cfg(not(unix))]
fn restore_mode(_path: &Path, _mode: u32) -> std::io::Result<()> {
    Ok(())
}
