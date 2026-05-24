//! Core error types.

use thiserror::Error;

/// Unified error types for the core crate.
#[derive(Debug, Error)]
pub enum CoreError {
    /// Manifest (package.json) error.
    #[error("manifest error: {0}")]
    Manifest(String),

    /// Dependency resolution error.
    #[error("resolution error: {0}")]
    Resolution(String),

    /// Package fetch error.
    #[error("fetch error: {0}")]
    Fetch(String),

    /// Store operation error.
    #[error("store error: {0}")]
    Store(String),

    /// Linking error.
    #[error("link error: {0}")]
    Link(String),

    /// Lockfile error.
    #[error("lockfile error: {0}")]
    Lockfile(String),

    /// Workspace error.
    #[error("workspace error: {0}")]
    Workspace(String),
}
