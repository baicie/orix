//! orix-lock.yaml management.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use orix_domain::{DependencyGraph, PackageId, PackageName, ResolvedPackage, Version};

/// Lockfile format version.
pub const LOCKFILE_VERSION: i32 = 1;

/// The lockfile root — mirrors pnpm's orix-lock.yaml structure.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lockfile {
    /// Lockfile version number.
    #[serde(rename = "lockfileVersion")]
    pub version: i32,
    /// Whether to save remote cache URLs.
    #[serde(rename = "saveRemoteCacheURLs", default)]
    pub save_remote_cache_urls: bool,
    /// Per-importer dependency resolutions.
    pub importers: BTreeMap<String, ImporterLock>,
    /// Resolved packages keyed by package ID.
    pub packages: BTreeMap<String, PackageLock>,
}

/// Dependency resolutions for one importer (root or workspace package).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImporterLock {
    /// Resolved production dependencies.
    #[serde(default)]
    pub dependencies: BTreeMap<String, ResolvedDep>,
    /// Resolved development dependencies.
    #[serde(rename = "devDependencies", default)]
    pub dev_dependencies: BTreeMap<String, ResolvedDep>,
    /// Resolved optional dependencies.
    #[serde(rename = "optionalDependencies", default)]
    pub optional_dependencies: BTreeMap<String, ResolvedDep>,
    /// Original dependency specifiers (for diffing).
    #[serde(default)]
    pub specifiers: BTreeMap<String, String>,
}

/// A single resolved dependency entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedDep {
    /// Resolved version string.
    pub version: String,
    /// Original specifier from package.json.
    #[serde(rename = "specifier", default)]
    pub specifier: String,
    /// Registry package ID.
    #[serde(rename = "id", default)]
    pub id: Option<String>,
    /// Whether this is a dev dependency.
    #[serde(rename = "dev", default)]
    pub dev: Option<bool>,
    /// Whether this is an optional dependency.
    #[serde(rename = "optional", default)]
    pub optional: Option<bool>,
    /// Node engine constraint.
    #[serde(rename = "engines", default)]
    pub engines: Option<String>,
    /// Supported OS constraints.
    #[serde(rename = "os", default)]
    pub os: Option<Vec<String>>,
    /// Supported CPU constraints.
    #[serde(rename = "cpu", default)]
    pub cpu: Option<Vec<String>>,
    /// Transitive dependencies of this package.
    #[serde(rename = "dependencies", default)]
    pub dependencies: BTreeMap<String, String>,
    /// Transitive optional dependencies.
    #[serde(rename = "optionalDependencies", default)]
    pub optional_dependencies: BTreeMap<String, String>,
}

/// A resolved package entry in the lockfile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageLock {
    /// Registry package ID.
    #[serde(rename = "id", default)]
    pub id: Option<String>,
    /// Local path (for workspace packages).
    #[serde(rename = "local", default)]
    pub local: Option<String>,
    /// Integrity hash.
    pub integrity: Option<String>,
    /// Package name.
    pub name: Option<String>,
    /// Package version.
    pub version: Option<String>,
    /// Resolution details.
    pub resolution: Option<PackageResolution>,
    /// Transitive dependencies.
    #[serde(default)]
    pub dependencies: BTreeMap<String, String>,
    /// Transitive optional dependencies.
    #[serde(rename = "optionalDependencies", default)]
    pub optional_dependencies: BTreeMap<String, String>,
    /// Node engine constraint.
    #[serde(rename = "engines", default)]
    pub engines: Option<String>,
    /// Supported OS constraints.
    #[serde(rename = "os", default)]
    pub os: Option<Vec<String>>,
    /// Supported CPU constraints.
    #[serde(rename = "cpu", default)]
    pub cpu: Option<Vec<String>>,
}

