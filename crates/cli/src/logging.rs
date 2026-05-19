//! Debug logging for orix CLI.
//!
//! Design:
//! - User-facing progress UI is handled by `reporter`.
//! - Developer diagnostics are handled by `tracing`.
//! - Debug logs are written to a file by default when `--debug` is enabled.
//! - Console tracing is opt-in via `ORIX_LOG` / `--log` and should disable live progress.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use tracing_subscriber::fmt;
use tracing_subscriber::prelude::*;
use tracing_subscriber::EnvFilter;

use crate::reporter::ColorMode;

/// Runtime logging configuration parsed from CLI flags and environment variables.
#[derive(Debug, Clone)]
pub struct LogConfig {
    /// EnvFilter-compatible filter, for example:
    /// `orix=debug,orix_resolver=trace`.
    pub filter: Option<String>,

    /// Enable debug log file.
    pub debug: bool,

    /// Explicit log file path.
    pub log_file: Option<PathBuf>,

    /// Color mode for console logs.
    pub color_mode: ColorMode,
}

/// Guard returned by `init_logging`.
///
/// Keep this value alive until process exit, otherwise non-blocking file logging
/// may stop flushing.
#[derive(Debug)]
pub struct LogHandle {
    _file_guard: Option<tracing_appender::non_blocking::WorkerGuard>,
    log_file: Option<PathBuf>,
    console_enabled: bool,
    #[allow(dead_code)]
    file_enabled: bool,
}

impl LogHandle {
    /// Whether console tracing is enabled.
    ///
    /// When this is true, crossterm progress should be disabled to avoid
    /// stderr contention.
    pub fn console_enabled(&self) -> bool {
        self.console_enabled
    }

    /// Whether file logging is enabled.
    #[allow(dead_code)]
    pub fn file_enabled(&self) -> bool {
        self.file_enabled
    }

    /// File path used for debug log output.
    pub fn log_file(&self) -> Option<&Path> {
        self.log_file.as_deref()
    }
}

/// Initialize tracing.
///
/// Rules:
/// - no `--debug`, no `--log`, no `--log-file` => tracing disabled
/// - `--debug` => file logging enabled, default filter `orix=debug`
/// - `--log-file` => file logging enabled
/// - `--log` / `ORIX_LOG` => console logging enabled
pub fn init_logging(config: LogConfig) -> Result<LogHandle> {
    let console_enabled = config.filter.is_some() && !config.debug && config.log_file.is_none();
    let file_enabled = config.debug || config.log_file.is_some();

    if !console_enabled && !file_enabled {
        return Ok(LogHandle {
            _file_guard: None,
            log_file: None,
            console_enabled: false,
            file_enabled: false,
        });
    }

    let filter = build_filter(config.filter.as_deref(), config.debug, file_enabled)?;

    let use_ansi = match config.color_mode {
        ColorMode::Always => true,
        ColorMode::Never => false,
        ColorMode::Auto => std::io::IsTerminal::is_terminal(&std::io::stderr()),
    };

    let console_layer = if console_enabled {
        Some(
            fmt::layer()
                .compact()
                .with_ansi(use_ansi)
                .with_target(true)
                .with_thread_ids(false)
                .with_thread_names(false)
                .with_writer(std::io::stderr),
        )
    } else {
        None
    };

    let mut file_guard = None;
    let mut final_log_file = None;

    let file_layer = if file_enabled {
        let log_file = resolve_log_file(config.log_file)?;
        ensure_parent_dir(&log_file)?;

        let file_name = log_file
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("orix-debug.log")
            .to_string();

        let log_dir = log_file
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));

        let appender = tracing_appender::rolling::never(log_dir, file_name);
        let (non_blocking, guard) = tracing_appender::non_blocking(appender);

        file_guard = Some(guard);
        final_log_file = Some(log_file);

        Some(
            fmt::layer()
                .compact()
                .with_ansi(false)
                .with_target(true)
                .with_thread_ids(true)
                .with_thread_names(false)
                .with_writer(non_blocking),
        )
    } else {
        None
    };

    tracing_subscriber::registry()
        .with(filter)
        .with(console_layer)
        .with(file_layer)
        .try_init()
        .context("failed to initialize tracing subscriber")?;

    Ok(LogHandle {
        _file_guard: file_guard,
        log_file: final_log_file,
        console_enabled,
        file_enabled,
    })
}

fn build_filter(filter: Option<&str>, debug: bool, file_enabled: bool) -> Result<EnvFilter> {
    let value = match filter {
        Some(filter) if !filter.trim().is_empty() => filter.to_string(),
        _ if debug || file_enabled => {
            "orix=debug,orix_core=debug,orix_resolver=debug,orix_fetcher=debug,orix_linker=debug,orix_store=debug".to_string()
        }
        _ => "warn".to_string(),
    };

    EnvFilter::try_new(value).context("invalid ORIX_LOG filter")
}

fn resolve_log_file(explicit: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        return Ok(path);
    }

    let base = dirs::data_local_dir()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);

    let pid = std::process::id();

    Ok(base
        .join("orix")
        .join("logs")
        .join(format!("orix-{ts}-{pid}.log")))
}

fn ensure_parent_dir(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create log directory {}", parent.display()))?;
    }

    Ok(())
}
