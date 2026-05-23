//! Prune command handler.

use crate::errors;
use orix_core::{prune, ConfigOverrides};

use super::{CHECKMARK, CROSS, INFO};

pub(crate) fn run_prune(
    project_root: &std::path::Path,
    keep_lockfile: bool,
    dry_run: bool,
    overrides: &ConfigOverrides,
) {
    let _ = overrides; // Reserved for future use (e.g., custom store dir)

    let report = match prune(project_root, keep_lockfile, dry_run) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{}", errors::format_error(&e, project_root));
            std::process::exit(1);
        }
    };

    if dry_run {
        if report.items_removed > 0 {
            println!(
                " {} Would remove {} items from node_modules ({} bytes)",
                INFO, report.items_removed, report.node_modules_bytes
            );
        } else {
            println!(" {} Nothing to remove", INFO);
        }
        if report.lockfile_removed && !keep_lockfile {
            println!(" {} Would remove orix-lock.yaml", INFO);
        }
    } else {
        println!(
            " {} Removed {} items from node_modules ({} bytes)",
            CHECKMARK,
            report.items_removed,
            report.node_modules_bytes
        );
        if report.lockfile_removed {
            println!(" {} Removed orix-lock.yaml", CROSS);
        }
    }
}
