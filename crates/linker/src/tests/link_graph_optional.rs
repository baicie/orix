use std::collections::HashSet;
use std::fs;

use orix_domain::DependencyGraph;
use orix_store::Store;

use super::helpers::*;
use crate::Linker;

#[test]
fn link_graph_links_optional_dependencies_after_all_packages_are_imported() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let store = Store::open(temp.path().join("store"))?;
    import_package(&store, temp.path(), "rollup", "4.0.0")?;
    import_package(&store, temp.path(), "@rollup/rollup-darwin-arm64", "4.0.0")?;

    let mut graph = DependencyGraph::new();
    graph.insert(resolved_package_with_optional(
        "rollup",
        "4.0.0",
        Vec::new(),
        vec![("@rollup/rollup-darwin-arm64", "4.0.0")],
    )?);
    graph.insert(resolved_package(
        "@rollup/rollup-darwin-arm64",
        "4.0.0",
        Vec::new(),
    )?);

    let linker = Linker::new(store, temp.path().join("node_modules"));
    let direct_deps = HashSet::from(["rollup".to_string()]);
    linker.link_graph(&graph, &direct_deps, None, &graph.graph_hash(), None)?;

    let native_link = temp
        .path()
        .join("node_modules")
        .join(".orix")
        .join("rollup@4.0.0")
        .join("node_modules")
        .join("rollup")
        .join("node_modules")
        .join("@rollup")
        .join("rollup-darwin-arm64");
    let resolved = fs::canonicalize(&native_link)?;
    let expected = fs::canonicalize(
        temp.path()
            .join("node_modules")
            .join(".orix")
            .join("@rollup")
            .join("rollup-darwin-arm64@4.0.0")
            .join("node_modules")
            .join("@rollup")
            .join("rollup-darwin-arm64"),
    )?;

    assert_eq!(resolved, expected);
    Ok(())
}

#[test]
fn link_graph_links_peer_dependencies_when_present_in_graph() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let store = Store::open(temp.path().join("store"))?;
    import_package(&store, temp.path(), "rollup-plugin-esbuild", "6.2.1")?;
    import_package(&store, temp.path(), "esbuild", "0.27.0")?;

    let mut graph = DependencyGraph::new();
    graph.insert(resolved_package_with_optional_and_peers(
        "rollup-plugin-esbuild",
        "6.2.1",
        Vec::new(),
        Vec::new(),
        vec![("esbuild", ">=0.18.0")],
    )?);
    graph.insert(resolved_package("esbuild", "0.27.0", Vec::new())?);

    let linker = Linker::new(store, temp.path().join("node_modules"));
    let direct_deps = HashSet::from(["rollup-plugin-esbuild".to_string()]);
    linker.link_graph(&graph, &direct_deps, None, &graph.graph_hash(), None)?;

    let peer_link = temp
        .path()
        .join("node_modules")
        .join(".orix")
        .join("rollup-plugin-esbuild@6.2.1")
        .join("node_modules")
        .join("rollup-plugin-esbuild")
        .join("node_modules")
        .join("esbuild");
    let resolved = fs::canonicalize(&peer_link)?;
    let expected = fs::canonicalize(
        temp.path()
            .join("node_modules")
            .join(".orix")
            .join("esbuild@0.27.0")
            .join("node_modules")
            .join("esbuild"),
    )?;

    assert_eq!(resolved, expected);
    Ok(())
}
