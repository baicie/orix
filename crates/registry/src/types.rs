//! Registry API types.

use std::collections::HashMap;

use serde::de::IgnoredAny;
use serde::{Deserialize, Deserializer};

/// The full packument for a package — metadata for all versions plus dist-tags.
#[derive(Deserialize, Debug, Clone)]
pub struct Packument {
    /// Package name.
    pub name: String,
    /// Available versions keyed by version string.
    pub versions: HashMap<String, PackageMetadata>,
    /// Distribution tags (e.g., latest, next, beta).
    #[serde(default)]
    pub dist_tags: HashMap<String, String>,
}

/// Metadata for a single published version of a package.
///
/// serde defaults to `deny_unknown_fields = false`, so any fields present in the
/// npm packument but not listed here are silently ignored. This avoids the
/// hit-and-miss cycle of adding fields every time a new npm extension appears.
#[derive(Deserialize, Debug, Clone)]
pub struct PackageMetadata {
    /// Package name.
    pub name: String,
    /// Version string.
    pub version: String,
    /// Regular dependencies.
    #[serde(default)]
    pub dependencies: HashMap<String, String>,
    /// Development dependencies.
    #[serde(default)]
    pub dev_dependencies: HashMap<String, String>,
    /// Optional dependencies.
    #[serde(default)]
    pub optional_dependencies: HashMap<String, String>,
    /// Peer dependencies.
    #[serde(default)]
    pub peer_dependencies: HashMap<String, String>,
    /// Peer dependencies metadata (e.g., optional: true).
    #[serde(default)]
    pub peer_dependencies_meta: HashMap<String, PeerDepMeta>,
    /// Engine constraints.
    #[serde(default, deserialize_with = "deserialize_engines")]
    pub engines: Option<Engines>,
    /// Supported operating systems.
    #[serde(default)]
    pub os: Vec<String>,
    /// Supported CPU architectures.
    #[serde(default)]
    pub cpu: Vec<String>,
    /// Distribution info (tarball URL, integrity, shasum).
    #[serde(default)]
    pub dist: Option<Dist>,
    /// Whether this version is marked optional.
    #[serde(default)]
    pub optional: bool,
    /// Deprecation message, if any.
    #[serde(default)]
    pub deprecated: Option<String>,
    /// Bin entries for CLI commands.
    #[serde(default, deserialize_with = "deserialize_bin")]
    pub bin: HashMap<String, String>,
    /// Directories map.
    #[serde(default)]
    pub directories: Directories,
    /// Whether a shrinkwrap is present.
    #[serde(default)]
    pub has_shrinkwrap: bool,
    /// Whether an install script is present.
    #[serde(default)]
    pub has_install_script: bool,
    /// Bundled dependencies.
    #[serde(default)]
    pub bundle_dependencies: Vec<String>,
    /// Scripts map.
    #[serde(default)]
    pub scripts: HashMap<String, String>,
    /// Funding info.
    #[serde(default)]
    pub funding: Option<serde_json::Value>,
    /// Repository info.
    #[serde(default)]
    pub repository: Option<serde_json::Value>,
    /// Homepage URL.
    #[serde(default)]
    pub homepage: Option<String>,
    /// Package description.
    #[serde(default)]
    pub description: Option<String>,
    /// License (SPDX identifier).
    #[serde(default)]
    pub license: Option<String>,
    /// Keywords.
    #[serde(default)]
    pub keywords: Vec<String>,
}

/// Metadata for peer dependency optional marker.
#[derive(Deserialize, Debug, Clone, Default)]
pub struct PeerDepMeta {
    /// Whether this peer dependency is optional.
    #[serde(default)]
    pub optional: bool,
}

/// Node/npm engine constraints.
#[derive(Deserialize, Debug, Clone, Default)]
pub struct Engines {
    /// Minimum Node version constraint.
    #[serde(default)]
    pub node: Option<String>,
    /// Minimum npm version constraint.
    #[serde(default)]
    pub npm: Option<String>,
}

/// Directories map (lib, bin, doc, etc.).
#[derive(Deserialize, Debug, Clone, Default)]
pub struct Directories {
    #[serde(default)]
    pub lib: Option<String>,
    #[serde(default)]
    pub bin: Option<String>,
}

