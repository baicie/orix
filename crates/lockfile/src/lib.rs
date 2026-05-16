//! orix-lock.yaml management.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Lockfile format version.
pub const LOCKFILE_VERSION: i32 = 1;

/// The lockfile root — mirrors pnpm's orix-lock.yaml structure.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    pub fn read(path: &std::path::Path) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        serde_yaml::from_str(&content)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e).into())
    }

    /// Write the lockfile to a YAML file atomically.
    pub fn write(&self, path: &std::path::Path) -> anyhow::Result<()> {
        let yaml = serde_yaml::to_string(self)?;
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, &yaml)?;
        std::fs::rename(&tmp, path)?;
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

        LockfileDiff {
            added: new_keys
                .difference(&old_keys)
                .map(|k| (*k).clone())
                .collect(),
            removed: old_keys
                .difference(&new_keys)
                .map(|k| (*k).clone())
                .collect(),
            importers_changed: Vec::new(),
        }
    }
}
