//! pnpm-lock.yaml format support.
//!
//! Supports reading pnpm lockfiles and converting them to orix format.
//! Phase 9 implements reading support (9.3) and export support (9.4).

use std::collections::BTreeMap;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::{Lockfile, LOCKFILE_VERSION};

/// pnpm lockfile version as a string.
#[derive(Debug, Clone, PartialEq)]
pub enum PnpmLockfileVersion {
    /// pnpm v6 lockfile (lockfileVersion: 6.x)
    V6,
    /// pnpm v9 lockfile (lockfileVersion: 9.x)
    V9,
    /// Unknown version
    Unknown(String),
}

impl From<&serde_yaml::Value> for PnpmLockfileVersion {
    fn from(v: &serde_yaml::Value) -> Self {
        match v {
            serde_yaml::Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    if i == 6 || i == 6.0f64 as i64 {
                        return PnpmLockfileVersion::V6;
                    }
                    if i == 9 || i == 9.0f64 as i64 {
                        return PnpmLockfileVersion::V9;
                    }
                    return PnpmLockfileVersion::Unknown(i.to_string());
                }
                PnpmLockfileVersion::Unknown(n.to_string())
            }
            serde_yaml::Value::String(s) => {
                if s == "6.0" || s == "6.1" || s == "6.2" || s == "6.3" || s == "6" {
                    PnpmLockfileVersion::V6
                } else if s.starts_with("9.") || s == "9" {
                    PnpmLockfileVersion::V9
                } else {
                    PnpmLockfileVersion::Unknown(s.clone())
                }
            }
            _ => PnpmLockfileVersion::Unknown("unknown".to_string()),
        }
    }
}

/// Root of a pnpm-lock.yaml file (v9 schema, also compatible with v6).
#[derive(Debug, Clone, Deserialize)]
pub struct PnpmLockfile {
    /// Lockfile version.
    #[serde(rename = "lockfileVersion")]
    pub lockfile_version: serde_yaml::Value,
    /// Importers section (one per package.json).
    #[serde(default)]
    pub importers: BTreeMap<String, PnpmImporter>,
    /// Packages section (resolved packages).
    #[serde(default)]
    pub packages: BTreeMap<String, PnpmPackage>,
    /// Snapshot imports (v9 only, simplified form).
    #[serde(default)]
    pub snapshots: BTreeMap<String, PnpmSnapshot>,
    /// Patch information.
    #[serde(rename = "patchedDependencies", default)]
    pub patched_dependencies: BTreeMap<String, serde_yaml::Value>,
}

/// An importer section in pnpm-lock.yaml.
#[derive(Debug, Clone, Deserialize)]
pub struct PnpmImporter {
    /// Resolved dependencies.
    #[serde(default)]
    pub dependencies: BTreeMap<String, PnpmResolvedDep>,
    /// Resolved dev dependencies.
    #[serde(rename = "devDependencies", default)]
    pub dev_dependencies: BTreeMap<String, PnpmResolvedDep>,
    /// Resolved optional dependencies.
    #[serde(rename = "optionalDependencies", default)]
    pub optional_dependencies: BTreeMap<String, PnpmResolvedDep>,
    /// Dependency specifiers (original versions).
    #[serde(default)]
    pub specifiers: BTreeMap<String, String>,
}

/// A resolved dependency entry in pnpm-lock.yaml.
#[derive(Debug, Clone, Deserialize)]
pub struct PnpmResolvedDep {
    /// Resolved version.
    pub version: String,
    /// Original specifier.
    #[serde(default)]
    pub specifier: Option<String>,
    /// Optional flag.
    #[serde(rename = "optional", default)]
    pub optional: Option<bool>,
    /// Engines constraint.
    #[serde(rename = "engines", default)]
    pub engines: Option<String>,
    /// OS constraints.
    #[serde(default)]
    pub os: Option<Vec<String>>,
    /// CPU constraints.
    #[serde(default)]
    pub cpu: Option<Vec<String>>,
    /// Whether this is a dev dependency.
    #[serde(rename = "dev", default)]
    pub dev: Option<bool>,
}

