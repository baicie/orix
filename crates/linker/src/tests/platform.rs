use std::fs;
use std::path::Path;

#[cfg(windows)]
use crate::linker_platform::cmd_compatible_path;
#[cfg(windows)]
use crate::linker_platform::relative_path;
use crate::linker_platform::{is_bare_drive_path, remove_link_path};
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

#[cfg(windows)]
#[test]
fn windows_remove_link_path_removes_junction_without_touching_target() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let target = temp.path().join("target");
    let link = temp.path().join("link");
    fs::create_dir_all(&target)?;
    fs::write(target.join("package.json"), "{}")?;

    let output = std::process::Command::new("cmd")
        .args(["/c", "mklink", "/J"])
        .arg(&link)
        .arg(&target)
        .output()?;
    if !output.status.success() {
        anyhow::bail!(
            "failed to create test junction: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    remove_link_path(&link)?;

    assert!(!fs::symlink_metadata(&link).is_ok());
    assert!(target.join("package.json").exists());
    Ok(())
}

#[cfg(windows)]
#[test]
fn windows_cmd_compatible_path_strips_verbatim_prefixes() {
    assert_eq!(
        cmd_compatible_path(Path::new(r"\\?\D:\workspace\project")),
        Path::new(r"D:\workspace\project")
    );
    assert_eq!(
        cmd_compatible_path(Path::new(r"\\?\UNC\server\share\project")),
        Path::new(r"\\server\share\project")
    );
}
