//! Shared domain types for orix.

use std::borrow::Cow;
use std::fmt;
use std::ops::Deref;

use semver::{Version as SemverVersion, VersionReq};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;
use url::Url;

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

    /// Parse and validate an npm package name.
    pub fn parse(name: &str) -> Result<Self, PackageNameError> {
        let normalized = normalize_package_name(name)?;
        Ok(Self(Cow::Owned(normalized)))
    }

    /// Return the package scope without the leading `@`, if present.
    pub fn scope(&self) -> Option<&str> {
        self.0
            .strip_prefix('@')
            .and_then(|name| name.split_once('/').map(|(scope, _)| scope))
    }

    /// Return the package name without its scope.
    pub fn unscoped(&self) -> &str {
        self.0
            .strip_prefix('@')
            .and_then(|name| name.split_once('/').map(|(_, unscoped)| unscoped))
            .unwrap_or(&self.0)
    }
}

/// Package name validation errors.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum PackageNameError {
    /// Package name was empty.
    #[error("package name is empty")]
    EmptyName,
    /// Scoped package name was malformed.
    #[error("invalid scoped package name: {0}")]
    InvalidScope(String),
    /// Package name contains invalid characters or path traversal.
    #[error("invalid package name: {0}")]
    InvalidCharacter(String),
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

fn normalize_package_name(name: &str) -> Result<String, PackageNameError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(PackageNameError::EmptyName);
    }
    if name.contains('\\') || name.contains("..") {
        return Err(PackageNameError::InvalidCharacter(name.to_string()));
    }

    if let Some(scoped) = name.strip_prefix('@') {
        let Some((scope, package)) = scoped.split_once('/') else {
            return Err(PackageNameError::InvalidScope(name.to_string()));
        };
        if scope.is_empty() || package.is_empty() || package.contains('/') {
            return Err(PackageNameError::InvalidScope(name.to_string()));
        }
        validate_package_segment(scope, name)?;
        validate_package_segment(package, name)?;
        Ok(format!(
            "@{}/{}",
            scope.to_lowercase(),
            package.to_lowercase()
        ))
    } else {
        if name.contains('/') {
            return Err(PackageNameError::InvalidCharacter(name.to_string()));
        }
        validate_package_segment(name, name)?;
        Ok(name.to_lowercase())
    }
}

fn validate_package_segment(segment: &str, original: &str) -> Result<(), PackageNameError> {
    if segment.is_empty()
        || segment == "."
        || segment == ".."
        || segment.chars().any(|ch| ch.is_control())
    {
        return Err(PackageNameError::InvalidCharacter(original.to_string()));
    }
    Ok(())
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
    /// Npm OR range: "^9.0.3 || ^10.1.2".
    AnyRange(Vec<VersionReq>),
    /// "latest" — resolves to the latest published version.
    Latest,
    /// Dist-tag: "next", "beta", etc.
    Tag(String),
    /// A local patch specification: "patch:pkg@1.0.0#./patches/pkg.patch"
    Patch(PatchSpec),
    /// A catalog: protocol reference: "catalog:" or "catalog:name"
    Catalog(CatalogConstraint),
    /// npm alias protocol: "npm:target@^1.0.0".
    Alias {
        /// The real package name to fetch from the registry.
        package: PackageName,
        /// The version constraint for the real package.
        constraint: Box<VersionConstraint>,
    },
}

/// A catalog: protocol specification (domain-level).
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct CatalogConstraint {
    /// The catalog name (None = default catalog, Some(name) = named catalog).
    pub catalog_name: Option<String>,
}

/// A patch: protocol specification.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct PatchSpec {
    /// The name of the package being patched.
    pub package_name: PackageName,
    /// The exact version of the package being patched.
    pub package_version: Version,
    /// The path to the patch file (relative to project root).
    #[serde(rename = "path")]
    pub patch_path: String,
}

