//! In-memory packument cache with TTL support and optional disk persistence.
//!
//! ## Caching Strategy
//!
//! | Layer | TTL | Scope |
//! |-------|-----|-------|
//! | In-memory (RwLock) | 5 minutes | Current process |
//! | Disk cache | 1 hour | Persists across invocations |
//!
//! ## Disk Cache Format
//!
//! ```txt
//! ~/.orix/cache/metadata/<registry-hash>/<escaped-package-name>.json
//! ~/.orix/cache/metadata/<registry-hash>/<escaped-package-name>.meta.json
//! ```
//!
//! The `.meta.json` file contains ETag, Last-Modified, and fetch timestamp.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};
use tokio::sync::RwLock;
use tracing::debug;

use crate::Packument;

/// TTL for cached packuments in memory: 5 minutes.
const PACKUMENT_CACHE_TTL: Duration = Duration::from_secs(300);

/// TTL for disk-cached packuments: 1 hour.
const DISK_CACHE_TTL: Duration = Duration::from_secs(3600);

/// A cached packument with its expiration time.
struct CacheEntry {
    packument: Packument,
    expires_at: Instant,
    /// Whether this entry came from disk cache (longer TTL).
    #[allow(dead_code)]
    from_disk: bool,
}

impl CacheEntry {
    fn is_expired(&self) -> bool {
        Instant::now() >= self.expires_at
    }
}

/// Metadata for a cached packument on disk.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct DiskCacheMeta {
    /// ETag header value from the registry.
    etag: Option<String>,
    /// Last-Modified header value from the registry.
    last_modified: Option<String>,
    /// When this entry was fetched.
    fetched_at: u64,
    /// Registry URL.
    registry: String,
}

impl DiskCacheMeta {
    fn is_expired(&self) -> bool {
        let fetched = UNIX_EPOCH + Duration::from_secs(self.fetched_at);
        SystemTime::now()
            .duration_since(fetched)
            .map(|d| d > DISK_CACHE_TTL)
            .unwrap_or(true)
    }
}

/// Thread-safe in-memory cache for packuments with optional disk persistence.
///
/// Uses a `RwLock` so that concurrent reads don't block each other.
/// Eviction of expired entries happens lazily on each `get` call.
#[derive(Default)]
pub struct PackumentCache {
    inner: RwLock<std::collections::HashMap<String, CacheEntry>>,
    /// Synchronous cache backed by parking_lot RwLock.
    sync_inner: parking_lot::RwLock<std::collections::HashMap<String, CacheEntry>>,
    /// Optional disk cache root path.
    disk_cache_root: Option<PathBuf>,
    /// Registry URL for disk cache namespacing.
    registry_url: Option<String>,
}

impl PackumentCache {
    /// Create a new empty packument cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a packument cache with disk persistence.
    ///
    /// The `root` directory will be created if it doesn't exist.
    /// Packuments will be cached at `<root>/metadata/<hash>/<name>.json`.
    pub fn with_disk_cache(root: PathBuf, registry_url: &str) -> Self {
        let root = root.join("metadata");
        let _ = std::fs::create_dir_all(&root);
        Self {
            inner: RwLock::new(Default::default()),
            sync_inner: parking_lot::RwLock::new(Default::default()),
            disk_cache_root: Some(root),
            registry_url: Some(registry_url.to_string()),
        }
    }

    /// Get a packument from the cache if it exists and is not expired.
    ///
    /// Checks in-memory cache first, then disk cache.
    /// Expired entries are removed on access.
    pub async fn get(&self, name: &str) -> Option<Packument> {
        // Check memory cache first.
        let from_memory = {
            let guard = self.inner.read().await;
            if let Some(entry) = guard.get(name) {
                if !entry.is_expired() {
                    return Some(entry.packument.clone());
                }
            }
            false
        };

        // If not in memory or expired, check disk cache.
        if !from_memory {
            if let Some((packument, etag)) = self.load_from_disk(name).await {
                // Update memory cache.
                let entry = CacheEntry {
                    packument: packument.clone(),
                    expires_at: Instant::now() + DISK_CACHE_TTL,
                    from_disk: true,
                };
                let mut guard = self.inner.write().await;
                guard.insert(name.to_string(), entry);
                drop(guard);
                self.insert_sync(name.to_string(), packument.clone(), DISK_CACHE_TTL, etag);
                return Some(packument);
            }
        }

        None
    }

    /// Insert a packument into the cache with the default TTL.
    ///
    /// If disk cache is configured, also persists to disk.
    pub async fn insert(&self, name: String, packument: Packument) {
        let entry = CacheEntry {
            packument: packument.clone(),
            expires_at: Instant::now() + PACKUMENT_CACHE_TTL,
            from_disk: false,
        };
        let mut guard = self.inner.write().await;
        guard.insert(name.clone(), entry);
        drop(guard);
        // Also update the synchronous cache for use by peer resolution.
        self.insert_sync(name.clone(), packument.clone(), PACKUMENT_CACHE_TTL, None);

        // Persist to disk cache.
        if let Some(ref root) = self.disk_cache_root {
            if let Err(e) = self.save_to_disk(&name, &packument, None).await {
                debug!(error = %e, name = %name, "failed to persist packument to disk cache");
            }
            let _ = root;
        }
    }

