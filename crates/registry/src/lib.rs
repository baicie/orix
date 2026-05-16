//! npm registry API client.

mod types;

pub use types::{Dist, PackageMetadata, Packument};

use anyhow::Result;
use thiserror::Error;
use url::Url;

use rpnpm_domain::PackageName;

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
}

impl RegistryClient {
    /// Create a new registry client.
    #[allow(clippy::expect_used)]
    pub fn new(base_url: Url) -> Self {
        let http_client = reqwest::Client::builder()
            .user_agent("rpnpm/0.1.0")
            .timeout(std::time::Duration::from_secs(30))
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()
            .expect("reqwest client should always build successfully");

        Self {
            base_url,
            http_client,
        }
    }

    /// Create a new registry client with authentication.
    #[allow(clippy::expect_used)]
    pub fn with_auth(base_url: Url, token: &str) -> Self {
        let http_client = reqwest::Client::builder()
            .user_agent("rpnpm/0.1.0")
            .timeout(std::time::Duration::from_secs(30))
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()
            .expect("reqwest client should always build successfully");

        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::AUTHORIZATION,
            reqwest::header::HeaderValue::from_str(&format!("Bearer {}", token))
                .expect("token is a valid header value"),
        );

        Self {
            base_url,
            http_client,
        }
    }

    /// Fetch the full packument for a package name.
    pub async fn fetch_packument(&self, name: &PackageName) -> Result<Packument> {
        let url = self.base_url.join(&format!("{}/", name.as_str()))?;
        let resp = self
            .http_client
            .get(url)
            .send()
            .await
            .map_err(|e| RegistryError::Network(e.to_string()))?;

        let status = resp.status();
        if status.as_u16() == 404 {
            anyhow::bail!(RegistryError::PackageNotFound(name.clone()));
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
}
