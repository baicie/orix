//! orix-lock.yaml management.

mod ops;
mod pnpm;
mod resolve;
mod types;

pub use pnpm::{PnpmImportError, PnpmLockfile};
pub use resolve::resolve_from_lockfile_packages;
pub use types::{
    ImporterLock, Lockfile, LockfileDiff, PackageLock, PackageResolution, ResolvedDep,
    LOCKFILE_VERSION,
};

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

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
            peer_dependencies: BTreeMap::new(),
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
            peer_dependencies: BTreeMap::new(),
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
            patch: None,
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
        assert!(Lockfile::diff_has_changes(&diff));
    }

    #[test]
    fn diff_has_changes_true_for_importers_only() {
        let mut old = Lockfile::empty();
        let mut old_importer = ImporterLock::default();
        old_importer
            .specifiers
            .insert("react".to_string(), "^18.0.0".to_string());
        old.importers.insert(".".to_string(), old_importer);

        let mut new = old.clone();
        new.importers
            .get_mut(".")
            .expect("root importer")
            .specifiers
            .insert("react".to_string(), "^19.0.0".to_string());

        let diff = Lockfile::diff(&old, &new);
        assert!(diff.added.is_empty());
        assert!(diff.removed.is_empty());
        assert!(diff.changed.is_empty());
        assert_eq!(diff.importers_changed, vec!["."]);
        assert!(Lockfile::diff_has_changes(&diff));
    }

    #[test]
    fn validate_frozen_rejects_specifier_change_disables_fast_path() {
        let mut manifest = Manifest::default();
        manifest
            .dependencies
            .insert("react".to_string(), "^19.0.0".to_string());

        let mut importer = ImporterLock::default();
        importer
            .dependencies
            .insert("react".to_string(), resolved_dep("18.2.0", "^18.2.0"));
        importer
            .specifiers
            .insert("react".to_string(), "^18.2.0".to_string());

        let mut lockfile = Lockfile::empty();
        lockfile.importers.insert(".".to_string(), importer);

        assert!(lockfile.validate(&manifest, ".").is_ok());
        assert!(lockfile.validate_frozen(&manifest, ".").is_err());
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
                peer_dependencies: BTreeMap::new(),
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
                peer_dependencies: BTreeMap::new(),
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
                peer_dependencies: BTreeMap::new(),
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
                    tarball: Some(
                        "https://registry.npmjs.org/react/-/react-18.2.0.tgz".to_string(),
                    ),
                    integrity: Some("sha512-abc".to_string()),
                    resolution_type: None,
                    path: None,
                }),
                dependencies: BTreeMap::from([("scheduler".to_string(), "0.23.0".to_string())]),
                optional_dependencies: BTreeMap::from([(
                    "fsevents".to_string(),
                    "2.3.3".to_string(),
                )]),
                peer_dependencies: BTreeMap::from([(
                    "esbuild".to_string(),
                    ">=0.18.0".to_string(),
                )]),
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
                        "https://registry.npmjs.org/scheduler/-/scheduler-0.23.0.tgz".to_string(),
                    ),
                    integrity: Some("sha512-xyz".to_string()),
                    resolution_type: None,
                    path: None,
                }),
                dependencies: BTreeMap::new(),
                optional_dependencies: BTreeMap::new(),
                peer_dependencies: BTreeMap::new(),
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
        assert!(graph.packages().any(|pkg| {
            pkg.id.name.as_str() == "react"
                && pkg
                    .peer_dependencies
                    .iter()
                    .any(|(name, range)| name.as_str() == "esbuild" && range == ">=0.18.0")
        }));
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
                peer_dependencies: BTreeMap::new(),
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
                peer_dependencies: BTreeMap::new(),
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
