#[allow(clippy::unwrap_used)]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;

    use crate::{
        detect_workspace_cycles, filter_workspace_packages, Catalog, Workspace, WorkspacePackage,
        WorkspaceSelector,
    };

    fn ws_with_pkgs(pkg_specs: Vec<(&str, Vec<&str>)>) -> Workspace {
        ws_with_pkg_deps(
            pkg_specs
                .into_iter()
                .map(|(name, deps)| {
                    (
                        name,
                        deps.into_iter().map(|dep| (dep, "workspace:*")).collect(),
                    )
                })
                .collect(),
        )
    }

    fn ws_with_pkg_deps(pkg_specs: Vec<(&str, Vec<(&str, &str)>)>) -> Workspace {
        let packages: Vec<WorkspacePackage> = pkg_specs
            .into_iter()
            .map(|(name, deps)| {
                let manifest = orix_manifest::Manifest {
                    name: Some(name.to_string()),
                    version: Some("1.0.0".to_string()),
                    dependencies: deps
                        .into_iter()
                        .map(|(dep, raw)| (dep.to_string(), raw.to_string()))
                        .collect(),
                    ..Default::default()
                };
                WorkspacePackage {
                    relative_path: PathBuf::from(name),
                    abs_path: PathBuf::from(name),
                    manifest,
                }
            })
            .collect();
        Workspace {
            root: PathBuf::from("."),
            packages,
            lockfile_path: PathBuf::from("orix-lock.yaml"),
            catalog: Catalog::new(),
            catalogs: HashMap::new(),
        }
    }

    #[test]
    fn detect_no_cycle_in_linear_deps() {
        let result = detect_workspace_cycles(&ws_with_pkgs(vec![
            ("pkg-a", vec!["pkg-b"]),
            ("pkg-b", vec!["pkg-c"]),
            ("pkg-c", vec![]),
        ]));
        assert!(result.is_empty(), "no cycle expected, got {:?}", result);
    }

    #[test]
    fn detect_self_cycle() {
        let result = detect_workspace_cycles(&ws_with_pkgs(vec![("pkg-a", vec!["pkg-a"])]));
        assert!(!result.is_empty(), "self-cycle should be detected");
        assert!(result.contains(&"pkg-a".to_string()));
    }

    #[test]
    fn detect_two_node_cycle() {
        let result = detect_workspace_cycles(&ws_with_pkgs(vec![
            ("pkg-a", vec!["pkg-b"]),
            ("pkg-b", vec!["pkg-a"]),
        ]));
        assert!(!result.is_empty(), "cycle should be detected");
    }

    #[test]
    fn no_false_positive_on_external_deps() {
        let result =
            detect_workspace_cycles(&ws_with_pkg_deps(vec![("pkg-a", vec![("lodash", "*")])]));
        assert!(
            result.is_empty(),
            "external deps should not cause cycle: {:?}",
            result
        );
    }

    #[test]
    fn package_name_collision_with_external_dep_is_not_cycle() {
        let result = detect_workspace_cycles(&ws_with_pkg_deps(vec![
            ("appName", vec![("react", "16.14.0")]),
            ("react", vec![("react", "catalog:react")]),
        ]));
        assert!(
            result.is_empty(),
            "registry deps should not become workspace edges: {:?}",
            result
        );
    }

    #[test]
    fn discover_skips_missing_workspace_files() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        std::fs::write(root.join("package.json"), "{}").unwrap();

        let ws = Workspace::discover(root).unwrap();
        assert!(ws.packages.is_empty());
    }

    #[test]
    fn discover_prefers_pnpm_yaml_over_orix_yaml() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();

        std::fs::create_dir_all(root.join("packages/pkg1")).unwrap();
        std::fs::write(
            root.join("packages/pkg1/package.json"),
            r#"{"name":"pkg1"}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("pnpm-workspace.yaml"),
            "packages:\n  - 'packages/pkg1'",
        )
        .unwrap();
        std::fs::write(
            root.join("orix-workspace.yaml"),
            "packages:\n  - 'packages/other'",
        )
        .unwrap();

        let ws = Workspace::discover(root).unwrap();
        assert_eq!(ws.packages.len(), 1);
        assert_eq!(ws.packages[0].manifest.name.as_deref(), Some("pkg1"));
    }

    #[test]
    fn discover_prefers_orix_yaml_over_root_package_json() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();

        std::fs::create_dir_all(root.join("packages/pkg1")).unwrap();
        std::fs::write(
            root.join("packages/pkg1/package.json"),
            r#"{"name":"pkg1"}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("package.json"),
            r#"{"orix":{"packages":["packages/other"]}}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("orix-workspace.yaml"),
            "packages:\n  - 'packages/pkg1'",
        )
        .unwrap();

        let ws = Workspace::discover(root).unwrap();
        assert_eq!(ws.packages.len(), 1);
        assert_eq!(ws.packages[0].manifest.name.as_deref(), Some("pkg1"));
    }

    #[test]
    fn discover_from_orix_yaml() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();

        std::fs::create_dir_all(root.join("apps/web")).unwrap();
        std::fs::create_dir_all(root.join("libs/shared")).unwrap();
        std::fs::write(root.join("apps/web/package.json"), r#"{"name":"@org/web"}"#).unwrap();
        std::fs::write(
            root.join("libs/shared/package.json"),
            r#"{"name":"@org/shared"}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("orix-workspace.yaml"),
            "packages:\n  - 'apps/*'\n  - 'libs/*'",
        )
        .unwrap();

        let ws = Workspace::discover(root).unwrap();
        assert_eq!(ws.packages.len(), 2);
        let names: Vec<_> = ws
            .packages
            .iter()
            .filter_map(|p| p.manifest.name.clone())
            .collect();
        assert!(names.contains(&"@org/web".to_string()));
        assert!(names.contains(&"@org/shared".to_string()));
    }

    #[test]
    fn discover_respects_negative_patterns() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();

        std::fs::create_dir_all(root.join("apps/web")).unwrap();
        std::fs::create_dir_all(root.join("apps/ignored")).unwrap();
        std::fs::write(root.join("apps/web/package.json"), r#"{"name":"web"}"#).unwrap();
        std::fs::write(
            root.join("apps/ignored/package.json"),
            r#"{"name":"ignored"}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("pnpm-workspace.yaml"),
            "packages:\n  - 'apps/*'\n  - '!apps/ignored'",
        )
        .unwrap();

        let ws = Workspace::discover(root).unwrap();
        let names: Vec<_> = ws
            .packages
            .iter()
            .filter_map(|p| p.manifest.name.as_deref())
            .collect();
        assert_eq!(names, vec!["web"]);
    }

    #[test]
    fn discover_ignores_node_modules_matches() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();

        std::fs::create_dir_all(root.join("packages/app")).unwrap();
        std::fs::create_dir_all(root.join("node_modules/react")).unwrap();
        std::fs::write(
            root.join("packages/app/package.json"),
            r#"{"name":"appName","dependencies":{"react":"^18.2.0"}}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("node_modules/react/package.json"),
            r#"{"name":"react","version":"18.2.0","dependencies":{"react":"^18.2.0"}}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("pnpm-workspace.yaml"),
            "packages:\n  - '**'\n  - '!**/node_modules/**'",
        )
        .unwrap();

        let ws = Workspace::discover(root).unwrap();
        assert_eq!(ws.packages.len(), 1);
        assert_eq!(ws.packages[0].manifest.name.as_deref(), Some("appName"));
        assert!(detect_workspace_cycles(&ws).is_empty());
    }

    #[test]
    fn discover_from_root_package_json() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();

        std::fs::create_dir_all(root.join("packages/pkg-a")).unwrap();
        std::fs::write(
            root.join("packages/pkg-a/package.json"),
            r#"{"name":"pkg-a"}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("package.json"),
            r#"{"name":"root","orix":{"packages":["packages/*"]}}"#,
        )
        .unwrap();

        let ws = Workspace::discover(root).unwrap();
        assert_eq!(ws.packages.len(), 1);
        assert_eq!(ws.packages[0].manifest.name.as_deref(), Some("pkg-a"));
    }

    #[test]
    fn discover_ignores_non_array_orix_packages() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        std::fs::write(
            root.join("package.json"),
            r#"{"name":"root","orix":{"packages":"packages/*"}}"#,
        )
        .unwrap();

        let ws = Workspace::discover(root).unwrap();
        assert!(ws.packages.is_empty());
    }

    #[test]
    fn workspace_selector_parse_package_name() {
        let sel = WorkspaceSelector::parse("@scope/pkg");
        assert!(matches!(sel, WorkspaceSelector::PackageName(n) if n == "@scope/pkg"));

        let sel = WorkspaceSelector::parse("lodash");
        assert!(matches!(sel, WorkspaceSelector::PackageName(n) if n == "lodash"));
    }

    #[test]
    fn workspace_selector_parse_relative_path() {
        let sel = WorkspaceSelector::parse("./example");
        #[allow(clippy::unwrap_used)]
        if let WorkspaceSelector::RelativePath(p) = &sel {
            assert_eq!(p, &PathBuf::from("./example"));
        } else {
            unreachable!("expected RelativePath, got {:?}", sel);
        }

        let sel = WorkspaceSelector::parse("../utils");
        #[allow(clippy::unwrap_used)]
        if let WorkspaceSelector::RelativePath(p) = &sel {
            assert_eq!(p, &PathBuf::from("../utils"));
        } else {
            unreachable!("expected RelativePath, got {:?}", sel);
        }
    }

    #[test]
    fn workspace_selector_parse_glob() {
        let sel = WorkspaceSelector::parse("./qiankun/*");
        assert!(matches!(sel, WorkspaceSelector::Glob(p) if p == "./qiankun/*"));

        let sel = WorkspaceSelector::parse("./packages/*/src");
        assert!(matches!(sel, WorkspaceSelector::Glob(p) if p == "./packages/*/src"));
    }

    #[test]
    fn workspace_selector_matches_package_name() {
        let pkg = WorkspacePackage {
            relative_path: PathBuf::from("packages/app"),
            abs_path: PathBuf::from("/root/packages/app"),
            manifest: orix_manifest::Manifest {
                name: Some("@org/app".to_string()),
                ..Default::default()
            },
        };

        let sel = WorkspaceSelector::parse("@org/app");
        assert!(sel.matches(&pkg, &PathBuf::from("/root")));

        let sel = WorkspaceSelector::parse("@org/other");
        assert!(!sel.matches(&pkg, &PathBuf::from("/root")));
    }

    #[test]
    fn workspace_selector_matches_relative_path() {
        let pkg = WorkspacePackage {
            relative_path: PathBuf::from("packages/app"),
            abs_path: PathBuf::from("/root/packages/app"),
            manifest: orix_manifest::Manifest {
                name: Some("app".to_string()),
                ..Default::default()
            },
        };

        let sel = WorkspaceSelector::parse("./packages/app");
        assert!(sel.matches(&pkg, &PathBuf::from("/root")));

        let sel = WorkspaceSelector::parse("./packages/other");
        assert!(!sel.matches(&pkg, &PathBuf::from("/root")));
    }

    #[test]
    fn workspace_selector_matches_glob() {
        let pkgs = vec![
            WorkspacePackage {
                relative_path: PathBuf::from("qiankun/app-a"),
                abs_path: PathBuf::from("/root/qiankun/app-a"),
                manifest: orix_manifest::Manifest {
                    name: Some("app-a".to_string()),
                    ..Default::default()
                },
            },
            WorkspacePackage {
                relative_path: PathBuf::from("qiankun/app-b"),
                abs_path: PathBuf::from("/root/qiankun/app-b"),
                manifest: orix_manifest::Manifest {
                    name: Some("app-b".to_string()),
                    ..Default::default()
                },
            },
            WorkspacePackage {
                relative_path: PathBuf::from("packages/web"),
                abs_path: PathBuf::from("/root/packages/web"),
                manifest: orix_manifest::Manifest {
                    name: Some("web".to_string()),
                    ..Default::default()
                },
            },
        ];

        let ws = Workspace {
            root: PathBuf::from("/root"),
            packages: pkgs,
            lockfile_path: PathBuf::from("orix-lock.yaml"),
            catalog: Catalog::new(),
            catalogs: HashMap::new(),
        };

        let sel = WorkspaceSelector::parse("./qiankun/*");
        let filtered = filter_workspace_packages(&ws, &[sel]);
        assert_eq!(filtered.len(), 2);

        let names: Vec<_> = filtered
            .iter()
            .filter_map(|p| p.manifest.name.clone())
            .collect();
        assert!(names.contains(&"app-a".to_string()));
        assert!(names.contains(&"app-b".to_string()));
        assert!(!names.contains(&"web".to_string()));
    }

    #[test]
    fn filter_workspace_packages_empty_selector_returns_all() {
        let pkgs = vec![
            WorkspacePackage {
                relative_path: PathBuf::from("pkg-a"),
                abs_path: PathBuf::from("/root/pkg-a"),
                manifest: orix_manifest::Manifest {
                    name: Some("pkg-a".to_string()),
                    ..Default::default()
                },
            },
            WorkspacePackage {
                relative_path: PathBuf::from("pkg-b"),
                abs_path: PathBuf::from("/root/pkg-b"),
                manifest: orix_manifest::Manifest {
                    name: Some("pkg-b".to_string()),
                    ..Default::default()
                },
            },
        ];

        let ws = Workspace {
            root: PathBuf::from("/root"),
            packages: pkgs,
            lockfile_path: PathBuf::from("orix-lock.yaml"),
            catalog: Catalog::new(),
            catalogs: HashMap::new(),
        };

        let filtered = filter_workspace_packages(&ws, &[]);
        assert_eq!(filtered.len(), 2);
    }

    #[test]
    fn filter_workspace_packages_multiple_selectors() {
        let pkgs = vec![
            WorkspacePackage {
                relative_path: PathBuf::from("packages/app"),
                abs_path: PathBuf::from("/root/packages/app"),
                manifest: orix_manifest::Manifest {
                    name: Some("@org/app".to_string()),
                    ..Default::default()
                },
            },
            WorkspacePackage {
                relative_path: PathBuf::from("packages/lib"),
                abs_path: PathBuf::from("/root/packages/lib"),
                manifest: orix_manifest::Manifest {
                    name: Some("@org/lib".to_string()),
                    ..Default::default()
                },
            },
            WorkspacePackage {
                relative_path: PathBuf::from("playground/demo"),
                abs_path: PathBuf::from("/root/playground/demo"),
                manifest: orix_manifest::Manifest {
                    name: Some("demo".to_string()),
                    ..Default::default()
                },
            },
        ];

        let ws = Workspace {
            root: PathBuf::from("/root"),
            packages: pkgs,
            lockfile_path: PathBuf::from("orix-lock.yaml"),
            catalog: Catalog::new(),
            catalogs: HashMap::new(),
        };

        let selectors = vec![
            WorkspaceSelector::parse("./packages/*"),
            WorkspaceSelector::parse("./playground/demo"),
        ];
        let filtered = filter_workspace_packages(&ws, &selectors);
        assert_eq!(filtered.len(), 3);
    }
}
