//! Store integrity verification.

use std::fs;

use anyhow::{Context, Result};
use walkdir::WalkDir;

use orix_domain::PackageId;

use crate::{sha256, IntegrityMeta, VerifyReport};

use super::Store;

impl Store {
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
    pub(crate) fn list_packages_unchecked(&self) -> Result<Vec<PackageId>> {
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
    pub(crate) fn get_integrity_unchecked(&self, pkg_id: &PackageId) -> Result<IntegrityMeta> {
        let path = self.package_path(pkg_id).join("integrity.json");
        let content = fs::read_to_string(&path)
            .with_context(|| format!("failed to read integrity for {}", pkg_id))?;
        serde_json::from_str(&content)
            .with_context(|| format!("failed to parse integrity for {}", pkg_id))
    }
}