/// A catalog: protocol specification.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct CatalogSpec {
    /// The catalog name (None = default catalog, Some(name) = named catalog).
    pub catalog_name: Option<String>,
}

impl VersionConstraint {
    /// Parse a constraint string like "^1.0.0", ">=2.0", "latest", "next".
    pub fn parse(raw: &str) -> anyhow::Result<Self> {
        let raw = raw.trim().to_string();

        // Handle patch: protocol.
        if let Some(rest) = raw.strip_prefix("patch:") {
            return Self::parse_patch_spec(rest).map(|spec| Self {
                raw,
                kind: ConstraintKind::Patch(spec),
            });
        }

        // Handle catalog: protocol.
        if let Some(after) = raw.strip_prefix("catalog:") {
            let catalog_name = if after.is_empty() {
                None
            } else {
                Some(after.to_string())
            };
            return Ok(Self {
                raw,
                kind: ConstraintKind::Catalog(CatalogConstraint { catalog_name }),
            });
        }

        // Handle npm alias protocol.
        if let Some(rest) = raw.strip_prefix("npm:") {
            let at_pos = rest.rfind('@').ok_or_else(|| {
                anyhow::anyhow!(
                    "invalid npm alias format: expected 'npm:name@version', got '{}'",
                    raw
                )
            })?;
            let package = PackageName::parse(&rest[..at_pos])?;
            let constraint = VersionConstraint::parse(&rest[at_pos + 1..])?;
            return Ok(Self {
                raw,
                kind: ConstraintKind::Alias {
                    package,
                    constraint: Box::new(constraint),
                },
            });
        }

        if raw == "latest" {
            return Ok(Self {
                raw: raw.clone(),
                kind: ConstraintKind::Latest,
            });
        }

        if let Ok(v) = Version::parse(&raw) {
            return Ok(Self {
                raw: raw.clone(),
                kind: ConstraintKind::Exact(v),
            });
        }

        if raw.contains("||") {
            let ranges: Result<Vec<_>, _> = raw
                .split("||")
                .map(str::trim)
                .filter(|part| !part.is_empty())
                .map(parse_npm_version_req)
                .collect();

            if let Ok(ranges) = ranges {
                if !ranges.is_empty() {
                    return Ok(Self {
                        raw: raw.clone(),
                        kind: ConstraintKind::AnyRange(ranges),
                    });
                }
            }
        }

        if let Ok(req) = parse_npm_version_req(&raw) {
            return Ok(Self {
                raw: raw.clone(),
                kind: ConstraintKind::Range(req),
            });
        }

        Ok(Self {
            raw: raw.clone(),
            kind: ConstraintKind::Tag(raw.clone()),
        })
    }

    fn parse_patch_spec(rest: &str) -> anyhow::Result<PatchSpec> {
        // Format: <name>@<version>#<patch_path>
        let hash_pos = rest.rfind('#').ok_or_else(|| {
            anyhow::anyhow!(
                "invalid patch: protocol format: expected 'name@version#path', got '{}'",
                rest
            )
        })?;
        let name_and_version = &rest[..hash_pos];
        let patch_path = rest[hash_pos + 1..].to_string();

        let at_pos = name_and_version.rfind('@').ok_or_else(|| {
            anyhow::anyhow!(
                "invalid patch: protocol format: expected 'name@version#path', got '{}'",
                rest
            )
        })?;
        let name_str = &name_and_version[..at_pos];
        let version_str = &name_and_version[at_pos + 1..];

        if name_str.is_empty() {
            anyhow::bail!("invalid patch: protocol: empty package name");
        }
        if version_str.is_empty() {
            anyhow::bail!("invalid patch: protocol: empty version");
        }
        if patch_path.is_empty() {
            anyhow::bail!("invalid patch: protocol: empty patch path");
        }

        Ok(PatchSpec {
            package_name: PackageName::from(name_str),
            package_version: Version::parse(version_str)?,
            patch_path,
        })
    }

