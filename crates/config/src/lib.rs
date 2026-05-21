//! Configuration loading from defaults, .npmrc files, and environment variables.

#![deny(clippy::unwrap_used, clippy::field_reassign_with_default)]

mod load;
mod platform;
mod types;

#[cfg(test)]
mod tests;

pub use orix_domain::PackageName;
pub use types::{ColorChoice, Config, ConfigOverrides};