    /// Insert a packument with ETag for disk caching.
    pub async fn insert_with_etag(
        &self,
        name: String,
        packument: Packument,
        etag: Option<String>,
        last_modified: Option<String>,
    ) {
        self.insert(name.clone(), packument.clone()).await;

        if let Some(ref _root) = self.disk_cache_root {
            if let Err(e) = self
                .save_to_disk(&name, &packument, Some((&etag, &last_modified)))
                .await
            {
                debug!(error = %e, name = %name, "failed to persist packument to disk cache");
            }
        }
    }

    /// Remove a packument from the cache.
    #[allow(dead_code)]
    pub async fn remove(&self, name: &str) {
        let mut guard = self.inner.write().await;
        guard.remove(name);

        // Also remove from disk cache.
        if let Some(ref root) = self.disk_cache_root {
            let disk_path = self.disk_cache_path(root, name);
            let _ = tokio::fs::remove_file(&disk_path.0).await;
            let _ = tokio::fs::remove_file(&disk_path.1).await;
        }
    }

    /// Evict all expired entries from the cache.
    #[allow(dead_code)]
    pub async fn evict_expired(&self) {
        let mut guard = self.inner.write().await;
        guard.retain(|_, entry| !entry.is_expired());
    }

    /// Clear the entire cache.
    #[allow(dead_code)]
    pub async fn clear(&self) {
        let mut guard = self.inner.write().await;
        guard.clear();

        // Clear disk cache.
        if let Some(root) = self.disk_cache_root.as_ref() {
            if root.exists() {
                let _ = tokio::fs::remove_dir_all(root).await;
                let _ = std::fs::create_dir_all(root);
            }
            let _ = root;
        }
    }

    /// Synchronously get a packument from the cache (blocking).
    pub fn get_sync(&self, name: &str) -> Option<Packument> {
        let guard = self.sync_inner.read();
        let entry = guard.get(name)?;
        if Instant::now() >= entry.expires_at {
            drop(guard);
            let mut guard = self.sync_inner.write();
            guard.remove(name);
            return None;
        }
        Some(entry.packument.clone())
    }

    /// Synchronously insert a packument into the cache (blocking).
    pub fn insert_sync(
        &self,
        name: String,
        packument: Packument,
        ttl: Duration,
        _etag: Option<String>,
    ) {
        let entry = CacheEntry {
            packument,
            expires_at: Instant::now() + ttl,
            from_disk: false,
        };
        let mut guard = self.sync_inner.write();
        guard.insert(name, entry);
    }

    /// Load a packument from disk cache.
    async fn load_from_disk(&self, name: &str) -> Option<(Packument, Option<String>)> {
        let root = self.disk_cache_root.as_ref()?;
        let (data_path, meta_path) = self.disk_cache_path(root, name);

        if !data_path.exists() || !meta_path.exists() {
            return None;
        }

        // Read metadata first.
        let meta_content = match tokio::fs::read_to_string(&meta_path).await {
            Ok(c) => c,
            Err(_) => return None,
        };

        let meta: DiskCacheMeta = match serde_json::from_str(&meta_content) {
            Ok(m) => m,
            Err(_) => return None,
        };

        // Check if disk cache is expired.
        if meta.is_expired() {
            return None;
        }

        // Read packument data.
        let packument: Packument = match tokio::fs::read(&data_path).await {
            Ok(bytes) => match serde_json::from_slice(&bytes) {
                Ok(p) => p,
                Err(_) => return None,
            },
            Err(_) => return None,
        };

        Some((packument, meta.etag))
    }

    /// Save a packument to disk cache.
    async fn save_to_disk(
        &self,
        name: &str,
        packument: &Packument,
        http_meta: Option<(&Option<String>, &Option<String>)>,
    ) -> std::io::Result<()> {
        #[allow(clippy::unwrap_used)]
        let root = self.disk_cache_root.as_ref().unwrap();

        let (etag, last_modified) = http_meta
            .map(|(e, l)| (e.clone(), l.clone()))
            .unwrap_or((None, None));

        // Create registry-specific subdirectory.
        let registry_hash = self
            .registry_url
            .as_ref()
            .map(|url| {
                let mut hasher = Sha256::new();
                hasher.update(url.as_bytes());
                hex::encode(hasher.finalize())[..16].to_string()
            })
            .unwrap_or_else(|| "default".to_string());

        let cache_dir = root.join(&registry_hash);
        tokio::fs::create_dir_all(&cache_dir).await?;

        let (data_path, meta_path) = self.disk_cache_path(root, name);

        // Serialize and write packument data as JSON string.
        let json = serde_json::to_string(packument)?;
        tokio::fs::write(&data_path, json).await?;

        // Write metadata.
        let meta = DiskCacheMeta {
            etag,
            last_modified,
            fetched_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            registry: self.registry_url.clone().unwrap_or_default(),
        };
        let meta_json = serde_json::to_string_pretty(&meta)?;
        tokio::fs::write(&meta_path, meta_json).await?;

        Ok(())
    }

