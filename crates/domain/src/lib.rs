//! Shared domain types for rpnpm.

use std::borrow::Cow;
use std::fmt;
use std::ops::Deref;

use semver::{Version as SemverVersion, VersionReq};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A normalized package name (always lowercase).
#[derive(Clone, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
pub struct PackageName(Cow<'static, str>);

impl PackageName {
    /// Create a new package name.
    #[inline]
    pub fn new(name: impl Into<Cow<'static, str>>) -> Self {
        Self(name.into())
    }

    /// Return the package name as a string slice.
    #[inline]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for PackageName {
    fn from(s: String) -> Self {
        Self(Cow::Owned(s.to_lowercase()))
    }
}

impl From<&str> for PackageName {
    fn from(s: &str) -> Self {
        Self(Cow::Owned(s.to_lowercase()))
    }
}

impl PartialOrd for PackageName {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PackageName {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.to_lowercase().cmp(&other.0.to_lowercase())
    }
}

impl Deref for PackageName {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PackageName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

// ─── Version ───────────────────────────────────────────────────────────────────

/// A semver version, normalized from a string.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
pub struct Version(SemverVersion);

impl Version {
    /// Parse a version string.
    pub fn parse(s: &str) -> anyhow::Result<Self> {
        Ok(Self(
            SemverVersion::parse(s).map_err(|e| anyhow::anyhow!("{}", e))?,
        ))
    }
}

impl Deref for Version {
    type Target = SemverVersion;
    fn deref(&self) -> &SemverVersion {
        &self.0
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Serialize for Version {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0.to_string())
    }
}

impl<'de> Deserialize<'de> for Version {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Self::parse(&s).map_err(serde::de::Error::custom)
    }
}

// ─── VersionConstraint ─────────────────────────────────────────────────────────

/// A version constraint from package.json: "^1.0.0", ">=2.0", "latest", etc.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct VersionConstraint {
    /// The raw constraint string.
    pub raw: String,
    /// The parsed constraint kind.
    pub kind: ConstraintKind,
}

/// The kind of version constraint.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ConstraintKind {
    /// Exact version match: "1.2.3"
    Exact(Version),
    /// Range constraint: "^1.0.0", ">=2.0", "*", etc.
    Range(VersionReq),
    /// "latest" — resolves to the latest published version.
    Latest,
    /// Dist-tag: "next", "beta", etc.
    Tag(String),
}

impl VersionConstraint {
    /// Parse a constraint string like "^1.0.0", ">=2.0", "latest", "next".
    pub fn parse(raw: &str) -> anyhow::Result<Self> {
        let raw = raw.trim().to_string();
        if raw == "latest" {
            return Ok(Self {
                raw: raw.clone(),
                kind: ConstraintKind::Latest,
            });
        }

        if raw.starts_with('^')
            || raw.starts_with('~')
            || raw.starts_with('>')
            || raw.starts_with('<')
            || raw.starts_with('=')
            || raw == "*"
        {
            let req = VersionReq::parse(&raw).map_err(|e| anyhow::anyhow!("{}", e))?;
            Ok(Self {
                raw: raw.clone(),
                kind: ConstraintKind::Range(req),
            })
        } else {
            let v = Version::parse(&raw)?;
            Ok(Self {
                raw: raw.clone(),
                kind: ConstraintKind::Exact(v),
            })
        }
    }

    /// Returns true if this is an exact version constraint.
    #[inline]
    pub fn is_exact(&self) -> bool {
        matches!(self.kind, ConstraintKind::Exact(_))
    }
}

// ─── PackageId ─────────────────────────────────────────────────────────────────

/// Uniquely identifies a package: name + version.
#[derive(Clone, PartialEq, Eq, Hash, Debug, Ord, PartialOrd, Serialize, Deserialize)]
pub struct PackageId {
    /// Package name.
    pub name: PackageName,
    /// Package version.
    pub version: Version,
}

impl PackageId {
    /// Create a new package ID.
    pub fn new(name: PackageName, version: Version) -> Self {
        Self { name, version }
    }

    /// Returns the key used in lockfiles and the store: "name@version"
    pub fn key(&self) -> String {
        format!("{}@{}", self.name, self.version)
    }
}

impl fmt::Display for PackageId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{}", self.name, self.version)
    }
}

// ─── ResolvedPackage ───────────────────────────────────────────────────────────

/// A package resolved from the registry with all metadata needed for install.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResolvedPackage {
    /// Unique package ID (name + version).
    pub id: PackageId,
    /// Integrity string (sha512/sha1).
    pub integrity: String,
    /// URL to the tarball.
    pub tarball: String,
    /// Regular dependencies.
    pub dependencies: Vec<(PackageName, String)>,
    /// Dev dependencies.
    pub dev_dependencies: Vec<(PackageName, String)>,
    /// Optional dependencies.
    pub optional_dependencies: Vec<(PackageName, String)>,
    #[serde(default)]
    /// Peer dependencies.
    pub peer_dependencies: Vec<(PackageName, String)>,
    #[serde(default)]
    /// Engine constraints (e.g., node >= 14).
    pub engines: Option<String>,
    #[serde(default)]
    /// Supported operating systems.
    pub os: Vec<String>,
    #[serde(default)]
    /// Supported CPU architectures.
    pub cpu: Vec<String>,
    /// Transitive dependency node keys that this package declares.
    /// Format: "name@version". Used by the linker to know which symlinks to create.
    #[serde(default)]
    pub depnodes: Vec<String>,
}

// ─── DependencyGraph ───────────────────────────────────────────────────────────

/// The complete resolved dependency graph for one importer.
#[derive(Clone, Debug, Default)]
pub struct DependencyGraph {
    inner: std::collections::BTreeMap<PackageId, ResolvedPackage>,
}

impl DependencyGraph {
    /// Create an empty dependency graph.
    pub fn new() -> Self {
        Self {
            inner: Default::default(),
        }
    }

    /// Insert a resolved package into the graph.
    pub fn insert(&mut self, pkg: ResolvedPackage) {
        self.inner.insert(pkg.id.clone(), pkg);
    }

    /// Look up a package by its ID.
    pub fn get(&self, id: &PackageId) -> Option<&ResolvedPackage> {
        self.inner.get(id)
    }

    /// Check whether a package ID exists in the graph.
    pub fn contains(&self, id: &PackageId) -> bool {
        self.inner.contains_key(id)
    }

    /// Iterate over all resolved packages.
    pub fn packages(&self) -> impl Iterator<Item = &ResolvedPackage> {
        self.inner.values()
    }

    /// Iterate over all package IDs.
    pub fn package_ids(&self) -> impl Iterator<Item = &PackageId> {
        self.inner.keys()
    }

    /// Number of packages in the graph.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// True when the graph has no packages.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

// ─── Platform utilities ─────────────────────────────────────────────────────────

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
        let test_file = tmp.join(format!("rpnpm_link_test_{}", std::process::id()));
        let test_link = tmp.join(format!("rpnpm_link_test_{}.lnk", std::process::id()));
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
