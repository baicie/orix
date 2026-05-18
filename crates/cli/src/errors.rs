//! Human-readable error messages for CLI output.

use std::path::Path;

use crate::styles::{ColorState, Style};

/// Categorizes common error patterns into user-friendly messages.
pub fn format_error(error: &anyhow::Error, project_root: &Path, color_state: ColorState) -> String {
    let cause_chain = error.chain().collect::<Vec<_>>();
    let top = cause_chain
        .first()
        .map(|e| e.to_string())
        .unwrap_or_default();

    let err_prefix = Style::ErrorPrefix.paint(EMOJI_ERR, color_state);

    // Workspace errors
    if (top.contains("workspace") || top.contains("pnpm-workspace"))
        && (top.contains("failed to read") || top.contains("parse"))
    {
        return format!(
            "{} Failed to read workspace configuration.\n  \
               Check pnpm-workspace.yaml for YAML syntax errors.\n  \
               {}",
            err_prefix,
            Style::Error.paint(&top, color_state)
        );
    }

    // Manifest errors
    if top.contains("failed to read") && top.contains("package.json") {
        return format!(
            "{} No package.json found in {}.\n  \
               Hint: Run this command in a directory containing a package.json,\n  \
               or use -C <path> to specify the project directory.",
            err_prefix,
            Style::Registry.paint(&project_root.display().to_string(), color_state)
        );
    }
    if top.contains("failed to parse") && top.contains("package.json") {
        return format!(
            "{} Invalid package.json: {}\n  \
               Hint: Check the JSON syntax of your package.json.",
            err_prefix,
            Style::Error.paint(&top, color_state)
        );
    }

    // Lockfile errors
    if top.contains("frozen lockfile validation failed") {
        return format!(
            "{} Lockfile mismatch.\n  \
               Your package.json dependencies have changed but the lockfile\n  \
               wasn't updated. Run 'orix install' without --frozen-lockfile first.",
            err_prefix
        );
    }
    if top.contains("No lockfile found") {
        return format!(
            "{} No lockfile found.\n  \
               Run 'orix install' without --frozen-lockfile to generate one.",
            err_prefix
        );
    }

    // Resolution errors
    if top.contains("failed to resolve dependencies") {
        if top.contains("workspace") {
            return format!(
                "{} Failed to resolve workspace dependencies.\n  \
                   Check that all packages referenced in pnpm-workspace.yaml exist.\n  \
                   {}",
                err_prefix,
                Style::Error.paint(&top, color_state)
            );
        }
        return format!(
            "{} Failed to resolve dependencies.\n  \
               Check your package.json for typos and valid version ranges.\n  \
               {}",
            err_prefix,
            Style::Error.paint(&top, color_state)
        );
    }

    // Registry / fetch errors
    if top.contains("failed to fetch") || top.contains("fetch packument") {
        if top.contains("not found") || top.contains("404") {
            let pkg = extract_package_name(&top);
            return format!(
                "{} Package '{}' not found in the registry.\n  \
                   Check the package name and version in package.json.",
                err_prefix,
                Style::PackageName.paint(&pkg, color_state)
            );
        }
        if top.contains("network") || top.contains("connection") || top.contains("timeout") {
            return format!(
                "{} Network error while fetching packages.\n  \
                   Check your internet connection and registry URL.\n  \
                   Run 'orix install --offline' to use cached packages only.\n  \
                   {}",
                err_prefix,
                Style::Error.paint(&top, color_state)
            );
        }
        if top.contains("403") || top.contains("unauthorized") || top.contains("authentication") {
            return format!(
                "{} Authentication failed for the registry.\n  \
                   Set your token with: npm config set //registry.npmjs.org/:_authToken YOUR_TOKEN\n  \
                   Or set the RPNPM_REGISTRY_TOKEN environment variable.",
                err_prefix
            );
        }
        return format!(
            "{} Failed to fetch packages.\n{}\n  \
               Hint: Try again, check your registry/cache settings, or run with {} for the full trace.",
            err_prefix,
            format_details(error, color_state),
            Style::Info.paint(INFO_ARG, color_state)
        );
    }

    // Linking errors
    if top.contains("failed to link") || top.contains("failed to symlink") {
        if top.contains("permission") || top.contains("denied") {
            return format!(
                "{} Permission denied when creating symlinks.\n  \
                   On Windows, try running as Administrator or enable Developer Mode.",
                err_prefix
            );
        }
        return format!(
            "{} Failed to create node_modules structure.\n  \
               {}",
            err_prefix,
            Style::Error.paint(&top, color_state)
        );
    }

    // Store errors
    if top.contains("failed to open store") {
        return format!(
            "{} Cannot access the package store.\n  \
               Check that the store directory is writable.\n  \
               {}",
            err_prefix,
            Style::Error.paint(&top, color_state)
        );
    }
    if top.contains("store verification failed") || top.contains("store verify") {
        return format!(
            "{} Store integrity check failed.\n  \
               Run 'orix store verify' for details,\n  \
               or 'orix store prune' to clean up corrupted entries.",
            err_prefix
        );
    }

    // Offline mode
    if top.contains("offline") {
        return format!(
            "{} Offline mode: required packages not in cache.\n  \
               Run 'orix install' without --offline first.",
            err_prefix
        );
    }

    // Script errors
    if top.contains("script") && (top.contains("not found") || top.contains("failed")) {
        if top.contains("not found") {
            return format!(
                "{} Script not found.\n  \
                   Hint: Check your package.json scripts field for available scripts.",
                err_prefix
            );
        }
        return format!(
            "{} Script failed.\n  {}",
            err_prefix,
            Style::Error.paint(&top, color_state)
        );
    }
    if top.contains("disabled by --ignore-scripts") {
        return format!(
            "{} Lifecycle scripts are disabled.\n  \
               Run without --ignore-scripts to enable script execution.",
            err_prefix
        );
    }

    // Generic fallback
    format!(
        "{} An error occurred:\n{}\n\n\
           Run with {} for more details.",
        err_prefix,
        format_details(error, color_state),
        Style::Info.paint(INFO_ARG, color_state)
    )
}

fn format_details(error: &anyhow::Error, color_state: ColorState) -> String {
    error
        .chain()
        .map(|cause| {
            cause
                .to_string()
                .lines()
                .map(|line| {
                    let padded = format!("  {}", line);
                    if color_state == ColorState::Enabled {
                        Style::Muted.paint(&padded, color_state)
                    } else {
                        padded
                    }
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .collect::<Vec<_>>()
        .join("\n  caused by: ")
}

/// Attempts to extract a package name from an error message.
fn extract_package_name(msg: &str) -> String {
    for (delim, offset) in [
        ("package '", 9),
        ("Package '", 9),
        ("fetch packument for '", 20),
        ("' not found", 11),
        ("/resolve/", 8),
    ] {
        if let Some(pos) = msg.find(delim) {
            let rest = &msg[pos + offset..];
            if let Some(end) = rest.find(['\'', '/', ' ']) {
                return rest[..end].to_string();
            }
            if let Some(end) = rest.find('\'') {
                return rest[..end].to_string();
            }
        }
    }
    "unknown".to_string()
}

const EMOJI_ERR: &str = "\u{26A0}";
const INFO_ARG: &str = "ORIX_LOG=debug";
