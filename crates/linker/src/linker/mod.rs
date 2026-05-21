//! Linker implementation.

mod layout;
mod link_graph;
pub(crate) mod prelude;

use super::linker_platform::*;
use prelude::*;

pub(crate) const VIRTUAL_STORE_DIR: &str = ".orix";
pub(crate) const METADATA_FILE: &str = "metadata.json";
/// Bump when link layout semantics change (forces relink on next install).
pub(crate) const LINK_PROTOCOL_VERSION: u32 = 2;

/// Marker written to node_modules/.orix/metadata.json after a successful link.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct LinkerMarker {
    /// Hash of the dependency graph at link time.
    pub graph_hash: String,
    /// Version of orix that performed the link.
    pub orix_version: String,
    /// Number of packages linked.
    pub package_count: usize,
    /// Layout protocol generation; mismatch invalidates cached layout.
    #[serde(default)]
    pub link_protocol_version: u32,
}

/// The linker creates the Orix virtual node_modules structure using hardlinks and symlinks.
pub struct Linker {
    store: Store,
    node_modules: PathBuf,
}

impl Linker {
    /// Create a new linker.
    pub fn new(store: Store, node_modules: PathBuf) -> Self {
        Self {
            store,
            node_modules,
        }
    }

    /// Path to the marker file.
    fn marker_path(&self) -> PathBuf {
        self.node_modules
            .join(VIRTUAL_STORE_DIR)
            .join(METADATA_FILE)
    }

    /// Read the linker marker, if it exists.
    fn read_marker(&self) -> Option<LinkerMarker> {
        let path = self.marker_path();
        let content = fs::read_to_string(&path).ok()?;
        serde_json::from_str(&content).ok()
    }

    /// Check whether the current layout marker matches the given graph hash.
    /// Returns true if the marker exists and the graph hash matches.
    pub fn is_layout_valid(&self, graph_hash: &str) -> bool {
        match self.read_marker() {
            Some(marker) => {
                marker.graph_hash == graph_hash
                    && marker.link_protocol_version == LINK_PROTOCOL_VERSION
                    && self.bin_shims_are_valid()
            }
            None => false,
        }
    }

    #[cfg(windows)]
    fn bin_shims_are_valid(&self) -> bool {
        let bin_dir = self.node_modules.join(".bin");
        if !bin_dir.is_dir() {
            return true;
        }

        fs::read_dir(&bin_dir)
            .into_iter()
            .flatten()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_type().map(|t| t.is_file()).unwrap_or(false))
            .all(|entry| {
                let path = entry.path();
                if path.extension().is_some_and(|ext| ext == "cmd") {
                    return true;
                }

                path.with_extension("cmd").exists()
            })
    }

    #[cfg(not(windows))]
    fn bin_shims_are_valid(&self) -> bool {
        let bin_dir = self.node_modules.join(".bin");
        if !bin_dir.is_dir() {
            return true;
        }

        fs::read_dir(&bin_dir)
            .into_iter()
            .flatten()
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                entry
                    .file_type()
                    .map(|t| t.is_symlink() || t.is_file())
                    .unwrap_or(false)
            })
            .all(|entry| {
                let executable = fs::metadata(entry.path())
                    .map(|metadata| metadata.mode() & 0o111 != 0)
                    .unwrap_or(false);
                executable && self.bin_shim_points_into_package(entry.path().as_path())
            })
    }

    #[cfg(not(windows))]
    fn bin_shim_points_into_package(&self, shim_path: &Path) -> bool {
        let target = match fs::read_link(shim_path) {
            Ok(target) => target,
            Err(_) => return true,
        };
        let resolved = if target.is_absolute() {
            target
        } else {
            shim_path
                .parent()
                .unwrap_or_else(|| Path::new(""))
                .join(target)
        };
        let resolved = fs::canonicalize(&resolved).unwrap_or(resolved);

        let virtual_store = self.node_modules.join(VIRTUAL_STORE_DIR);
        if !path_starts_with_lexically(&resolved, &virtual_store) {
            return true;
        }

        normal_components(&resolved)
            .iter()
            .any(|part| part == "node_modules")
    }

    /// Write the linker marker after a successful link.
    pub(crate) fn write_marker(&self, graph_hash: &str, package_count: usize) -> Result<()> {
        let marker = LinkerMarker {
            graph_hash: graph_hash.to_string(),
            orix_version: env!("CARGO_PKG_VERSION").to_string(),
            package_count,
            link_protocol_version: LINK_PROTOCOL_VERSION,
        };
        let json = serde_json::to_string_pretty(&marker)?;
        let path = self.marker_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, json)?;
        Ok(())
    }
}
