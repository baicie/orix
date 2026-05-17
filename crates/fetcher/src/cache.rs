//! Tarball cache for avoiding re-downloads.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use tokio::sync::RwLock;

use crate::verify_integrity;

/// An in-memory + disk cache for downloaded tarballs.
pub struct TarballCache {
    root: PathBuf,
    client: reqwest::Client,
    verified: Arc<RwLock<std::collections::HashMap<(String, String), PathBuf>>>,
}

impl TarballCache {
    /// Create a new tarball cache rooted at the given path.
    #[allow(clippy::expect_used)]
    pub fn new(root: PathBuf) -> Self {
        std::fs::create_dir_all(&root).ok();
        Self {
            root,
            client: reqwest::Client::builder()
                .user_agent("orix/0.1.0")
                .timeout(std::time::Duration::from_secs(60))
                .build()
                .expect("reqwest client should always build successfully"),
            verified: Arc::new(RwLock::new(Default::default())),
        }
    }

    /// Get a cached tarball or fetch it from the network.
    ///
    /// - `offline`: if true, only use locally cached tarballs (fails if not found)
    /// - `force`: if true, bypass cache and always re-download
    pub async fn get_or_fetch(
        &self,
        url: &str,
        integrity: &str,
        offline: bool,
        force: bool,
    ) -> Result<PathBuf> {
        let cached_path = self.root.join(cache_file_name(url));

        // Fast path: check in-memory verified map first
        {
            let verified = self.verified.read().await;
            let cached = verified.get(&(url.to_string(), integrity.to_string()));
            if !force {
                if let Some(path) = cached {
                    if path.exists() {
                        return Ok(path.clone());
                    }
                }
            }
        }

        // Offline mode: must find it in cache
        if offline {
            if cached_path.exists() {
                let content = tokio::fs::read(&cached_path).await?;
                verify_integrity(&content, integrity)
                    .with_context(|| "integrity mismatch for cached tarball in offline mode")?;
                return Ok(cached_path);
            }
            anyhow::bail!(
                "offline mode: tarball not in cache for {} (integrity: {})",
                url,
                integrity
            );
        }

        // Check disk cache (unless force is set)
        if !force && cached_path.exists() {
            let content = tokio::fs::read(&cached_path).await?;
            if verify_integrity(&content, integrity).is_ok() {
                let mut verified = self.verified.write().await;
                verified.insert(
                    (url.to_string(), integrity.to_string()),
                    cached_path.clone(),
                );
                return Ok(cached_path);
            }
            // Integrity mismatch — delete corrupted cache entry
            tokio::fs::remove_file(&cached_path).await.ok();
        }

        // Fetch from network with retry
        let max_retries = 3;
        let mut last_error = None;

        for attempt in 0..max_retries {
            match self.download_with_timeout(url).await {
                Ok(bytes) => {
                    if let Err(e) = verify_integrity(&bytes, integrity) {
                        last_error = Some(e);
                        continue;
                    }

                    tokio::fs::write(&cached_path, &bytes)
                        .await
                        .with_context(|| {
                            format!("failed to write tarball cache {}", cached_path.display())
                        })?;

                    let mut verified = self.verified.write().await;
                    verified.insert(
                        (url.to_string(), integrity.to_string()),
                        cached_path.clone(),
                    );

                    return Ok(cached_path);
                }
                Err(e) => {
                    last_error = Some(e);
                    if attempt < max_retries - 1 {
                        tokio::time::sleep(tokio::time::Duration::from_millis(
                            500 * (attempt + 1) as u64,
                        ))
                        .await;
                    }
                }
            }
        }

        Err(last_error
            .unwrap_or_else(|| anyhow::anyhow!("download failed after {} attempts", max_retries)))
    }

    /// Download tarball with timeout.
    async fn download_with_timeout(&self, url: &str) -> Result<Vec<u8>> {
        let resp = self
            .client
            .get(url)
            .send()
            .await
            .with_context(|| format!("failed to send request to {}", url))?;

        if !resp.status().is_success() {
            anyhow::bail!(
                "failed to download tarball: HTTP {} for {}",
                resp.status(),
                url
            );
        }

        let bytes = resp
            .bytes()
            .await
            .with_context(|| format!("failed to read response body from {}", url))?;

        Ok(bytes.into())
    }
}

fn cache_file_name(url: &str) -> String {
    let digest = Sha256::digest(url.as_bytes());
    format!("{}.tgz", hex::encode(digest))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_file_name_never_contains_path_separators() {
        let file_name = cache_file_name("https://registry.npmjs.org/left-pad/-/left-pad-1.3.0.tgz");

        assert!(!file_name.contains(['/', '\\']));
        assert_eq!(file_name.len(), 68);
    }
}
