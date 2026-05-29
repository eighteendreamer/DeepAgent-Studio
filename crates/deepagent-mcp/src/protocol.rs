//! MCP wire protocol — JSON-RPC 2.0 messages and the MCP method set.
//!
//! MCP is JSON-RPC 2.0 over the chosen transport. We model the request/response
//! envelopes plus the specific methods the client uses: `initialize`,
//! `tools/list`, and `tools/call`.

use serde::{Deserialize, Serialize};

/// JSON-RPC protocol version string.
pub const JSONRPC_VERSION: &str = "2.0";

/// The MCP protocol version this client speaks.
pub const MCP_PROTOCOL_VERSION: &str = "2024-11-05";

/// A JSON-RPC request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    /// Always "2.0".
    pub jsonrpc: String,
    /// Correlation id.
    pub id: u64,
    /// Method name (e.g. "tools/call").
    pub method: String,
    /// Method params.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

impl JsonRpcRequest {
    /// Build a request.
    pub fn new(id: u64, method: impl Into<String>, params: Option<serde_json::Value>) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id,
            method: method.into(),
            params,
        }
    }
}

/// A JSON-RPC response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    /// Always "2.0".
    pub jsonrpc: String,
    /// Correlation id.
    pub id: u64,
    /// Result on success.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    /// Error on failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

/// A JSON-RPC error object.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcError {
    /// Error code.
    pub code: i64,
    /// Error message.
    pub message: String,
    /// Optional structured data.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

/// A tool advertised by an MCP server (from `tools/list`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct McpToolDef {
    /// Tool name (server-local, not yet namespaced).
    pub name: String,
    /// Human/model-facing description.
    #[serde(default)]
    pub description: String,
    /// JSON Schema for arguments.
    #[serde(rename = "inputSchema", default)]
    pub input_schema: serde_json::Value,
}

/// The result of `tools/list`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolsListResult {
    /// Advertised tools.
    #[serde(default)]
    pub tools: Vec<McpToolDef>,
}

/// The result of `tools/call`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCallResult {
    /// Content blocks returned by the tool.
    #[serde(default)]
    pub content: Vec<ContentBlock>,
    /// Whether the call ended in an error.
    #[serde(rename = "isError", default)]
    pub is_error: bool,
}

/// A content block in a tool result (MCP supports text/image/resource; we model
/// the common text block and keep the rest as raw JSON).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ContentBlock {
    /// Plain text content.
    Text {
        /// The text.
        text: String,
    },
    /// Any other block type, preserved verbatim.
    #[serde(other)]
    Other,
}

impl ToolCallResult {
    /// Concatenate all text blocks (the common case for feeding a model).
    pub fn text(&self) -> String {
        self.content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Text { text } => Some(text.as_str()),
                ContentBlock::Other => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_serializes_with_version() {
        let req = JsonRpcRequest::new(1, "tools/list", None);
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["jsonrpc"], "2.0");
        assert_eq!(json["method"], "tools/list");
        assert!(json.get("params").is_none());
    }

    #[test]
    fn parses_tools_list_result() {
        let json = r#"{"tools":[{"name":"search","description":"search docs","inputSchema":{"type":"object"}}]}"#;
        let res: ToolsListResult = serde_json::from_str(json).unwrap();
        assert_eq!(res.tools.len(), 1);
        assert_eq!(res.tools[0].name, "search");
    }

    #[test]
    fn parses_tool_call_result_text() {
        let json = r#"{"content":[{"type":"text","text":"hello"},{"type":"text","text":"world"}],"isError":false}"#;
        let res: ToolCallResult = serde_json::from_str(json).unwrap();
        assert_eq!(res.text(), "hello\nworld");
        assert!(!res.is_error);
    }

    #[test]
    fn parses_error_response() {
        let json =
            r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"method not found"}}"#;
        let res: JsonRpcResponse = serde_json::from_str(json).unwrap();
        assert!(res.result.is_none());
        assert_eq!(res.error.unwrap().code, -32601);
    }

    #[test]
    fn unknown_content_block_is_other() {
        let json = r#"{"content":[{"type":"image","data":"..."}],"isError":false}"#;
        let res: ToolCallResult = serde_json::from_str(json).unwrap();
        assert_eq!(res.text(), "");
    }
}