/// A package entry in pnpm-lock.yaml (v9 packages section).
#[derive(Debug, Clone, Deserialize)]
pub struct PnpmPackage {
    #[serde(flatten)]
    pub resolution: PnpmResolution,
    /// Optional peer dependencies.
    #[serde(rename = "peerDependencies", default)]
    pub peer_dependencies: BTreeMap<String, String>,
    /// Optional peer dependencies metadata.
    #[serde(rename = "peerDependenciesMeta", default)]
    pub peer_dependencies_meta: BTreeMap<String, PnpmPeerDepMeta>,
    /// Dependencies of this package.
    #[serde(default)]
    pub dependencies: BTreeMap<String, String>,
    /// Optional dependencies of this package.
    #[serde(rename = "optionalDependencies", default)]
    pub optional_dependencies: BTreeMap<String, String>,
    /// Transitive peer dependencies.
    #[serde(rename = "transitivePeerDependencies", default)]
    pub transitive_peer_dependencies: Vec<String>,
    /// Engines constraint.
    #[serde(rename = "engines", default)]
    pub engines: Option<String>,
    /// OS constraints.
    #[serde(default)]
    pub os: Vec<String>,
    /// CPU constraints.
    #[serde(default)]
    pub cpu: Vec<String>,
    /// Whether this package has no peer dependencies.
    #[serde(rename = "hasNoPeerDependencies", default)]
    pub has_no_peer_dependencies: Option<bool>,
}

/// Peer dependency metadata.
#[derive(Debug, Clone, Deserialize)]
pub struct PnpmPeerDepMeta {
    /// Whether this peer dependency is optional.
    #[serde(default)]
    pub optional: Option<bool>,
}

/// Resolution details for a pnpm package.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PnpmResolution {
    /// Tarball URL.
    #[serde(rename = "tarball", default)]
    pub tarball: Option<String>,
    /// Integrity hash.
    #[serde(default)]
    pub integrity: Option<String>,
    /// Local path (for workspace packages).
    #[serde(default)]
    pub path: Option<String>,
    /// Snapshot directory (v9 content-addressable store).
    #[serde(rename = "dir", default)]
    pub dir: Option<String>,
}

/// A snapshot entry in pnpm-lock.yaml v9 (simplified packages format).
#[derive(Debug, Clone, Deserialize)]
pub struct PnpmSnapshot {
    /// Resolution information.
    #[serde(flatten)]
    pub resolution: PnpmResolution,
    /// Dependencies.
    #[serde(default)]
    pub dependencies: BTreeMap<String, String>,
}

/// Errors that can occur when importing a pnpm lockfile.
#[derive(Debug, thiserror::Error)]
pub enum PnpmImportError {
    /// Unsupported pnpm lockfile version.
    #[error("unsupported pnpm lockfile version: {0}")]
    UnsupportedVersion(String),

    /// Failed to parse pnpm lockfile content.
    #[error("failed to parse pnpm lockfile: {0}")]
    ParseError(String),

    /// The pnpm lockfile file is empty.
    #[error("pnpm lockfile is empty")]
    Empty,

    /// The pnpm lockfile is missing the importers section.
    #[error("missing importers section")]
    MissingImporters,
}

