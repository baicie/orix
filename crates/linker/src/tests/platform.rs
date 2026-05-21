use std::fs;
use std::path::Path;

use crate::linker_platform::is_bare_drive_path;
use crate::Linker;

#[test]
fn is_bare_drive_path_detects_drive_letter_only() {
    assert!(is_bare_drive_path(Path::new("D:")));
    assert!(is_bare_drive_path(Path::new("d:\\")));
    assert!(!is_bare_drive_path(Path::new("D:\\workspace\\proj")));
}

#[cfg(windows)]
#[test]
fn windows_absolutizes_relative_junction_target() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let target = temp
        .path()
        .join("node_modules")
        .join(".orix")
        .join("dep@1.0.0")
        .join("node_modules")
        .join("dep");
    let link = temp
        .path()
        .join("node_modules")
        .join(".orix")
        .join("parent@1.0.0")
        .join("node_modules")
        .join("parent")
        .join("node_modules")
        .join("dep");
    fs::create_dir_all(&target)?;
    let link_parent = link
        .parent()
        .ok_or_else(|| anyhow::anyhow!("test link should have a parent"))?;
    fs::create_dir_all(link_parent)?;

    let relative = relative_path(link_parent, &target);
    let absolute = Linker::absolutize_link_target(&relative, &link)?;

    assert!(absolute.is_absolute());
    assert_eq!(absolute, target.canonicalize()?);
    Ok(())
}
