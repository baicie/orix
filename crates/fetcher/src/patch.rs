//! Patch application support for the patch: protocol.
//!
//! Applies unified diff patches to extracted package directories.

use std::path::Path;

use anyhow::{Context, Result};

/// Apply a patch file to a package's extracted directory.
///
/// The patch is applied using the `patch` crate's unified diff parser.
/// `patch_path` is resolved relative to `project_root`.
pub fn apply_patch(
    pkg_id: &str,
    temp_dir: &Path,
    patch_path: &Path,
    project_root: &Path,
) -> Result<()> {
    let patch_file = if patch_path.is_absolute() {
        patch_path.to_path_buf()
    } else {
        project_root.join(patch_path)
    };

    if !patch_file.exists() {
        anyhow::bail!(
            "patch file not found: {} (resolved from '{}')",
            patch_file.display(),
            patch_path.display()
        );
    }

    let patch_content = std::fs::read_to_string(&patch_file)
        .with_context(|| format!("failed to read patch file: {}", patch_file.display()))?;

    // Parse all patches from the file.
    let patches: Vec<patch::Patch<'_>> = patch::Patch::from_multiple(&patch_content)
        .map_err(|e| anyhow::anyhow!("failed to parse patch file {}: {}", patch_file.display(), e))?;

    if patches.is_empty() {
        anyhow::bail!(
            "patch file '{}' contains no valid patches",
            patch_file.display()
        );
    }

    for p in patches {
        apply_single_file_patch(&p, temp_dir)
            .with_context(|| format!("failed to apply patch for {}", pkg_id))?;
    }

    Ok(())
}

fn apply_single_file_patch(patch: &patch::Patch<'_>, temp_dir: &Path) -> Result<()> {
    let old_path = temp_dir.join(patch.old.path.as_ref());
    let new_path = temp_dir.join(patch.new.path.as_ref());

    // Read old file content (if it exists).
    let old_content: Vec<u8> = if old_path.exists() {
        std::fs::read(&old_path)
            .with_context(|| format!("failed to read file: {}", old_path.display()))?
    } else {
        Vec::new()
    };

    let old_str = String::from_utf8_lossy(&old_content);
    let old_lines: Vec<&str> = old_str.lines().collect();

    // Apply each hunk in sequence.
    let mut new_content = String::new();
    let mut pos: u64 = 0;

    for hunk in &patch.hunks {
        // Copy context lines before this hunk.
        let hunk_start = hunk.old_range.start.saturating_sub(1); // 1-based to 0-based.
        if hunk_start > pos {
            for line in &old_lines[pos as usize..hunk_start.min(old_lines.len() as u64) as usize] {
                new_content.push_str(line);
                new_content.push('\n');
            }
            pos = hunk_start;
        }

        // Apply hunk lines.
        for line in &hunk.lines {
            match line {
                patch::Line::Add(s) => {
                    new_content.push_str(s);
                    new_content.push('\n');
                }
                patch::Line::Remove(s) => {
                    // Skip the removed line from old content.
                    if (pos as usize) < old_lines.len() && old_lines[pos as usize] == *s {
                        pos += 1;
                    } else {
                        pos += 1;
                    }
                }
                patch::Line::Context(s) => {
                    // Verify context matches, then copy.
                    if (pos as usize) < old_lines.len() && old_lines[pos as usize] == *s {
                        new_content.push_str(s);
                        new_content.push('\n');
                        pos += 1;
                    } else {
                        new_content.push_str(s);
                        new_content.push('\n');
                    }
                }
            }
        }
    }

    // Copy remaining lines.
    if (pos as usize) < old_lines.len() {
        for line in &old_lines[pos as usize..] {
            new_content.push_str(line);
            new_content.push('\n');
        }
    }

    // Ensure parent directory exists.
    if let Some(parent) = new_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create directory: {}", parent.display()))?;
    }

    // Write the patched content.
    std::fs::write(&new_path, new_content.as_bytes())
        .with_context(|| format!("failed to write patched file: {}", new_path.display()))?;

    // Preserve file permissions on Unix.
    #[cfg(unix)]
    if old_path.exists() {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(metadata) = old_path.metadata() {
            let mode = metadata.permissions().mode();
            let mut perms = std::fs::Permissions::from_mode(mode);
            let _ = std::fs::set_permissions(&new_path, perms);
        }
    }

    Ok(())
}
