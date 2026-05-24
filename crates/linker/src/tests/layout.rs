use std::collections::HashSet;
use std::fs;

use orix_domain::DependencyGraph;
use orix_store::Store;

use super::helpers::*;
use crate::Linker;

#[test]
fn prune_stale_layout_removes_only_obsolete_virtual_store_entries() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let store = Store::open(temp.path().join("store"))?;
    import_package(&store, temp.path(), "left-pkg", "1.0.0")?;
    import_package(&store, temp.path(), "right-pkg", "1.0.0")?;

    let mut old_graph = DependencyGraph::new();
    old_graph.insert(resolved_package("left-pkg", "1.0.0", Vec::new())?);
    old_graph.insert(resolved_package("right-pkg", "1.0.0", Vec::new())?);

    let linker = Linker::new(store, temp.path().join("node_modules"));
    let direct = HashSet::from(["left-pkg".to_string(), "right-pkg".to_string()]);
    linker.link_graph(&old_graph, &direct, None, &old_graph.graph_hash(), None)?;

    assert!(temp
        .path()
        .join("node_modules")
        .join(".orix")
        .join("right-pkg@1.0.0")
        .exists());

    let mut new_graph = DependencyGraph::new();
    new_graph.insert(resolved_package("left-pkg", "1.0.0", Vec::new())?);
    let new_direct = HashSet::from(["left-pkg".to_string()]);
    linker.prune_stale_layout(&new_graph, &new_direct)?;

    assert!(temp
        .path()
        .join("node_modules")
        .join(".orix")
        .join("left-pkg@1.0.0")
        .exists());
    assert!(!temp
        .path()
        .join("node_modules")
        .join(".orix")
        .join("right-pkg@1.0.0")
        .exists());
    assert!(temp.path().join("node_modules").exists());

    Ok(())
}

#[test]
fn unlink_removes_node_modules_directory() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let store = Store::open(temp.path().join("store"))?;
    let nm_dir = temp.path().join("node_modules");
    fs::create_dir_all(&nm_dir)?;
    fs::write(nm_dir.join("dummy.txt"), b"placeholder")?;

    let linker = Linker::new(store, nm_dir.clone());
    linker.unlink()?;

    assert!(!nm_dir.exists());
    Ok(())
}

#[test]
fn unlink_does_not_error_when_node_modules_missing() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let store = Store::open(temp.path().join("store"))?;
    let nm_dir = temp.path().join("nonexistent_node_modules");

    let linker = Linker::new(store, nm_dir);
    linker.unlink()?; // Should succeed without error

    Ok(())
}

#[test]
fn link_local_package_creates_symlink_to_source_directory() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let store = Store::open(temp.path().join("store"))?;
    let nm_dir = temp.path().join("node_modules");
    let source_dir = temp.path().join("packages").join("local-pkg");
    fs::create_dir_all(&source_dir)?;
    fs::write(
        source_dir.join("package.json"),
        r#"{"name":"local-pkg","version":"1.0.0"}"#,
    )?;

    let linker = Linker::new(store, nm_dir.clone());
    let created = linker.link_local_package("local-pkg", &source_dir)?;

    assert_eq!(created, 1);
    assert!(nm_dir.join("local-pkg").exists());
    Ok(())
}

#[test]
fn link_local_package_skips_existing_symlink() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let store = Store::open(temp.path().join("store"))?;
    let nm_dir = temp.path().join("node_modules");
    fs::create_dir_all(&nm_dir)?;
    let source_dir = temp.path().join("packages").join("local-pkg");
    fs::create_dir_all(&source_dir)?;

    let linker = Linker::new(store, nm_dir.clone());
    linker.link_local_package("local-pkg", &source_dir)?;
    let created = linker.link_local_package("local-pkg", &source_dir)?;

    assert_eq!(created, 0); // Second call should not create again
    Ok(())
}
