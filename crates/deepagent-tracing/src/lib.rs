//! # deepagent-tracing
//!
//! Observability bootstrap for the runtime (开发计划.md Phase 1 §6 and Phase 10).
//!
//! This crate centralizes initialization of the [`tracing`] subscriber so every
//! binary (desktop app, CLI, tests) gets consistent structured logging. It also
//! provides a lightweight in-process [`metrics::Metrics`] registry for the
//! runtime counters surfaced later in the Agent Timeline (tokens, cache hits,
//! tool latency, retries, cost).
//!
//! OpenTelemetry export is layered in during Phase 10; the API here is designed
//! so that wiring an OTLP exporter later does not change call sites.

pub mod metrics;
pub mod trace_context;

use std::sync::Once;

use tracing_subscriber::{fmt, prelude::*, EnvFilter};

static INIT: Once = Once::new();

/// Output format for the tracing subscriber.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFormat {
    /// Human-readable, good for local development.
    Pretty,
    /// Single-line JSON, good for log aggregation / production.
    Json,
}

/// Configuration for [`init`].
#[derive(Debug, Clone)]
pub struct TracingConfig {
    /// Default level filter when `RUST_LOG` is unset (e.g. "info").
    pub default_directive: String,
    /// Output format.
    pub format: LogFormat,
    /// Whether to include source file/line in events.
    pub with_location: bool,
}

impl Default for TracingConfig {
    fn default() -> Self {
        Self {
            default_directive: "info,deepagent=debug".to_string(),
            format: LogFormat::Pretty,
            with_location: false,
        }
    }
}

/// Initialize global tracing. Safe to call multiple times; only the first call
/// has any effect (subsequent calls are no-ops), which makes it convenient to
/// call from individual tests.
pub fn init(config: TracingConfig) {
    INIT.call_once(|| {
        let filter = EnvFilter::try_from_default_env()
            .or_else(|_| EnvFilter::try_new(&config.default_directive))
            .unwrap_or_else(|_| EnvFilter::new("info"));

        let registry = tracing_subscriber::registry().with(filter);

        match config.format {
            LogFormat::Pretty => {
                let layer = fmt::layer()
                    .with_target(true)
                    .with_file(config.with_location)
                    .with_line_number(config.with_location);
                let _ = registry.with(layer).try_init();
            }
            LogFormat::Json => {
                let layer = fmt::layer()
                    .json()
                    .with_target(true)
                    .with_file(config.with_location)
                    .with_line_number(config.with_location);
                let _ = registry.with(layer).try_init();
            }
        }
    });
}

/// Convenience: initialize with development-friendly defaults.
pub fn init_dev() {
    init(TracingConfig::default());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_is_idempotent() {
        init_dev();
        init_dev(); // must not panic
        init(TracingConfig {
            format: LogFormat::Json,
            ..Default::default()
        });
    }
}
