//! Build dependency graphs from lockfile snapshots.

use orix_domain::{DependencyGraph, PackageId, PackageName, ResolvedPackage, Version};

use crate::types::Lockfile;

/// Resolve dependencies from a lockfile (frozen/install-from-lock workflow).
pub fn resolve_from_lockfile(lockfile: &Lockfile) -> DependencyGraph {
    let mut graph = DependencyGraph::new();

    for (key, pkg) in &lockfile.packages {
        let tarball = match pkg.resolution.as_ref().and_then(|r| r.tarball.clone()) {
            Some(t) => t,
            None => continue,
        };

        let Some(snapshot) = lockfile.snapshots.get(key) else {
            continue;
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

        let deps: Vec<(PackageName, String)> = snapshot
            .dependencies
            .iter()
            .map(|(k, v)| (PackageName::from(k.as_str()), v.clone()))
            .collect();
        let opt_deps: Vec<(PackageName, String)> = snapshot
            .optional_dependencies
            .iter()
            .map(|(k, v)| (PackageName::from(k.as_str()), v.clone()))
            .collect();
        let peer_deps: Vec<(PackageName, String)> = snapshot
            .peer_dependencies
            .iter()
            .map(|(k, v)| (PackageName::from(k.as_str()), v.clone()))
            .collect();

        let depnodes: Vec<String> = deps
            .iter()
            .chain(opt_deps.iter())
            .chain(peer_deps.iter())
            .map(|(n, _)| n.to_string())
            .collect();

        let resolved = ResolvedPackage {
            id: pkg_id.clone(),
            integrity,
            tarball,
            dependencies: deps,
            dev_dependencies: Vec::new(),
            optional_dependencies: opt_deps,
            peer_dependencies: peer_deps,
            engines: pkg.engines.clone(),
            os: pkg.os.clone().unwrap_or_default(),
            cpu: pkg.cpu.clone().unwrap_or_default(),
            depnodes,
            patch: None,
        };
        graph.insert(resolved);
    }

    graph
}
