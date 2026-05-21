//! Workspace dependency cycle detection.

use super::types::Workspace;

#[allow(dead_code)]
/// Cycle detection result: a list of packages involved in a dependency cycle.
pub type CycleReport = Vec<String>;

/// Detects circular workspace dependencies using DFS with three-color marking.
///
/// Returns an empty `Vec` if no cycles exist, or the packages involved in the
/// first cycle found.
///
/// Color state: 0 = unvisited (white), 1 = in-progress (gray), 2 = done (black).
pub fn detect_workspace_cycles(workspace: &Workspace) -> Vec<String> {
    use std::collections::HashMap;

    let mut adj: HashMap<String, Vec<String>> = HashMap::new();
    for pkg in &workspace.packages {
        if let Some(ref name) = pkg.manifest.name {
            let deps: Vec<String> = pkg
                .manifest
                .dependencies
                .keys()
                .chain(pkg.manifest.dev_dependencies.keys())
                .chain(pkg.manifest.optional_dependencies.keys())
                .filter(|k| {
                    workspace
                        .packages
                        .iter()
                        .any(|p| p.manifest.name.as_ref() == Some(k))
                })
                .cloned()
                .collect();
            adj.insert(name.clone(), deps);
        }
    }

    let mut color: HashMap<String, u8> = HashMap::new();
    let mut parent: HashMap<String, String> = HashMap::new();
    let mut cycle: Vec<String> = Vec::new();

    fn dfs(
        name: &str,
        adj: &HashMap<String, Vec<String>>,
        color: &mut HashMap<String, u8>,
        parent: &mut HashMap<String, String>,
        cycle: &mut Vec<String>,
    ) -> bool {
        color.insert(name.to_string(), 1);
        if let Some(neighbors) = adj.get(name) {
            for neighbor in neighbors {
                let n_color = *color.get(neighbor).unwrap_or(&0);
                if n_color == 1 {
                    let mut cur = name.to_string();
                    cycle.clear();
                    cycle.push(cur.clone());
                    while let Some(p) = parent.get(&cur) {
                        cycle.push(p.clone());
                        cur = p.clone();
                        if p == neighbor {
                            break;
                        }
                    }
                    cycle.reverse();
                    return true;
                }
                if n_color == 0 {
                    parent.insert(neighbor.clone(), name.to_string());
                    if dfs(neighbor, adj, color, parent, cycle) {
                        return true;
                    }
                }
            }
        }
        color.insert(name.to_string(), 2);
        false
    }

    for pkg in &workspace.packages {
        if let Some(ref name) = pkg.manifest.name {
            if *color.get(name).unwrap_or(&0) == 0
                && dfs(name, &adj, &mut color, &mut parent, &mut cycle)
            {
                return cycle;
            }
        }
    }

    Vec::new()
}