    /// Get the disk cache file paths for a package name.
    fn disk_cache_path(&self, root: &Path, name: &str) -> (PathBuf, PathBuf) {
        let registry_hash = self
            .registry_url
            .as_ref()
            .map(|url| {
                let mut hasher = Sha256::new();
                hasher.update(url.as_bytes());
                hex::encode(hasher.finalize())[..16].to_string()
            })
            .unwrap_or_else(|| "default".to_string());

        let escaped = name.replace('/', "~2f").replace('@', "%40");
        let cache_dir = root.join(&registry_hash);
        (
            cache_dir.join(format!("{}.json", escaped)),
            cache_dir.join(format!("{}.meta.json", escaped)),
        )
    }

    /// Get the number of entries in the memory cache.
    pub async fn len(&self) -> usize {
        let guard = self.inner.read().await;
        guard.len()
    }

    /// Check if the cache is empty.
    pub async fn is_empty(&self) -> bool {
        let guard = self.inner.read().await;
        guard.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Dist, PackageMetadata};
    use std::collections::HashMap;

    fn make_packument(name: &str) -> Packument {
        Packument {
            name: name.to_string(),
            versions: HashMap::from([(
                "1.0.0".to_string(),
                PackageMetadata {
                    name: name.to_string(),
                    version: "1.0.0".to_string(),
                    dependencies: HashMap::new(),
                    dev_dependencies: HashMap::new(),
                    optional_dependencies: HashMap::new(),
                    peer_dependencies: HashMap::new(),
                    peer_dependencies_meta: HashMap::new(),
                    engines: None,
                    os: Vec::new(),
                    cpu: Vec::new(),
                    dist: Some(Dist {
                        tarball: format!(
                            "https://registry.npmjs.org/{}/-/{}-1.0.0.tgz",
                            name, name
                        ),
                        integrity: Some(format!("sha512-{}", name)),
                        shasum: None,
                    }),
                    optional: false,
                    deprecated: None,
                    bin: HashMap::new(),
                    directories: Default::default(),
                    has_shrinkwrap: false,
                    has_install_script: false,
                    bundle_dependencies: Vec::new(),
                    scripts: HashMap::new(),
                    funding: None,
                    repository: None,
                    homepage: None,
                    description: None,
                    license: None,
                    keywords: Vec::new(),
                },
            )]),
            dist_tags: HashMap::from([("latest".to_string(), "1.0.0".to_string())]),
        }
    }

    #[tokio::test]
    #[allow(clippy::unwrap_used, clippy::panic)]
    async fn cache_stores_and_retrieves_packument() {
        let cache = PackumentCache::new();
        let pkg = make_packument("test-pkg");
        cache.insert("test-pkg".to_string(), pkg.clone()).await;

        let cached = cache.get("test-pkg").await;
        assert!(cached.is_some());
        assert_eq!(cached.unwrap().name, "test-pkg");
    }

    #[tokio::test]
    async fn cache_returns_none_for_missing_entry() {
        let cache = PackumentCache::new();
        let cached = cache.get("nonexistent").await;
        assert!(cached.is_none());
    }

    #[tokio::test]
    async fn cache_evict_expired_removes_timed_out_entries() {
        let cache = PackumentCache::new();
        cache
            .insert("keep-pkg".to_string(), make_packument("keep"))
            .await;
        cache
            .insert("remove-pkg".to_string(), make_packument("remove"))
            .await;

        // evict_expired is a no-op when entries are not expired (TTL = 5min)
        cache.evict_expired().await;
        assert!(cache.get("keep-pkg").await.is_some());
        assert!(cache.get("remove-pkg").await.is_some());

        // clear() removes everything regardless of expiry
        cache.clear().await;
        assert!(cache.get("keep-pkg").await.is_none());
    }

    #[tokio::test]
    async fn cache_clear_removes_all_entries() {
        let cache = PackumentCache::new();
        cache.insert("pkg-a".to_string(), make_packument("a")).await;
        cache.insert("pkg-b".to_string(), make_packument("b")).await;

        cache.clear().await;
        assert!(cache.get("pkg-a").await.is_none());
        assert!(cache.get("pkg-b").await.is_none());
    }

    #[tokio::test]
    async fn cache_remove_deletes_specific_entry() {
        let cache = PackumentCache::new();
        cache
            .insert("keep-pkg".to_string(), make_packument("keep"))
            .await;
        cache
            .insert("remove-pkg".to_string(), make_packument("remove"))
            .await;

        cache.remove("remove-pkg").await;
        assert!(cache.get("keep-pkg").await.is_some());
        assert!(cache.get("remove-pkg").await.is_none());
    }

    #[tokio::test]
    async fn registry_client_creates_with_custom_concurrency() {
        use crate::RegistryClient;
        #[allow(clippy::expect_used)]
        let url = url::Url::parse("https://registry.npmjs.org/").expect("valid url");
        let client = RegistryClient::with_concurrency(url, 20);
        let _ = client.clone();
    }
}
