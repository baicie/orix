//! npm Subresource Integrity parsing.

use thiserror::Error;

/// Parsed npm Subresource Integrity metadata.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Integrity {
    /// Digest entries in the order they appeared in the source string.
    pub algorithms: Vec<IntegrityDigest>,
}

/// A single algorithm/digest pair from an integrity string.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IntegrityDigest {
    /// Digest algorithm.
    pub algorithm: IntegrityAlgorithm,
    /// Base64-encoded digest.
    pub digest_base64: String,
}

/// Supported integrity algorithms.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum IntegrityAlgorithm {
    /// Legacy SHA-1 integrity.
    Sha1,
    /// Modern SHA-512 integrity.
    Sha512,
}

/// Integrity parsing errors.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum IntegrityError {
    /// Integrity string was empty.
    #[error("integrity string is empty")]
    Empty,
    /// Integrity digest did not contain a supported algorithm.
    #[error("unsupported integrity algorithm: {0}")]
    UnsupportedAlgorithm(String),
    /// Integrity digest did not contain a base64 payload.
    #[error("invalid integrity digest: {0}")]
    InvalidDigest(String),
}

impl Integrity {
    /// Parse an npm integrity string such as `sha512-... sha1-...`.
    pub fn parse(input: &str) -> Result<Self, IntegrityError> {
        let input = input.trim();
        if input.is_empty() {
            return Err(IntegrityError::Empty);
        }

        let algorithms: Result<Vec<_>, _> = input
            .split_whitespace()
            .map(|part| {
                let (algorithm, digest) = part
                    .split_once('-')
                    .ok_or_else(|| IntegrityError::InvalidDigest(part.to_string()))?;
                if digest.is_empty() {
                    return Err(IntegrityError::InvalidDigest(part.to_string()));
                }

                let algorithm = match algorithm {
                    "sha512" => IntegrityAlgorithm::Sha512,
                    "sha1" => IntegrityAlgorithm::Sha1,
                    other => return Err(IntegrityError::UnsupportedAlgorithm(other.to_string())),
                };

                Ok(IntegrityDigest {
                    algorithm,
                    digest_base64: digest.to_string(),
                })
            })
            .collect();

        Ok(Self {
            algorithms: algorithms?,
        })
    }

    /// Return the strongest digest available.
    pub fn strongest(&self) -> Option<&IntegrityDigest> {
        self.algorithms.iter().max_by_key(|digest| digest.algorithm)
    }
}