/// Resolution details for a package.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageResolution {
    /// Tarball URL.
    pub tarball: Option<String>,
    /// Integrity hash.
    pub integrity: Option<String>,
    /// Resolution type.
    #[serde(rename = "type", default)]
    pub resolution_type: Option<String>,
    /// Local file path (for workspace packages).
    pub path: Option<String>,
}

/// The diff between two lockfile states.
#[derive(Debug, Default)]
pub struct LockfileDiff {
    /// Packages added since the old lockfile.
    pub added: Vec<String>,
    /// Packages removed since the old lockfile.
    pub removed: Vec<String>,
    /// Packages whose lockfile entry changed while keeping the same package key.
    pub changed: Vec<String>,
    /// Importers whose specifiers changed.
    pub importers_changed: Vec<String>,
}

impl Lockfile {
    /// Create an empty lockfile.
    pub fn empty() -> Self {
        Self {
            version: LOCKFILE_VERSION,
            save_remote_cache_urls: true,
            importers: Default::default(),
            packages: Default::default(),
        }
    }

    /// Read a lockfile from a YAML file.
    pub fn read(path: &Path) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        serde_yaml::from_str(&content)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e).into())
    }

    /// Write the lockfile to a YAML file atomically.
    pub fn write(&self, path: &Path) -> anyhow::Result<()> {
        let yaml = serde_yaml::to_string(self)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let tmp = temporary_lockfile_path(path);
        if let Err(error) = std::fs::write(&tmp, &yaml).and_then(|()| std::fs::rename(&tmp, path)) {
            let _ = std::fs::remove_file(&tmp);
            return Err(error.into());
        }

        Ok(())
    }

    /// Update the lockfile from a manifest and resolved dependency graph.
    pub fn update(
        &self,
        manifest: &orix_manifest::Manifest,
        graph: &orix_domain::DependencyGraph,
        importer_id: &str,
    ) -> Self {
        use std::collections::BTreeMap;

        let mut lockfile = self.clone();

        let importer = lockfile
            .importers
            .entry(importer_id.to_string())
            .or_default();

        importer.specifiers.clear();
        for (name, raw) in &manifest.dependencies {
            importer.specifiers.insert(name.clone(), raw.clone());
        }
        for (name, raw) in &manifest.dev_dependencies {
            importer.specifiers.insert(name.clone(), raw.clone());
        }
        for (name, raw) in &manifest.optional_dependencies {
            importer.specifiers.insert(name.clone(), raw.clone());
        }

        importer.dependencies.clear();
        importer.dev_dependencies.clear();
        importer.optional_dependencies.clear();

        for (name, raw) in &manifest.dependencies {
            if let Some(pkg) = graph.packages().find(|p| p.id.name.as_str() == name) {
                let deps: BTreeMap<String, String> = pkg
                    .dependencies
                    .iter()
                    .map(|(n, c)| (n.to_string(), c.clone()))
                    .collect();
                let opt_deps: BTreeMap<String, String> = pkg
                    .optional_dependencies
                    .iter()
                    .map(|(n, c)| (n.to_string(), c.clone()))
                    .collect();
                importer.dependencies.insert(
                    name.clone(),
                    ResolvedDep {
                        version: pkg.id.version.to_string(),
                        specifier: raw.clone(),
                        id: Some(format!("registry.npmjs.org/{}/{}", name, pkg.id.version)),
                        dev: Some(false),
                        optional: Some(false),
                        engines: pkg.engines.clone(),
                        os: Some(pkg.os.clone()),
                        cpu: Some(pkg.cpu.clone()),
                        dependencies: deps,
                        optional_dependencies: opt_deps,
                    },
                );
            }
        }

        for (name, raw) in &manifest.dev_dependencies {
            if let Some(pkg) = graph.packages().find(|p| p.id.name.as_str() == name) {
                let deps: BTreeMap<String, String> = pkg
                    .dependencies
                    .iter()
                    .map(|(n, c)| (n.to_string(), c.clone()))
                    .collect();
                let opt_deps: BTreeMap<String, String> = pkg
                    .optional_dependencies
                    .iter()
                    .map(|(n, c)| (n.to_string(), c.clone()))
                    .collect();
                importer.dev_dependencies.insert(
                    name.clone(),
                    ResolvedDep {
                        version: pkg.id.version.to_string(),
                        specifier: raw.clone(),
                        id: Some(format!("registry.npmjs.org/{}/{}", name, pkg.id.version)),
                        dev: Some(true),
                        optional: Some(false),
                        engines: pkg.engines.clone(),
                        os: Some(pkg.os.clone()),
                        cpu: Some(pkg.cpu.clone()),
                        dependencies: deps,
                        optional_dependencies: opt_deps,
                    },
                );
            }
        }

        for (name, raw) in &manifest.optional_dependencies {
            if let Some(pkg) = graph.packages().find(|p| p.id.name.as_str() == name) {
                let deps: BTreeMap<String, String> = pkg
                    .dependencies
                    .iter()
                    .map(|(n, c)| (n.to_string(), c.clone()))
                    .collect();
                let opt_deps: BTreeMap<String, String> = pkg
                    .optional_dependencies
                    .iter()
                    .map(|(n, c)| (n.to_string(), c.clone()))
                    .collect();
                importer.optional_dependencies.insert(
                    name.clone(),
                    ResolvedDep {
                        version: pkg.id.version.to_string(),
                        specifier: raw.clone(),
                        id: Some(format!("registry.npmjs.org/{}/{}", name, pkg.id.version)),
                        dev: Some(false),
                        optional: Some(true),
                        engines: pkg.engines.clone(),
                        os: Some(pkg.os.clone()),
                        cpu: Some(pkg.cpu.clone()),
                        dependencies: deps,
                        optional_dependencies: opt_deps,
                    },
                );
            }
        }

        for pkg in graph.packages() {
            let key = format!("/{}@{}", pkg.id.name, pkg.id.version);
            let deps: BTreeMap<String, String> = pkg
                .dependencies
                .iter()
                .map(|(n, c)| (n.to_string(), c.clone()))
                .collect();
            let opt_deps: BTreeMap<String, String> = pkg
                .optional_dependencies
                .iter()
                .map(|(n, c)| (n.to_string(), c.clone()))
                .collect();
            lockfile.packages.insert(
                key,
                PackageLock {
                    id: Some(format!(
                        "registry.npmjs.org/{}/{}",
                        pkg.id.name, pkg.id.version
                    )),
                    local: None,
                    integrity: Some(pkg.integrity.clone()),
                    name: Some(pkg.id.name.to_string()),
                    version: Some(pkg.id.version.to_string()),
                    resolution: Some(PackageResolution {
                        tarball: Some(pkg.tarball.clone()),
                        integrity: Some(pkg.integrity.clone()),
                        resolution_type: None,
                        path: None,
                    }),
                    dependencies: deps,
                    optional_dependencies: opt_deps,
                    engines: pkg.engines.clone(),
                    os: Some(pkg.os.clone()),
                    cpu: Some(pkg.cpu.clone()),
                },
            );
        }

        lockfile
    }

    /// Compute the diff between two lockfiles.
    pub fn diff(old: &Lockfile, new: &Lockfile) -> LockfileDiff {
        use std::collections::HashSet;

        let old_keys: HashSet<_> = old.packages.keys().collect();
        let new_keys: HashSet<_> = new.packages.keys().collect();

        let mut added: Vec<_> = new_keys
            .difference(&old_keys)
            .map(|k| (*k).clone())
            .collect();
        let mut removed: Vec<_> = old_keys
            .difference(&new_keys)
            .map(|k| (*k).clone())
            .collect();
        let mut changed: Vec<_> = old_keys
            .intersection(&new_keys)
            .filter_map(|key| {
                if old.packages.get(*key) != new.packages.get(*key) {
                    Some((*key).clone())
                } else {
                    None
                }
            })
            .collect();

        let mut importers_changed: Vec<_> = old
            .importers
            .keys()
            .chain(new.importers.keys())
            .filter(|importer_id| {
                old.importers
                    .get(*importer_id)
                    .map(|importer| &importer.specifiers)
                    != new
                        .importers
                        .get(*importer_id)
                        .map(|importer| &importer.specifiers)
            })
            .cloned()
            .collect();

        added.sort();
        removed.sort();
        changed.sort();
        importers_changed.sort();
        importers_changed.dedup();

        LockfileDiff {
            added,
            removed,
            changed,
            importers_changed,
        }
    }

    /// Validate that this lockfile exactly matches the manifest dependency specifiers.
    pub fn validate_frozen(
        &self,
        manifest: &orix_manifest::Manifest,
        importer_id: &str,
    ) -> anyhow::Result<()> {
        let importer = self.importers.get(importer_id).ok_or_else(|| {
            anyhow::anyhow!(
                "Lockfile mismatch: importer '{}' is missing from lockfile",
                importer_id
            )
        })?;

        validate_dependency_group(
            "dependencies",
            &manifest.dependencies,
            &importer.dependencies,
            importer_id,
        )?;
        validate_dependency_group(
            "devDependencies",
            &manifest.dev_dependencies,
            &importer.dev_dependencies,
            importer_id,
        )?;
        validate_dependency_group(
            "optionalDependencies",
            &manifest.optional_dependencies,
            &importer.optional_dependencies,
            importer_id,
        )?;

        Ok(())
    }

    /// Return all package IDs referenced by the lockfile package section.
    pub fn package_ids(&self) -> anyhow::Result<Vec<orix_domain::PackageId>> {
        self.packages
            .keys()
            .map(|key| {
                let key = key.trim_start_matches('/');
                let (name, version) = key
                    .rsplit_once('@')
                    .ok_or_else(|| anyhow::anyhow!("invalid lockfile package key '{}'", key))?;
                Ok(orix_domain::PackageId::new(
                    orix_domain::PackageName::from(name.to_string()),
                    orix_domain::Version::parse(version)?,
                ))
            })
            .collect()
    }

    /// Remove all packages from the lockfile that are not transitively referenced
    /// by any importer. Returns the number of packages removed.
    pub fn retain_only_referenced_packages(&mut self) -> usize {
        let mut referenced_keys = std::collections::HashSet::new();

        for importer in self.importers.values() {
            for (name, dep) in importer.dependencies.iter() {
                referenced_keys.insert(format!("/{}@{}", name, dep.version));
                for (dep_name, dep_ver) in dep.dependencies.iter() {
                    referenced_keys.insert(format!("/{}@{}", dep_name, dep_ver));
                }
                for (dep_name, dep_ver) in dep.optional_dependencies.iter() {
                    referenced_keys.insert(format!("/{}@{}", dep_name, dep_ver));
                }
            }
            for (name, dep) in importer.dev_dependencies.iter() {
                referenced_keys.insert(format!("/{}@{}", name, dep.version));
                for (dep_name, dep_ver) in dep.dependencies.iter() {
                    referenced_keys.insert(format!("/{}@{}", dep_name, dep_ver));
                }
                for (dep_name, dep_ver) in dep.optional_dependencies.iter() {
                    referenced_keys.insert(format!("/{}@{}", dep_name, dep_ver));
                }
            }
            for (name, dep) in importer.optional_dependencies.iter() {
                referenced_keys.insert(format!("/{}@{}", name, dep.version));
                for (dep_name, dep_ver) in dep.dependencies.iter() {
                    referenced_keys.insert(format!("/{}@{}", dep_name, dep_ver));
                }
                for (dep_name, dep_ver) in dep.optional_dependencies.iter() {
                    referenced_keys.insert(format!("/{}@{}", dep_name, dep_ver));
                }
            }
        }

        let before = self.packages.len();
        self.packages.retain(|key, _| referenced_keys.contains(key));
        before - self.packages.len()
    }
}

