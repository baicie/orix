//! Lockfile read/write, update, diff, and validation.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::types::{
    Lockfile, LockfileDiff, PackageLock, PackageResolution, ResolvedDep, LOCKFILE_VERSION,
};

impl Lockfile {
    /// Create an empty lockfile.
    pub fn empty() -> Self {
        Self {
            version: LOCKFILE_VERSION,
            save_remote_cache_urls: true,
            importers: Default::default(),
            packages: Default::default(),
            graph_hash: None,
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
        lockfile.version = LOCKFILE_VERSION;

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
                importer.dependencies.insert(
                    name.clone(),
                    resolved_importer_dep(pkg.id.version.to_string(), raw.clone()),
                );
            }
        }

        for (name, raw) in &manifest.dev_dependencies {
            if let Some(pkg) = graph.packages().find(|p| p.id.name.as_str() == name) {
                importer.dev_dependencies.insert(
                    name.clone(),
                    resolved_importer_dep(pkg.id.version.to_string(), raw.clone()),
                );
            }
        }

        for (name, raw) in &manifest.optional_dependencies {
            if let Some(pkg) = graph.packages().find(|p| p.id.name.as_str() == name) {
                importer.optional_dependencies.insert(
                    name.clone(),
                    resolved_importer_dep(pkg.id.version.to_string(), raw.clone()),
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
            let peer_deps: BTreeMap<String, String> = pkg
                .peer_dependencies
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
                    peer_dependencies: peer_deps,
                    engines: pkg.engines.clone(),
                    os: non_empty_vec(pkg.os.clone()),
                    cpu: non_empty_vec(pkg.cpu.clone()),
                },
            );
        }

        lockfile.graph_hash = Some(graph.graph_hash());

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

    /// Returns true when the diff contains any package or importer changes.
    pub fn diff_has_changes(diff: &LockfileDiff) -> bool {
        !diff.added.is_empty()
            || !diff.removed.is_empty()
            || !diff.changed.is_empty()
            || !diff.importers_changed.is_empty()
    }

    /// Validate that this lockfile exactly matches the manifest dependency specifiers.
    pub fn validate_frozen(
        &self,
        manifest: &orix_manifest::Manifest,
        importer_id: &str,
    ) -> anyhow::Result<()> {
        if self.version != LOCKFILE_VERSION {
            anyhow::bail!(
                "Lockfile version {} is not supported by this orix version (expected {}). Run orix install to regenerate it.",
                self.version,
                LOCKFILE_VERSION
            );
        }

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

    /// Validate that the lockfile file is structurally usable (version + importer present).
    ///
    /// Does **not** compare dependency specifiers to `package.json`. Use
    /// [`Self::validate_frozen`] before taking the install fast path.
    pub fn validate(
        &self,
        _manifest: &orix_manifest::Manifest,
        importer_id: &str,
    ) -> anyhow::Result<()> {
        if self.version != LOCKFILE_VERSION {
            anyhow::bail!(
                "Lockfile version {} is not supported by this orix version (expected {})",
                self.version,
                LOCKFILE_VERSION
            );
        }

        if self.importers.contains_key(importer_id) {
            Ok(())
        } else {
            anyhow::bail!("Lockfile is missing importer '{}'", importer_id);
        }
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

fn resolved_importer_dep(version: String, specifier: String) -> ResolvedDep {
    ResolvedDep {
        version,
        specifier,
        id: None,
        dev: None,
        optional: None,
        engines: None,
        os: None,
        cpu: None,
        dependencies: BTreeMap::new(),
        optional_dependencies: BTreeMap::new(),
        peer_dependencies: BTreeMap::new(),
    }
}

fn non_empty_vec(values: Vec<String>) -> Option<Vec<String>> {
    if values.is_empty() {
        None
    } else {
        Some(values)
    }
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
