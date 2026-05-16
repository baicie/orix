//! Monorepo workspace support.

#![deny(clippy::unwrap_used)]

mod workspace;

pub use workspace::{Workspace, WorkspacePackage};

use std::path::PathBuf;

/// A workspace protocol reference parsed from package.json.
/// Examples: "workspace:*", "workspace:^1.0.0", "workspace:file:./packages/foo"
#[derive(Debug, Clone)]
pub struct WorkspaceSpec {
    /// Package name (for workspace:name references).
    pub name: Option<String>,
    /// File path (for workspace:file: references).
    pub path: PathBuf,
}

impl WorkspaceSpec {
    /// Parse a workspace protocol specifier.
    pub fn parse(spec: &str) -> Self {
        let spec = spec.trim();
        if let Some(path) = spec.strip_prefix("workspace:file:") {
            Self {
                name: None,
                path: PathBuf::from(path),
            }
        } else if let Some(name) = spec.strip_prefix("workspace:") {
            Self {
                name: Some(name.to_string()),
                path: PathBuf::new(),
            }
        } else {
            Self {
                name: Some(spec.to_string()),
                path: PathBuf::new(),
            }
        }
    }
}
