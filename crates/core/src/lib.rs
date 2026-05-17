//! Install pipeline orchestration for orix.

pub use crate::pipeline::{
    add, install, remove, store_path, store_path_with_overrides, store_prune,
    store_prune_with_overrides, store_verify, store_verify_with_overrides, DepType, InstallEvent,
    InstallOpts, InstallReport, RemoveReport,
};

pub mod error;
pub mod pipeline;

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