impl PnpmLockfile {
    /// Read and parse a pnpm-lock.yaml file.
    pub fn read(path: &std::path::Path) -> Result<Self, PnpmImportError> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| PnpmImportError::ParseError(e.to_string()))?;

        if content.trim().is_empty() {
            return Err(PnpmImportError::Empty);
        }

        let lockfile: PnpmLockfile = serde_yaml::from_str(&content)
            .map_err(|e| PnpmImportError::ParseError(e.to_string()))?;

        Ok(lockfile)
    }

    /// Detect the pnpm lockfile version.
    pub fn version(&self) -> PnpmLockfileVersion {
        PnpmLockfileVersion::from(&self.lockfile_version)
    }

    /// Check if this lockfile version is supported.
    pub fn is_supported(&self) -> bool {
        matches!(
            self.version(),
            PnpmLockfileVersion::V6 | PnpmLockfileVersion::V9
        )
    }

    /// Convert a pnpm-lock.yaml to an orix Lockfile.
    ///
    /// Package keys are normalized from pnpm's `/pkg@ver` format to
    /// orix's `/pkg@ver` format. Importers are mapped 1:1.
    pub fn into_orix_lockfile(self) -> Lockfile {
        let mut orix_packages: BTreeMap<String, crate::PackageLock> = BTreeMap::new();

        // Convert packages section (v9 format).
        for (key, pkg) in self.packages {
            let orix_key = normalize_package_key(&key);
            let orix_pkg = crate::PackageLock {
                id: None,
                local: pkg.resolution.path.clone(),
                integrity: pkg.resolution.integrity.clone(),
                name: extract_name_from_key(&orix_key),
                version: extract_version_from_key(&orix_key),
                resolution: Some(crate::PackageResolution {
                    tarball: pkg.resolution.tarball.clone(),
                    integrity: pkg.resolution.integrity.clone(),
                    resolution_type: None,
                    path: pkg.resolution.path.clone(),
                }),
                dependencies: pkg.dependencies,
                optional_dependencies: pkg.optional_dependencies,
                engines: pkg.engines,
                os: if pkg.os.is_empty() {
                    None
                } else {
                    Some(pkg.os)
                },
                cpu: if pkg.cpu.is_empty() {
                    None
                } else {
                    Some(pkg.cpu)
                },
            };
            orix_packages.insert(orix_key, orix_pkg);
        }

        // Convert importers to orix format.
        let mut orix_importers: BTreeMap<String, crate::ImporterLock> = BTreeMap::new();

        for (importer_id, importer) in self.importers {
            let mut orix_deps: BTreeMap<String, crate::ResolvedDep> = BTreeMap::new();
            let mut orix_dev_deps: BTreeMap<String, crate::ResolvedDep> = BTreeMap::new();
            let mut orix_opt_deps: BTreeMap<String, crate::ResolvedDep> = BTreeMap::new();

            for (name, dep) in importer.dependencies {
                orix_deps.insert(
                    name.clone(),
                    crate::ResolvedDep {
                        version: dep.version.clone(),
                        specifier: dep.specifier.unwrap_or_else(|| dep.version.clone()),
                        id: Some(format!("/{}@{}", name, dep.version)),
                        dev: dep.dev.or(Some(false)),
                        optional: dep.optional.or(Some(false)),
                        engines: dep.engines,
                        os: dep.os,
                        cpu: dep.cpu,
                        dependencies: BTreeMap::new(),
                        optional_dependencies: BTreeMap::new(),
                    },
                );
            }

            for (name, dep) in importer.dev_dependencies {
                orix_dev_deps.insert(
                    name.clone(),
                    crate::ResolvedDep {
                        version: dep.version.clone(),
                        specifier: dep.specifier.unwrap_or_else(|| dep.version.clone()),
                        id: Some(format!("/{}@{}", name, dep.version)),
                        dev: dep.dev.or(Some(true)),
                        optional: dep.optional.or(Some(false)),
                        engines: dep.engines,
                        os: dep.os,
                        cpu: dep.cpu,
                        dependencies: BTreeMap::new(),
                        optional_dependencies: BTreeMap::new(),
                    },
                );
            }

            for (name, dep) in importer.optional_dependencies {
                orix_opt_deps.insert(
                    name.clone(),
                    crate::ResolvedDep {
                        version: dep.version.clone(),
                        specifier: dep.specifier.unwrap_or_else(|| dep.version.clone()),
                        id: Some(format!("/{}@{}", name, dep.version)),
                        dev: dep.dev.or(Some(false)),
                        optional: dep.optional.or(Some(true)),
                        engines: dep.engines,
                        os: dep.os,
                        cpu: dep.cpu,
                        dependencies: BTreeMap::new(),
                        optional_dependencies: BTreeMap::new(),
                    },
                );
            }

            orix_importers.insert(
                importer_id,
                crate::ImporterLock {
                    dependencies: orix_deps,
                    dev_dependencies: orix_dev_deps,
                    optional_dependencies: orix_opt_deps,
                    specifiers: importer.specifiers,
                },
            );
        }

        Lockfile {
            version: LOCKFILE_VERSION,
            save_remote_cache_urls: true,
            importers: orix_importers,
            packages: orix_packages,
            graph_hash: None,
        }
    }
}

/// Normalize a pnpm package key to orix format.
///
/// pnpm v9 uses `/pkg@ver` (no leading slash on scope).
/// We ensure consistent formatting.
fn normalize_package_key(key: &str) -> String {
    // Strip any leading slashes that pnpm might add inconsistently.
    let key = key.trim_start_matches('/');

    // pnpm v9 uses snapshot keys like `/node_modules/pkg@ver`.
    // Extract just the name@version part.
    if let Some(at_pos) = key.rfind('@') {
        let (name_part, ver_part) = key.split_at(at_pos);
        // Find the name part - strip node_modules/ prefix if present.
        let name = if let Some(nm_pos) = name_part.rfind("node_modules/") {
            &name_part[nm_pos + "node_modules/".len()..]
        } else {
            name_part
        };
        // Remove leading @ if it's a scoped package (already in name).
        let name = name.trim_start_matches('/');
        return format!("/{}{}", name, ver_part);
    }

    format!("/{}", key)
}

