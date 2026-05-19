//! Human-readable error messages for CLI output.

use std::path::Path;

/// Categorizes common error patterns into user-friendly messages.
pub fn format_error(error: &anyhow::Error, project_root: &Path) -> String {
    let cause_chain = error.chain().collect::<Vec<_>>();
    let top = cause_chain
        .first()
        .map(|e| e.to_string())
        .unwrap_or_default();

    // Workspace errors
    if (top.contains("workspace") || top.contains("pnpm-workspace"))
        && (top.contains("failed to read") || top.contains("parse"))
    {
        return format!(
            "{} Failed to read workspace configuration.\n  \
               Check pnpm-workspace.yaml for YAML syntax errors.\n  \
               {}",
            EMOJI_ERR, top
        );
    }

    // Manifest errors
    if top.contains("failed to read") && top.contains("package.json") {
        return format!(
            "{} No package.json found in {}.\n  \
               Hint: Run this command in a directory containing a package.json,\n  \
               or use -C <path> to specify the project directory.",
            EMOJI_ERR,
            project_root.display()
        );
    }
    if top.contains("failed to parse") && top.contains("package.json") {
        return format!(
            "{} Invalid package.json: {}\n  \
               Hint: Check the JSON syntax of your package.json.",
            EMOJI_ERR, top
        );
    }

    // Lockfile errors
    if top.contains("frozen lockfile validation failed") {
        return format!(
            "{} Lockfile mismatch.\n  \
               Your package.json dependencies have changed but the lockfile\n  \
               wasn't updated. Run 'orix install' without --frozen-lockfile first.",
            EMOJI_ERR
        );
    }
    if top.contains("No lockfile found") {
        return format!(
            "{} No lockfile found.\n  \
               Run 'orix install' without --frozen-lockfile to generate one.",
            EMOJI_ERR
        );
    }

    // Resolution errors
    if top.contains("failed to resolve dependencies") {
        if top.contains("workspace") {
            return format!(
                "{} Failed to resolve workspace dependencies.\n  \
                   Check that all packages referenced in pnpm-workspace.yaml exist.\n  \
                   {}",
                EMOJI_ERR, top
            );
        }
        return format!(
            "{} Failed to resolve dependencies.\n  \
               Check your package.json for typos and valid version ranges.\n  \
               {}",
            EMOJI_ERR,
            format_details(error)
        );
    }

    // Registry / fetch errors
    if top.contains("failed to fetch") || top.contains("fetch packument") {
        if top.contains("not found") || top.contains("404") {
            let pkg = extract_package_name(&top);
            return format!(
                "{} Package '{}' not found in the registry.\n  \
                   Check the package name and version in package.json.",
                EMOJI_ERR, pkg
            );
        }
        if top.contains("network") || top.contains("connection") || top.contains("timeout") {
            return format!(
                "{} Network error while fetching packages.\n  \
                   Check your internet connection and registry URL.\n  \
                   Run 'orix install --offline' to use cached packages only.\n  \
                   {}",
                EMOJI_ERR, top
            );
        }
        if top.contains("403") || top.contains("unauthorized") || top.contains("authentication") {
            return format!(
                "{} Authentication failed for the registry.\n  \
                   Set your token with: npm config set //registry.npmjs.org/:_authToken YOUR_TOKEN\n  \
                   Or set the RPNPM_REGISTRY_TOKEN environment variable.",
                EMOJI_ERR
            );
        }
        return format!(
            "{} Failed to fetch packages.\n{}\n  \
               Hint: Try again, check your registry/cache settings, or run with {} for the full trace.",
            EMOJI_ERR,
            format_details(error),
            INFO_ARG
        );
    }

    // Linking errors
    if top.contains("failed to link") || top.contains("failed to symlink") {
        if top.contains("permission") || top.contains("denied") {
            return format!(
                "{} Permission denied when creating symlinks.\n  \
                   On Windows, try running as Administrator or enable Developer Mode.",
                EMOJI_ERR
            );
        }
        return format!(
            "{} Failed to create node_modules structure.\n  \
               {}",
            EMOJI_ERR, top
        );
    }

    // Store errors
    if top.contains("failed to open store") {
        return format!(
            "{} Cannot access the package store.\n  \
               Check that the store directory is writable.\n  \
               {}",
            EMOJI_ERR, top
        );
    }
    if top.contains("store verification failed") || top.contains("store verify") {
        return format!(
            "{} Store integrity check failed.\n  \
               Run 'orix store verify' for details,\n  \
               or 'orix store prune' to clean up corrupted entries.",
            EMOJI_ERR
        );
    }

    // Offline mode
    if top.contains("offline") {
        return format!(
            "{} Offline mode: required packages not in cache.\n  \
               Run 'orix install' without --offline first.",
            EMOJI_ERR
        );
    }

    // Script errors
    if top.contains("script") && (top.contains("not found") || top.contains("failed")) {
        if top.contains("not found") {
            return format!(
                "{} Script not found.\n  \
                   Hint: Check your package.json scripts field for available scripts.",
                EMOJI_ERR
            );
        }
        return format!("{} Script failed.\n  {}", EMOJI_ERR, top);
    }
    if top.contains("disabled by --ignore-scripts") {
        return format!(
            "{} Lifecycle scripts are disabled.\n  \
               Run without --ignore-scripts to enable script execution.",
            EMOJI_ERR
        );
    }

    // Generic fallback
    format!(
        "{} An error occurred:\n{}\n\n\
           Run with {} for more details.",
        EMOJI_ERR,
        format_details(error),
        INFO_ARG
    )
}

fn format_details(error: &anyhow::Error) -> String {
    error
        .chain()
        .map(|cause| {
            cause
                .to_string()
                .lines()
                .map(|line| format!("  {}", line))
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
