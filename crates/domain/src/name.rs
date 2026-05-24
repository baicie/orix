//! Package name types.

use std::borrow::Cow;
use std::fmt;
use std::ops::Deref;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// A validated package name.
#[derive(Clone, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
pub struct PackageName(Cow<'static, str>);

impl PackageName {
    /// Create a new package name.
    #[inline]
    pub fn new(name: impl Into<Cow<'static, str>>) -> Self {
        Self(name.into())
    }

    /// Return the package name as a string slice.
    #[inline]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Parse and validate an npm package name.
    pub fn parse(name: &str) -> Result<Self, PackageNameError> {
        let normalized = normalize_package_name(name)?;
        Ok(Self(Cow::Owned(normalized)))
    }

    /// Return the package scope without the leading `@`, if present.
    pub fn scope(&self) -> Option<&str> {
        self.0
            .strip_prefix('@')
            .and_then(|name| name.split_once('/').map(|(scope, _)| scope))
    }

    /// Return the package name without its scope.
    pub fn unscoped(&self) -> &str {
        self.0
            .strip_prefix('@')
            .and_then(|name| name.split_once('/').map(|(_, unscoped)| unscoped))
            .unwrap_or(&self.0)
    }
}

/// Package name validation errors.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum PackageNameError {
    /// Package name was empty.
    #[error("package name is empty")]
    EmptyName,
    /// Scoped package name was malformed.
    #[error("invalid scoped package name: {0}")]
    InvalidScope(String),
    /// Package name contains invalid characters or path traversal.
    #[error("invalid package name: {0}")]
    InvalidCharacter(String),
}

impl From<String> for PackageName {
    fn from(s: String) -> Self {
        Self(Cow::Owned(s))
    }
}

impl From<&str> for PackageName {
    fn from(s: &str) -> Self {
        Self(Cow::Owned(s.to_string()))
    }
}

impl PartialOrd for PackageName {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PackageName {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.to_lowercase().cmp(&other.0.to_lowercase())
    }
}

impl Deref for PackageName {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PackageName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

fn normalize_package_name(name: &str) -> Result<String, PackageNameError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(PackageNameError::EmptyName);
    }
    if name.contains('\\') || name.contains("..") {
        return Err(PackageNameError::InvalidCharacter(name.to_string()));
    }

    if let Some(scoped) = name.strip_prefix('@') {
        let Some((scope, package)) = scoped.split_once('/') else {
            return Err(PackageNameError::InvalidScope(name.to_string()));
        };
        if scope.is_empty() || package.is_empty() || package.contains('/') {
            return Err(PackageNameError::InvalidScope(name.to_string()));
        }
        validate_package_segment(scope, name)?;
        validate_package_segment(package, name)?;
        Ok(format!("@{scope}/{package}"))
    } else {
        if name.contains('/') {
            return Err(PackageNameError::InvalidCharacter(name.to_string()));
        }
        validate_package_segment(name, name)?;
        Ok(name.to_string())
    }
}

fn validate_package_segment(segment: &str, original: &str) -> Result<(), PackageNameError> {
    if segment.is_empty()
        || segment == "."
        || segment == ".."
        || segment.chars().any(|ch| ch.is_control())
    {
        return Err(PackageNameError::InvalidCharacter(original.to_string()));
    }
    Ok(())
}