    /// Returns true if this is an exact version constraint.
    #[inline]
    pub fn is_exact(&self) -> bool {
        matches!(self.kind, ConstraintKind::Exact(_))
    }

    /// Returns true if this is a patch: protocol constraint.
    #[inline]
    pub fn is_patch(&self) -> bool {
        matches!(self.kind, ConstraintKind::Patch(_))
    }
}

fn parse_npm_version_req(raw: &str) -> Result<VersionReq, semver::Error> {
    VersionReq::parse(raw).or_else(|_| {
        if let Some(normalized) = normalize_npm_hyphen_range(raw) {
            return VersionReq::parse(&normalized);
        }

        let tokens = raw.split_whitespace().collect::<Vec<_>>();
        let mut normalized_parts = Vec::with_capacity(tokens.len());
        let mut i = 0;
        while i < tokens.len() {
            let token = tokens[i];
            if matches!(token, ">" | ">=" | "<" | "<=" | "=") && i + 1 < tokens.len() {
                normalized_parts.push(format!("{}{}", token, tokens[i + 1]));
                i += 2;
            } else {
                normalized_parts.push(token.to_string());
                i += 1;
            }
        }
        let normalized = normalized_parts.join(", ");
        VersionReq::parse(&normalized)
    })
}

fn normalize_npm_hyphen_range(raw: &str) -> Option<String> {
    let (lower, upper) = raw.split_once(" - ")?;
    let lower = normalize_partial_version_lower(lower.trim())?;
    let upper = normalize_partial_version_upper(upper.trim())?;
    Some(format!(">={lower}, {upper}"))
}

fn normalize_partial_version_lower(raw: &str) -> Option<String> {
    let parts = parse_partial_version_parts(raw)?;
    Some(format!(
        "{}.{}.{}",
        parts[0],
        parts.get(1).copied().unwrap_or(0),
        parts.get(2).copied().unwrap_or(0)
    ))
}

fn normalize_partial_version_upper(raw: &str) -> Option<String> {
    let parts = parse_partial_version_parts(raw)?;
    match parts.as_slice() {
        [major] => Some(format!("<{}.0.0", major + 1)),
        [major, minor] => Some(format!("<{major}.{}.0", minor + 1)),
        [major, minor, patch] => Some(format!("<={major}.{minor}.{patch}")),
        _ => None,
    }
}

fn parse_partial_version_parts(raw: &str) -> Option<Vec<u64>> {
    let parts = raw
        .split('.')
        .map(str::trim)
        .map(str::parse::<u64>)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    (!parts.is_empty() && parts.len() <= 3).then_some(parts)
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
    /// Patch applied to this package (if any, from patch: protocol).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patch: Option<PatchSpec>,
}

// ─── DependencyGraph ───────────────────────────────────────────────────────────

/// The complete resolved dependency graph for one importer.
#[derive(Clone, Debug, Default)]
pub struct DependencyGraph {
    inner: std::collections::BTreeMap<PackageId, ResolvedPackage>,
    /// Diagnostic messages collected during resolution.
    pub diagnostics: Vec<ResolverDiagnostic>,
}

