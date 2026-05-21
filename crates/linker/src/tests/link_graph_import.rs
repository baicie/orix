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
fn link_graph_skips_file_import_when_package_already_complete() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let store = Store::open(temp.path().join("store"))?;
    import_package(&store, temp.path(), "lodash", "4.17.21")?;

    let mut graph = DependencyGraph::new();
    graph.insert(resolved_package("lodash", "4.17.21", Vec::new())?);

    let linker = Linker::new(store, temp.path().join("node_modules"));
    let direct_deps = HashSet::from(["lodash".to_string()]);
    let hash = graph.graph_hash();

    let first = linker.link_graph(&graph, &direct_deps, None, &hash)?;
    assert!(
        first.hardlinked_files > 0 || first.copied_files > 0,
        "first link should import files (hardlink or copy)"
    );

    let second = linker.link_graph(&graph, &direct_deps, None, &hash)?;
    assert_eq!(
        second.hardlinked_files, 0,
        "second link should skip integrity-complete packages"
    );
    assert_eq!(second.copied_files, 0, "second link should not copy files");

    Ok(())
}
