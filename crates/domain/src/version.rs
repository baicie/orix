//! Version and constraint types.

use std::fmt;
use std::ops::Deref;

use semver::{Version as SemverVersion, VersionReq};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::name::PackageName;

/// A semver version, normalized from a string.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
pub struct Version(SemverVersion);

impl Version {
    /// Parse a version string.
    pub fn parse(s: &str) -> anyhow::Result<Self> {
        let s = s
            .strip_prefix('v')
            .or_else(|| s.strip_prefix('V'))
            .unwrap_or(s);
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
    let mut parts = Vec::new();
    for part in raw.split('.').map(str::trim) {
        if matches!(part, "x" | "X" | "*") {
            break;
        }

        parts.push(part.parse::<u64>().ok()?);
    }

    (!parts.is_empty() && parts.len() <= 3).then_some(parts)
}