/// Resolve dependencies from a lockfile packages section (frozen/install-from-lock workflow).
pub fn resolve_from_lockfile_packages(packages: &BTreeMap<String, PackageLock>) -> DependencyGraph {
    let mut graph = DependencyGraph::new();

    for (key, pkg) in packages {
        let tarball = match pkg.resolution.as_ref().and_then(|r| r.tarball.clone()) {
            Some(t) => t,
            None => continue,
        };

        let integrity = pkg.integrity.clone().unwrap_or_default();
        let key_str = key.trim_start_matches('/');
        let (name_str, ver_str) = key_str.rsplit_once('@').unwrap_or((key_str, ""));

        let name = PackageName::from(name_str);
        let version = match Version::parse(ver_str) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let pkg_id = PackageId::new(name.clone(), version);

        let deps: Vec<(PackageName, String)> = pkg
            .dependencies
            .iter()
            .map(|(k, v)| (PackageName::from(k.as_str()), v.clone()))
            .collect();
        let opt_deps: Vec<(PackageName, String)> = pkg
            .optional_dependencies
            .iter()
            .map(|(k, v)| (PackageName::from(k.as_str()), v.clone()))
            .collect();

        let depnodes: Vec<String> = deps
            .iter()
            .chain(opt_deps.iter())
            .map(|(n, _)| n.to_string())
            .collect();

        let resolved = ResolvedPackage {
            id: pkg_id.clone(),
            integrity,
            tarball,
            dependencies: deps,
            dev_dependencies: Vec::new(),
            optional_dependencies: opt_deps,
            peer_dependencies: Vec::new(),
            engines: pkg.engines.clone(),
            os: pkg.os.clone().unwrap_or_default(),
            cpu: pkg.cpu.clone().unwrap_or_default(),
            depnodes,
        };
        graph.insert(resolved);
    }

    graph
}

