//! Package binary linking.

use anyhow::Context;

use crate::linker::prelude::*;
use crate::linker::{Linker, VIRTUAL_STORE_DIR};
use crate::linker_platform::*;
use tracing::trace;

impl Linker {
    /// Link bin executables from a package into the .orix/<pkg>/bin directory.
    pub(crate) fn link_package_bins(
        &self,
        pkg_key: &str,
        store_files: &Path,
        link_global_bins: bool,
        report: &mut LinkReport,
    ) -> Result<()> {
        let pkg_json_path = store_files.join("package.json");
        if !pkg_json_path.exists() {
            return Ok(());
        }

        let pkg_json_content = std::fs::read_to_string(&pkg_json_path)?;
        let pkg_json: serde_json::Value =
            serde_json::from_str(&pkg_json_content).unwrap_or_default();

        let bin_value = match pkg_json.get("bin") {
            Some(v) => v,
            None => return Ok(()),
        };

        let bin_entries: Vec<(String, String)> = match bin_value {
            serde_json::Value::String(s) => {
                let pkg_name = pkg_json.get("name").and_then(|v| v.as_str()).unwrap_or("");
                vec![(pkg_name.to_string(), s.clone())]
            }
            serde_json::Value::Object(m) => m
                .iter()
                .map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string()))
                .collect(),
            _ => return Ok(()),
        };

        let pkg_name = pkg_json.get("name").and_then(|v| v.as_str()).unwrap_or("");
        if pkg_name.is_empty() {
            return Ok(());
        }

        // Package dir: .orix/<pkg>@<ver>/node_modules/<pkg>/.
        let package_store_dir = self.node_modules.join(VIRTUAL_STORE_DIR).join(pkg_key);
        let package_dir =
            Self::package_path_in_node_modules(&package_store_dir.join("node_modules"), pkg_name);
        let global_bin_dir = self.node_modules.join(".bin");

        for (bin_name, bin_path) in bin_entries {
            if bin_name.is_empty() || bin_path.is_empty() {
                continue;
            }

            // The actual bin file in the store.
            let bin_source = store_files.join(&bin_path);
            if !bin_source.exists() {
                trace!(
                    pkg = %pkg_key,
                    bin = %bin_name,
                    missing = %bin_source.display(),
                    "bin source not in store"
                );
                continue;
            }

            // Flatten scoped bin names: "@antfu/eslint-config" -> "eslint-config"
            let flat_bin_name = std::path::Path::new(&bin_name)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(&bin_name);

            // Shim bin name (also flattened).
            let shim_bin_name = flat_bin_name;

            let package_bin = package_dir.join(&bin_path);
            if !package_bin.exists() {
                trace!(
                    pkg = %pkg_key,
                    bin = %bin_name,
                    missing = %package_bin.display(),
                    "bin target not in linked package"
                );
                continue;
            }

            Self::ensure_bin_executable(&package_bin).with_context(|| {
                format!("failed to make bin executable: {}", package_bin.display())
            })?;

            if !link_global_bins {
                continue;
            }

            // Global shims: only create if the bin file was successfully placed.
            if package_bin.exists() {
                #[cfg(windows)]
                {
                    // Resolve the bin to an absolute path so the shim works from any cwd.
                    let absolute_bin = package_bin.canonicalize().with_context(|| {
                        format!(
                            "failed to resolve bin target {} for shim",
                            package_bin.display()
                        )
                    })?;

                    Self::create_windows_bin_shims(&global_bin_dir, shim_bin_name, &absolute_bin)
                        .with_context(|| {
                        format!("failed to create Windows bin shim for {}", bin_name)
                    })?;
                    report.symlinks_created += 2;
                }

                #[cfg(not(windows))]
                {
                    let shim_link = global_bin_dir.join(shim_bin_name);
                    if !path_exists_or_symlink(&shim_link) {
                        if let Some(parent) = shim_link.parent() {
                            fs::create_dir_all(parent)?;
                        }
                        let rel = relative_path(
                            shim_link.parent().unwrap_or(std::path::Path::new(".")),
                            &package_bin,
                        );
                        std::os::unix::fs::symlink(&rel, &shim_link)?;
                        report.symlinks_created += 1;
                    }
                }
            }
        }

        Ok(())
    }

    #[cfg(unix)]
    fn ensure_bin_executable(path: &Path) -> io::Result<()> {
        let metadata = fs::metadata(path)?;
        let mode = metadata.mode();
        if mode & 0o111 != 0 {
            return Ok(());
        }
        fs::set_permissions(path, PermissionsExt::from_mode((mode | 0o111) & 0o777))
    }

    #[cfg(not(unix))]
    fn ensure_bin_executable(_path: &Path) -> io::Result<()> {
        Ok(())
    }

    #[cfg(windows)]
    fn create_windows_bin_shims(
        global_bin_dir: &Path,
        shim_bin_name: &str,
        absolute_bin_path: &Path,
    ) -> Result<()> {
        fs::create_dir_all(global_bin_dir)
            .with_context(|| format!("failed to create {}", global_bin_dir.display()))?;

        let cmd_path = global_bin_dir.join(format!("{shim_bin_name}.cmd"));
        let ps1_path = global_bin_dir.join(format!("{shim_bin_name}.ps1"));

        let target = absolute_bin_path.display().to_string().replace('/', "\\");

        let cmd_content = format!(
            "@ECHO off\r\n\
SETLOCAL\r\n\
SET \"basedir=%~dp0\"\r\n\
IF EXIST \"%basedir%\\node.exe\" (\r\n\
  SET \"_prog=%basedir%\\node.exe\"\r\n\
) ELSE (\r\n\
  SET \"_prog=node\"\r\n\
)\r\n\
\"%_prog%\" \"{target}\" %*\r\n"
        );

        let ps1_target = target.replace('\\', "/");
        let ps1_content = format!(
            "$basedir = Split-Path $MyInvocation.MyCommand.Definition -Parent\n\
$exe = Join-Path $basedir 'node.exe'\n\
if (Test-Path $exe) {{\n\
  & $exe '{ps1_target}' @args\n\
}} else {{\n\
  & node '{ps1_target}' @args\n\
}}\n"
        );

        fs::write(&cmd_path, &cmd_content)
            .with_context(|| format!("failed to write {}", cmd_path.display()))?;

        fs::write(&ps1_path, &ps1_content)
            .with_context(|| format!("failed to write {}", ps1_path.display()))?;

        Ok(())
    }
}
