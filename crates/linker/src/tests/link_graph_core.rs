use std::collections::HashSet;
use std::fs;

use orix_domain::DependencyGraph;
use orix_store::Store;

use super::helpers::*;
use crate::Linker;

#[test]
fn link_graph_creates_valid_layout_for_direct_and_transitive_deps() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let store = Store::open(temp.path().join("store"))?;
    import_package(&store, temp.path(), "react", "18.2.0")?;
    import_package(&store, temp.path(), "scheduler", "0.23.0")?;

    let mut graph = DependencyGraph::new();
    graph.insert(resolved_package(
        "react",
        "18.2.0",
        vec![("scheduler", "0.23.0")],
    )?);
    graph.insert(resolved_package("scheduler", "0.23.0", Vec::new())?);

    let linker = Linker::new(store, temp.path().join("node_modules"));
    let direct_deps = HashSet::from(["react".to_string()]);
    linker.link_graph(&graph, &direct_deps, None, &graph.graph_hash(), None)?;

    let report = linker.validate_layout(&direct_deps)?;

    assert!(report.is_ok());
    assert!(temp.path().join("node_modules").join("react").exists());
    assert!(temp
        .path()
        .join("node_modules")
        .join(".orix")
        .join("react@18.2.0")
        .exists());
    assert!(!temp.path().join("node_modules").join(".pnpm").exists());
    assert!(!temp.path().join("node_modules").join("scheduler").exists());
    Ok(())
}

#[test]
fn validate_layout_reports_missing_direct_dependency() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let store = Store::open(temp.path().join("store"))?;
    let linker = Linker::new(store, temp.path().join("node_modules"));
    fs::create_dir_all(temp.path().join("node_modules"))?;
    let direct_deps = HashSet::from(["react".to_string()]);

    let report = linker.validate_layout(&direct_deps)?;

    assert!(!report.is_ok());
    assert_eq!(report.broken.len(), 1);
    Ok(())
}

#[test]
fn link_graph_supports_scoped_direct_dependencies() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let store = Store::open(temp.path().join("store"))?;
    import_package(&store, temp.path(), "@scope/pkg", "1.0.0")?;

    let mut graph = DependencyGraph::new();
    graph.insert(resolved_package("@scope/pkg", "1.0.0", Vec::new())?);

    let linker = Linker::new(store, temp.path().join("node_modules"));
    let direct_deps = HashSet::from(["@scope/pkg".to_string()]);
    linker.link_graph(&graph, &direct_deps, None, &graph.graph_hash(), None)?;

    let report = linker.validate_layout(&direct_deps)?;

    assert!(report.is_ok());
    assert!(temp
        .path()
        .join("node_modules")
        .join("@scope")
        .join("pkg")
        .exists());
    Ok(())
}

#[test]
fn link_graph_supports_scoped_transitive_dependencies() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let store = Store::open(temp.path().join("store"))?;
    import_package(&store, temp.path(), "@scope/parent", "1.0.0")?;
    import_package(&store, temp.path(), "@scope/child", "1.0.0")?;

    let mut graph = DependencyGraph::new();
    graph.insert(resolved_package(
        "@scope/parent",
        "1.0.0",
        vec![("@scope/child", "1.0.0")],
    )?);
    graph.insert(resolved_package("@scope/child", "1.0.0", Vec::new())?);

    let linker = Linker::new(store, temp.path().join("node_modules"));
    let direct_deps = HashSet::from(["@scope/parent".to_string()]);
    linker.link_graph(&graph, &direct_deps, None, &graph.graph_hash(), None)?;

    let report = linker.validate_layout(&direct_deps)?;

    assert!(report.is_ok(), "{:?}", report.broken);
    assert!(!temp
        .path()
        .join("node_modules")
        .join("@scope")
        .join("child")
        .exists());
    assert!(temp
        .path()
        .join("node_modules")
        .join(".orix")
        .join("@scope")
        .join("parent@1.0.0")
        .join("node_modules")
        .join("@scope")
        .join("parent")
        .join("node_modules")
        .join("@scope")
        .join("child")
        .exists());
    let dep_link = temp
        .path()
        .join("node_modules")
        .join(".orix")
        .join("@scope")
        .join("parent@1.0.0")
        .join("node_modules")
        .join("@scope")
        .join("parent")
        .join("node_modules")
        .join("@scope")
        .join("child");
    let resolved = fs::canonicalize(&dep_link)?;
    let expected = fs::canonicalize(
        temp.path()
            .join("node_modules")
            .join(".orix")
            .join("@scope")
            .join("child@1.0.0")
            .join("node_modules")
            .join("@scope")
            .join("child"),
    )?;
    assert_eq!(resolved, expected);
    Ok(())
}
