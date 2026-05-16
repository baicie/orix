//! node_modules/.pnpm structure and symlink/hardlink generation.

mod linker;

pub use linker::Linker;

use serde::{Deserialize, Serialize};

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
}
