//! Store and cache command output.

use crate::errors;
use orix_core::{
    cache_clean_with_overrides, cache_path_with_overrides, store_path_with_overrides,
    store_prune_with_overrides, store_verify_with_overrides, ConfigOverrides,
};

use super::{CHECKMARK, CROSS, INFO};

pub(crate) fn print_store_path(project_root: &std::path::Path, overrides: &ConfigOverrides) {
    let path = match store_path_with_overrides(project_root, overrides) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{}", errors::format_error(&e, project_root));
            std::process::exit(1);
        }
    };
    println!("{}", path.display());
}

pub(crate) fn print_store_prune(
    project_root: &std::path::Path,
    overrides: &ConfigOverrides,
    dry_run: bool,
) {
    let report = match store_prune_with_overrides(project_root, dry_run, overrides) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{}", errors::format_error(&e, project_root));
            std::process::exit(1);
        }
    };
    if dry_run {
        println!(
            " {} Would remove {} packages and {} content files",
            INFO, report.packages_removed, report.files_removed
        );
    } else {
        println!(
            " {} Removed {} packages and {} content files",
            CHECKMARK, report.packages_removed, report.files_removed
        );
    }
    println!(" {} Bytes reclaimed: {}", INFO, report.bytes_reclaimed);
}

pub(crate) fn print_store_verify(project_root: &std::path::Path, overrides: &ConfigOverrides) {
    let report = match store_verify_with_overrides(project_root, overrides) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{}", errors::format_error(&e, project_root));
            std::process::exit(1);
        }
    };
    println!(" {} Packages checked: {}", INFO, report.packages_checked);
    println!(" {} Files checked: {}", INFO, report.files_checked);
    if report.is_ok() {
        println!(" {} Store verified", CHECKMARK);
    } else {
        for missing in &report.missing {
            eprintln!("{} missing: {}", CROSS, missing);
        }
        for corrupted in &report.corrupted {
            eprintln!("{} corrupted: {}", CROSS, corrupted);
        }
        std::process::exit(1);
    }
}

pub(crate) fn print_cache_path(project_root: &std::path::Path, overrides: &ConfigOverrides) {
    let path = match cache_path_with_overrides(project_root, overrides) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{}", errors::format_error(&e, project_root));
            std::process::exit(1);
        }
    };
    println!("{}", path.display());
}

pub(crate) fn print_cache_clean(project_root: &std::path::Path, overrides: &ConfigOverrides) {
    let report = match cache_clean_with_overrides(project_root, overrides) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{}", errors::format_error(&e, project_root));
            std::process::exit(1);
        }
    };

    if report.existed {
        println!(" {} Cleared cache: {}", CHECKMARK, report.path.display());
        println!(" {} Bytes reclaimed: {}", INFO, report.bytes_reclaimed);
    } else {
        println!(
            " {} Cache is already empty: {}",
            INFO,
            report.path.display()
        );
    }
}
