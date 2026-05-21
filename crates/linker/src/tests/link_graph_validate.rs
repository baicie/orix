use std::collections::HashSet;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;

use anyhow::Context;
use orix_domain::DependencyGraph;
use orix_store::Store;

use super::helpers::*;
use crate::linker_platform::normal_components;
use crate::Linker;

#[test]
fn layout_is_invalid_when_unix_bin_target_is_not_executable() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let store = Store::open(temp.path().join("store"))?;
    let linker = Linker::new(store, temp.path().join("node_modules"));
    let graph_hash = "same-graph";

    let bin_dir = temp.path().join("node_modules").join(".bin");
    let target_dir = temp
        .path()
        .join("node_modules")
        .join(".orix")
        .join("rollup@4.0.0")
        .join("bin");
    let target = target_dir.join("rollup");
    fs::create_dir_all(&bin_dir)?;
    fs::create_dir_all(&target_dir)?;
    fs::write(&target, "#!/usr/bin/env node\n")?;
    fs::set_permissions(&target, PermissionsExt::from_mode(0o644))?;
    std::os::unix::fs::symlink("../.orix/rollup@4.0.0/bin/rollup", bin_dir.join("rollup"))?;
    linker.write_marker(graph_hash, 1)?;

    assert!(!linker.is_layout_valid(graph_hash));

    fs::set_permissions(&target, PermissionsExt::from_mode(0o755))?;

    assert!(linker.is_layout_valid(graph_hash));
    Ok(())
}

#[cfg(windows)]
#[test]
fn link_graph_creates_windows_cmd_shim_for_bins() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let store = Store::open(temp.path().join("store"))?;
    import_package_with_manifest(
        &store,
        temp.path(),
        "rollup",
        "4.0.0",
        r#"{"name":"rollup","version":"4.0.0","bin":{"rollup":"./bin/index.mjs"}}"#,
    )?;

    let mut graph = DependencyGraph::new();
    graph.insert(resolved_package("rollup", "4.0.0", Vec::new())?);

    let linker = Linker::new(store, temp.path().join("node_modules"));
    let direct_deps = HashSet::from(["rollup".to_string()]);
    linker.link_graph(&graph, &direct_deps, None, &graph.graph_hash())?;

    let shim = temp
        .path()
        .join("node_modules")
        .join(".bin")
        .join("rollup.cmd");
    let content = fs::read_to_string(&shim)?;

    assert!(shim.exists());
    // Shim uses %~dp0 to find the .bin directory at runtime.
    // Target is a relative path like ..\.orix\rollup@4.0.0\bin\rollup
    assert!(content.contains("basedir=%~dp0"));
    assert!(content.contains("node"));
    assert!(content.contains("index.mjs"));
    assert!(content.contains("%*"));
    Ok(())
}

#[cfg(windows)]
#[test]
fn layout_is_invalid_when_windows_bin_shim_is_missing() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let store = Store::open(temp.path().join("store"))?;
    let linker = Linker::new(store, temp.path().join("node_modules"));
    let graph_hash = "same-graph";

    fs::create_dir_all(temp.path().join("node_modules").join(".bin"))?;
    fs::write(
        temp.path().join("node_modules").join(".bin").join("rollup"),
        "#!/usr/bin/env node\n",
    )?;
    linker.write_marker(graph_hash, 1)?;

    assert!(!linker.is_layout_valid(graph_hash));

    fs::write(
        temp.path()
            .join("node_modules")
            .join(".bin")
            .join("rollup.cmd"),
        "@ECHO off\r\n",
    )?;

    assert!(linker.is_layout_valid(graph_hash));
    Ok(())
}
