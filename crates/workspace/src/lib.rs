//! Monorepo workspace support.

#![deny(clippy::unwrap_used)]

mod workspace;

pub use workspace::{
    detect_workspace_cycles, filter_workspace_packages, Catalog, CatalogSpec, Workspace,
    WorkspacePackage, WorkspaceSelector,
};

use std::path::PathBuf;

/// A workspace protocol reference parsed from package.json.
/// Examples: "workspace:*", "workspace:^1.0.0", "workspace:file:./packages/foo"
#[derive(Debug, Clone)]
pub struct WorkspaceSpec {
    /// Bare package name without version constraint.
    pub name: Option<String>,
    /// Full version constraint string (e.g., "^1.0.0", ">=2.0.0", "*").
    pub constraint: Option<String>,
    /// File path (for workspace:file: references).
    pub path: PathBuf,
    /// Whether this was parsed with the `workspace:` protocol prefix.
    has_workspace_prefix: bool,
}

/// Known workspace protocol constraint prefixes.
const WORKSPACE_PREFIXES: &[&str] = &["workspace:^", "workspace:~", "workspace:>=", "workspace:<="];

impl WorkspaceSpec {
    /// Parse a workspace protocol specifier.
    ///
    /// Supports:
    /// - `workspace:*` — use exact version from local package.json
    /// - `workspace:^1.0.0` — apply caret range to local version
    /// - `workspace:~1.2.3` — apply tilde range to local version
    /// - `workspace:>=1.0.0` — apply gte range to local version
    /// - `workspace:file:../utils` — link from local file path
    /// - `workspace:@scope/pkg` — bare workspace reference (uses local version)
    pub fn parse(spec: &str) -> Self {
        let spec = spec.trim();

        // workspace:file:... — link from local file path
        if let Some(path) = spec.strip_prefix("workspace:file:") {
            return Self {
                name: None,
                constraint: None,
                path: PathBuf::from(path),
                has_workspace_prefix: true,
            };
        }

        // workspace:^, workspace:~, workspace:>=, workspace:<= — constraint variant
        for prefix in WORKSPACE_PREFIXES {
            if let Some(rest) = spec.strip_prefix(prefix) {
                let constraint_part = rest;
                let full_constraint = &prefix["workspace:".len()..];
                let version = constraint_part
                    .strip_prefix('^')
                    .or_else(|| constraint_part.strip_prefix('~'))
                    .or_else(|| constraint_part.strip_prefix(">="))
                    .or_else(|| constraint_part.strip_prefix("<="))
                    .unwrap_or(constraint_part);
                return Self {
                    name: Some(version.to_string()),
                    constraint: Some(format!("{}{}", full_constraint, version)),
                    path: PathBuf::new(),
                    has_workspace_prefix: true,
                };
            }
        }

        // workspace:* — bare workspace reference
        if let Some(after) = spec.strip_prefix("workspace:") {
            if after == "*" {
                return Self {
                    name: None,
                    constraint: Some("*".to_string()),
                    path: PathBuf::new(),
                    has_workspace_prefix: true,
                };
            }
            // workspace:@scope/pkg — bare name reference
            return Self {
                name: Some(after.to_string()),
                constraint: None,
                path: PathBuf::new(),
                has_workspace_prefix: true,
            };
        }

        // Plain name (no workspace: prefix)
        Self {
            name: Some(spec.to_string()),
            constraint: None,
            path: PathBuf::new(),
            has_workspace_prefix: false,
        }
    }

    /// Returns true if this is a workspace protocol reference.
    pub fn is_workspace_spec(&self) -> bool {
        self.has_workspace_prefix
    }

    /// Returns the version constraint to use, or "*" if none specified.
    pub fn version_constraint(&self) -> &str {
        match self.constraint.as_deref() {
            Some(c) if c.starts_with("^") => &c[1..],
            Some(c) if c.starts_with("~") => &c[1..],
            Some(c) if c.starts_with(">=") => &c[2..],
            Some(c) if c.starts_with("<=") => &c[2..],
            Some("*") | None => "*",
            Some(c) => c,
        }
    }
}

#[cfg(test)]
mod workspace_spec_tests {
    use super::*;

    #[test]
    fn parse_workspace_star() {
        let spec = WorkspaceSpec::parse("workspace:*");
        assert!(spec.name.is_none());
        assert_eq!(spec.constraint, Some("*".to_string()));
        assert!(spec.path.as_os_str().is_empty());
        assert_eq!(spec.version_constraint(), "*");
    }

    #[test]
    fn parse_workspace_caret() {
        let spec = WorkspaceSpec::parse("workspace:^1.0.0");
        assert_eq!(spec.name, Some("1.0.0".to_string()));
        assert_eq!(spec.constraint, Some("^1.0.0".to_string()));
        assert_eq!(spec.version_constraint(), "1.0.0");
    }

    #[test]
    fn parse_workspace_tilde() {
        let spec = WorkspaceSpec::parse("workspace:~1.2.3");
        assert_eq!(spec.name, Some("1.2.3".to_string()));
        assert_eq!(spec.constraint, Some("~1.2.3".to_string()));
        assert_eq!(spec.version_constraint(), "1.2.3");
    }

    #[test]
    fn parse_workspace_gte() {
        let spec = WorkspaceSpec::parse("workspace:>=1.0.0");
        assert_eq!(spec.name, Some("1.0.0".to_string()));
        assert_eq!(spec.constraint, Some(">=1.0.0".to_string()));
        assert_eq!(spec.version_constraint(), "1.0.0");
    }

    #[test]
    fn parse_workspace_file() {
        let spec = WorkspaceSpec::parse("workspace:file:../utils");
        assert!(spec.name.is_none());
        assert!(spec.constraint.is_none());
        assert_eq!(spec.path, PathBuf::from("../utils"));
    }

    #[test]
    fn parse_workspace_bare() {
        let spec = WorkspaceSpec::parse("workspace:@scope/pkg");
        assert_eq!(spec.name, Some("@scope/pkg".to_string()));
        assert!(spec.constraint.is_none());
    }

    #[test]
    fn parse_plain_name() {
        let spec = WorkspaceSpec::parse("@scope/pkg");
        assert_eq!(spec.name, Some("@scope/pkg".to_string()));
    }

    #[test]
    fn is_workspace_spec_recognizes_workspace_protocol() {
        assert!(WorkspaceSpec::parse("workspace:*").is_workspace_spec());
        assert!(WorkspaceSpec::parse("workspace:^1.0.0").is_workspace_spec());
        assert!(WorkspaceSpec::parse("workspace:file:../utils").is_workspace_spec());
        assert!(WorkspaceSpec::parse("workspace:@scope/pkg").is_workspace_spec());
        assert!(!WorkspaceSpec::parse("@scope/pkg").is_workspace_spec());
    }

    #[test]
    fn version_constraint_defaults_to_star() {
        assert_eq!(WorkspaceSpec::parse("@scope/pkg").version_constraint(), "*");
        assert_eq!(
            WorkspaceSpec::parse("workspace:*").version_constraint(),
            "*"
        );
        assert_eq!(
            WorkspaceSpec::parse("workspace:^1.0.0").version_constraint(),
            "1.0.0"
        );
        assert_eq!(
            WorkspaceSpec::parse("workspace:~1.0.0").version_constraint(),
            "1.0.0"
        );
        assert_eq!(
            WorkspaceSpec::parse("workspace:>=1.0.0").version_constraint(),
            "1.0.0"
        );
    }
}
