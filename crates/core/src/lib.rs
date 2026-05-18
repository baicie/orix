//! Install pipeline orchestration for orix.

pub use crate::pipeline::{
    add, cache_clean, cache_clean_with_overrides, cache_path, cache_path_with_overrides, install,
    remove, store_path, store_path_with_overrides, store_prune, store_prune_with_overrides,
    store_verify, store_verify_with_overrides, CacheCleanReport, DepType, InstallOpts,
    InstallReport, RemoveReport,
};

pub mod error;
pub mod pipeline;
pub mod reporter;

pub use error::CoreError;
pub use orix_config::{Config, ConfigOverrides};
pub use orix_domain::{
    DependencyGraph, PackageId, PackageName, ResolvedPackage, Version, VersionConstraint,
};
pub use orix_fetcher::{FetchReport, Fetcher, TarballCache};
pub use orix_linker::{LinkReport, Linker};
pub use orix_lockfile::Lockfile;
pub use orix_manifest::Manifest;
pub use orix_resolver::Resolver;
pub use orix_store::Store;
pub use orix_workspace::Workspace;
