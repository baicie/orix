//! npm registry API client.

mod cache;
mod types;

pub use cache::PackumentCache;
pub use types::{Dist, PackageMetadata, Packument};

use std::sync::Arc;

use anyhow::Result;
use thiserror::Error;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use url::Url;

use orix_domain::{package_metadata_url, PackageName};

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
    pub async fn fetch_packument(&self, name: &PackageName) -> Result<Packument> {
        let name_str = name.as_str().to_string();

        // Check memory cache first.
        if let Some(cached) = self.cache.get(&name_str).await {
            return Ok(cached);
        }

        // Acquire a concurrency permit before making the HTTP request.
        let _permit: OwnedSemaphorePermit = self.concurrency.clone().acquire_owned().await?;

        // Do the HTTP request.
        let url = package_metadata_url(&self.base_url, name)?;
        let packument = self.do_fetch_packument(&url).await?;

        // Cache the result.
        self.cache.insert(name_str, packument.clone()).await;

        Ok(packument)
    }

    /// Perform the actual HTTP fetch for a packument.
    async fn do_fetch_packument(&self, url: &Url) -> Result<Packument> {
        let resp = self
            .http_client
            .get(url.clone())
            .send()
            .await
            .map_err(|e| RegistryError::Network(e.to_string()))?;

        let status = resp.status();
        if status.as_u16() == 404 {
            anyhow::bail!(RegistryError::PackageNotFound(PackageName::from(
                url.as_str()
            )));
        }
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_else(|_| String::new());
            anyhow::bail!(RegistryError::Http(status.as_u16(), text));
        }

        let packument: Packument = resp
            .json()
            .await
            .map_err(|e| RegistryError::Other(e.to_string()))?;

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