/// Deserialize `bin` field — handles both shorthand string and map forms.
///
/// npm allows `"bin": "./path/to/bin.js"` as a shorthand when the package has
/// exactly one command (which defaults to `package.name`). The full form is
/// `bin: { "cmd": "./path/to/bin.js" }`.
fn deserialize_bin<'de, D>(deserializer: D) -> Result<HashMap<String, String>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum BinField {
        Map(HashMap<String, String>),
        String(String),
    }

    match BinField::deserialize(deserializer)? {
        BinField::Map(m) => Ok(m),
        BinField::String(s) => Ok(HashMap::from([(String::new(), s)])),
    }
}

/// Distribution info for a published package version.
#[derive(Deserialize, Debug, Clone, Default)]
pub struct Dist {
    /// URL to the tarball.
    pub tarball: String,
    /// Integrity hash (sha512/sha1).
    #[serde(default)]
    pub integrity: Option<String>,
    /// SHA1 shasum (legacy, superseded by integrity).
    #[serde(default)]
    pub shasum: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum EnginesField {
    Object(Engines),
    Other(IgnoredAny),
}

fn deserialize_engines<'de, D>(deserializer: D) -> Result<Option<Engines>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(match Option::<EnginesField>::deserialize(deserializer)? {
        Some(EnginesField::Object(engines)) => Some(engines),
        Some(EnginesField::Other(_)) | None => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_metadata_ignores_legacy_array_engines() -> anyhow::Result<()> {
        let metadata: PackageMetadata = serde_json::from_str(
            r#"{
                "name": "jsdom",
                "version": "0.1.13",
                "engines": ["v8", "ejs", "node", "rhino"],
                "dist": { "tarball": "https://registry.example/jsdom.tgz" }
            }"#,
        )?;

        assert!(metadata.engines.is_none());
        Ok(())
    }

    #[test]
    fn package_metadata_reads_node_engine_object() -> anyhow::Result<()> {
        let metadata: PackageMetadata = serde_json::from_str(
            r#"{
                "name": "jsdom",
                "version": "27.3.0",
                "engines": { "node": "^20.19.0 || ^22.12.0 || >=24.0.0" },
                "dist": { "tarball": "https://registry.example/jsdom.tgz" }
            }"#,
        )?;

        assert_eq!(
            metadata.engines.and_then(|engines| engines.node),
            Some("^20.19.0 || ^22.12.0 || >=24.0.0".to_string())
        );
        Ok(())
    }

    #[test]
    fn package_metadata_works_without_dist() -> anyhow::Result<()> {
        // Some abbreviated packument versions may omit dist on optional deps.
        let metadata: PackageMetadata = serde_json::from_str(
            r#"{
                "name": "@types/node",
                "version": "20.0.0",
                "optionalDependencies": {
                    "fsevents": "2.3.0"
                }
            }"#,
        )?;

        assert!(metadata.dist.is_none());
        Ok(())
    }

    #[test]
    fn package_metadata_deserializes_bin_shorthand_string() -> anyhow::Result<()> {
        // npm allows "bin": "./path/to/bin.js" as shorthand.
        let metadata: PackageMetadata = serde_json::from_str(
            r#"{
                "name": "@babel/parser",
                "version": "7.24.0",
                "bin": "./bin/babel-parser.js",
                "dist": { "tarball": "https://registry.example/babel-parser.tgz" }
            }"#,
        )?;

        assert_eq!(metadata.bin.len(), 1);
        let (_, path) = metadata
            .bin
            .iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("bin entry should exist"))?;
        assert_eq!(path, "./bin/babel-parser.js");
        Ok(())
    }

    #[test]
    fn package_metadata_ignores_unknown_fields() -> anyhow::Result<()> {
        // Unknown fields must be silently ignored (serde default behavior).
        let metadata: PackageMetadata = serde_json::from_str(
            r#"{
                "name": "some-pkg",
                "version": "1.0.0",
                "dist": { "tarball": "https://registry.example/pkg.tgz" },
                "unknownField": "hello",
                "anotherUnknown": { "nested": true },
                "playwrightMagicField": "value"
            }"#,
        )?;

        assert_eq!(metadata.name, "some-pkg");
        assert_eq!(metadata.version, "1.0.0");
        Ok(())
    }
}
