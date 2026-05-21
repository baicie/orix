//! Platform path helpers.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use orix_domain::{ConstraintKind, DependencyGraph, PackageId, PackageName, VersionConstraint};

pub(crate) fn path_exists_or_symlink(path: &Path) -> bool {
    path.exists() || fs::symlink_metadata(path).is_ok()
}

pub(crate) fn remove_link_path(path: &Path) -> io::Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if meta.is_dir() {
        fs::remove_dir(path)
    } else {
        fs::remove_file(path)
    }
}

/// True for paths like `D:` that make Node resolve modules to a drive root (`EISDIR`).
pub(crate) fn is_bare_drive_path(path: &Path) -> bool {
    let s = path.as_os_str().to_string_lossy();
    let trimmed = s.trim_end_matches(['\\', '/']);
    let bytes = trimmed.as_bytes();
    bytes.len() == 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

/// True when a canonicalized path is only a volume root (e.g. `D:\` with no package segments).
pub(crate) fn resolves_to_drive_root_only(path: &Path) -> bool {
    if is_bare_drive_path(path) {
        return true;
    }
    let Ok(canon) = path.canonicalize() else {
        return false;
    };
    !canon.is_dir() || normal_components(&canon).is_empty()
}

#[cfg(windows)]
pub(crate) fn volume_root(path: &Path) -> Option<String> {
    let s = path.to_string_lossy();
    let bytes = s.as_bytes();
    if bytes.len() >= 2 && bytes[1] == b':' {
        let root_len = if bytes.len() >= 3 && bytes[2] == b'\\' {
            3
        } else {
            2
        };
        return Some(s[..root_len].to_string());
    }
    None
}

#[cfg(not(windows))]
pub(crate) fn path_starts_with_lexically(path: &Path, prefix: &Path) -> bool {
    let path_components = normal_components(path);
    let prefix_components = normal_components(prefix);
    path_components.starts_with(&prefix_components)
}

pub(crate) fn select_dependency_key(
    graph: &DependencyGraph,
    dep_name: &PackageName,
    raw: &str,
) -> Option<String> {
    let constraint = VersionConstraint::parse(raw).ok()?;
    graph
        .packages()
        .filter(|pkg| pkg.id.name == *dep_name && package_matches_constraint(&pkg.id, &constraint))
        .map(|pkg| pkg.id.key())
        .last()
}

pub(crate) fn package_matches_constraint(
    pkg_id: &PackageId,
    constraint: &VersionConstraint,
) -> bool {
    match &constraint.kind {
        ConstraintKind::Exact(version) => pkg_id.version == *version,
        ConstraintKind::Range(req) => req.matches(&pkg_id.version),
        ConstraintKind::AnyRange(ranges) => ranges.iter().any(|req| req.matches(&pkg_id.version)),
        ConstraintKind::Alias { constraint, .. } => package_matches_constraint(pkg_id, constraint),
        ConstraintKind::Patch(spec) => pkg_id.version == spec.package_version,
        ConstraintKind::Latest | ConstraintKind::Tag(_) | ConstraintKind::Catalog(_) => true,
    }
}

pub(crate) fn relative_path(from_dir: &Path, to_path: &Path) -> PathBuf {
    let from_components = normal_components(from_dir);
    let to_components = normal_components(to_path);
    let common_len = from_components
        .iter()
        .zip(to_components.iter())
        .take_while(|(from, to)| from == to)
        .count();

    let mut result = PathBuf::new();
    for _ in common_len..from_components.len() {
        result.push("..");
    }
    for component in &to_components[common_len..] {
        result.push(component);
    }

    if result.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        result
    }
}

pub(crate) fn normal_components(path: &Path) -> Vec<String> {
    path.components()
        .filter_map(|component| match component {
            std::path::Component::Normal(part) => part.to_str().map(ToOwned::to_owned),
            std::path::Component::ParentDir => Some("..".to_string()),
            std::path::Component::CurDir
            | std::path::Component::RootDir
            | std::path::Component::Prefix(_) => None,
        })
        .collect()
}
