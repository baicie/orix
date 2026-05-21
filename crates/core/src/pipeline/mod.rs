//! Install pipeline orchestration.

mod add_remove;
mod deploy;
mod fetch;
mod install;
mod lifecycle;
mod lockfile_io;
mod prelude;
mod store_cmd;
mod types;

pub use add_remove::{add, remove, DepType};
pub use deploy::{deploy, DeployOpts, DeployReport};
pub use install::install;
pub use lockfile_io::{export_pnpm_lockfile, import_pnpm_lockfile, ExportReport, ImportReport};
pub use store_cmd::{
    cache_clean, cache_clean_with_overrides, cache_path, cache_path_with_overrides, store_path,
    store_path_with_overrides, store_prune, store_prune_with_overrides, store_verify,
    store_verify_with_overrides,
};
pub use types::{CacheCleanReport, InstallOpts, InstallReport, LockfileDiffReport, RemoveReport};
