//! npm registry API client.

mod cache;
mod singleflight;
mod types;

pub use cache::PackumentCache;
pub use singleflight::PackumentSingleFlight;
pub use types::{Dist, PackageMetadata, Packument, PeerDepMeta};

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use thiserror::Error;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tracing::{debug, error, instrument};
use url::Url;

use orix_domain::{package_metadata_url, PackageName};

const PACKUMENT_MAX_RETRIES: usize = 6;

/// Errors from the npm registry client.
#[derive(Error, Debug)]
pub enum RegistryError {
    /// Package not found on the registry.
    #[error("package '{0}' not found on registry")]
    PackageNotFound(PackageName),

    /// Network-level error.
    #[error("network error: {0}")]
    Network(String),

    /// Non-success HTTP response.
    #[error("HTTP error: {0} {1}")]
    Http(u16, String),

    /// Miscellaneous registry error.
    #[error("registry error: {0}")]
    Other(String),
}

/// Client for interacting with the npm registry API.
#[derive(Clone)]
pub struct RegistryClient {
    base_url: Url,
    http_client: reqwest::Client,
    /// Shared packument cache with TTL.
    cache: Arc<PackumentCache>,
    /// Semaphore to limit concurrent HTTP requests per client.
    concurrency: Arc<Semaphore>,
    /// Single-flight tracker for deduplicating concurrent requests.
    singleflight: Arc<PackumentSingleFlight>,
    /// Disk cache root path (if configured).
    #[allow(dead_code)]
    disk_cache_root: Option<PathBuf>,
}

impl RegistryClient {
    /// Create a new registry client with default concurrency of 10.
    #[allow(clippy::expect_used)]
    pub fn new(base_url: Url) -> Self {
        Self::with_concurrency(base_url, 10)
    }

    /// Create a new registry client with a custom concurrency limit.
    #[allow(clippy::expect_used)]
    pub fn with_concurrency(base_url: Url, concurrency: usize) -> Self {
        let http_client = reqwest::Client::builder()
            .user_agent("orix/0.1.0")
            .timeout(std::time::Duration::from_secs(30))
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()
            .expect("reqwest client should always build successfully");

        Self {
            base_url,
            http_client,
            cache: Arc::new(PackumentCache::new()),
            concurrency: Arc::new(Semaphore::new(concurrency.max(1))),
            singleflight: Arc::new(PackumentSingleFlight::new()),
            disk_cache_root: None,
        }
    }

    /// Create a new registry client with disk cache persistence.
    #[allow(clippy::expect_used)]
    pub fn with_disk_cache(base_url: Url, cache_root: PathBuf) -> Self {
        Self::with_disk_cache_concurrency(base_url, cache_root, 10)
    }

    /// Create a new registry client with disk cache persistence and custom concurrency.
    #[allow(clippy::expect_used)]
    pub fn with_disk_cache_concurrency(
        base_url: Url,
        cache_root: PathBuf,
        concurrency: usize,
    ) -> Self {
        let http_client = reqwest::Client::builder()
            .user_agent("orix/0.1.0")
            .timeout(std::time::Duration::from_secs(30))
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()
            .expect("reqwest client should always build successfully");

        let registry_url = base_url.to_string();
        Self {
            base_url,
            http_client,
            cache: Arc::new(PackumentCache::with_disk_cache(
                cache_root.clone(),
                &registry_url,
            )),
            concurrency: Arc::new(Semaphore::new(concurrency.max(1))),
            singleflight: Arc::new(PackumentSingleFlight::new()),
            disk_cache_root: Some(cache_root),
        }
    }

    /// Create a new registry client with authentication.
    #[allow(clippy::expect_used)]
    pub fn with_auth(base_url: Url, token: &str) -> Self {
        Self::with_auth_concurrency(base_url, token, 10)
    }

    /// Create a new authenticated registry client with disk cache persistence.
    #[allow(clippy::expect_used)]
    pub fn with_auth_disk_cache_concurrency(
        base_url: Url,
        token: &str,
        cache_root: PathBuf,
        concurrency: usize,
    ) -> Self {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::AUTHORIZATION,
            reqwest::header::HeaderValue::from_str(&format!("Bearer {}", token))
                .expect("token is a valid header value"),
        );

        let http_client = reqwest::Client::builder()
            .user_agent("orix/0.1.0")
            .default_headers(headers)
            .timeout(std::time::Duration::from_secs(30))
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()
            .expect("reqwest client should always build successfully");

