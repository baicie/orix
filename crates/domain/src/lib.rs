//! Shared domain types for orix.

mod graph;
mod integrity;
mod name;
mod package;
mod peer;
mod platform;
mod registry;
mod script;
mod version;

pub use graph::DependencyGraph;
pub use integrity::{Integrity, IntegrityAlgorithm, IntegrityDigest, IntegrityError};
pub use name::{PackageName, PackageNameError};
pub use package::{PackageId, ResolvedPackage};
pub use peer::{PackageInstanceId, PeerContext, PeerRequirement, ResolverDiagnostic};
pub use platform::{
    check_platform_compatibility, current_cpu, current_os, symlink_available, PlatformMismatch,
};
pub use registry::{default_tarball_url, package_metadata_url};
pub use script::ScriptRef;
pub use version::{
    CatalogConstraint, CatalogSpec, ConstraintKind, PatchSpec, Version, VersionConstraint,
};

#[cfg(test)]
mod tests {
    use super::*;
    use url::Url;

    #[test]
    fn version_constraint_parse_treats_bare_non_semver_as_dist_tag() -> anyhow::Result<()> {
        let constraint = VersionConstraint::parse("next")?;

        assert!(matches!(constraint.kind, ConstraintKind::Tag(tag) if tag == "next"));
        Ok(())
    }

    #[test]
    fn version_constraint_parse_treats_npm_x_range_as_range() -> anyhow::Result<()> {
        let constraint = VersionConstraint::parse("1.6.x")?;

        assert!(matches!(constraint.kind, ConstraintKind::Range(_)));
        Ok(())
    }

    #[test]
    fn version_constraint_parse_treats_npm_or_range_as_any_range() -> anyhow::Result<()> {
        let constraint = VersionConstraint::parse("^9.0.3 || ^10.1.2")?;

        assert!(matches!(constraint.kind, ConstraintKind::AnyRange(ranges) if ranges.len() == 2));
        Ok(())
    }

    #[test]
    fn version_constraint_parse_normalizes_spaced_comparator_range() -> anyhow::Result<()> {
        let constraint = VersionConstraint::parse(">= 2.1.2 < 3.0.0")?;

        assert!(matches!(constraint.kind, ConstraintKind::Range(_)));
        Ok(())
    }

    #[test]
    fn version_constraint_parse_supports_npm_hyphen_range() -> anyhow::Result<()> {
        let constraint = VersionConstraint::parse("0.81 - 0.85")?;

        assert!(matches!(constraint.kind, ConstraintKind::Range(_)));
        Ok(())
    }

    #[test]
    fn version_constraint_parse_supports_npm_hyphen_x_range() -> anyhow::Result<()> {
        let constraint = VersionConstraint::parse("7.x - 11.x")?;

        assert!(matches!(constraint.kind, ConstraintKind::Range(_)));
        Ok(())
    }

    #[test]
    fn version_constraint_parse_supports_v_prefixed_exact_version() -> anyhow::Result<()> {
        let constraint = VersionConstraint::parse("v0.5.0")?;

        assert!(
            matches!(constraint.kind, ConstraintKind::Exact(version) if version.to_string() == "0.5.0")
        );
        Ok(())
    }

    #[test]
    fn version_constraint_parse_supports_npm_alias() -> anyhow::Result<()> {
        let constraint = VersionConstraint::parse("npm:wrap-ansi@^7.0.0")?;

        assert!(
            matches!(constraint.kind, ConstraintKind::Alias { package, .. } if package.as_str() == "wrap-ansi")
        );
        Ok(())
    }

    #[test]
    fn package_name_parse_preserves_scoped_name_case() -> anyhow::Result<()> {
        let name = PackageName::parse("@Scope/Package")?;

        assert_eq!(name.as_str(), "@Scope/Package");
        assert_eq!(name.scope(), Some("Scope"));
        assert_eq!(name.unscoped(), "Package");
        Ok(())
    }

    #[test]
    fn package_name_parse_preserves_legacy_uppercase_names() -> anyhow::Result<()> {
        let name = PackageName::parse("JSONStream")?;

        assert_eq!(name.as_str(), "JSONStream");
        Ok(())
    }

    #[test]
    fn package_name_parse_rejects_path_like_names() {
        let result = PackageName::parse("../pkg");

        assert!(matches!(result, Err(PackageNameError::InvalidCharacter(_))));
    }

