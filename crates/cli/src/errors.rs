//! Human-readable error messages for CLI output.

use std::path::Path;

/// Categorizes common error patterns into user-friendly messages.
pub fn format_error(error: &anyhow::Error, project_root: &Path) -> String {
    let cause_chain = error.chain().collect::<Vec<_>>();

    // Check for manifest errors
    if let Some(e) = cause_chain.first() {
        let msg = e.to_string();
        if msg.contains("failed to read") && msg.contains("package.json") {
            return format!(
                "{} No package.json found in {}.\n  \
                   Hint: Run this command in a directory containing a package.json,\n  \
                   or use -C <path> to specify the project directory.",
                EMOJI_ERR,
                project_root.display()
            );
        }
        if msg.contains("failed to parse") && msg.contains("package.json") {
            return format!(
                "{} Invalid package.json: {}\n  \
                   Hint: Check the JSON syntax of your package.json.",
                EMOJI_ERR, e
            );
        }
        if msg.contains("frozen lockfile validation failed") {
            return format!(
                "{} Lockfile mismatch.\n  \
                   Your package.json dependencies have changed but the lockfile\n  \
                   wasn't updated. Run 'orix install' without --frozen-lockfile first.",
                EMOJI_ERR
            );
        }
        if msg.contains("No lockfile found") {
            return format!(
                "{} No lockfile found.\n  \
                   Run 'orix install' without --frozen-lockfile to generate one.",
                EMOJI_ERR
            );
        }
        if msg.contains("failed to resolve dependencies") {
            return format!(
                "{} Failed to resolve dependencies.\n  \
                   {}",
                EMOJI_ERR, e
            );
        }
        if msg.contains("failed to fetch") {
            if msg.contains("not found") {
                let pkg = extract_package_name(&msg);
                return format!(
                    "{} Package '{}' not found in the registry.\n  \
                       Check the package name and version in package.json.",
                    EMOJI_ERR, pkg
                );
            }
            if msg.contains("network") || msg.contains("connection") {
                return format!(
                    "{} Network error while fetching packages.\n  \
                       Check your internet connection and registry URL.\n  \
                       {}",
                    EMOJI_ERR, e
                );
            }
            return format!("{} Failed to fetch packages: {}", EMOJI_ERR, e);
        }
        if msg.contains("failed to link") {
            return format!(
                "{} Failed to create node_modules structure.\n  \
                   {}",
                EMOJI_ERR, e
            );
        }
        if msg.contains("failed to open store") {
            return format!(
                "{} Cannot access the package store.\n  \
                   {}",
                EMOJI_ERR, e
            );
        }
        if msg.contains("store verification failed") {
            return format!(
                "{} Store integrity check failed.\n  \
                   Run 'orix store verify' for details.",
                EMOJI_ERR
            );
        }
        if msg.contains("offline mode") {
            return format!(
                "{} Offline mode: required packages not in cache.\n  \
                   Run 'orix install' without --offline first.",
                EMOJI_ERR
            );
        }
    }

    // Generic fallback
    format!(
        "{} An error occurred:\n  {}\n\n\
           Run with {} for more details.",
        EMOJI_ERR, error, INFO_ARG
    )
}

fn extract_package_name(msg: &str) -> &str {
    if let Some(pos) = msg.find('\'') {
        let rest = &msg[pos + 1..];
        if let Some(end) = rest.find('\'') {
            return &rest[..end];
        }
    }
    if let Some(pos) = msg.find("package '") {
        let rest = &msg[pos + 9..];
        if let Some(end) = rest.find('\'') {
            return &rest[..end];
        }
    }
    "unknown"
}

const EMOJI_ERR: &str = "\u{26A0}";
const INFO_ARG: &str = "ORIX_LOG=debug";
