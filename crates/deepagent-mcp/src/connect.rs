//! Transport selection: build the right [`McpTransport`] for a server config.
//!
//! This centralizes the stdio-vs-network decision so callers (the registry,
//! the app shell) don't branch on [`TransportType`] themselves. Network
//! transports (`http`/`sse`) require the `http` feature; without it they return
//! a clear error rather than silently failing.

use std::sync::Arc;

use deepagent_core::error::{CoreError, Result};

use crate::config::{McpServerConfig, TransportType};
use crate::stdio::StdioTransport;
use crate::transport::McpTransport;

/// Build a transport for `config`, choosing stdio or HTTP/SSE by its effective
/// type. The config is validated first.
pub fn connect_transport(config: &McpServerConfig) -> Result<Arc<dyn McpTransport>> {
    config.validate()?;
    match config.effective_type()? {
        TransportType::Stdio => Ok(Arc::new(StdioTransport::spawn(config)?)),
        TransportType::Http | TransportType::Sse => connect_network(config),
        TransportType::Ws => Err(CoreError::invalid(
            "WebSocket MCP transport is not yet implemented; use stdio or http/sse",
        )),
    }
}

#[cfg(feature = "http")]
fn connect_network(config: &McpServerConfig) -> Result<Arc<dyn McpTransport>> {
    Ok(Arc::new(crate::http::HttpTransport::connect(config)?))
}

#[cfg(not(feature = "http"))]
fn connect_network(_config: &McpServerConfig) -> Result<Arc<dyn McpTransport>> {
    Err(CoreError::invalid(
        "network MCP transport requires building deepagent-mcp with the 'http' feature",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn network_cfg(t: TransportType, url: &str) -> McpServerConfig {
        McpServerConfig {
            transport: Some(t),
            command: None,
            args: vec![],
            env: Default::default(),
            url: Some(url.into()),
            headers: Default::default(),
        }
    }

    #[test]
    fn ws_is_unimplemented() {
        let cfg = network_cfg(TransportType::Ws, "wss://example.com/ws");
        assert!(connect_transport(&cfg).is_err());
    }

    #[cfg(feature = "http")]
    #[test]
    fn http_builds_transport() {
        let cfg = network_cfg(TransportType::Http, "https://mcp.example.com/mcp");
        assert!(connect_transport(&cfg).is_ok());
    }

    #[cfg(not(feature = "http"))]
    #[test]
    fn http_without_feature_errors() {
        let cfg = network_cfg(TransportType::Http, "https://mcp.example.com/mcp");
        assert!(connect_transport(&cfg).is_err());
    }
}
