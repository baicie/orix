//! Directory and file symlinks.

use crate::linker::prelude::*;
use crate::linker::Linker;

#[cfg(windows)]
use tracing::debug;

impl Linker {
    #[cfg(windows)]
    fn resolve_dir_link_target(target: &Path, link: &Path) -> io::Result<PathBuf> {
        if target.is_absolute() {
            fs::canonicalize(target).or_else(|_| Ok(target.to_path_buf()))
        } else {
            #[cfg(windows)]
            {
                Self::absolutize_link_target(target, link)
            }
            #[cfg(not(windows))]
            {
                let parent = link.parent().ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidInput, "link path has no parent")
                })?;
                parent.join(target).canonicalize()
            }
        }
    }

    /// Create a directory link, falling back to junction on Windows when needed.
    ///
    /// On Windows, junction targets must be absolute; relative targets can make Node
    /// resolve module paths to a bare drive letter (`D:`) and fail with `EISDIR`.
    pub(crate) fn create_dir_link(target: &Path, link: &Path) -> io::Result<()> {
        #[cfg(windows)]
        {
            if link.exists() || fs::symlink_metadata(link).is_ok() {
                return Ok(());
            }

            let absolute_target = Self::resolve_dir_link_target(target, link)?;

            match std::os::windows::fs::symlink_dir(&absolute_target, link) {
                Ok(_) => return Ok(()),
                Err(e) => {
                    debug!(
                        target = %absolute_target.display(),
                        link = %link.display(),
                        error = %e,
                        "directory symlink failed; trying junction fallback"
                    );
                }
            }

            Self::create_junction(&absolute_target, link)
        }
        #[cfg(not(windows))]
        {
            std::os::unix::fs::symlink(target, link)
        }
    }

    /// Create a file link for package binaries.
    #[cfg(not(windows))]
    #[allow(dead_code)]
    fn create_file_link(target: &Path, link: &Path) -> io::Result<()> {
        #[cfg(windows)]
        {
            let absolute_target = Self::absolutize_link_target(target, link)?;
            match fs::hard_link(&absolute_target, link) {
                Ok(_) => Ok(()),
                Err(e) => {
                    debug!(
                        target = %absolute_target.display(),
                        link = %link.display(),
                        error = %e,
                        "binary hardlink failed; copying file"
                    );
                    fs::copy(&absolute_target, link).map(|_| ())
                }
            }
        }
        #[cfg(not(windows))]
        {
            std::os::unix::fs::symlink(target, link)
        }
    }

    #[cfg(windows)]
    pub(crate) fn absolutize_link_target(target: &Path, link: &Path) -> io::Result<PathBuf> {
        if target.is_absolute() {
            return target.canonicalize();
        }

        let parent = link.parent().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "link path has no parent")
        })?;
        parent.join(target).canonicalize()
    }

    /// Create a Windows junction point (directory symbolic link alternative).
    /// Junctions don't require admin privileges on Windows Vista+.
    #[cfg(windows)]
    fn create_junction(target: &Path, link: &Path) -> io::Result<()> {
        use std::process::Command;

        // junction tool requires the link to not exist, and target must be absolute
        if link.exists() {
            return Ok(());
        }

        let target_str = target.display().to_string();
        let link_str = link.display().to_string();

        let output = Command::new("cmd")
            .args(["/C", "mklink", "/J", &link_str, &target_str])
            .output();

        match output {
            Ok(o) if o.status.success() => Ok(()),
            Ok(o) => Err(io::Error::other(format!(
                "failed to create junction {} -> {}: {}{}",
                link.display(),
                target.display(),
                String::from_utf8_lossy(&o.stderr),
                String::from_utf8_lossy(&o.stdout)
            ))),
            Err(e) => Err(e),
        }
    }
}
