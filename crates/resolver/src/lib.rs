//! Dependency resolution with semver matching.

#![deny(clippy::unwrap_used)]

mod resolver;

pub use resolver::{ResolveProgressEvent, Resolver, SkippedOptionalDep};

use anyhow::Result;
use orix_domain::{PackageName, VersionConstraint};

/// Parse a string like "react@^18.2.0" into (name, constraint).
pub fn parse_package_spec(spec: &str) -> Result<(PackageName, VersionConstraint)> {
    let spec = spec.trim();
    let version_separator = spec.rfind('@').filter(|&at| {
        at > 0 && (!spec.starts_with('@') || spec[..at].contains('/')) && !spec[at + 1..].is_empty()
    });

    let (name, version_str) = if let Some(at) = version_separator {
        let before = &spec[..at];
        let after = &spec[at + 1..];
        if before.is_empty() {
            anyhow::bail!("invalid package spec: {}", spec);
        }
        (before, after)
    } else {
        (spec, "*")
    };

    let name = PackageName::from(name);
    let constraint = VersionConstraint::parse(version_str)?;
    Ok((name, constraint))
}

#[cfg(test)]
mod tests {
    use super::*;
    use orix_domain::ConstraintKind;

    #[test]
    fn test_parse_package_spec() -> anyhow::Result<()> {
        let (name, constraint) = parse_package_spec("react@^18.2.0")?;
        assert_eq!(name.as_str(), "react");
        assert!(matches!(constraint.kind, ConstraintKind::Range(_)));

        let (name, constraint) = parse_package_spec("lodash")?;
        assert_eq!(name.as_str(), "lodash");
        assert!(matches!(constraint.kind, ConstraintKind::Range(_)));

        Ok(())
    }

    #[test]
    fn parse_package_spec_supports_scoped_packages() -> anyhow::Result<()> {
        let (name, constraint) = parse_package_spec("@scope/pkg")?;
        assert_eq!(name.as_str(), "@scope/pkg");
        assert!(matches!(constraint.kind, ConstraintKind::Range(_)));

        let (name, constraint) = parse_package_spec("@scope/pkg@1.2.3")?;
        assert_eq!(name.as_str(), "@scope/pkg");
        assert!(matches!(constraint.kind, ConstraintKind::Exact(_)));

        let (name, constraint) = parse_package_spec("@scope/pkg@next")?;
        assert_eq!(name.as_str(), "@scope/pkg");
        assert!(matches!(constraint.kind, ConstraintKind::Tag(tag) if tag == "next"));

        Ok(())
    }
}
