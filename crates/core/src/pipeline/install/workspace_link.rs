//! Workspace package linking during install.

use std::collections::{hash_map::DefaultHasher, HashMap, HashSet};
use std::hash::{Hash, Hasher};

use orix_domain::{
    ConstraintKind, DependencyGraph, PackageId, PackageName, ResolvedPackage, VersionConstraint,
};

use crate::pipeline::prelude::*;
use crate::pipeline::types::link_error;

/// Link `node_modules` for each workspace member package.
pub(crate) fn link_workspace_packages(
    store: &Store,
    graph: &orix_domain::DependencyGraph,
    workspace: &Workspace,
    progress_tx: &Option<mpsc::Sender<InstallEvent>>,
) -> Result<()> {
    let started = Instant::now();
    let graph_index = GraphIndex::new(graph);
    let graph_hash = graph.graph_hash();
    let mut linked_members = 0_u32;
    let mut skipped_members = 0_u32;

    for ws_pkg in &workspace.packages {
        let nm_dir = ws_pkg.abs_path.join("node_modules");
        let pkg_linker = Linker::new(store.clone(), nm_dir.clone());

        let pkg_specs: Vec<(String, String)> = ws_pkg
            .manifest
            .dependencies
            .iter()
            .chain(ws_pkg.manifest.dev_dependencies.iter())
            .chain(ws_pkg.manifest.optional_dependencies.iter())
            .map(|(name, raw)| (name.clone(), raw.clone()))
            .collect();
        let pkg_deps: HashSet<String> = pkg_specs.iter().map(|(name, _)| name.clone()).collect();
        let importer_hash = workspace_importer_hash(&pkg_specs, &graph_hash);
        if workspace_marker_matches(&nm_dir, &importer_hash) {
            skipped_members += 1;
            continue;
        }

        if let Err(e) = pkg_linker.prune_stale_direct_links(&pkg_deps) {
            return Err(link_error(
                progress_tx,
                format!(
                    "failed to prune stale node_modules for {}: {}",
                    ws_pkg.manifest.name.as_deref().unwrap_or("?"),
                    e
                ),
            ));
        }

        if let Err(e) = link_workspace_direct_deps(&pkg_linker, &graph_index, workspace, &pkg_specs)
        {
            return Err(link_error(
                progress_tx,
                format!(
                    "failed to link packages for {}: {}",
                    ws_pkg.manifest.name.as_deref().unwrap_or("?"),
                    e
                ),
            ));
        }
        write_workspace_marker(&nm_dir, &importer_hash)?;
        linked_members += 1;
    }

    debug!(
        target: crate::pipeline::perf::PERF_TARGET,
        phase = "workspace_link",
        duration_ms = started.elapsed().as_millis() as u64,
        members = workspace.packages.len(),
        linked_members,
        skipped_members,
        "workspace member link complete"
    );

    Ok(())
}

#[derive(serde::Serialize, serde::Deserialize)]
struct WorkspaceLinkMarker {
    importer_hash: String,
    marker_version: u32,
}

const WORKSPACE_LINK_MARKER_VERSION: u32 = 1;
const WORKSPACE_LINK_MARKER_FILE: &str = ".orix-workspace-link.json";

fn workspace_marker_path(node_modules: &Path) -> PathBuf {
    node_modules.join(".orix").join(WORKSPACE_LINK_MARKER_FILE)
}

fn workspace_marker_matches(node_modules: &Path, importer_hash: &str) -> bool {
    let path = workspace_marker_path(node_modules);
    let Ok(content) = fs::read_to_string(path) else {
        return false;
    };
    let Ok(marker) = serde_json::from_str::<WorkspaceLinkMarker>(&content) else {
        return false;
    };
    marker.marker_version == WORKSPACE_LINK_MARKER_VERSION && marker.importer_hash == importer_hash
}

fn write_workspace_marker(node_modules: &Path, importer_hash: &str) -> Result<()> {
    let marker = WorkspaceLinkMarker {
        importer_hash: importer_hash.to_string(),
        marker_version: WORKSPACE_LINK_MARKER_VERSION,
    };
    let path = workspace_marker_path(node_modules);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, serde_json::to_string(&marker)?)?;
    Ok(())
}

