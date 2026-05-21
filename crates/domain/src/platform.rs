//! Platform compatibility and symlink capability.

use std::fmt;

/// Checks whether this package is compatible with the current OS and CPU.
///
/// Returns `None` if compatible, or `Some(PlatformMismatch)` describing why not.
pub fn check_platform_compatibility(
    pkg_os: &[String],
    pkg_cpu: &[String],
) -> Option<PlatformMismatch> {
    let os_ok = pkg_os.is_empty() || pkg_os.iter().any(|o| os_matches(o));
    let cpu_ok = pkg_cpu.is_empty() || pkg_cpu.iter().any(|c| cpu_matches(c));

    if !os_ok {
        return Some(PlatformMismatch::Os {
            package_supports: pkg_os.to_vec(),
            current: current_os(),
        });
    }
    if !cpu_ok {
        return Some(PlatformMismatch::Cpu {
            package_supports: pkg_cpu.to_vec(),
            current: current_cpu(),
        });
    }
    None
}

/// Returns the normalized current OS identifier.
pub fn current_os() -> String {
    #[cfg(windows)]
    {
        "win32".to_string()
    }
    #[cfg(target_os = "macos")]
    {
        "darwin".to_string()
    }
    #[cfg(target_os = "linux")]
    {
        "linux".to_string()
    }
    #[cfg(all(not(windows), not(target_os = "macos"), not(target_os = "linux")))]
    {
        std::env::consts::OS.to_string()
    }
}

/// Returns the normalized current CPU architecture identifier.
pub fn current_cpu() -> String {
    match std::env::consts::ARCH {
        "x86_64" => "x64".to_string(),
        "aarch64" => "arm64".to_string(),
        other => other.to_string(),
    }
}

/// Check if an OS constraint matches the current OS.
fn os_matches(constraint: &str) -> bool {
    let os = current_os();
    match constraint {
        "win32" => os == "win32",
        "darwin" => os == "darwin",
        "linux" => os == "linux",
        "freebsd" => os == "freebsd",
        "openbsd" => os == "openbsd",
        "sunos" => os == "sunos",
        "android" => os == "android",
        "!win32" => os != "win32",
        "!darwin" => os != "darwin",
        "!linux" => os != "linux",
        _ => {
            if let Some(negated) = constraint.strip_prefix('!') {
                os != negated
            } else {
                os == constraint
            }
        }
    }
}

/// Check if a CPU constraint matches the current CPU.
fn cpu_matches(constraint: &str) -> bool {
    let cpu = current_cpu();
    match constraint {
        "x64" => cpu == "x64" || cpu == "x86_64",
        "x86" => cpu == "x86" || cpu == "i686",
        "arm64" => cpu == "arm64" || cpu == "aarch64",
        "arm" => cpu == "arm" || cpu == "armv7",
        "ppc64" => cpu == "ppc64",
        "riscv64" => cpu == "riscv64",
        "s390x" => cpu == "s390x",
        "!x64" => cpu != "x64",
        _ => {
            if let Some(negated) = constraint.strip_prefix('!') {
                cpu != negated
            } else {
                cpu == constraint
            }
        }
    }
}

/// Reason why a package is not compatible with the current platform.
#[derive(Debug, Clone)]
pub enum PlatformMismatch {
    /// Package requires a different OS.
    Os {
        /// OS identifiers the package supports (e.g., ["darwin", "linux"]).
        package_supports: Vec<String>,
        /// The current OS identifier.
        current: String,
    },
    /// Package requires a different CPU.
    Cpu {
        /// CPU architectures the package supports (e.g., ["x64", "arm64"]).
        package_supports: Vec<String>,
        /// The current CPU architecture.
        current: String,
    },
}

impl fmt::Display for PlatformMismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PlatformMismatch::Os {
                package_supports,
                current,
            } => {
                write!(
                    f,
                    "OS mismatch: package requires one of {:?}, current is '{}'",
                    package_supports, current
                )
            }
            PlatformMismatch::Cpu {
                package_supports,
                current,
            } => {
                write!(
                    f,
                    "CPU mismatch: package requires one of {:?}, current is '{}'",
                    package_supports, current
                )
            }
        }
    }
}

/// Checks whether the current user has permission to create symlinks.
/// On Windows, this requires developer mode or administrator privileges.
pub fn symlink_available() -> bool {
    #[cfg(windows)]
    {
        let tmp = std::env::temp_dir();
        let test_file = tmp.join(format!("orix_link_test_{}", std::process::id()));
        let test_link = tmp.join(format!("orix_link_test_{}.lnk", std::process::id()));
        // Write a test file then try to symlink it.
        if std::fs::write(&test_file, b"test").is_ok() {
            let result = std::os::windows::fs::symlink_file(&test_file, &test_link);
            let _ = std::fs::remove_file(&test_file);
            let _ = std::fs::remove_file(&test_link);
            return result.is_ok();
        }
        false
    }
    #[cfg(not(windows))]
    {
        true
    }
}
