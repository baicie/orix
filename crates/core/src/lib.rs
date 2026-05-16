//! Install pipeline orchestration for orix.

pub use crate::pipeline::{add, remove, DepType, InstallOpts, InstallReport, RemoveReport};

pub mod error;
pub mod pipeline;

pub use error::CoreError;
pub use orix_config::Config;
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
