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
fn link_graph_creates_parent_dirs_for_scoped_bin_names() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let store = Store::open(temp.path().join("store"))?;
    import_package_with_manifest(
        &store,
        temp.path(),
        "@antfu/eslint-config",
        "9.0.0",
        r#"{"name":"@antfu/eslint-config","version":"9.0.0","bin":"./bin/index.mjs"}"#,
    )?;

    let mut graph = DependencyGraph::new();
    graph.insert(resolved_package(
        "@antfu/eslint-config",
        "9.0.0",
        Vec::new(),
    )?);

    let linker = Linker::new(store, temp.path().join("node_modules"));
    let direct_deps = HashSet::from(["@antfu/eslint-config".to_string()]);
    linker.link_graph(&graph, &direct_deps, None, &graph.graph_hash())?;

    // Scoped bin names are flattened to avoid @ and / in Windows filenames.
    // The shim should be eslint-config (not @antfu/eslint-config).
    let bin_dir = temp.path().join("node_modules").join(".bin");

    #[cfg(windows)]
    {
        // Windows creates .cmd and .ps1 shims with the flattened name.
        assert!(
            bin_dir.join("eslint-config.cmd").exists(),
            "flattened .cmd shim should exist"
        );
        assert!(
            bin_dir.join("eslint-config.ps1").exists(),
            "flattened .ps1 shim should exist"
        );
        // The original scoped path should NOT exist as a file.
        assert!(
            !bin_dir.join("@antfu").join("eslint-config").exists(),
            "scoped path should not exist on Windows"
        );
    }

    #[cfg(not(windows))]
    {
        // Unix also uses flattened name for consistency across platforms.
        // @antfu/eslint-config -> eslint-config
        assert!(
            bin_dir.join("eslint-config").exists(),
            "flattened bin symlink should exist on Unix"
        );
    }

    Ok(())
}

#[cfg(unix)]
#[test]
fn link_graph_makes_package_bins_executable() -> anyhow::Result<()> {
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

    let shim = temp.path().join("node_modules").join(".bin").join("rollup");
    let target_metadata = fs::metadata(&shim)?;

    assert!(
        target_metadata.mode() & 0o111 != 0,
        "bin shim target should be executable"
    );
    assert!(linker.is_layout_valid(&graph.graph_hash()));
    Ok(())
}

#[cfg(unix)]
#[test]
fn link_graph_keeps_bins_inside_package_for_relative_requires() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let store = Store::open(temp.path().join("store"))?;
    import_package_with_rollup_style_bin(&store, temp.path())?;

    let mut graph = DependencyGraph::new();
    graph.insert(resolved_package("rollup", "4.0.0", Vec::new())?);

    let linker = Linker::new(store, temp.path().join("node_modules"));
    let direct_deps = HashSet::from(["rollup".to_string()]);
    linker.link_graph(&graph, &direct_deps, None, &graph.graph_hash())?;

    let shim = temp.path().join("node_modules").join(".bin").join("rollup");
    let shim_target = fs::read_link(&shim)?;
    let shim_parent = shim
        .parent()
        .context("rollup shim should have a parent directory")?;
    let resolved = fs::canonicalize(shim_parent.join(shim_target))?;
    let expected = temp
        .path()
        .join("node_modules")
        .join(".orix")
        .join("rollup@4.0.0")
        .join("node_modules")
        .join("rollup")
        .join("bin")
        .join("rollup");

    assert_eq!(
        normal_components(&resolved),
        normal_components(&fs::canonicalize(expected)?)
    );
    let resolved_parent = resolved
        .parent()
        .context("resolved rollup bin should have a parent directory")?;
    assert!(resolved_parent.join("../shared/rollup.js").exists());
    assert!(!temp
        .path()
        .join("node_modules")
        .join(".orix")
        .join("rollup@4.0.0")
        .join("bin")
        .join("rollup")
        .exists());
    Ok(())
}

#[cfg(unix)]
#[test]
fn link_graph_prefers_direct_version_for_top_level_bins() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let store = Store::open(temp.path().join("store"))?;
    import_package_with_manifest(
        &store,
        temp.path(),
        "rollup",
        "1.32.1",
        r#"{"name":"rollup","version":"1.32.1","bin":{"rollup":"./bin/index.mjs"}}"#,
    )?;
    import_package_with_manifest(
        &store,
        temp.path(),
        "rollup",
        "4.60.4",
        r#"{"name":"rollup","version":"4.60.4","bin":{"rollup":"./bin/index.mjs"}}"#,
    )?;

    let mut graph = DependencyGraph::new();
    graph.insert(resolved_package("rollup", "1.32.1", Vec::new())?);
    graph.insert(resolved_package("rollup", "4.60.4", Vec::new())?);

    let linker = Linker::new(store, temp.path().join("node_modules"));
    let direct_deps = HashSet::from(["rollup".to_string()]);
    linker.link_graph(&graph, &direct_deps, None, &graph.graph_hash())?;

    let direct_link = temp.path().join("node_modules").join("rollup");
    let direct_expected = temp
        .path()
        .join("node_modules")
        .join(".orix")
        .join("rollup@4.60.4")
        .join("node_modules")
        .join("rollup");
    assert_eq!(
        normal_components(&fs::canonicalize(direct_link)?),
        normal_components(&fs::canonicalize(direct_expected)?)
    );

    let shim = temp.path().join("node_modules").join(".bin").join("rollup");
    let shim_target = fs::read_link(&shim)?;
    let shim_parent = shim
        .parent()
        .context("rollup shim should have a parent directory")?;
    let resolved = fs::canonicalize(shim_parent.join(shim_target))?;
    assert!(
        normal_components(&resolved)
            .iter()
            .any(|part| part == "rollup@4.60.4"),
        "rollup shim should point to direct rollup version"
    );
    Ok(())
}

#[test]
fn link_graph_selects_internal_dependency_by_declared_range() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let store = Store::open(temp.path().join("store"))?;
    import_package(&store, temp.path(), "rollup-pluginutils", "2.8.2")?;
    import_package(&store, temp.path(), "estree-walker", "0.6.1")?;
    import_package(&store, temp.path(), "estree-walker", "3.0.3")?;

    let mut graph = DependencyGraph::new();
    graph.insert(resolved_package(
        "rollup-pluginutils",
        "2.8.2",
        vec![("estree-walker", "^0.6.1")],
    )?);
    graph.insert(resolved_package("estree-walker", "0.6.1", Vec::new())?);
    graph.insert(resolved_package("estree-walker", "3.0.3", Vec::new())?);

    let linker = Linker::new(store, temp.path().join("node_modules"));
    let direct_deps = HashSet::from(["rollup-pluginutils".to_string()]);
    linker.link_graph(&graph, &direct_deps, None, &graph.graph_hash())?;

    let dep_link = temp
        .path()
        .join("node_modules")
        .join(".orix")
        .join("rollup-pluginutils@2.8.2")
        .join("node_modules")
        .join("rollup-pluginutils")
        .join("node_modules")
        .join("estree-walker");
    let expected = temp
        .path()
        .join("node_modules")
        .join(".orix")
        .join("estree-walker@0.6.1")
        .join("node_modules")
        .join("estree-walker");

    assert_eq!(
        normal_components(&fs::canonicalize(dep_link)?),
        normal_components(&fs::canonicalize(expected)?)
    );
    Ok(())
}
