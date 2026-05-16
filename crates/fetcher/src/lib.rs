//! Tarball download, integrity verification, and extraction.

mod cache;
mod fetcher;

pub use cache::TarballCache;
pub use fetcher::{FetchReport, Fetcher};

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use flate2::read::GzDecoder;
use sha2::{Sha256, Sha512};
use tar::Archive;

/// Compute the SHA-256 digest of file content.
pub fn sha256_digest(content: &[u8]) -> String {
    use sha2::Digest;
    let mut hasher = Sha256::new();
    hasher.update(content);
    hex::encode(hasher.finalize())
}

/// Compute the SHA-512 digest of file content.
pub fn sha512_digest(content: &[u8]) -> String {
    use sha2::Digest;
    let mut hasher = Sha512::new();
    hasher.update(content);
    hex::encode(hasher.finalize())
}

/// Verify that the content matches the given integrity string.
pub fn verify_integrity(content: &[u8], expected: &str) -> Result<()> {
    if let Some(expected_hash) = expected.strip_prefix("sha512-") {
        let actual = sha512_digest(content);
        if actual != expected_hash {
            anyhow::bail!(
                "integrity mismatch: expected sha512-{}, got sha512-{}",
                expected_hash,
                actual
            );
        }
    } else if let Some(expected_hash) = expected.strip_prefix("sha256-") {
        let actual = sha256_digest(content);
        if actual != expected_hash {
            anyhow::bail!(
                "integrity mismatch: expected sha256-{}, got sha256-{}",
                expected_hash,
                actual
            );
        }
    } else if let Some(expected_hash) = expected.strip_prefix("sha1-") {
        use sha1::Digest;
        let mut hasher = sha1::Sha1::new();
        Digest::update(&mut hasher, content);
        let actual = hex::encode(Digest::finalize(hasher));
        if actual != expected_hash {
            anyhow::bail!(
                "integrity mismatch: expected sha1-{}, got sha1-{}",
                expected_hash,
                actual
            );
        }
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
