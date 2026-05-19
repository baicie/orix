//! Registry API types.

use std::collections::HashMap;

use serde::{de::IgnoredAny, Deserialize, Deserializer};

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
    pub dist: Dist,
    /// Whether this version is marked optional.
    #[serde(default)]
    pub optional: bool,
}

/// Node/npm engine constraints.
#[derive(Deserialize, Debug, Clone, Default)]
pub struct Engines {
    /// Minimum Node version constraint.
    #[serde(default)]
    pub node: Option<String>,
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
    fn package_metadata_ignores_legacy_array_engines() {
        let metadata: PackageMetadata = serde_json::from_str(
            r#"{
                "name": "jsdom",
                "version": "0.1.13",
                "engines": ["v8", "ejs", "node", "rhino"],
                "dist": { "tarball": "https://registry.example/jsdom.tgz" }
            }"#,
        )
        .expect("legacy engines array should deserialize");

        assert!(metadata.engines.is_none());
    }

    #[test]
    fn package_metadata_reads_node_engine_object() {
        let metadata: PackageMetadata = serde_json::from_str(
            r#"{
                "name": "jsdom",
                "version": "27.3.0",
                "engines": { "node": "^20.19.0 || ^22.12.0 || >=24.0.0" },
                "dist": { "tarball": "https://registry.example/jsdom.tgz" }
            }"#,
        )
        .expect("engine object should deserialize");

        assert_eq!(
            metadata.engines.and_then(|engines| engines.node),
            Some("^20.19.0 || ^22.12.0 || >=24.0.0".to_string())
        );
    }
}
