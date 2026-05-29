//! MCP server configuration — the `.mcp.json` schema used by Claude Code.
//!
//! A config maps a server name to a [`McpServerConfig`]. Servers are one of four
//! transport types (开发计划.md Phase 12; claude_code_ui_Agent MCP skill):
//! - **stdio** — a local child process (`command` + `args` + `env`),
//! - **sse** — hosted Server-Sent-Events endpoint (`url` + `headers`),
//! - **http** — REST endpoint (`url` + `headers`),
//! - **ws** — WebSocket endpoint (`url` + `headers`).
//!
//! All string fields support `${VAR}` environment-variable expansion (and
//! `${CLAUDE_PLUGIN_ROOT}` / `${CLAUDE_PROJECT_DIR}` style placeholders supplied
//! by the caller), matching Claude Code's behaviour.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use deepagent_core::error::{CoreError, Result};

/// The top-level `.mcp.json` document: `{ "mcpServers": { name: config } }`.
///
/// Claude Code also accepts a bare `{ name: config }` map; [`McpConfig::parse`]
/// handles both shapes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpConfig {
    /// Servers keyed by name.
    #[serde(rename = "mcpServers", default)]
    pub servers: BTreeMap<String, McpServerConfig>,
}

impl McpConfig {
    /// Parse from JSON, accepting either `{ "mcpServers": {...} }` or a bare
    /// `{ name: config }` map.
    pub fn parse(json: &str) -> Result<Self> {
        // Try the wrapped form first.
        if let Ok(cfg) = serde_json::from_str::<McpConfig>(json) {
            if !cfg.servers.is_empty() {
                return Ok(cfg);
            }
        }
        // Fall back to a bare map of name -> config, ignoring `_comment` keys.
        let raw: BTreeMap<String, serde_json::Value> = serde_json::from_str(json)?;
        let mut servers = BTreeMap::new();
        for (name, value) in raw {
            if name.starts_with('_') || name == "mcpServers" {
                continue;
            }
            let cfg: McpServerConfig = serde_json::from_value(value)
                .map_err(|e| CoreError::invalid(format!("server '{name}': {e}")))?;
            servers.insert(name, cfg);
        }
        Ok(McpConfig { servers })
    }

    /// Expand `${VAR}` placeholders in every server config using `lookup`.
    pub fn expand_with<F>(&mut self, lookup: &F)
    where
        F: Fn(&str) -> Option<String>,
    {
        for cfg in self.servers.values_mut() {
            cfg.expand_with(lookup);
        }
    }
}

/// A single MCP server's configuration. The transport type is inferred from the
/// `type` field; stdio is the default when `command` is present and `type` is
/// absent (matching Claude Code).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServerConfig {
    /// Explicit transport type. Optional for stdio (inferred from `command`).
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub transport: Option<TransportType>,

    // --- stdio fields ---
    /// Program to execute (stdio).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Process arguments (stdio).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    /// Environment variables for the process (stdio).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,

    // --- network fields (sse / http / ws) ---
    /// Endpoint URL (sse/http/ws).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Request headers (sse/http/ws).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
}

/// MCP transport type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TransportType {
    /// Local child process over stdin/stdout.
    Stdio,
    /// Server-Sent Events (hosted, OAuth).
    Sse,
    /// REST/HTTP (token auth).
    Http,
    /// WebSocket (real-time).
    Ws,
}

impl McpServerConfig {
    /// The effective transport type (inferred when `type` is omitted).
    pub fn effective_type(&self) -> Result<TransportType> {
        if let Some(t) = self.transport {
            return Ok(t);
        }
        if self.command.is_some() {
            Ok(TransportType::Stdio)
        } else if self.url.is_some() {
            // A url without a type is ambiguous; default to http.
            Ok(TransportType::Http)
        } else {
            Err(CoreError::invalid(
                "MCP server config has neither 'command' (stdio) nor 'url' (network)".to_string(),
            ))
        }
    }

    /// Validate that required fields for the (effective) type are present, and
    /// that network URLs use secure schemes.
    pub fn validate(&self) -> Result<()> {
        match self.effective_type()? {
            TransportType::Stdio => {
                if self.command.as_deref().unwrap_or("").is_empty() {
                    return Err(CoreError::invalid("stdio MCP server requires 'command'"));
                }
            }
            TransportType::Sse | TransportType::Http => {
                let url = self
                    .url
                    .as_deref()
                    .ok_or_else(|| CoreError::invalid("network MCP server requires 'url'"))?;
                if !url.starts_with("https://") && !url.starts_with("http://localhost") {
                    return Err(CoreError::invalid(format!(
                        "insecure MCP url '{url}' — use https:// (http allowed only for localhost)"
                    )));
                }
            }
            TransportType::Ws => {
                let url = self
                    .url
                    .as_deref()
                    .ok_or_else(|| CoreError::invalid("ws MCP server requires 'url'"))?;
                if !url.starts_with("wss://") && !url.starts_with("ws://localhost") {
                    return Err(CoreError::invalid(format!(
                        "insecure MCP url '{url}' — use wss:// (ws allowed only for localhost)"
                    )));
                }
            }
        }
        Ok(())
    }

