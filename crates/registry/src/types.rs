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
    #[serde(rename = "devDependencies", default)]
    pub dev_dependencies: HashMap<String, String>,
    /// Optional dependencies.
    #[serde(rename = "optionalDependencies", default)]
    pub optional_dependencies: HashMap<String, String>,
    /// Peer dependencies.
    #[serde(rename = "peerDependencies", default)]
    pub peer_dependencies: HashMap<String, String>,
    /// Peer dependencies metadata (e.g., optional: true).
    #[serde(rename = "peerDependenciesMeta", default)]
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
    #[serde(default, deserialize_with = "deserialize_optional_string")]
    pub deprecated: Option<String>,
    /// Bin entries for CLI commands.
    #[serde(default, deserialize_with = "deserialize_bin")]
    pub bin: HashMap<String, String>,
    /// Directories map.
    #[serde(default)]
    pub directories: Directories,
    /// Whether a shrinkwrap is present.
    #[serde(rename = "hasShrinkwrap", default)]
    pub has_shrinkwrap: bool,
    /// Whether an install script is present.
    #[serde(rename = "hasInstallScript", default)]
    pub has_install_script: bool,
    /// Bundled dependencies.
    #[serde(
        rename = "bundleDependencies",
        alias = "bundledDependencies",
        default,
        deserialize_with = "deserialize_bundle_dependencies"
    )]
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
    #[serde(default, deserialize_with = "deserialize_optional_string")]
    pub homepage: Option<String>,
    /// Package description.
    #[serde(default, deserialize_with = "deserialize_optional_string")]
    pub description: Option<String>,
    /// License (SPDX identifier).
    #[serde(default, deserialize_with = "deserialize_optional_string")]
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

fn deserialize_bundle_dependencies<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum BundleDependenciesField {
        List(Vec<String>),
        One(String),
        Other(IgnoredAny),
    }

    Ok(
        match Option::<BundleDependenciesField>::deserialize(deserializer)? {
            Some(BundleDependenciesField::List(deps)) => deps,
            Some(BundleDependenciesField::One(dep)) => vec![dep],
            Some(BundleDependenciesField::Other(_)) | None => Vec::new(),
        },
    )
}

fn deserialize_optional_string<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OptionalStringField {
        String(String),
        Other(IgnoredAny),
    }

    Ok(
        match Option::<OptionalStringField>::deserialize(deserializer)? {
            Some(OptionalStringField::String(value)) => Some(value),
            Some(OptionalStringField::Other(_)) | None => None,
        },
    )
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
        assert_eq!(
            metadata.optional_dependencies.get("fsevents"),
            Some(&"2.3.0".to_string())
        );
        Ok(())
    }

    #[test]
    fn package_metadata_reads_npm_camel_case_dependency_fields() -> anyhow::Result<()> {
        let metadata: PackageMetadata = serde_json::from_str(
            r#"{
                "name": "rollup",
                "version": "4.60.4",
                "dependencies": {
                    "@types/estree": "1.0.8"
                },
                "devDependencies": {
                    "typescript": "5.9.3"
                },
                "optionalDependencies": {
                    "@rollup/rollup-darwin-arm64": "4.60.4"
                },
                "peerDependencies": {
                    "node-gyp": "*"
                },
                "peerDependenciesMeta": {
                    "node-gyp": {
                        "optional": true
                    }
                },
                "hasShrinkwrap": true,
                "hasInstallScript": true,
                "bundleDependencies": ["bundled-dep"],
                "dist": { "tarball": "https://registry.example/rollup.tgz" }
            }"#,
        )?;

        assert_eq!(
            metadata.dependencies.get("@types/estree"),
            Some(&"1.0.8".to_string())
        );
        assert_eq!(
            metadata.dev_dependencies.get("typescript"),
            Some(&"5.9.3".to_string())
        );
        assert_eq!(
            metadata
                .optional_dependencies
                .get("@rollup/rollup-darwin-arm64"),
            Some(&"4.60.4".to_string())
        );
        assert_eq!(
            metadata.peer_dependencies.get("node-gyp"),
            Some(&"*".to_string())
        );
        assert!(metadata
            .peer_dependencies_meta
            .get("node-gyp")
            .is_some_and(|meta| meta.optional));
        assert!(metadata.has_shrinkwrap);
        assert!(metadata.has_install_script);
        assert_eq!(metadata.bundle_dependencies, vec!["bundled-dep"]);
        Ok(())
    }

    #[test]
    fn package_metadata_accepts_boolean_bundle_dependencies() -> anyhow::Result<()> {
        let metadata: PackageMetadata = serde_json::from_str(
            r#"{
                "name": "rolldown",
                "version": "1.0.0",
                "bundleDependencies": false,
                "dist": { "tarball": "https://registry.example/rolldown.tgz" }
            }"#,
        )?;

        assert!(metadata.bundle_dependencies.is_empty());
        Ok(())
    }

    #[test]
    fn package_metadata_ignores_boolean_optional_text_fields() -> anyhow::Result<()> {
        let metadata: PackageMetadata = serde_json::from_str(
            r#"{
                "name": "react-is",
                "version": "16.8.0-alpha.0",
                "deprecated": false,
                "homepage": false,
                "description": false,
                "license": false,
                "dist": { "tarball": "https://registry.example/react-is.tgz" }
            }"#,
        )?;

        assert!(metadata.deprecated.is_none());
        assert!(metadata.homepage.is_none());
        assert!(metadata.description.is_none());
        assert!(metadata.license.is_none());
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
