use super::types::{PATH_SEP, VIRTUAL_STORE_DIR};

use std::path::{Path, PathBuf};

use orix_config::Config;
use orix_domain::PackageId;
/// Returns whether lifecycle scripts for dependency `pkg_name` are allowed by config.
pub fn dependency_scripts_allowed(config: &Config, pkg_name: &str) -> bool {
    if config.ignore_scripts {
        return false;
    }
    config
        .allow_scripts
        .iter()
        .any(|p| pkg_name == p || (p.ends_with("/*") && pkg_name.starts_with(&p[..p.len() - 1])))
}

/// Strip a leading `--` from script arguments (npm compatibility).
pub fn normalize_script_args(args: Vec<String>) -> Vec<String> {
    if args.first().is_some_and(|s| s == "--") {
        args[1..].to_vec()
    } else {
        args
    }
}

/// Installed package root under `node_modules/.orix/<key>/node_modules/<name>`.
pub fn installed_package_dir(project_root: &Path, pkg_id: &PackageId) -> PathBuf {
    let pkg_key = pkg_id.key();
    let base = project_root
        .join("node_modules")
        .join(VIRTUAL_STORE_DIR)
        .join(&pkg_key)
        .join("node_modules");
    package_path_in_node_modules(&base, pkg_id.name.as_str())
}

pub(crate) fn package_path_in_node_modules(root: &Path, package_name: &str) -> PathBuf {
    package_name
        .split('/')
        .fold(root.to_path_buf(), |path, part| path.join(part))
}

/// Topological install order: dependencies before dependents.
pub fn graph_install_order(graph: &orix_domain::DependencyGraph) -> Vec<PackageId> {
    use std::collections::{HashMap, HashSet, VecDeque};

    let ids: HashSet<PackageId> = graph.package_ids().cloned().collect();
    let key_to_id: HashMap<String, PackageId> =
        ids.iter().map(|id| (id.key(), id.clone())).collect();
    let mut in_degree: HashMap<PackageId, usize> = ids.iter().map(|id| (id.clone(), 0)).collect();
    let mut dependents: HashMap<PackageId, Vec<PackageId>> =
        ids.iter().map(|id| (id.clone(), Vec::new())).collect();

    for pkg in graph.packages() {
        for dep_key in &pkg.depnodes {
            let Some(dep_id) = key_to_id.get(dep_key) else {
                continue;
            };
            if dep_id == &pkg.id {
                continue;
            }
            if let Some(deg) = in_degree.get_mut(&pkg.id) {
                *deg += 1;
            }
            dependents
                .entry(dep_id.clone())
                .or_default()
                .push(pkg.id.clone());
        }
    }

    let mut queue: VecDeque<PackageId> = in_degree
        .iter()
        .filter(|(_, &d)| d == 0)
        .map(|(id, _)| id.clone())
        .collect();
    let mut order = Vec::with_capacity(ids.len());

    while let Some(id) = queue.pop_front() {
        order.push(id.clone());
        if let Some(deps) = dependents.get(&id) {
            for dependent in deps {
                if let Some(deg) = in_degree.get_mut(dependent) {
                    *deg = deg.saturating_sub(1);
                    if *deg == 0 {
                        queue.push_back(dependent.clone());
                    }
                }
            }
        }
    }

    for id in ids {
        if !order.contains(&id) {
            order.push(id);
        }
    }

    order
}

/// Remove invalid PATH segments (e.g. bare `D:` on Windows) that break Node resolution.
pub(crate) fn sanitize_path_env(path: &str) -> String {
    path.split(PATH_SEP)
        .filter(|part| !is_invalid_path_segment(part))
        .collect::<Vec<_>>()
        .join(PATH_SEP)
}

fn is_invalid_path_segment(segment: &str) -> bool {
    let t = segment.trim();
    if t.is_empty() {
        return true;
    }
    let bytes = t.as_bytes();
    // Bare drive letter `D:` or root-only `D:\` / `D:/` breaks Node's realpathSync.
    if bytes.len() >= 2 && bytes[1] == b':' {
        let rest = &t[2..];
        return rest.is_empty() || rest == "\\" || rest == "/";
    }
    false
}

