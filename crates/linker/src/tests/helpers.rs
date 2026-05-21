use std::collections::{HashMap, HashSet};
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use orix_domain::{DependencyGraph, PackageId, PackageName, ResolvedPackage, Version};
use orix_store::Store;

use anyhow::Context;

use crate::linker::Linker;
use crate::linker_platform::*;
use crate::{LayoutReport, LinkReport};

const VIRTUAL_STORE_DIR: &str = ".orix";

pub(crate) fn pkg_id(name: &str, version: &str) -> anyhow::Result<PackageId> {
    Ok(PackageId::new(
        PackageName::from(name),
        Version::parse(version)?,
    ))
}

pub(crate) fn resolved_package(
    name: &str,
    version: &str,
    dependencies: Vec<(&str, &str)>,
) -> anyhow::Result<ResolvedPackage> {
    resolved_package_with_optional(name, version, dependencies, Vec::new())
}

pub(crate) fn resolved_package_with_optional(
    name: &str,
    version: &str,
    dependencies: Vec<(&str, &str)>,
    optional_dependencies: Vec<(&str, &str)>,
) -> anyhow::Result<ResolvedPackage> {
    resolved_package_with_optional_and_peers(
        name,
        version,
        dependencies,
        optional_dependencies,
        Vec::new(),
    )
}

pub(crate) fn resolved_package_with_optional_and_peers(
    name: &str,
    version: &str,
    dependencies: Vec<(&str, &str)>,
    optional_dependencies: Vec<(&str, &str)>,
    peer_dependencies: Vec<(&str, &str)>,
) -> anyhow::Result<ResolvedPackage> {
    Ok(ResolvedPackage {
        id: pkg_id(name, version)?,
        integrity: String::new(),
        tarball: String::new(),
        dependencies: dependencies
            .into_iter()
            .map(|(name, version)| (PackageName::from(name), version.to_string()))
            .collect(),
        dev_dependencies: Vec::new(),
        optional_dependencies: optional_dependencies
            .into_iter()
            .map(|(name, version)| (PackageName::from(name), version.to_string()))
            .collect(),
        peer_dependencies: peer_dependencies
            .into_iter()
            .map(|(name, version)| (PackageName::from(name), version.to_string()))
            .collect(),
        engines: None,
        os: Vec::new(),
        cpu: Vec::new(),
        depnodes: Vec::new(),
        patch: None,
    })
}

pub(crate) fn write_package(root: &Path, name: &str, version: &str) -> anyhow::Result<()> {
    fs::create_dir_all(root)?;
    fs::write(
        root.join("package.json"),
        format!(r#"{{"name":"{}","version":"{}"}}"#, name, version),
    )?;
    fs::write(root.join("index.js"), "module.exports = 1;\n")?;
    Ok(())
}

pub(crate) fn import_package(
    store: &Store,
    temp_root: &Path,
    name: &str,
    version: &str,
) -> anyhow::Result<PackageId> {
    let source = temp_root.join(format!("{}-{}", name.replace('/', "-"), version));
    write_package(&source, name, version)?;
    let id = pkg_id(name, version)?;
    store.import_package(&id, &source, Vec::new(), None)?;
    Ok(id)
}

pub(crate) fn import_package_with_manifest(
    store: &Store,
    temp_root: &Path,
    name: &str,
    version: &str,
    manifest: &str,
) -> anyhow::Result<PackageId> {
    let source = temp_root.join(format!("{}-{}", name.replace('/', "-"), version));
    fs::create_dir_all(source.join("bin"))?;
    fs::write(source.join("package.json"), manifest)?;
    fs::write(source.join("index.js"), "module.exports = 1;\n")?;
    fs::write(
        source.join("bin").join("index.mjs"),
        "#!/usr/bin/env node\n",
    )?;
    let id = pkg_id(name, version)?;
    store.import_package(&id, &source, Vec::new(), None)?;
    Ok(id)
}

#[cfg(unix)]
pub(crate) fn import_package_with_rollup_style_bin(
    store: &Store,
    temp_root: &Path,
) -> anyhow::Result<PackageId> {
    let source = temp_root.join("rollup-4.0.0-relative-bin");
    fs::create_dir_all(source.join("bin"))?;
    fs::create_dir_all(source.join("shared"))?;
    fs::write(
        source.join("package.json"),
        r#"{"name":"rollup","version":"4.0.0","bin":{"rollup":"./bin/rollup"}}"#,
    )?;
    fs::write(
        source.join("bin").join("rollup"),
        "#!/usr/bin/env node\nrequire('../shared/rollup.js');\n",
    )?;
    fs::write(
        source.join("shared").join("rollup.js"),
        "module.exports = 1;\n",
    )?;
    let id = pkg_id("rollup", "4.0.0")?;
    store.import_package(&id, &source, Vec::new(), None)?;
    Ok(id)
}
