//! npm registry API client.

mod cache;
mod types;

pub use cache::PackumentCache;
pub use types::{Dist, PackageMetadata, Packument, PeerDepMeta};

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
        }
    }

    /// Create a new registry client with authentication.
    #[allow(clippy::expect_used)]
    pub fn with_auth(base_url: Url, token: &str) -> Self {
        Self::with_auth_concurrency(base_url, token, 10)
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
        }
    }

    /// Fetch the full packument for a package name.
    ///
    /// Results are cached in memory with a 5-minute TTL.
    /// Deduplication of concurrent requests for the same package name is handled
    /// by the resolver's `in_flight_resolution` set (not here), which prevents
    /// duplicate resolution tasks from being dispatched.
    #[instrument(skip(self), fields(pkg = %name))]
    pub async fn fetch_packument(&self, name: &PackageName) -> Result<Packument> {
        let name_str = name.as_str().to_string();

        // Check memory cache first.
        if let Some(cached) = self.cache.get(&name_str).await {
            debug!("packument cache hit");
            return Ok(cached);
        }
        debug!("packument cache miss, acquiring concurrency permit");

        // Acquire a concurrency permit before making the HTTP request.
        let _permit: OwnedSemaphorePermit = self.concurrency.clone().acquire_owned().await?;

        debug!("concurrency permit acquired, fetching from registry");
        // Do the HTTP request.
        let url = package_metadata_url(&self.base_url, name)?;
        debug!(url = %url, "making HTTP request to registry");
        let packument = self.do_fetch_packument(&url).await?;

        debug!("packument fetched successfully, caching result");
        // Cache the result.
        self.cache.insert(name_str, packument.clone()).await;

        Ok(packument)
    }

    /// Perform the actual HTTP fetch for a packument.
    #[instrument(skip(self), fields(url = %url))]
    async fn do_fetch_packument(&self, url: &Url) -> Result<Packument> {
        let mut last_error = None;

        for attempt in 0..PACKUMENT_MAX_RETRIES {
            debug!(attempt, "attempting to fetch packument");
            match self.do_fetch_packument_once(url).await {
                Ok(packument) => {
                    debug!("packument fetched successfully");
                    return Ok(packument);
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
    async fn do_fetch_packument_once(&self, url: &Url) -> Result<Packument> {
        debug!("sending HTTP request");
        let resp = self
            .http_client
            .get(url.clone())
            .header(
                reqwest::header::ACCEPT,
                "application/vnd.npm.install-v1+json, application/json",
            )
            .send()
            .await
            .map_err(|e| RegistryError::Network(e.to_string()))?;

        let status = resp.status();
        debug!(status = %status, "received HTTP response");
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
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
        Ok(packument)
    }

    /// Returns a reference to the shared packument cache.
    #[allow(dead_code)]
    pub fn cache(&self) -> &Arc<PackumentCache> {
        &self.cache
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

        self.cache
            .insert_sync(name.as_str().to_string(), packument.clone(), Duration::MAX);

        Ok(packument)
    }
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