impl DependencyGraph {
    /// Create an empty dependency graph.
    pub fn new() -> Self {
        Self {
            inner: Default::default(),
            diagnostics: Vec::new(),
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

    /// Merge another graph into this one (packages with the same ID are deduplicated by ID).
    pub fn merge(&mut self, other: DependencyGraph) {
        self.inner.extend(other.inner);
    }

    /// Number of packages in the graph.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// True when the graph has no packages.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Compute a stable SHA-256 hash of the dependency graph.
    /// Used by the linker fast path to detect whether node_modules layout is still valid.
    pub fn graph_hash(&self) -> String {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        for pkg in self.inner.values() {
            hasher.update(pkg.id.key().as_bytes());
        }
        hex::encode(hasher.finalize())
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

// ─── ScriptRef ────────────────────────────────────────────────────────────────

/// A named script entry from package.json scripts.
#[derive(Debug, Clone)]
pub struct ScriptRef<'a> {
    /// Script name (e.g., "prebuild", "build", "postbuild").
    pub name: String,
    /// Script command string.
    pub command: &'a str,
}

// ─── PeerDependencies ─────────────────────────────────────────────────────────

/// Peer context: the resolved peers visible from a package's installation point.
/// Used by the peer-aware dependency resolver to determine which instance of a
/// package to install.
#[derive(Debug, Clone, Default, Eq, PartialEq, Hash)]
pub struct PeerContext {
    /// Resolved peer packages, keyed by name.
    pub resolved: std::collections::BTreeMap<PackageName, PackageId>,
}

impl PeerContext {
    /// Returns true if no peers are present.
    pub fn is_empty(&self) -> bool {
        self.resolved.is_empty()
    }

    /// Insert a resolved peer.
    pub fn insert(&mut self, name: PackageName, id: PackageId) {
        self.resolved.insert(name, id);
    }

    /// Generate the peer suffix for lockfile keys, sorted by package name.
    /// Format: "(react@18.2.0)(lodash@4.17.21)"
    /// Empty context produces an empty string.
    pub fn key(&self) -> String {
        let mut parts: Vec<String> = self
            .resolved
            .values()
            .map(|id| format!("({})", id))
            .collect();
        parts.sort();
        parts.join("")
    }
}

/// Package instance ID: combines source identity (name + version) with the peer
/// context at the installation point. Two packages with the same source but
/// different peer environments resolve to different instance IDs.
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct PackageInstanceId {
    /// Source package identity.
    pub package: PackageId,
    /// Peer environment at the installation point.
    pub peer_context: PeerContext,
}

impl PackageInstanceId {
    /// Create a new package instance ID.
    pub fn new(package: PackageId, peer_context: PeerContext) -> Self {
        Self {
            package,
            peer_context,
        }
    }

    /// Generate the full lockfile key including peer suffix.
    /// Format: "name@ver(peer1@ver1)(peer2@ver2)"
    pub fn key(&self) -> String {
        let suffix = self.peer_context.key();
        if suffix.is_empty() {
            self.package.key()
        } else {
            format!("{}{}", self.package.key(), suffix)
        }
    }

    /// Return a version of this instance ID without peer context
    /// (for lockfile v1 compatibility).
    pub fn without_peers(&self) -> PackageInstanceId {
        PackageInstanceId {
            package: self.package.clone(),
            peer_context: PeerContext::default(),
        }
    }
}

/// Peer requirement: describes what a package declares as a peer dependency.
#[derive(Debug, Clone)]
pub struct PeerRequirement {
    /// Package that makes this requirement.
    pub requester: PackageId,
    /// Name of the required peer package.
    pub name: PackageName,
    /// Version constraint on the peer.
    pub range: VersionConstraint,
    /// Whether the peer is optional.
    pub optional: bool,
}

// ─── Resolver diagnostics ─────────────────────────────────────────────────────

/// Diagnostic messages produced during dependency resolution.
#[derive(Debug, Clone)]
pub enum ResolverDiagnostic {
    /// A required peer dependency was not found in the environment.
    MissingPeer {
        /// The package that declared the peer dependency.
        requester: PackageId,
        /// Name of the missing peer package.
        peer_name: PackageName,
        /// Version constraint that could not be satisfied.
        range: VersionConstraint,
    },
    /// An optional peer dependency was not found (informational only).
    OptionalPeerMissing {
        /// The package that declared the peer dependency.
        requester: PackageId,
        /// Name of the missing optional peer package.
        peer_name: PackageName,
        /// Version constraint that could not be satisfied.
        range: VersionConstraint,
    },
    /// A peer dependency version conflict detected.
    PeerVersionConflict {
        /// The package that declared the peer dependency.
        requester: PackageId,
        /// Name of the conflicting peer package.
        peer_name: PackageName,
        /// Version range that was requested.
        requested_range: VersionConstraint,
        /// The version that was actually found.
        found_version: Version,
    },
}

impl fmt::Display for ResolverDiagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ResolverDiagnostic::MissingPeer {
                requester,
                peer_name,
                range,
            } => {
                writeln!(f, "warning: unmet peer dependency")?;
                writeln!(f, "  {} requires {}@{}", requester, peer_name, range.raw)?;
                write!(f, "hint: install the required peer dependency")
            }
            ResolverDiagnostic::OptionalPeerMissing {
                requester,
                peer_name,
                range,
            } => {
                writeln!(f, "info: optional peer not found")?;
                write!(
                    f,
                    "  {} prefers {}@{} (optional)",
                    requester, peer_name, range.raw
                )
            }
            ResolverDiagnostic::PeerVersionConflict {
                requester,
                peer_name,
                requested_range,
                found_version,
            } => {
                writeln!(f, "warning: peer dependency version conflict")?;
                writeln!(
                    f,
                    "  {} requires {}@{}",
                    requester, peer_name, requested_range.raw
                )?;
                writeln!(f, "  found {}@{}", peer_name, found_version)?;
                write!(
                    f,
                    "hint: update {} to satisfy the range, or install a compatible {} version",
                    peer_name, requester
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

// ─── Integrity ────────────────────────────────────────────────────────────────

/// Parsed npm Subresource Integrity metadata.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Integrity {
    /// Digest entries in the order they appeared in the source string.
    pub algorithms: Vec<IntegrityDigest>,
}

/// A single algorithm/digest pair from an integrity string.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IntegrityDigest {
    /// Digest algorithm.
    pub algorithm: IntegrityAlgorithm,
    /// Base64-encoded digest.
    pub digest_base64: String,
}

/// Supported integrity algorithms.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum IntegrityAlgorithm {
    /// Legacy SHA-1 integrity.
    Sha1,
    /// Modern SHA-512 integrity.
    Sha512,
}

/// Integrity parsing errors.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum IntegrityError {
    /// Integrity string was empty.
    #[error("integrity string is empty")]
    Empty,
    /// Integrity digest did not contain a supported algorithm.
    #[error("unsupported integrity algorithm: {0}")]
    UnsupportedAlgorithm(String),
    /// Integrity digest did not contain a base64 payload.
    #[error("invalid integrity digest: {0}")]
    InvalidDigest(String),
}

impl Integrity {
    /// Parse an npm integrity string such as `sha512-... sha1-...`.
    pub fn parse(input: &str) -> Result<Self, IntegrityError> {
        let input = input.trim();
        if input.is_empty() {
            return Err(IntegrityError::Empty);
        }

        let algorithms: Result<Vec<_>, _> = input
            .split_whitespace()
            .map(|part| {
                let (algorithm, digest) = part
                    .split_once('-')
                    .ok_or_else(|| IntegrityError::InvalidDigest(part.to_string()))?;
                if digest.is_empty() {
                    return Err(IntegrityError::InvalidDigest(part.to_string()));
                }

                let algorithm = match algorithm {
                    "sha512" => IntegrityAlgorithm::Sha512,
                    "sha1" => IntegrityAlgorithm::Sha1,
                    other => return Err(IntegrityError::UnsupportedAlgorithm(other.to_string())),
                };

                Ok(IntegrityDigest {
                    algorithm,
                    digest_base64: digest.to_string(),
                })
            })
            .collect();

        Ok(Self {
            algorithms: algorithms?,
        })
    }