        let registry_url = base_url.to_string();
        Self {
            base_url,
            http_client,
            cache: Arc::new(PackumentCache::with_disk_cache(
                cache_root.clone(),
                &registry_url,
            )),
            concurrency: Arc::new(Semaphore::new(concurrency.max(1))),
            singleflight: Arc::new(PackumentSingleFlight::new()),
            disk_cache_root: Some(cache_root),
        }
    }

    /// Create a new registry client with authentication and concurrency limit.
    #[allow(clippy::expect_used)]
    pub fn with_auth_concurrency(base_url: Url, token: &str, concurrency: usize) -> Self {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::AUTHORIZATION,
            reqwest::header::HeaderValue::from_str(&format!("Bearer {}", token))
                .expect("token is a valid header value"),
        );

        let http_client = reqwest::Client::builder()
            .user_agent("orix/0.1.0")
            .default_headers(headers)
            .timeout(std::time::Duration::from_secs(30))
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()
            .expect("reqwest client should always build successfully");

        Self {
            base_url,
            http_client,
            cache: Arc::new(PackumentCache::new()),
            concurrency: Arc::new(Semaphore::new(concurrency.max(1))),
            singleflight: Arc::new(PackumentSingleFlight::new()),
            disk_cache_root: None,
        }
    }

    /// Fetch the full packument for a package name.
    ///
    /// Results are cached in memory (5 min TTL) and optionally on disk (1 hour TTL).
    /// Deduplication of concurrent requests for the same package name is handled
    /// by the single-flight mechanism.
    #[instrument(skip(self), fields(pkg = %name))]
    pub async fn fetch_packument(&self, name: &PackageName) -> Result<Packument> {
        let name_str = name.as_str().to_string();

        // Check memory cache first.
        if let Some(cached) = self.cache.get(&name_str).await {
            debug!("packument cache hit (memory)");
            return Ok(cached);
        }
        debug!("packument cache miss, acquiring concurrency permit");

        // Try single-flight: if another request is in flight, wait for it.
        if let Some(_flight_guard) = self.singleflight.register(&name_str).await {
            // We're the primary requester - check disk cache and fetch.
            // First check disk cache again (in case it was added while waiting for the lock).
            if let Some(cached) = self.cache.get(&name_str).await {
                debug!("packument cache hit (disk)");
                self.singleflight.unregister(&name_str).await;
                return Ok(cached);
            }

            let result = async {
                // Acquire a concurrency permit before making the HTTP request.
                let _permit: OwnedSemaphorePermit =
                    self.concurrency.clone().acquire_owned().await?;

                debug!("concurrency permit acquired, fetching from registry");
                // Do the HTTP request.
                let url = package_metadata_url(&self.base_url, name)?;
                debug!(url = %url, "making HTTP request to registry");
                let (packument, etag) = self.do_fetch_packument(&url).await?;

                debug!("packument fetched successfully, caching result");
                // Cache the result with ETag.
                self.cache
                    .insert_with_etag(name_str.clone(), packument.clone(), etag, None)
                    .await;

                Ok(packument)
            }
            .await;

            self.singleflight.unregister(&name_str).await;
            result
        } else {
            // Another request is in flight - wait for it to complete.
            debug!("another request in flight, waiting for result");
            self.singleflight.wait_until_complete(&name_str).await;

            // Check cache again.
            if let Some(cached) = self.cache.get(&name_str).await {
                debug!("packument cache hit (after waiting)");
                return Ok(cached);
            }

            // Fallback: make the request ourselves.
            let _permit: OwnedSemaphorePermit = self.concurrency.clone().acquire_owned().await?;
            let url = package_metadata_url(&self.base_url, name)?;
            let (packument, etag) = self.do_fetch_packument(&url).await?;
            self.cache
                .insert_with_etag(name_str, packument.clone(), etag, None)
                .await;
            Ok(packument)
        }
    }

    /// Perform the actual HTTP fetch for a packument.
    #[instrument(skip(self), fields(url = %url))]
    async fn do_fetch_packument(&self, url: &Url) -> Result<(Packument, Option<String>)> {
        let mut last_error = None;

        for attempt in 0..PACKUMENT_MAX_RETRIES {
            debug!(attempt, "attempting to fetch packument");
            match self.do_fetch_packument_once(url).await {
                Ok(result) => {
                    debug!("packument fetched successfully");
                    return Ok(result);
                }
                Err(error) if is_retryable_packument_error(&error) => {
                    last_error = Some(error);
                    debug!(attempt, "retryable error, will retry");
                    if attempt + 1 < PACKUMENT_MAX_RETRIES {
                        tokio::time::sleep(packument_retry_delay(attempt)).await;
                    }
                }
                Err(error) => {
                    error!(error = %error, "non-retryable error");
                    return Err(error);
                }
            }
        }

        error!("exhausted all retries");
        Err(last_error
            .unwrap_or_else(|| anyhow::anyhow!("packument request did not run"))
            .context(format!(
                "failed to fetch packument from {url} after {PACKUMENT_MAX_RETRIES} attempts"
            )))
    }

    #[instrument(skip(self), fields(url = %url))]
    async fn do_fetch_packument_once(&self, url: &Url) -> Result<(Packument, Option<String>)> {
        debug!("sending HTTP request");
        let mut request = self.http_client.get(url.clone());

        // Add If-None-Match header if we have an ETag.
        // This is handled by checking disk cache before making the request.
        // The actual ETag is stored with the packument on disk.

        request = request.header(
            reqwest::header::ACCEPT,
            "application/vnd.npm.install-v1+json, application/json",
        );

        let resp = request
            .send()
            .await
            .map_err(|e| RegistryError::Network(e.to_string()))?;

        let status = resp.status();
        debug!(status = %status, "received HTTP response");

        // Check for 304 Not Modified - means cache is still valid.
        if status.as_u16() == 304 {
            debug!("received 304 Not Modified, cache is valid");
            // Return None for ETag to indicate we should use cached version.
            return Ok((
                serde_json::from_str("{}").unwrap_or_else(|_| Packument {
                    name: String::new(),
                    versions: std::collections::HashMap::new(),
                    dist_tags: std::collections::HashMap::new(),
                }),
                None,
            ));
        }

        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);

        // Extract ETag header.
        let etag = resp
            .headers()
            .get(reqwest::header::ETAG)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);

        if status.as_u16() == 404 {
            anyhow::bail!(RegistryError::PackageNotFound(PackageName::from(
                url.as_str()
            )));
        }
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_else(|_| String::new());
            anyhow::bail!(RegistryError::Http(status.as_u16(), text));
        }

        debug!("reading response body");
        let body = resp
            .bytes()
            .await
            .with_context(|| format!("failed to read response body from {url}"))?;
        debug!(
            body_size = body.len(),
            "response body received, parsing JSON"
        );
        let packument: Packument = serde_json::from_slice(&body).map_err(|e| {
            RegistryError::Other(format!(
                "failed to decode packument JSON from {url}: {e}; content-type: {}; body prefix: {}",
                content_type.as_deref().unwrap_or("<unknown>"),
                body_prefix(&body)
            ))
        })?;

        debug!("packument parsed successfully");
        Ok((packument, etag))
    }

    /// Returns a reference to the shared packument cache.
    pub fn cache(&self) -> &Arc<PackumentCache> {
        &self.cache
    }

    /// Returns a reference to the single-flight tracker.
    pub fn singleflight(&self) -> &Arc<PackumentSingleFlight> {
        &self.singleflight
    }

    /// Synchronously fetch a packument (blocking).
    ///
    /// Checks the in-memory cache before making a blocking HTTP request.
    pub fn fetch_packument_sync(&mut self, name: &PackageName) -> Result<Packument> {
        use std::time::Duration;

        if let Some(cached) = self.cache.get_sync(name.as_str()) {
            return Ok(cached);
        }

        let rt = tokio::runtime::Handle::current();
        let packument = rt.block_on(self.fetch_packument(name))?;

        self.cache.insert_sync(
            name.as_str().to_string(),
            packument.clone(),
            Duration::MAX,
            None,
        );

        Ok(packument)
    }

    /// Clear the packument cache.
    pub async fn clear_cache(&self) {
        self.cache.clear().await;
    }

    /// Get cache statistics.
    pub async fn cache_stats(&self) -> CacheStats {
        CacheStats {
            entries: self.cache.len().await,
        }
    }
}

