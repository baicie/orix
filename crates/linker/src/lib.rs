//! node_modules/.orix structure and symlink/hardlink generation.

mod linker;
mod linker_platform;

#[cfg(test)]
mod tests;

pub use linker::Linker;

use serde::{Deserialize, Serialize};

/// Optional callback invoked as each package is linked (`done`, `total`, package name).
pub type LinkProgressCallback<'a> = Option<&'a mut dyn FnMut(usize, usize, &str)>;

/// Report from a link operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinkReport {
    /// Number of files hardlinked.
    pub hardlinked_files: u64,
    /// Number of files copied (fallback).
    pub copied_files: u64,
    /// Number of symlinks created.
    pub symlinks_created: u64,
    /// Estimated bytes saved by hardlinking.
    pub bytes_saved: u64,
    /// Whether the link phase was skipped because the layout was already valid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skipped: Option<String>,
}

/// Report from validating a generated node_modules layout.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LayoutReport {
    /// Broken links or missing expected package entries.
    pub broken: Vec<String>,
    /// Non-fatal layout observations.
    pub warnings: Vec<String>,
}

impl LayoutReport {
    /// Returns true when no broken layout entries were found.
    pub fn is_ok(&self) -> bool {
        self.broken.is_empty()
    }
}