fn temporary_lockfile_path(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("orix-lock.yaml");
    let tmp_name = format!(".{}.{}.tmp", file_name, std::process::id());
    path.with_file_name(tmp_name)
}

fn validate_dependency_group(
    group_name: &str,
    manifest_deps: &BTreeMap<String, String>,
    locked_deps: &BTreeMap<String, ResolvedDep>,
    importer_id: &str,
) -> anyhow::Result<()> {
    for (name, constraint) in manifest_deps {
        let locked = locked_deps.get(name).ok_or_else(|| {
            anyhow::anyhow!(
                "Lockfile mismatch: '{}' is declared in {} for importer '{}' but not in lockfile",
                name,
                group_name,
                importer_id
            )
        })?;

        if locked.specifier != *constraint {
            anyhow::bail!(
                "Lockfile mismatch: '{}' specifier is '{}' in lockfile but '{}' in package.json",
                name,
                locked.specifier,
                constraint
            );
        }
    }

    for name in locked_deps.keys() {
        if !manifest_deps.contains_key(name) {
            anyhow::bail!(
                "Lockfile mismatch: '{}' exists in {} for importer '{}' but is not declared in package.json",
                name,
                group_name,
                importer_id
            );
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use orix_domain::{DependencyGraph, PackageId, PackageName, ResolvedPackage, Version};
    use orix_manifest::Manifest;

    fn resolved_dep(version: &str, specifier: &str) -> ResolvedDep {
        ResolvedDep {
            version: version.to_string(),
            specifier: specifier.to_string(),
            id: None,
            dev: None,
            optional: None,
            engines: None,
            os: None,
            cpu: None,
            dependencies: BTreeMap::new(),
            optional_dependencies: BTreeMap::new(),
        }
    }

    fn package_lock(name: &str, version: &str, integrity: &str) -> PackageLock {
        PackageLock {
            id: Some(format!("registry.npmjs.org/{}/{}", name, version)),
            local: None,
            integrity: Some(integrity.to_string()),
            name: Some(name.to_string()),
            version: Some(version.to_string()),
            resolution: Some(PackageResolution {
                tarball: Some(format!(
                    "https://registry.npmjs.org/{}/-/{}-{}.tgz",
                    name, name, version
                )),
                integrity: Some(integrity.to_string()),
                resolution_type: None,
                path: None,
            }),
            dependencies: BTreeMap::new(),
            optional_dependencies: BTreeMap::new(),
            engines: None,
            os: None,
            cpu: None,
        }
    }

    fn resolved_package(name: &str, version: &str) -> anyhow::Result<ResolvedPackage> {
        Ok(ResolvedPackage {
            id: PackageId::new(PackageName::from(name), Version::parse(version)?),
            integrity: format!("sha512-{}", version),
            tarball: format!(
                "https://registry.npmjs.org/{}/-/{}-{}.tgz",
                name, name, version
            ),
            dependencies: Vec::new(),
            dev_dependencies: Vec::new(),
            optional_dependencies: Vec::new(),
            peer_dependencies: Vec::new(),
            engines: None,
            os: Vec::new(),
            cpu: Vec::new(),
            depnodes: Vec::new(),
        })
    }

    #[test]
    fn frozen_validation_accepts_matching_dependency_groups() {
        let mut manifest = Manifest::default();
        manifest
            .dependencies
            .insert("react".to_string(), "^18.2.0".to_string());
        manifest
            .dev_dependencies
            .insert("vite".to_string(), "^5.0.0".to_string());
        manifest
            .optional_dependencies
            .insert("fsevents".to_string(), "^2.3.3".to_string());

        let mut importer = ImporterLock::default();
        importer
            .dependencies
            .insert("react".to_string(), resolved_dep("18.2.0", "^18.2.0"));
        importer
            .dev_dependencies
            .insert("vite".to_string(), resolved_dep("5.0.0", "^5.0.0"));
        importer
            .optional_dependencies
            .insert("fsevents".to_string(), resolved_dep("2.3.3", "^2.3.3"));

        let mut lockfile = Lockfile::empty();
        lockfile.importers.insert(".".to_string(), importer);

        assert!(lockfile.validate_frozen(&manifest, ".").is_ok());
    }

    #[test]
    fn frozen_validation_rejects_changed_specifier() {
        let mut manifest = Manifest::default();
        manifest
            .dependencies
            .insert("react".to_string(), "^19.0.0".to_string());

        let mut importer = ImporterLock::default();
        importer
            .dependencies
            .insert("react".to_string(), resolved_dep("18.2.0", "^18.2.0"));

        let mut lockfile = Lockfile::empty();
        lockfile.importers.insert(".".to_string(), importer);

        let result = lockfile.validate_frozen(&manifest, ".");
        assert!(matches!(result, Err(error) if error.to_string().contains("specifier")));
    }

    #[test]
    fn frozen_validation_rejects_stale_locked_dependency() {
        let manifest = Manifest::default();

        let mut importer = ImporterLock::default();
        importer
            .dependencies
            .insert("react".to_string(), resolved_dep("18.2.0", "^18.2.0"));

        let mut lockfile = Lockfile::empty();
        lockfile.importers.insert(".".to_string(), importer);

        let result = lockfile.validate_frozen(&manifest, ".");
        assert!(matches!(result, Err(error) if error.to_string().contains("not declared")));
    }

    #[test]
    fn write_and_read_roundtrip_preserves_lockfile() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("nested").join("orix-lock.yaml");
        let mut lockfile = Lockfile::empty();
        lockfile.packages.insert(
            "/react@18.2.0".to_string(),
            package_lock("react", "18.2.0", "sha512-a"),
        );

        lockfile.write(&path)?;
        let read = Lockfile::read(&path)?;

        assert_eq!(read, lockfile);
        Ok(())
    }

    #[test]
    fn diff_reports_added_removed_changed_and_importer_specifiers() {
        let mut old = Lockfile::empty();
        old.packages.insert(
            "/react@18.2.0".to_string(),
            package_lock("react", "18.2.0", "sha512-old"),
        );
        old.packages.insert(
            "/vite@5.0.0".to_string(),
            package_lock("vite", "5.0.0", "sha512-vite"),
        );
        let mut old_importer = ImporterLock::default();
        old_importer
            .specifiers
            .insert("react".to_string(), "^18.0.0".to_string());
        old.importers.insert(".".to_string(), old_importer);

        let mut new = Lockfile::empty();
        new.packages.insert(
            "/react@18.2.0".to_string(),
            package_lock("react", "18.2.0", "sha512-new"),
        );
        new.packages.insert(
            "/lodash@4.17.21".to_string(),
            package_lock("lodash", "4.17.21", "sha512-lodash"),
        );
        let mut new_importer = ImporterLock::default();
        new_importer
            .specifiers
            .insert("react".to_string(), "^18.2.0".to_string());
        new.importers.insert(".".to_string(), new_importer);

        let diff = Lockfile::diff(&old, &new);

        assert_eq!(diff.added, vec!["/lodash@4.17.21"]);
        assert_eq!(diff.removed, vec!["/vite@5.0.0"]);
        assert_eq!(diff.changed, vec!["/react@18.2.0"]);
        assert_eq!(diff.importers_changed, vec!["."]);
    }

    #[test]
    fn update_writes_importer_specifiers() -> anyhow::Result<()> {
        let mut manifest = Manifest::default();
        manifest
            .dependencies
            .insert("react".to_string(), "^18.2.0".to_string());
        manifest
            .dev_dependencies
            .insert("vite".to_string(), "^5.0.0".to_string());
        manifest
            .optional_dependencies
            .insert("fsevents".to_string(), "^2.3.3".to_string());

        let mut graph = DependencyGraph::new();
        graph.insert(resolved_package("react", "18.2.0")?);
        graph.insert(resolved_package("vite", "5.0.0")?);
        graph.insert(resolved_package("fsevents", "2.3.3")?);

        let lockfile = Lockfile::empty().update(&manifest, &graph, ".");
        let importer = lockfile
            .importers
            .get(".")
            .ok_or_else(|| anyhow::anyhow!("missing root importer"))?;

        assert_eq!(
            importer.specifiers.get("react"),
            Some(&"^18.2.0".to_string())
        );
        assert_eq!(importer.specifiers.get("vite"), Some(&"^5.0.0".to_string()));
        assert_eq!(
            importer.specifiers.get("fsevents"),
            Some(&"^2.3.3".to_string())
        );
        Ok(())
    }

    #[test]
    fn retain_only_referenced_packages_removes_unused_entries() {
        let mut lockfile = Lockfile::empty();

        // Add two packages to the lockfile
        lockfile.packages.insert(
            "/react@18.2.0".to_string(),
            PackageLock {
                id: Some("registry.npmjs.org/react/18.2.0".to_string()),
                local: None,
                integrity: Some("sha512-react".to_string()),
                name: Some("react".to_string()),
                version: Some("18.2.0".to_string()),
                resolution: None,
                dependencies: BTreeMap::new(),
                optional_dependencies: BTreeMap::new(),
                engines: None,
                os: None,
                cpu: None,
            },
        );
        lockfile.packages.insert(
            "/vite@5.0.0".to_string(),
            PackageLock {
                id: Some("registry.npmjs.org/vite/5.0.0".to_string()),
                local: None,
                integrity: Some("sha512-vite".to_string()),
                name: Some("vite".to_string()),
                version: Some("5.0.0".to_string()),
                resolution: None,
                dependencies: BTreeMap::new(),
                optional_dependencies: BTreeMap::new(),
                engines: None,
                os: None,
                cpu: None,
            },
        );

        // Only react is referenced by an importer
        let mut importer = ImporterLock::default();
        importer.dependencies.insert(
            "react".to_string(),
            ResolvedDep {
                version: "18.2.0".to_string(),
                id: Some("registry.npmjs.org/react/18.2.0".to_string()),
                specifier: "^18.0.0".to_string(),
                dev: Some(false),
                optional: Some(false),
                engines: None,
                os: None,
                cpu: None,
                dependencies: BTreeMap::new(),
                optional_dependencies: BTreeMap::new(),
            },
        );
        lockfile.importers.insert(".".to_string(), importer);

        let removed = lockfile.retain_only_referenced_packages();

        assert_eq!(removed, 1);
        assert!(lockfile.packages.contains_key("/react@18.2.0"));
        assert!(!lockfile.packages.contains_key("/vite@5.0.0"));
    }

    #[test]
    fn resolve_from_lockfile_packages_builds_graph() {
        let mut packages = BTreeMap::new();
        packages.insert(
            "/react@18.2.0".to_string(),
            PackageLock {
                resolution: Some(PackageResolution {
                    tarball: Some("https://registry.npmjs.org/react/-/react-18.2.0.tgz"
                        .to_string()),
                    integrity: Some("sha512-abc".to_string()),
                    resolution_type: None,
                    path: None,
                }),
                dependencies: BTreeMap::from([("scheduler".to_string(), "0.23.0".to_string())]),
                optional_dependencies: BTreeMap::from([("fsevents".to_string(), "2.3.3".to_string())]),
                id: None,
                local: None,
                integrity: None,
                name: None,
                version: None,
                engines: None,
                os: None,
                cpu: None,
            },
        );
        packages.insert(
            "/scheduler@0.23.0".to_string(),
            PackageLock {
                resolution: Some(PackageResolution {
                    tarball: Some(
                        "https://registry.npmjs.org/scheduler/-/scheduler-0.23.0.tgz"
                            .to_string(),
                    ),
                    integrity: Some("sha512-xyz".to_string()),
                    resolution_type: None,
                    path: None,
                }),
                dependencies: BTreeMap::new(),
                optional_dependencies: BTreeMap::new(),
                id: None,
                local: None,
                integrity: None,
                name: None,
                version: None,
                engines: None,
                os: None,
                cpu: None,
            },
        );

        let graph = resolve_from_lockfile_packages(&packages);

        assert_eq!(graph.len(), 2);
        let pkg_ids: Vec<_> = graph.packages().map(|p| p.id.key()).collect();
        assert!(
            pkg_ids.iter().any(|k| k.contains("react@18.2.0")),
            "expected react@18.2.0 in {:?}",
            pkg_ids
        );
        assert!(
            pkg_ids.iter().any(|k| k.contains("scheduler@0.23.0")),
            "expected scheduler@0.23.0 in {:?}",
            pkg_ids
        );
    }

    #[test]
    fn resolve_from_lockfile_packages_skips_packages_without_tarball() {
        let mut packages = BTreeMap::new();
        packages.insert(
            "/react@18.2.0".to_string(),
            PackageLock {
                resolution: Some(PackageResolution {
                    tarball: Some("https://registry.npmjs.org/react.tgz".to_string()),
                    integrity: None,
                    resolution_type: None,
                    path: None,
                }),
                id: None,
                local: None,
                integrity: None,
                name: None,
                version: None,
                dependencies: BTreeMap::new(),
                optional_dependencies: BTreeMap::new(),
                engines: None,
                os: None,
                cpu: None,
            },
        );
        // No tarball — should be skipped
        packages.insert(
            "/vite@5.0.0".to_string(),
            PackageLock {
                resolution: Some(PackageResolution {
                    tarball: None,
                    integrity: None,
                    resolution_type: None,
                    path: None,
                }),
                id: None,
                local: None,
                integrity: None,
                name: None,
                version: None,
                dependencies: BTreeMap::new(),
                optional_dependencies: BTreeMap::new(),
                engines: None,
                os: None,
                cpu: None,
            },
        );

        let graph = resolve_from_lockfile_packages(&packages);

        assert_eq!(graph.len(), 1);
        assert!(graph.packages().any(|p| p.id.name.as_str() == "react"));
        assert!(!graph.packages().any(|p| p.id.name.as_str() == "vite"));
    }
}