/// Cache statistics.
#[derive(Debug, Clone)]
pub struct CacheStats {
    /// Number of entries in the cache.
    pub entries: usize,
}

fn is_retryable_packument_error(error: &anyhow::Error) -> bool {
    for cause in error.chain() {
        let message = cause.to_string();
        if message.contains("response body")
            || message.contains("error reading a body from connection")
            || message.contains("unexpected EOF")
        {
            return true;
        }

        if let Some(reqwest_error) = cause.downcast_ref::<reqwest::Error>() {
            return reqwest_error.is_timeout()
                || reqwest_error.is_connect()
                || reqwest_error.is_request()
                || reqwest_error.is_body();
        }

        if let Some(registry_error) = cause.downcast_ref::<RegistryError>() {
            return match registry_error {
                RegistryError::Network(_) => true,
                RegistryError::Http(status, _) => *status == 429 || *status >= 500,
                RegistryError::PackageNotFound(_) | RegistryError::Other(_) => false,
            };
        }
    }

    false
}

fn packument_retry_delay(attempt: usize) -> Duration {
    if cfg!(test) {
        return Duration::from_millis(1);
    }

    let millis = 250_u64.saturating_mul(1 << attempt.min(3));
    Duration::from_millis(millis)
}

fn body_prefix(body: &[u8]) -> String {
    const LIMIT: usize = 200;

    let prefix = String::from_utf8_lossy(&body[..body.len().min(LIMIT)]);
    let escaped = prefix
        .chars()
        .flat_map(char::escape_default)
        .collect::<String>();

    if body.len() > LIMIT {
        format!("{escaped}...")
    } else {
        escaped
    }
}

#[cfg(test)]
mod tests;
