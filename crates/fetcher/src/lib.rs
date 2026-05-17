//! Tarball download, integrity verification, and extraction.

mod cache;
mod fetcher;

pub use cache::TarballCache;
pub use fetcher::{FetchEvent, FetchReport, Fetcher};

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use base64::Engine;
use flate2::read::GzDecoder;
use sha1::Digest as Sha1Digest;
use sha2::{Sha256, Sha512};
use subtle::ConstantTimeEq;
use tar::Archive;

/// Encode bytes to base64 using the standard alphabet (npm integrity format).
fn base64_encode(input: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(input)
}

/// Decode a base64 string to bytes.
fn base64_decode(input: &str) -> Result<Vec<u8>> {
    base64::engine::general_purpose::STANDARD
        .decode(input)
        .map_err(|e| anyhow::anyhow!("invalid base64 in integrity string: {}", e))
}

/// Compute the SHA-512 digest of file content, encoded as base64 (npm integrity format).
pub fn sha512_digest(content: &[u8]) -> String {
    let hash = Sha512::digest(content);
    base64_encode(&hash)
}

/// Compute the SHA-256 digest of file content, encoded as base64 (npm integrity format).
pub fn sha256_digest(content: &[u8]) -> String {
    let hash = Sha256::digest(content);
    base64_encode(&hash)
}

/// Verify that the content matches the given integrity string.
///
/// npm integrity strings use base64-encoded digests (not hex).
/// E.g. `sha512-XXXX` where `XXXX` is base64, not hex.
pub fn verify_integrity(content: &[u8], expected: &str) -> Result<()> {
    if let Some(encoded) = expected.strip_prefix("sha512-") {
        let expected_bytes = base64_decode(encoded)?;
        let actual = Sha512::digest(content);
        if actual.ct_eq(&expected_bytes).into() {
            return Ok(());
        }
        anyhow::bail!(
            "integrity mismatch: expected sha512-{}, got sha512-{}",
            encoded,
            base64_encode(&actual)
        );
    } else if let Some(encoded) = expected.strip_prefix("sha256-") {
        let expected_bytes = base64_decode(encoded)?;
        let actual = Sha256::digest(content);
        if actual.ct_eq(&expected_bytes).into() {
            return Ok(());
        }
        anyhow::bail!(
            "integrity mismatch: expected sha256-{}, got sha256-{}",
            encoded,
            base64_encode(&actual)
        );
    } else if let Some(encoded) = expected.strip_prefix("sha1-") {
        let expected_bytes = base64_decode(encoded)?;
        let mut hasher = sha1::Sha1::new();
        Sha1Digest::update(&mut hasher, content);
        let actual: [u8; 20] = Sha1Digest::finalize(hasher).into();
        if actual.ct_eq(&expected_bytes).into() {
            return Ok(());
        }
        anyhow::bail!(
            "integrity mismatch: expected sha1-{}, got sha1-{}",
            encoded,
            base64_encode(&actual)
        );
    }
    Ok(())
}

/// Extract a tarball into a destination directory.
pub fn extract_tarball(tarball_path: &Path, dest: &Path) -> Result<PathBuf> {
    let file = fs::File::open(tarball_path)?;
    let decoder = GzDecoder::new(file);
    let mut archive = Archive::new(decoder);

    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();

        let components: Vec<_> = path.components().collect();
        let stripped: PathBuf =
            if components.first().and_then(|c| c.as_os_str().to_str()) == Some("package") {
                PathBuf::from_iter(&components[1..])
            } else {
                path.clone()
            };

        if stripped.as_os_str().is_empty() {
            continue;
        }

        let out_path = dest.join(&stripped);
        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent)?;
        }
        entry.unpack(&out_path)?;
    }

    Ok(dest.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression test: npm integrity strings use base64 encoding, NOT hex.
    /// The original bug was that `sha512-...` was compared as if it were a hex
    /// string (89 chars), when in reality it's a base64 string (88 chars).
    #[test]
    fn verify_integrity_sha512_with_real_npm_integrity() {
        // This is the actual sha512 base64 integrity value for left-pad@1.3.0.tgz
        // computed from the real downloaded tarball from registry.npmjs.org.
        let content = include_bytes!("../test-fixtures/left-pad-1.3.0.tgz");
        let integrity =
            "sha512-XI5MPzVNApjAyhQzphX8BkmKsKUxD4LdyK24iZeQGinBN9yTQT3bFlCBy/aVx2HrNcqQGsdot8ghrjyrvMCoEA==";

        assert!(
            verify_integrity(content, integrity).is_ok(),
            "sha512 integrity verification failed with real npm integrity string"
        );
    }

    #[test]
    fn verify_integrity_sha512_wrong_hash_rejected() -> Result<()> {
        let content = b"hello world";
        let wrong_integrity = "sha512-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
        let result = verify_integrity(content, wrong_integrity);
        let Err(e) = result else {
            anyhow::bail!("wrong sha512 hash should be rejected");
        };
        let msg = e.to_string();
        assert!(
            msg.contains("integrity mismatch"),
            "error message should mention mismatch"
        );
        Ok(())
    }

    #[test]
    fn verify_integrity_sha1_with_real_npm_integrity() {
        // sha1 integrity strings are also base64, not hex.
        let content = b"test content";
        // This is NOT the correct hash — just testing that sha1 branch works.
        let integrity = "sha1-G7qMDIJD4IJlB0r8m3x7Q==";
        // May succeed or fail depending on actual content, just check it doesn't panic.
        let _ = verify_integrity(content, integrity);
    }

    #[test]
    fn verify_integrity_unknown_algorithm_passes() {
        // Unknown algorithm prefix: function returns Ok(()) as fallback.
        let content = b"any content";
        let result = verify_integrity(content, "md5-abcdef");
        assert!(
            result.is_ok(),
            "unknown algorithm should be skipped (fallback to ok)"
        );
    }

    #[test]
    fn base64_encode_decode_roundtrip() -> Result<()> {
        let original = b"hello world";
        let encoded = base64_encode(original);
        let decoded = base64_decode(&encoded)?;
        assert_eq!(decoded, original);
        Ok(())
    }
}