/// Join args into a shell-safe string (for appending to a command).
pub(crate) fn shell_args_join(args: &[String]) -> String {
    args.iter()
        .map(|a| {
            if a.contains(' ') || a.contains('"') || a.contains('\'') || a.contains('$') {
                format!("\"{}\"", a.replace('"', "\\\""))
            } else {
                a.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use orix_manifest::Manifest;
    use std::collections::BTreeMap;

    use crate::{LifecycleEvent, ScriptRunner};

    fn test_manifest() -> Manifest {
        let mut scripts = BTreeMap::new();
        scripts.insert("prebuild".to_string(), "echo pre".to_string());
        scripts.insert("build".to_string(), "tsc".to_string());
        scripts.insert("postbuild".to_string(), "echo post".to_string());
        Manifest {
            name: Some("test-pkg".to_string()),
            version: Some("1.0.0".to_string()),
            scripts,
            ..Default::default()
        }
    }

    #[allow(clippy::unwrap_used)]
    fn test_config() -> Config {
        let tmp = tempfile::tempdir().unwrap();
        Config::load(tmp.path()).unwrap()
    }

    #[test]
    fn lifecycle_event_script_name() {
        assert_eq!(LifecycleEvent::Preinstall.script_name(), "preinstall");
        assert_eq!(LifecycleEvent::Install.script_name(), "install");
        assert_eq!(LifecycleEvent::Postinstall.script_name(), "postinstall");
        assert_eq!(LifecycleEvent::Prepare.script_name(), "prepare");
    }

    #[test]
    fn scripts_enabled_when_ignore_scripts_false() {
        let config = test_config();
        let runner = ScriptRunner::new(config, test_manifest(), PathBuf::from("."), None);
        assert!(runner.scripts_enabled());
    }

    #[test]
    fn scripts_disabled_when_ignore_scripts_true() -> anyhow::Result<()> {
        let tmp = tempfile::tempdir()?;
        let config = Config::load_with_overrides(
            tmp.path(),
            &orix_config::ConfigOverrides {
                ignore_scripts: Some(true),
                ..Default::default()
            },
        )?;
        let runner = ScriptRunner::new(config, test_manifest(), PathBuf::from("."), None);
        assert!(!runner.scripts_enabled());
        Ok(())
    }

    #[test]
    fn dependency_scripts_disabled_by_default() {
        let config = test_config();
        let runner = ScriptRunner::new(config, test_manifest(), PathBuf::from("."), None);
        assert!(!runner.dependency_scripts_allowed("esbuild"));
    }

    #[test]
    fn dependency_scripts_allowed_when_in_allow_list() -> anyhow::Result<()> {
        let tmp = tempfile::tempdir()?;
        let config = Config::load_with_overrides(
            tmp.path(),
            &orix_config::ConfigOverrides {
                allow_scripts: Some(vec!["esbuild".to_string()]),
                ..Default::default()
            },
        )?;
        let runner = ScriptRunner::new(config, test_manifest(), PathBuf::from("."), None);
        assert!(runner.dependency_scripts_allowed("esbuild"));
        assert!(!runner.dependency_scripts_allowed("typescript"));
        Ok(())
    }

    #[test]
    fn sanitize_path_env_drops_bare_drive_letter() {
        let path = format!("D:{PATH_SEP}D:\\workspace\\proj\\node_modules\\.bin{PATH_SEP}node.exe");
        let sanitized = sanitize_path_env(&path);
        assert!(!sanitized.contains("D:;"));
        assert!(sanitized.contains("node_modules"));
    }

    #[test]
    fn normalize_script_args_strips_leading_double_dash() {
        assert_eq!(
            normalize_script_args(vec!["--".to_string(), "-a".to_string(), "2".to_string()]),
            vec!["-a".to_string(), "2".to_string()]
        );
        assert_eq!(
            normalize_script_args(vec!["-w".to_string()]),
            vec!["-w".to_string()]
        );
    }

    #[test]
    fn shell_args_join_simple() {
        assert_eq!(
            shell_args_join(&["build".to_string(), "--flag".to_string()]),
            "build --flag"
        );
    }

    #[test]
    fn shell_args_join_with_spaces() {
        assert_eq!(
            shell_args_join(&[
                "build".to_string(),
                "--config".to_string(),
                "a b".to_string()
            ]),
            r#"build --config "a b""#
        );
    }

    #[test]
    fn shell_args_join_with_quotes() {
        let result = shell_args_join(&[
            "build".to_string(),
            "--flag".to_string(),
            r#"a"b"c"#.to_string(),
        ]);
        assert!(result.contains(r#"\""#));
    }
}
