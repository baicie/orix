//! Workspace discovery and management.

mod cycles;
mod discover;
mod types;

#[cfg(test)]
mod tests;

pub use cycles::detect_workspace_cycles;
pub use types::{
    filter_workspace_packages, Catalog, CatalogSpec, Workspace, WorkspacePackage, WorkspaceSelector,
};
