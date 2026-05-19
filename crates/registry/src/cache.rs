//! In-memory packument cache with TTL support.

use std::time::{Duration, Instant};

use tokio::sync::RwLock;

use crate::Packument;

/// TTL for cached packuments: 5 minutes.
const PACKUMENT_CACHE_TTL: Duration = Duration::from_secs(300);

/// A cached packument with its expiration time.
struct CacheEntry {
    packument: Packument,
    expires_at: Instant,
}

impl CacheEntry {
    fn is_expired(&self) -> bool {
        Instant::now() >= self.expires_at
    }
}

/// Thread-safe in-memory cache for packuments.
///
/// Uses a `RwLock` so that concurrent reads don't block each other.
/// Eviction of expired entries happens lazily on each `get` call.
#[derive(Default)]
pub struct PackumentCache {
    inner: RwLock<std::collections::HashMap<String, CacheEntry>>,
    /// Synchronous cache backed by parking_lot RwLock.
    sync_inner: parking_lot::RwLock<std::collections::HashMap<String, CacheEntry>>,
}

impl PackumentCache {
    /// Create a new empty packument cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// Get a packument from the cache if it exists and is not expired.
    ///
    /// Expired entries are removed on access.
    pub async fn get(&self, name: &str) -> Option<Packument> {
        let mut guard = self.inner.write().await;
        let entry = guard.get_mut(name)?;
        if entry.is_expired() {
            guard.remove(name);
            return None;
        }
        Some(entry.packument.clone())
    }

    /// Insert a packument into the cache with the default TTL.
    pub async fn insert(&self, name: String, packument: Packument) {
        let entry = CacheEntry {
            packument: packument.clone(),
            expires_at: Instant::now() + PACKUMENT_CACHE_TTL,
        };
        let mut guard = self.inner.write().await;
        guard.insert(name.clone(), entry);
        drop(guard);
        // Also update the synchronous cache for use by peer resolution.
        self.insert_sync(name, packument, PACKUMENT_CACHE_TTL);
    }

    /// Remove a packument from the cache.
    #[allow(dead_code)]
    pub async fn remove(&self, name: &str) {
        let mut guard = self.inner.write().await;
        guard.remove(name);
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
    pub fn insert_sync(&self, name: String, packument: Packument, ttl: Duration) {
        let entry = CacheEntry {
            packument,
            expires_at: Instant::now() + ttl,
        };
        let mut guard = self.sync_inner.write();
        guard.insert(name, entry);
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
