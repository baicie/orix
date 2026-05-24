//! Streaming install pipeline configuration and utilities.
//!
//! This module provides configuration for the streaming pipeline feature,
//! which allows resolve, fetch, and import phases to overlap.
//!
//! ## Configuration
//!
//! Use the `ORIX_PIPELINE` environment variable:
//!
//! - `ORIX_PIPELINE=streaming` - Enable streaming pipeline (default when implemented)
//! - `ORIX_PIPELINE=serial` - Disable streaming, use serial pipeline
//!
//! ## Status
//!
//! This is a scaffold for future implementation. The streaming pipeline
//! is not yet integrated into the main install flow.

use tracing::debug;

/// Streaming pipeline configuration.
#[derive(Debug, Clone)]
pub struct StreamingConfig {
    /// Channel buffer size for resolved packages.
    pub resolve_queue_size: usize,
    /// Channel buffer size for fetch progress.
    pub fetch_queue_size: usize,
    /// Resolve concurrency.
    pub resolve_concurrency: usize,
    /// Fetch concurrency.
    pub fetch_concurrency: usize,
    /// Enable streaming mode.
    pub enabled: bool,
}

impl Default for StreamingConfig {
    fn default() -> Self {
        Self {
            resolve_queue_size: 1024,
            fetch_queue_size: 8192,
            resolve_concurrency: 10,
            fetch_concurrency: 10,
            enabled: false, // Disabled by default until fully implemented
        }
    }
}

impl StreamingConfig {
    /// Create config from environment variable `ORIX_PIPELINE`.
    pub fn from_env() -> Self {
        let mut config = Self::default();

        if let Ok(val) = std::env::var("ORIX_PIPELINE") {
            match val.as_str() {
                "streaming" => {
                    debug!(target: "orix::perf", "streaming pipeline enabled via ORIX_PIPELINE");
                    config.enabled = true;
                }
                "serial" | "disabled" => {
                    debug!(target: "orix::perf", "streaming pipeline disabled");
                    config.enabled = false;
                }
                _ => {
                    debug!(
                        target: "orix::perf",
                        value = %val,
                        "unknown ORIX_PIPELINE value, using default"
                    );
                }
            }
        }

        config
    }
}

/// Check if streaming pipeline should be used based on configuration.
pub fn should_use_streaming() -> bool {
    StreamingConfig::from_env().enabled
}

/// Result of a streaming pipeline execution.
#[derive(Debug)]
#[allow(dead_code)]
pub struct StreamingResult {
    /// Number of packages resolved.
    pub packages_resolved: usize,
    /// Number of packages fetched.
    pub packages_fetched: usize,
    /// Whether streaming was used.
    pub streaming_used: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streaming_config_defaults_disabled() {
        let config = StreamingConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.resolve_queue_size, 1024);
        assert_eq!(config.fetch_queue_size, 8192);
    }

    #[test]
    fn streaming_config_from_env_unknown() {
        std::env::set_var("ORIX_PIPELINE", "unknown");
        let config = StreamingConfig::from_env();
        assert!(!config.enabled);
        std::env::remove_var("ORIX_PIPELINE");
    }

    #[test]
    fn streaming_config_from_env_disabled() {
        std::env::set_var("ORIX_PIPELINE", "serial");
        let config = StreamingConfig::from_env();
        assert!(!config.enabled);
        std::env::remove_var("ORIX_PIPELINE");
    }

    #[test]
    fn streaming_config_from_env_enabled() {
        std::env::set_var("ORIX_PIPELINE", "streaming");
        let config = StreamingConfig::from_env();
        assert!(config.enabled);
        std::env::remove_var("ORIX_PIPELINE");
    }

    #[test]
    fn should_use_streaming_respects_env() {
        std::env::set_var("ORIX_PIPELINE", "streaming");
        assert!(should_use_streaming());

        std::env::set_var("ORIX_PIPELINE", "serial");
        assert!(!should_use_streaming());

        std::env::remove_var("ORIX_PIPELINE");
    }
}
