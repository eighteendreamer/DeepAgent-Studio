//! # deepagent-mcp
//!
//! Model Context Protocol support (开发计划.md Phase 12; modeled on Claude
//! Code's MCP integration).
//!
//! MCP lets the runtime integrate external services as tools. This crate
//! provides the full client side:
//! - [`config`] — the `.mcp.json` schema (stdio / SSE / HTTP / ws server types)
//!   with `${VAR}` environment-variable expansion.
//! - [`protocol`] — JSON-RPC 2.0 envelopes + the MCP method set
//!   (`initialize` / `tools/list` / `tools/call`).
//! - [`transport`] — the [`transport::McpTransport`] abstraction + a
//!   [`transport::MockTransport`] for offline tests.
//! - [`stdio`] — the [`stdio::StdioTransport`] that spawns a local server
//!   process and speaks newline-delimited JSON-RPC.
//! - [`http`] — the [`http::HttpTransport`] (behind the `http` feature) that
//!   speaks MCP's Streamable HTTP transport (JSON or SSE replies) to hosted
//!   `http`/`sse` servers, applying configured auth headers.
//! - [`client`] — the [`client::McpClient`] driving the JSON-RPC lifecycle.
//! - [`registry`] — the [`registry::McpRegistry`] that connects servers and
//!   exposes their tools under the `mcp__<server>__<tool>` namespace, routing
//!   invocations to the owning server.
//! - [`adapter`] — bridges MCP [`registry::RemoteTool`]s into the runtime's
//!   [`deepagent_tools::Tool`] system so they flow through the same capability
//!   registry, permission gating, and runtime loop as built-in tools.
//!
//! Transports are unified behind the [`transport::McpTransport`] trait: stdio
//! (local processes) and HTTP/SSE (hosted servers, `--features http`) are both
//! implemented; WebSocket slots in behind the same trait. A
//! [`transport::MockTransport`] enables full offline testing.

pub mod adapter;
pub mod client;
pub mod config;
pub mod connect;
#[cfg(feature = "http")]
pub mod http;
pub mod protocol;
pub mod registry;
pub mod stdio;
pub mod transport;

pub use adapter::{adapters_for, McpToolAdapter};
pub use client::McpClient;
pub use config::{McpConfig, McpServerConfig, TransportType};
pub use connect::connect_transport;
pub use protocol::{McpToolDef, SseFrames, ToolCallResult, ToolsListResult};
pub use registry::{namespaced_name, split_namespaced, McpRegistry, RemoteTool};
pub use stdio::StdioTransport;
pub use transport::{McpTransport, MockTransport};

#[cfg(feature = "http")]
pub use http::HttpTransport;
