//! Install pipeline orchestration for rpnpm.

pub use crate::pipeline::{add, remove, DepType, InstallOpts, InstallReport, RemoveReport};

pub mod error;
pub mod pipeline;

pub use error::CoreError;
pub use rpnpm_config::Config;
pub use rpnpm_domain::{
    DependencyGraph, PackageId, PackageName, ResolvedPackage, Version, VersionConstraint,
};
pub use rpnpm_fetcher::{FetchReport, Fetcher, TarballCache};
pub use rpnpm_linker::{LinkReport, Linker};
pub use rpnpm_lockfile::Lockfile;
pub use rpnpm_manifest::Manifest;
pub use rpnpm_resolver::Resolver;
pub use rpnpm_store::Store;
pub use rpnpm_workspace::Workspace;
