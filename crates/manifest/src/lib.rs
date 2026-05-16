//! package.json parsing and validation.

#![deny(clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

pub use rpnpm_domain::{PackageName, Version, VersionConstraint};

/// A parsed package.json manifest.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Manifest {
    /// Package name (e.g., "my-package").
    pub name: Option<String>,
    /// Package version (e.g., "1.0.0").
    pub version: Option<String>,
    /// Package description.
    #[serde(default)]
    pub description: Option<String>,
    /// Production dependencies.
    #[serde(default)]
    pub dependencies: BTreeMap<String, String>,
    /// Development dependencies.
    #[serde(default)]
    pub dev_dependencies: BTreeMap<String, String>,
    /// Peer dependencies.
    #[serde(default)]
    pub peer_dependencies: BTreeMap<String, String>,
    /// Optional dependencies.
    #[serde(default)]
    pub optional_dependencies: BTreeMap<String, String>,
    /// Lifecycle scripts.
    #[serde(default)]
    pub scripts: BTreeMap<String, String>,
    /// Engine constraints.
    #[serde(default)]
    pub engines: Option<Engines>,
    /// Supported operating systems.
    #[serde(default)]
    pub os: Vec<String>,
    /// Supported CPU architectures.
    #[serde(default)]
    pub cpu: Vec<String>,
    /// Bin entries: maps CLI command names to their entry point paths.
    #[serde(default)]
    pub bin: BinField,
    /// Files included in the package (for validation).
    #[serde(default)]
    pub files: Vec<String>,
    /// Whether this is a private package.
    #[serde(default)]
    pub private: Option<bool>,
    /// Type module ("module" or "commonjs").
    #[serde(rename = "type", default)]
    pub module_type: Option<String>,
}

/// Node/npm engine constraints.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Engines {
    /// Minimum Node version (e.g., ">=14").
    #[serde(default)]
    pub node: Option<String>,
}

/// The `bin` field of package.json. Can be a string (shorthand) or a map of names to paths.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(untagged)]
pub enum BinField {
    /// Shorthand: the package name maps directly to this path.
    #[default]
    Empty,
    /// A single bin entry (shorthand form: `bin: "./bin/cli.js"`).
    Shorthand(String),
    /// Multiple bin entries (normal form: `bin: { "my-tool": "./bin/cli.js" }`).
    Map(BTreeMap<String, String>),
}

impl BinField {
    /// Returns all bin entries as (command_name, file_path) pairs.
    pub fn entries(&self, pkg_name: &str) -> Vec<(String, String)> {
        match self {
            BinField::Empty => Vec::new(),
            BinField::Shorthand(path) => vec![(pkg_name.to_string(), path.clone())],
            BinField::Map(m) => m.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
        }
    }

    /// Returns true if the bin field is empty / not set.
    pub fn is_empty(&self) -> bool {
        matches!(self, BinField::Empty)
    }
}

impl Manifest {
    /// Read and parse a package.json file.
    pub fn read(path: &Path) -> Result<Self> {
        let source = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        serde_json::from_str(&source).with_context(|| format!("failed to parse {}", path.display()))
    }

    /// Write the manifest back to a package.json file.
    pub fn write(&self, path: &Path) -> Result<()> {
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(path, json)
            .with_context(|| format!("failed to write {}", path.display()))?;
        Ok(())
    }

    /// Returns true if this manifest has any declared dependencies.
    pub fn has_dependencies(&self) -> bool {
        !self.dependencies.is_empty()
            || !self.dev_dependencies.is_empty()
            || !self.optional_dependencies.is_empty()
    }

    /// Get the resolved name as a PackageName, if present.
    pub fn name_as_pkg_name(&self) -> Option<PackageName> {
        self.name.as_ref().map(|n| PackageName::from(n.as_str()))
    }

    /// Get the resolved version as a Version, if present.
    pub fn version_as_version(&self) -> Option<Version> {
        self.version.as_ref().and_then(|v| Version::parse(v).ok())
    }

    /// Resolve the bin path for a given command name, given the package root directory.
    /// Returns the absolute path to the bin executable.
    pub fn resolve_bin(&self, cmd_name: &str, package_root: &Path) -> Option<PathBuf> {
        let bin_entries = self.bin.entries(self.name.as_deref().unwrap_or(""));
        bin_entries
            .into_iter()
            .find(|(name, _)| name == cmd_name)
            .map(|(_, path)| package_root.join(path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::expect_used)]
    fn test_read_empty_manifest() {
        let tmp = tempfile::NamedTempFile::with_suffix(".json")
            .expect("tempfile creation should succeed");
        std::fs::write(tmp.path(), r#"{}"#).expect("write should succeed");
        let m = Manifest::read(tmp.path()).expect("manifest read should succeed");
        assert!(m.name.is_none());
        assert!(m.version.is_none());
        assert!(m.dependencies.is_empty());
    }

    #[test]
    #[allow(clippy::expect_used)]
    fn test_roundtrip() {
        let manifest = Manifest {
            name: Some("my-pkg".into()),
            version: Some("1.0.0".into()),
            dependencies: BTreeMap::from([("react".into(), "^18.0.0".into())]),
            bin: BinField::Map(BTreeMap::from([("my-tool".into(), "./bin/cli.js".into())])),
            ..Default::default()
        };
        let json = serde_json::to_string_pretty(&manifest).expect("serialization should succeed");
        let reparsed: Manifest =
            serde_json::from_str(&json).expect("deserialization should succeed");
        assert_eq!(reparsed.name, manifest.name);
        assert_eq!(reparsed.dependencies, manifest.dependencies);
        assert!(!reparsed.bin.is_empty());
    }

    #[test]
    fn test_bin_field_shorthand() {
        let shorthand = BinField::Shorthand("./bin/cli.js".into());
        let entries = shorthand.entries("my-pkg");
        assert_eq!(entries, vec![("my-pkg".into(), "./bin/cli.js".into())]);
    }

    #[test]
    fn test_bin_field_map() {
        let map = BinField::Map(BTreeMap::from([
            ("tool-a".into(), "./bin/a.js".into()),
            ("tool-b".into(), "./bin/b.js".into()),
        ]));
        let entries = map.entries("my-pkg");
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().any(|(k, _)| k == "tool-a"));
        assert!(entries.iter().any(|(k, _)| k == "tool-b"));
    }
}