/// Extract the package name from a normalized key.
fn extract_name_from_key(key: &str) -> Option<String> {
    let key = key.trim_start_matches('/');
    let at_pos = key.rfind('@')?;
    let name = &key[..at_pos];
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

/// Extract the version from a normalized key.
fn extract_version_from_key(key: &str) -> Option<String> {
    let key = key.trim_start_matches('/');
    let at_pos = key.rfind('@')?;
    let version = &key[at_pos + 1..];
    if version.is_empty() {
        None
    } else {
        Some(version.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pnpm_lockfile_version_parsing() {
        let v6 = PnpmLockfileVersion::from(&serde_yaml::Value::Number(6.into()));
        assert!(matches!(v6, PnpmLockfileVersion::V6));

        let v9 = PnpmLockfileVersion::from(&serde_yaml::Value::Number(9.into()));
        assert!(matches!(v9, PnpmLockfileVersion::V9));

        let v6str = PnpmLockfileVersion::from(&serde_yaml::Value::String("6.0".to_string()));
        assert!(matches!(v6str, PnpmLockfileVersion::V6));

        let v9str = PnpmLockfileVersion::from(&serde_yaml::Value::String("9.1".to_string()));
        assert!(matches!(v9str, PnpmLockfileVersion::V9));
    }

    #[test]
    fn test_normalize_package_key_simple() {
        assert_eq!(normalize_package_key("lodash@4.17.21"), "/lodash@4.17.21");
        assert_eq!(normalize_package_key("/react@18.2.0"), "/react@18.2.0");
    }

    #[test]
    fn test_normalize_package_key_scoped() {
        assert_eq!(
            normalize_package_key("@babel/core@7.24.0"),
            "/@babel/core@7.24.0"
        );
    }

    #[test]
    fn test_normalize_package_key_with_node_modules() {
        assert_eq!(
            normalize_package_key("/node_modules/lodash@4.17.21"),
            "/lodash@4.17.21"
        );
    }

    #[test]
    fn test_normalize_package_key_with_peer_suffix() {
        // Peer suffix format: /pkg@ver(peer@ver)(peer2@ver2)
        assert_eq!(
            normalize_package_key("/react-dom@18.2.0(react@18.2.0)"),
            "/react-dom@18.2.0(react@18.2.0)"
        );
    }

    #[test]
    fn test_extract_name_from_key() {
        assert_eq!(
            extract_name_from_key("/lodash@4.17.21"),
            Some("lodash".to_string())
        );
        assert_eq!(
            extract_name_from_key("/@babel/core@7.24.0"),
            Some("@babel/core".to_string())
        );
        assert_eq!(extract_name_from_key("/@version"), None);
    }

    #[test]
    fn test_extract_version_from_key() {
        assert_eq!(
            extract_version_from_key("/lodash@4.17.21"),
            Some("4.17.21".to_string())
        );
        assert_eq!(
            extract_version_from_key("/@babel/core@7.24.0"),
            Some("7.24.0".to_string())
        );
        assert_eq!(extract_version_from_key("/lodash@"), None);
    }

    #[test]
    fn test_into_orix_lockfile_v9() -> Result<()> {
        use anyhow::Context;
        let pnpm_yaml = r#"
lockfileVersion: 9
importers:
  .:
    specifiers:
      react: ^18.0.0
    dependencies:
      react:
        version: 18.2.0
        specifier: ^18.0.0
packages:
  /react@18.2.0:
    resolution:
      tarball: https://registry.npmjs.org/react/-/react-18.2.0.tgz
      integrity: sha512-abc
    dependencies:
      scheduler: 0.23.0
"#;
        let pnpm: PnpmLockfile = serde_yaml::from_str(pnpm_yaml)?;
        let orix = pnpm.into_orix_lockfile();

        assert_eq!(orix.version, LOCKFILE_VERSION);
        assert!(orix.importers.contains_key("."));
        assert!(orix.packages.contains_key("/react@18.2.0"));

        let importer = orix.importers.get(".").context("missing root importer")?;
        assert_eq!(
            importer.specifiers.get("react"),
            Some(&"^18.0.0".to_string())
        );
        assert_eq!(
            importer.dependencies.get("react"),
            Some(&crate::ResolvedDep {
                version: "18.2.0".to_string(),
                specifier: "^18.0.0".to_string(),
                id: Some("/react@18.2.0".to_string()),
                dev: Some(false),
                optional: Some(false),
                engines: None,
                os: None,
                cpu: None,
                dependencies: BTreeMap::new(),
                optional_dependencies: BTreeMap::new(),
            })
        );
        Ok(())
    }
}
