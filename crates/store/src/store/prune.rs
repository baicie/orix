//! Store pruning.

use std::collections::HashSet;
use std::fs;
use std::path::Path;

use anyhow::Result;
use walkdir::WalkDir;

use orix_domain::PackageId;

use crate::PruneReport;

use super::Store;

impl Store {
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
