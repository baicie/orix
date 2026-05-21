#[allow(clippy::unwrap_used)]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;

    use crate::{detect_workspace_cycles, Catalog, Workspace, WorkspacePackage};

    fn ws_with_pkgs(pkg_specs: Vec<(&str, Vec<&str>)>) -> Workspace {
        let packages: Vec<WorkspacePackage> = pkg_specs
            .into_iter()
            .map(|(name, deps)| {
                let manifest = orix_manifest::Manifest {
                    name: Some(name.to_string()),
                    version: Some("1.0.0".to_string()),
                    dependencies: deps
                        .into_iter()
                        .map(|d| (d.to_string(), "*".to_string()))
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
        let result = detect_workspace_cycles(&ws_with_pkgs(vec![("pkg-a", vec!["lodash"])]));
        assert!(
            result.is_empty(),
            "external deps should not cause cycle: {:?}",
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
}