fn workspace_importer_hash(specs: &[(String, String)], graph_hash: &str) -> String {
    let mut specs = specs.to_vec();
    specs.sort();
    let mut hasher = DefaultHasher::new();
    graph_hash.hash(&mut hasher);
    specs.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn link_workspace_direct_deps(
    linker: &Linker,
    graph_index: &GraphIndex,
    workspace: &Workspace,
    specs: &[(String, String)],
) -> Result<()> {
    let root_virtual_store = workspace.root.join("node_modules").join(".orix");
    let mut report = LinkReport {
        hardlinked_files: 0,
        copied_files: 0,
        symlinks_created: 0,
        bytes_saved: 0,
        skipped: None,
    };

    for (name, raw) in specs {
        let Some(dep_key) =
            graph_index.select_dependency_key(&PackageName::from(name.as_str()), raw)
        else {
            continue;
        };
        let target = Linker::package_path_in_node_modules(
            &root_virtual_store.join(dep_key).join("node_modules"),
            name,
        );
        if !target.exists() {
            continue;
        }

        linker.link_direct_package_from(name, &target, &mut report)?;
    }

    Ok(())
}

struct GraphIndex {
    packages_by_key: HashMap<String, ResolvedPackage>,
    keys_by_name: HashMap<String, Vec<String>>,
}

impl GraphIndex {
    fn new(graph: &DependencyGraph) -> Self {
        let mut packages_by_key = HashMap::new();
        let mut keys_by_name: HashMap<String, Vec<String>> = HashMap::new();

        for pkg in graph.packages() {
            let key = pkg.id.key();
            keys_by_name
                .entry(pkg.id.name.to_string())
                .or_default()
                .push(key.clone());
            packages_by_key.insert(key, pkg.clone());
        }

        Self {
            packages_by_key,
            keys_by_name,
        }
    }

    #[cfg(test)]
    fn subgraph_for_direct_specs(&self, specs: &[(String, String)]) -> DependencyGraph {
        let mut subgraph = DependencyGraph::new();
        let mut visited = HashSet::new();
        let mut queue = std::collections::VecDeque::new();

        for (name, raw) in specs {
            if let Some(key) = self.select_dependency_key(&PackageName::from(name.as_str()), raw) {
                queue.push_back(key);
            }
        }

        while let Some(key) = queue.pop_front() {
            if !visited.insert(key.clone()) {
                continue;
            }

            let Some(pkg) = self.packages_by_key.get(&key) else {
                continue;
            };
            subgraph.insert(pkg.clone());

            for (dep_name, raw) in pkg
                .dependencies
                .iter()
                .chain(pkg.optional_dependencies.iter())
                .chain(pkg.peer_dependencies.iter())
            {
                if let Some(dep_key) = self.select_dependency_key(dep_name, raw) {
                    queue.push_back(dep_key);
                }
            }
        }

        subgraph
    }

    fn select_dependency_key(&self, dep_name: &PackageName, raw: &str) -> Option<String> {
        let keys = self.keys_by_name.get(dep_name.as_str())?;
        let constraint = VersionConstraint::parse(raw).ok();

        constraint
            .as_ref()
            .and_then(|constraint| {
                keys.iter()
                    .rev()
                    .find(|key| {
                        self.packages_by_key
                            .get(*key)
                            .is_some_and(|pkg| package_matches_constraint(&pkg.id, constraint))
                    })
                    .cloned()
            })
            .or_else(|| keys.last().cloned())
    }
}

fn package_matches_constraint(pkg_id: &PackageId, constraint: &VersionConstraint) -> bool {
    match &constraint.kind {
        ConstraintKind::Exact(version) => pkg_id.version == *version,
        ConstraintKind::Range(req) => req.matches(&pkg_id.version),
        ConstraintKind::AnyRange(ranges) => ranges.iter().any(|req| req.matches(&pkg_id.version)),
        ConstraintKind::Alias { constraint, .. } => package_matches_constraint(pkg_id, constraint),
        ConstraintKind::Patch(spec) => pkg_id.version == spec.package_version,
        ConstraintKind::Latest | ConstraintKind::Tag(_) | ConstraintKind::Catalog(_) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orix_domain::Version;

    fn package(name: &str, version: &str, deps: &[(&str, &str)]) -> Result<ResolvedPackage> {
        Ok(ResolvedPackage {
            id: PackageId::new(PackageName::from(name), Version::parse(version)?),
            integrity: format!("sha512-{name}-{version}"),
            tarball: format!("https://registry.example/{name}-{version}.tgz"),
            dependencies: deps
                .iter()
                .map(|(dep, raw)| (PackageName::from(*dep), (*raw).to_string()))
                .collect(),
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
    fn workspace_subgraph_contains_only_dependency_closure() -> Result<()> {
        let mut graph = DependencyGraph::new();
        graph.insert(package("a", "1.0.0", &[("b", "^2.0.0")])?);
        graph.insert(package("b", "1.0.0", &[])?);
        graph.insert(package("b", "2.0.0", &[("c", "1.0.0")])?);
        graph.insert(package("c", "1.0.0", &[])?);
        graph.insert(package("unrelated", "1.0.0", &[])?);

        let subgraph = GraphIndex::new(&graph)
            .subgraph_for_direct_specs(&[("a".to_string(), "^1.0.0".to_string())]);

        assert_eq!(subgraph.len(), 3);
        assert!(subgraph.contains(&PackageId::new(
            PackageName::from("a"),
            Version::parse("1.0.0")?
        )));
        assert!(subgraph.contains(&PackageId::new(
            PackageName::from("b"),
            Version::parse("2.0.0")?
        )));
        assert!(subgraph.contains(&PackageId::new(
            PackageName::from("c"),
            Version::parse("1.0.0")?
        )));
        assert!(!subgraph.contains(&PackageId::new(
            PackageName::from("unrelated"),
            Version::parse("1.0.0")?
        )));
        Ok(())
    }

    #[test]
    fn workspace_subgraph_falls_back_for_workspace_protocol_specs() -> Result<()> {
        let mut graph = DependencyGraph::new();
        graph.insert(package("local-pkg", "0.0.0", &[("dep", "1.0.0")])?);
        graph.insert(package("dep", "1.0.0", &[])?);
        graph.insert(package("unrelated", "1.0.0", &[])?);

        let subgraph = GraphIndex::new(&graph)
            .subgraph_for_direct_specs(&[("local-pkg".to_string(), "workspace:*".to_string())]);

        assert_eq!(subgraph.len(), 2);
        assert!(subgraph.contains(&PackageId::new(
            PackageName::from("local-pkg"),
            Version::parse("0.0.0")?
        )));
        assert!(subgraph.contains(&PackageId::new(
            PackageName::from("dep"),
            Version::parse("1.0.0")?
        )));
        Ok(())
    }
}
