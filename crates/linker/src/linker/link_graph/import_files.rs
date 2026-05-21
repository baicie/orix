//! Import package files from the store.

use anyhow::Context;

use crate::linker::prelude::*;
use crate::linker::Linker;
use tracing::{trace, warn};

impl Linker {
    /// Import package files from the store into a package directory.
    ///
    /// Optimizations:
    /// - Iterates `integrity.files` instead of WalkDir to avoid scanning the store directory.
    /// - Pre-creates all needed directories in one pass before linking/copying files.
    /// - Falls back to copy on EXDEV and remembers the decision per-package.
    /// - Writes `package.json` last as a completion marker.
    pub(crate) fn import_package_files(
        &self,
        pkg_id: &orix_domain::PackageId,
        pkg_dir: &Path,
        store_files: &Path,
        report: &mut LinkReport,
    ) -> Result<()> {
        let pkg_key = pkg_id.key();

        // Read integrity metadata to get the file list without WalkDir.
        let integrity = match self.store.get_integrity(pkg_id) {
            Ok(i) => i,
            Err(e) => {
                warn!(pkg = %pkg_key, error = %e, "failed to read integrity metadata, skipping files");
                return Ok(());
            }
        };

        // Collect and pre-create all needed directories in one pass.
        let mut dirs: HashSet<PathBuf> = HashSet::new();
        for (rel_path, _) in &integrity.files {
            if let Some(parent) = Path::new(rel_path).parent() {
                if !parent.as_os_str().is_empty() {
                    dirs.insert(parent.to_path_buf());
                }
            }
        }
        // Sort by depth so shallow dirs are created before deep ones.
        let mut dirs: Vec<_> = dirs.into_iter().collect();
        dirs.sort_by_key(|p| p.components().count());
        for dir in &dirs {
            let full = pkg_dir.join(dir);
            if let Err(e) = fs::create_dir_all(&full) {
                warn!(pkg = %pkg_key, error = %e, "failed to create dir {}", full.display());
            }
        }

        // Same volume: hardlink; cross-volume: copy for the whole package (remembered per call).
        let mut use_copy = !Self::same_volume(store_files, pkg_dir);

        let mut hardlink_ok = 0u64;
        let mut copy_ok = 0u64;
        let mut hardlink_fail = 0u64;
        let mut copy_fail = 0u64;

        // Separate package.json to write it last.
        let mut normal_files: Vec<&(String, String)> = Vec::new();
        let mut package_json: Option<&(String, String)> = None;

        for entry in &integrity.files {
            if entry.0 == "package.json" {
                package_json = Some(entry);
            } else {
                normal_files.push(entry);
            }
        }

        // Import all files except package.json.
        for (rel_path, _) in normal_files {
            let src = store_files.join(rel_path);
            let dest = pkg_dir.join(rel_path);

            if !src.exists() {
                warn!(pkg = %pkg_key, missing = %src.display(), "file missing from store");
                continue;
            }

            if use_copy {
                Self::copy_with_mode(&src, &dest, &mut copy_ok, &mut copy_fail, &pkg_key)?;
            } else {
                #[allow(clippy::incompatible_msrv)]
                match fs::hard_link(&src, &dest) {
                    Ok(_) => hardlink_ok += 1,
                    Err(e) if e.kind() == io::ErrorKind::CrossesDevices => {
                        use_copy = true;
                        Self::copy_with_mode(&src, &dest, &mut copy_ok, &mut copy_fail, &pkg_key)?;
                    }
                    Err(e)
                        if e.kind() == io::ErrorKind::PermissionDenied
                            || e.kind() == io::ErrorKind::NotFound =>
                    {
                        Self::copy_with_mode(&src, &dest, &mut copy_ok, &mut copy_fail, &pkg_key)?;
                    }
                    Err(e) => {
                        hardlink_fail += 1;
                        warn!(pkg = %pkg_key, error = %e, "hard_link failed {} -> {}", src.display(), dest.display());
                    }
                }
            }
        }

        // Write package.json last as the completion marker.
        if let Some((rel_path, _)) = package_json {
            let src = store_files.join(rel_path);
            let dest = pkg_dir.join(rel_path);
            if src.exists() {
                match fs::copy(&src, &dest) {
                    Ok(_) => {
                        if use_copy {
                            copy_ok += 1;
                        } else {
                            hardlink_ok += 1;
                        }
                    }
                    Err(e) => {
                        copy_fail += 1;
                        warn!(pkg = %pkg_key, error = %e, "failed to write package.json {}", dest.display());
                    }
                }
            }
        }

        report.hardlinked_files += hardlink_ok;
        report.copied_files += copy_ok;
        trace!(pkg = %pkg_key, hardlink_ok, copy_ok, hardlink_fail, copy_fail, use_copy, "imported package files");
        if hardlink_ok == 0 && copy_ok == 0 && hardlink_fail == 0 && copy_fail == 0 {
            warn!(pkg = %pkg_key, "no files were imported");
        }

        Ok(())
    }

    /// Copy a file preserving its Unix permissions (mode bits).
    pub(crate) fn copy_with_mode(
        src: &Path,
        dest: &Path,
        copy_ok: &mut u64,
        _copy_fail: &mut u64,
        _pkg_key: &str,
    ) -> Result<()> {
        #[cfg(unix)]
        {
            let mode = fs::metadata(src)
                .map(|m| m.mode())
                .with_context(|| format!("failed to stat {} for permission copy", src.display()))?;
            fs::copy(src, dest).with_context(|| {
                format!("failed to copy {} -> {}", src.display(), dest.display())
            })?;
            fs::set_permissions(dest, PermissionsExt::from_mode(mode & 0o777))
                .with_context(|| format!("failed to set permissions on {}", dest.display()))?;
            *copy_ok += 1;
        }
        #[cfg(not(unix))]
        {
            if fs::copy(src, dest).is_ok() {
                *copy_ok += 1;
            } else {
                *_copy_fail += 1;
            }
        }
        Ok(())
    }
}
