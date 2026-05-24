//! Lifecycle script execution.

mod runner;
mod types;
mod util;

pub use runner::ScriptRunner;
pub use types::{LifecycleEvent, ScriptError, ScriptKind, ScriptOutput, VIRTUAL_STORE_DIR};
pub use util::{
    dependency_scripts_allowed, graph_install_order, installed_package_dir, normalize_script_args,
};