    #[test]
    fn integrity_parse_returns_strongest_digest() -> anyhow::Result<()> {
        let integrity = Integrity::parse("sha1-abc sha512-def")?;

        assert_eq!(
            integrity.strongest().map(|digest| digest.algorithm),
            Some(IntegrityAlgorithm::Sha512)
        );
        Ok(())
    }

    #[test]
    fn integrity_parse_rejects_unknown_algorithm() {
        let result = Integrity::parse("md5-abc");

        assert!(matches!(
            result,
            Err(IntegrityError::UnsupportedAlgorithm(algorithm)) if algorithm == "md5"
        ));
    }

    #[test]
    fn package_metadata_url_encodes_scoped_package_slash() -> anyhow::Result<()> {
        let registry = Url::parse("https://registry.npmjs.org")?;
        let name = PackageName::parse("@scope/pkg")?;

        let url = package_metadata_url(&registry, &name)?;

        assert_eq!(url.as_str(), "https://registry.npmjs.org/@scope%2fpkg");
        Ok(())
    }

    #[test]
    fn package_metadata_url_preserves_legacy_uppercase_names() -> anyhow::Result<()> {
        let registry = Url::parse("https://registry.npmjs.org")?;
        let name = PackageName::parse("JSONStream")?;

        let url = package_metadata_url(&registry, &name)?;

        assert_eq!(url.as_str(), "https://registry.npmjs.org/JSONStream");
        Ok(())
    }

    #[test]
    fn default_tarball_url_uses_unscoped_tarball_name() -> anyhow::Result<()> {
        let registry = Url::parse("https://registry.npmjs.org/")?;
        let id = PackageId::new(PackageName::parse("@scope/pkg")?, Version::parse("1.2.3")?);

        let url = default_tarball_url(&registry, &id)?;

        assert_eq!(
            url.as_str(),
            "https://registry.npmjs.org/@scope/pkg/-/pkg-1.2.3.tgz"
        );
        Ok(())
    }

    #[test]
    fn peer_context_key_generates_sorted_suffix() -> anyhow::Result<()> {
        let mut ctx = PeerContext::default();
        ctx.insert(
            PackageName::from("lodash"),
            PackageId::new(PackageName::from("lodash"), Version::parse("4.17.21")?),
        );
        ctx.insert(
            PackageName::from("react"),
            PackageId::new(PackageName::from("react"), Version::parse("18.2.0")?),
        );
        let key = ctx.key();
        assert!(key.contains("(lodash@4.17.21)"));
        assert!(key.contains("(react@18.2.0)"));
        Ok(())
    }

    #[test]
    fn peer_context_key_empty_when_no_peers() {
        let ctx = PeerContext::default();
        assert!(ctx.is_empty());
        assert_eq!(ctx.key(), "");
    }

    #[test]
    fn package_instance_id_key_includes_peer_suffix() -> anyhow::Result<()> {
        let mut ctx = PeerContext::default();
        ctx.insert(
            PackageName::from("react"),
            PackageId::new(PackageName::from("react"), Version::parse("18.2.0")?),
        );
        let instance = PackageInstanceId::new(
            PackageId::new(PackageName::from("react-dom"), Version::parse("18.2.0")?),
            ctx,
        );
        let key = instance.key();
        assert!(key.contains("react-dom@18.2.0"));
        assert!(key.contains("react@18.2.0"));
        Ok(())
    }

    #[test]
    fn resolver_diagnostic_display_formats_missing_peer() -> anyhow::Result<()> {
        let diag = ResolverDiagnostic::MissingPeer {
            requester: PackageId::new(PackageName::from("react-dom"), Version::parse("18.2.0")?),
            peer_name: PackageName::from("react"),
            range: VersionConstraint::parse("^18.0.0")?,
        };
        let msg = diag.to_string();
        assert!(msg.contains("unmet peer dependency"));
        assert!(msg.contains("react-dom@18.2.0"));
        assert!(msg.contains("^18.0.0"));
        Ok(())
    }

    #[test]
    fn resolver_diagnostic_display_formats_peer_conflict() -> anyhow::Result<()> {
        let diag = ResolverDiagnostic::PeerVersionConflict {
            requester: PackageId::new(PackageName::from("react-dom"), Version::parse("18.2.0")?),
            peer_name: PackageName::from("react"),
            requested_range: VersionConstraint::parse("^18.0.0")?,
            found_version: Version::parse("17.0.2")?,
        };
        let msg = diag.to_string();
        assert!(msg.contains("version conflict"));
        assert!(msg.contains("found"));
        assert!(msg.contains("17.0.2"));
        Ok(())
    }
}