    /// Expand `${VAR}` placeholders in all string fields using `lookup`.
    pub fn expand_with<F>(&mut self, lookup: &F)
    where
        F: Fn(&str) -> Option<String>,
    {
        if let Some(c) = &mut self.command {
            *c = expand(c, lookup);
        }
        for a in &mut self.args {
            *a = expand(a, lookup);
        }
        for v in self.env.values_mut() {
            *v = expand(v, lookup);
        }
        if let Some(u) = &mut self.url {
            *u = expand(u, lookup);
        }
        for v in self.headers.values_mut() {
            *v = expand(v, lookup);
        }
    }
}

/// Expand `${VAR}` occurrences in `input`. Unknown variables expand to empty
/// (matching shell-style expansion); `$${` is not specially handled.
pub fn expand<F>(input: &str, lookup: &F) -> String
where
    F: Fn(&str) -> Option<String>,
{
    let mut out = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'$' && i + 1 < bytes.len() && bytes[i + 1] == b'{' {
            if let Some(end) = input[i + 2..].find('}') {
                let var = &input[i + 2..i + 2 + end];
                out.push_str(&lookup(var).unwrap_or_default());
                i = i + 2 + end + 1;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_wrapped_form() {
        let json = r#"{"mcpServers":{"fs":{"command":"npx","args":["-y","server-filesystem"]}}}"#;
        let cfg = McpConfig::parse(json).unwrap();
        assert_eq!(cfg.servers.len(), 1);
        let fs = &cfg.servers["fs"];
        assert_eq!(fs.command.as_deref(), Some("npx"));
        assert_eq!(fs.effective_type().unwrap(), TransportType::Stdio);
    }

    #[test]
    fn parses_bare_map_ignoring_comments() {
        let json = r#"{"_comment":"hi","api":{"type":"http","url":"https://x.com/mcp"}}"#;
        let cfg = McpConfig::parse(json).unwrap();
        assert_eq!(cfg.servers.len(), 1);
        assert_eq!(
            cfg.servers["api"].effective_type().unwrap(),
            TransportType::Http
        );
    }

    #[test]
    fn infers_stdio_and_http() {
        let stdio = McpServerConfig {
            transport: None,
            command: Some("node".into()),
            args: vec![],
            env: Default::default(),
            url: None,
            headers: Default::default(),
        };
        assert_eq!(stdio.effective_type().unwrap(), TransportType::Stdio);
    }

    #[test]
    fn validates_secure_urls() {
        let mut cfg = McpServerConfig {
            transport: Some(TransportType::Sse),
            command: None,
            args: vec![],
            env: Default::default(),
            url: Some("http://evil.com/sse".into()),
            headers: Default::default(),
        };
        assert!(cfg.validate().is_err());
        cfg.url = Some("https://mcp.asana.com/sse".into());
        assert!(cfg.validate().is_ok());
        // localhost http is allowed.
        cfg.url = Some("http://localhost:8080/sse".into());
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn ws_requires_wss() {
        let cfg = McpServerConfig {
            transport: Some(TransportType::Ws),
            command: None,
            args: vec![],
            env: Default::default(),
            url: Some("ws://example.com/ws".into()),
            headers: Default::default(),
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn expands_env_vars() {
        let mut cfg = McpServerConfig {
            transport: Some(TransportType::Http),
            command: None,
            args: vec![],
            env: Default::default(),
            url: Some("${API_URL}/mcp".into()),
            headers: {
                let mut h = BTreeMap::new();
                h.insert("Authorization".into(), "Bearer ${API_TOKEN}".into());
                h
            },
        };
        let lookup = |v: &str| match v {
            "API_URL" => Some("https://api.example.com".into()),
            "API_TOKEN" => Some("secret123".into()),
            _ => None,
        };
        cfg.expand_with(&lookup);
        assert_eq!(cfg.url.as_deref(), Some("https://api.example.com/mcp"));
        assert_eq!(cfg.headers["Authorization"], "Bearer secret123");
    }

    #[test]
    fn unknown_var_expands_empty() {
        let out = expand("prefix-${MISSING}-suffix", &|_| None);
        assert_eq!(out, "prefix--suffix");
    }

    #[test]
    fn plugin_root_placeholder_expands() {
        let out = expand("${CLAUDE_PLUGIN_ROOT}/servers/db", &|v| {
            if v == "CLAUDE_PLUGIN_ROOT" {
                Some("/plugins/db".into())
            } else {
                None
            }
        });
        assert_eq!(out, "/plugins/db/servers/db");
    }
}
