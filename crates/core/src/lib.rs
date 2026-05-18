//! Install pipeline orchestration for orix.

pub use crate::pipeline::{
    add, cache_clean, cache_clean_with_overrides, cache_path, cache_path_with_overrides, deploy,
    export_pnpm_lockfile, import_pnpm_lockfile, install, remove, store_path,
    store_path_with_overrides, store_prune, store_prune_with_overrides, store_verify,
    store_verify_with_overrides, CacheCleanReport, DepType, DeployOpts, DeployReport, ExportReport,
    ImportReport, InstallOpts, InstallReport, RemoveReport,
};

pub mod error;
pub mod pipeline;
pub mod reporter;
pub mod script;

pub use error::CoreError;
pub use orix_config::{Config, ConfigOverrides};
pub use orix_domain::{
    DependencyGraph, PackageId, PackageName, ResolvedPackage, Version, VersionConstraint,
};
pub use orix_fetcher::{FetchReport, Fetcher, TarballCache};
pub use orix_linker::{LinkReport, Linker};
pub use orix_lockfile::{Lockfile, PnpmImportError, PnpmLockfile};
pub use orix_manifest::Manifest;
pub use orix_resolver::Resolver;
pub use orix_store::Store;
pub use orix_workspace::Workspace;

// Script runner exports.
pub use crate::script::{LifecycleEvent, ScriptError, ScriptKind, ScriptOutput, ScriptRunner};