    /// Return the strongest digest available.
    pub fn strongest(&self) -> Option<&IntegrityDigest> {
        self.algorithms.iter().max_by_key(|digest| digest.algorithm)
    }
}

// ─── Registry URL helpers ─────────────────────────────────────────────────────

/// Build the packument metadata URL for a package name.
pub fn package_metadata_url(registry: &Url, name: &PackageName) -> anyhow::Result<Url> {
    let mut registry = registry.clone();
    if !registry.path().ends_with('/') {
        let path = format!("{}/", registry.path());
        registry.set_path(&path);
    }

    let encoded_name = name.as_str().replace('/', "%2f");
    Ok(registry.join(&encoded_name)?)
}

/// Build the conventional npm tarball URL for a package.
pub fn default_tarball_url(registry: &Url, id: &PackageId) -> anyhow::Result<Url> {
    let mut registry = registry.clone();
    if !registry.path().ends_with('/') {
        let path = format!("{}/", registry.path());
        registry.set_path(&path);
    }

    let unscoped = id.name.unscoped();
    let path = format!("{}/-/{}-{}.tgz", id.name.as_str(), unscoped, id.version);
    Ok(registry.join(&path)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_constraint_parse_treats_bare_non_semver_as_dist_tag() -> anyhow::Result<()> {
        let constraint = VersionConstraint::parse("next")?;

        assert!(matches!(constraint.kind, ConstraintKind::Tag(tag) if tag == "next"));
        Ok(())
    }

    #[test]
    fn version_constraint_parse_treats_npm_x_range_as_range() -> anyhow::Result<()> {
        let constraint = VersionConstraint::parse("1.6.x")?;

        assert!(matches!(constraint.kind, ConstraintKind::Range(_)));
        Ok(())
    }

    #[test]
    fn version_constraint_parse_treats_npm_or_range_as_any_range() -> anyhow::Result<()> {
        let constraint = VersionConstraint::parse("^9.0.3 || ^10.1.2")?;

        assert!(matches!(constraint.kind, ConstraintKind::AnyRange(ranges) if ranges.len() == 2));
        Ok(())
    }

    #[test]
    fn version_constraint_parse_normalizes_spaced_comparator_range() -> anyhow::Result<()> {
        let constraint = VersionConstraint::parse(">= 2.1.2 < 3.0.0")?;

        assert!(matches!(constraint.kind, ConstraintKind::Range(_)));
        Ok(())
    }

    #[test]
    fn version_constraint_parse_supports_npm_hyphen_range() -> anyhow::Result<()> {
        let constraint = VersionConstraint::parse("0.81 - 0.85")?;

        assert!(matches!(constraint.kind, ConstraintKind::Range(_)));
        Ok(())
    }

    #[test]
    fn version_constraint_parse_supports_npm_alias() -> anyhow::Result<()> {
        let constraint = VersionConstraint::parse("npm:wrap-ansi@^7.0.0")?;

        assert!(
            matches!(constraint.kind, ConstraintKind::Alias { package, .. } if package.as_str() == "wrap-ansi")
        );
        Ok(())
    }

    #[test]
    fn package_name_parse_normalizes_scoped_names() -> anyhow::Result<()> {
        let name = PackageName::parse("@Scope/Package")?;

        assert_eq!(name.as_str(), "@scope/package");
        assert_eq!(name.scope(), Some("scope"));
        assert_eq!(name.unscoped(), "package");
        Ok(())
    }

    #[test]
    fn package_name_parse_rejects_path_like_names() {
        let result = PackageName::parse("../pkg");

        assert!(matches!(result, Err(PackageNameError::InvalidCharacter(_))));
    }

    #[test]
    fn integrity_parse_returns_strongest_digest() -> anyhow::Result<()> {
        let integrity = Integrity::parse("sha1-abc sha512-def")?;

        assert_eq!(
            integrity.strongest().map(|digest| digest.algorithm),
            Some(IntegrityAlgorithm::Sha512)
        );
        Ok(())
    }

    #[test]
    fn integrity_parse_rejects_unknown_algorithm() {
        let result = Integrity::parse("md5-abc");

        assert!(matches!(
            result,
            Err(IntegrityError::UnsupportedAlgorithm(algorithm)) if algorithm == "md5"
        ));
    }

    #[test]
    fn package_metadata_url_encodes_scoped_package_slash() -> anyhow::Result<()> {
        let registry = Url::parse("https://registry.npmjs.org")?;
        let name = PackageName::parse("@scope/pkg")?;

        let url = package_metadata_url(&registry, &name)?;

        assert_eq!(url.as_str(), "https://registry.npmjs.org/@scope%2fpkg");
        Ok(())
    }

    #[test]
    fn default_tarball_url_uses_unscoped_tarball_name() -> anyhow::Result<()> {
        let registry = Url::parse("https://registry.npmjs.org/")?;
        let id = PackageId::new(PackageName::parse("@scope/pkg")?, Version::parse("1.2.3")?);

        let url = default_tarball_url(&registry, &id)?;

        assert_eq!(
            url.as_str(),
            "https://registry.npmjs.org/@scope/pkg/-/pkg-1.2.3.tgz"
        );
        Ok(())
    }

    #[test]
    fn peer_context_key_generates_sorted_suffix() -> anyhow::Result<()> {
        let mut ctx = PeerContext::default();
        ctx.insert(
            PackageName::from("lodash"),
            PackageId::new(PackageName::from("lodash"), Version::parse("4.17.21")?),
        );
        ctx.insert(
            PackageName::from("react"),
            PackageId::new(PackageName::from("react"), Version::parse("18.2.0")?),
        );
        let key = ctx.key();
        assert!(key.contains("(lodash@4.17.21)"));
        assert!(key.contains("(react@18.2.0)"));
        Ok(())
    }

    #[test]
    fn peer_context_key_empty_when_no_peers() {
        let ctx = PeerContext::default();
        assert!(ctx.is_empty());
        assert_eq!(ctx.key(), "");
    }

    #[test]
    fn package_instance_id_key_includes_peer_suffix() -> anyhow::Result<()> {
        let mut ctx = PeerContext::default();
        ctx.insert(
            PackageName::from("react"),
            PackageId::new(PackageName::from("react"), Version::parse("18.2.0")?),
        );
        let instance = PackageInstanceId::new(
            PackageId::new(PackageName::from("react-dom"), Version::parse("18.2.0")?),
            ctx,
        );
        let key = instance.key();
        assert!(key.contains("react-dom@18.2.0"));
        assert!(key.contains("react@18.2.0"));
        Ok(())
    }

    #[test]
    fn package_instance_id_key_without_peers_matches_package_key() -> anyhow::Result<()> {
        let ctx = PeerContext::default();
        let instance = PackageInstanceId::new(
            PackageId::new(PackageName::from("react-dom"), Version::parse("18.2.0")?),
            ctx,
        );
        assert_eq!(instance.key(), instance.without_peers().key());
        Ok(())
    }

    #[test]
    fn resolver_diagnostic_display_formats_missing_peer() -> anyhow::Result<()> {
        let diag = ResolverDiagnostic::MissingPeer {
            requester: PackageId::new(PackageName::from("react-dom"), Version::parse("18.2.0")?),
            peer_name: PackageName::from("react"),
            range: VersionConstraint::parse("^18.0.0")?,
        };
        let msg = diag.to_string();
        assert!(msg.contains("unmet peer dependency"));
        assert!(msg.contains("react-dom@18.2.0"));
        assert!(msg.contains("^18.0.0"));
        Ok(())
    }

    #[test]
    fn resolver_diagnostic_display_formats_peer_conflict() -> anyhow::Result<()> {
        let diag = ResolverDiagnostic::PeerVersionConflict {
            requester: PackageId::new(PackageName::from("react-dom"), Version::parse("18.2.0")?),
            peer_name: PackageName::from("react"),
            requested_range: VersionConstraint::parse("^18.0.0")?,
            found_version: Version::parse("17.0.2")?,
        };
        let msg = diag.to_string();
        assert!(msg.contains("version conflict"));
        assert!(msg.contains("found"));
        assert!(msg.contains("17.0.2"));
        Ok(())
    }
}
