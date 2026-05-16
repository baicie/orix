//! Dependency resolution with semver matching.

#![deny(clippy::unwrap_used)]

mod resolver;

pub use resolver::{resolve_from_lockfile_packages, Resolver, SkippedOptionalDep};

use anyhow::Result;
use rpnpm_domain::{PackageName, VersionConstraint};

/// Parse a string like "react@^18.2.0" into (name, constraint).
pub fn parse_package_spec(spec: &str) -> Result<(PackageName, VersionConstraint)> {
    let spec = spec.trim();
    let (name, version_str) = if let Some(at) = spec.rfind('@') {
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
    use rpnpm_domain::ConstraintKind;

    #[test]
    #[allow(clippy::expect_used)]
    fn test_parse_package_spec() {
        let (name, constraint) =
            parse_package_spec("react@^18.2.0").expect("parsing should succeed");
        assert_eq!(name.as_str(), "react");
        assert!(matches!(constraint.kind, ConstraintKind::Range(_)));

        let (name, constraint) = parse_package_spec("lodash").expect("parsing should succeed");
        assert_eq!(name.as_str(), "lodash");
        assert!(matches!(constraint.kind, ConstraintKind::Range(_)));
    }
}
