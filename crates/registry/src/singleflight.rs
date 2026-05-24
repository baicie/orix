//! Single-flight pattern for deduplicating concurrent packument requests.
//!
//! When multiple concurrent resolutions request the same package name,
//! only one HTTP request is made and the result is shared.

use std::collections::HashMap;
use std::time::Duration;

use tokio::sync::watch;

/// A single-flight guard that allows sharing a single in-flight request.
///
/// When dropped, signals that the request is complete.
pub struct SingleFlightGuard {
    /// Sender to notify waiting tasks.
    _sender: watch::Sender<()>,
}

impl Drop for SingleFlightGuard {
    fn drop(&mut self) {
        // Dropping the sender notifies all receivers.
    }
}

/// Shared state for single-flight packument fetching.
#[derive(Default)]
pub struct PackumentSingleFlight {
    /// Map of package name -> watch channel for in-flight requests.
    /// The watch channel closes when the request completes.
    in_flight: tokio::sync::RwLock<HashMap<String, watch::Receiver<()>>>,
}

impl PackumentSingleFlight {
    /// Create a new single-flight tracker.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a new in-flight request for a package.
    ///
    /// Returns `None` if another request for this package is already in flight.
    /// Returns `Some(guard)` if this is the first request for this package.
    pub async fn register(&self, name: &str) -> Option<SingleFlightGuard> {
        let mut guard = self.in_flight.write().await;

        // Check if already in flight.
        if guard.contains_key(name) {
            return None;
        }

        // Create a new watch channel.
        let (sender, receiver) = watch::channel(());

        // Use entry API to insert and get the receiver.
        guard.insert(name.to_string(), receiver);

        Some(SingleFlightGuard { _sender: sender })
    }

    /// Wait for another in-flight request to complete.
    ///
    /// Returns immediately if there's no in-flight request.
    /// Returns `true` if the request completed (success or failure).
    pub async fn wait(&self, name: &str, _timeout: Duration) -> bool {
        let receiver = {
            let guard = self.in_flight.read().await;
            guard.get(name).cloned()
        };

        if let Some(mut receiver) = receiver {
            // Wait for the channel to close (sender dropped = request complete).
            let _ = receiver.changed().await;
            true
        } else {
            true // No in-flight request, nothing to wait for.
        }
    }

    /// Wait for another in-flight request to complete (without timeout).
    ///
    /// Returns when the channel is closed.
    pub async fn wait_until_complete(&self, name: &str) {
        if let Some(mut receiver) = {
            let guard = self.in_flight.read().await;
            guard.get(name).cloned()
        } {
            let _ = receiver.changed().await;
        }
    }

    /// Unregister an in-flight request (called when request completes or fails).
    pub async fn unregister(&self, name: &str) {
        let mut guard = self.in_flight.write().await;
        guard.remove(name);
    }

    /// Get the number of in-flight requests.
    pub async fn len(&self) -> usize {
        let guard = self.in_flight.read().await;
        guard.len()
    }

    /// Check if there are any in-flight requests.
    pub async fn is_empty(&self) -> bool {
        let guard = self.in_flight.read().await;
        guard.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn singleflight_register_returns_none_when_in_flight() {
        let sf = PackumentSingleFlight::new();

        let guard1 = sf.register("lodash").await;
        assert!(guard1.is_some());

        let guard2 = sf.register("lodash").await;
        assert!(guard2.is_none());
    }

    #[tokio::test]
    async fn singleflight_unregister_allows_new_requests() {
        let sf = PackumentSingleFlight::new();

        let guard = sf.register("lodash").await;
        assert!(guard.is_some());

        sf.unregister("lodash").await;

        let guard2 = sf.register("lodash").await;
        assert!(guard2.is_some());
    }

    #[tokio::test]
    async fn singleflight_len_tracks_in_flight_requests() {
        let sf = PackumentSingleFlight::new();

        assert_eq!(sf.len().await, 0);

        sf.register("lodash").await;
        sf.register("react").await;

        assert_eq!(sf.len().await, 2);

        sf.unregister("lodash").await;
        assert_eq!(sf.len().await, 1);
    }
}
