//! Version selection from packuments.

#![deny(clippy::unwrap_used)]

use anyhow::{Context, Result};

use orix_domain::{Version, VersionConstraint};
use orix_registry::Packument;

/// Select the best version for a package from a packument.
pub(crate) fn select_version_impl(
    packument: &Packument,
    constraint: &VersionConstraint,
) -> Result<Version> {
    match &constraint.kind {
        orix_domain::ConstraintKind::Exact(v) => Ok(v.clone()),

        orix_domain::ConstraintKind::Range(range) => {
            let mut candidates: Vec<_> = packument
                .versions
                .keys()
                .filter_map(|v| Version::parse(v).ok())
                .filter(|v| range.matches(v))
                .collect();
            candidates.sort();
            candidates
                .pop()
                .with_context(|| format!("no version satisfies {}", constraint.raw))
        }

        orix_domain::ConstraintKind::AnyRange(ranges) => {
            let mut candidates: Vec<_> = packument
                .versions
                .keys()
                .filter_map(|v| Version::parse(v).ok())
                .filter(|v| ranges.iter().any(|range| range.matches(v)))
                .collect();
            candidates.sort();
            candidates
                .pop()
                .with_context(|| format!("no version satisfies {}", constraint.raw))
        }

        orix_domain::ConstraintKind::Latest => packument
            .dist_tags
            .get("latest")
            .and_then(|v| Version::parse(v).ok())
            .with_context(|| "no dist-tags.latest found in packument"),

        orix_domain::ConstraintKind::Tag(tag) => packument
            .dist_tags
            .get(tag)
            .and_then(|v| Version::parse(v).ok())
            .with_context(|| format!("tag '{}' not found in packument", tag)),

        orix_domain::ConstraintKind::Patch(spec) => Ok(spec.package_version.clone()),

        orix_domain::ConstraintKind::Catalog(_) => {
            anyhow::bail!(
                "catalog reference '{}' was not expanded — workspace catalog not available",
                constraint.raw
            );
        }

        orix_domain::ConstraintKind::Alias { constraint, .. } => {
            select_version_impl(packument, constraint)
        }
    }
}
