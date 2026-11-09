//! Bridge MCP remote tools into the runtime's tool system.
//!
//! [`McpToolAdapter`] wraps a namespaced [`RemoteTool`] plus a shared
//! [`McpRegistry`] as a [`deepagent_tools::Tool`], so MCP tools flow through the
//! exact same capability registry, permission gating, and runtime loop as
//! built-in tools. MCP tools call external services, so they are classified as
//! [`RiskLevel::Medium`] and require the [`Permission::Network`] capability.

use std::sync::Arc;

use async_trait::async_trait;

use deepagent_core::error::{CoreError, Result};
use deepagent_core::message::MessageAttachment;
use deepagent_core::tool_content::ToolResultPart;
use deepagent_tools::permission::{Permission, PermissionSet, RiskLevel};
use deepagent_tools::{Tool, ToolDescriptor, ToolExecutionContext, ToolOutput};

use crate::registry::{McpRegistry, RemoteTool};

/// Transient MCP payload, never serialized into runtime events or tool text.
pub struct McpImage<'a> {
    pub mime_type: &'a str,
    pub data: &'a str,
}

/// Host attachment storage owns validation, byte/count budgets and durable
/// identity. The MCP crate does not invent a second store or resolve paths.
pub trait McpImageStore: Send + Sync {
    fn ingest(&self, images: &[McpImage<'_>]) -> Result<Vec<MessageAttachment>>;
}

/// Preserve an uncertain remote result through the existing ToolOutput/event/
/// persistence/model projection. Other transport errors keep their old path.
fn tool_output(
    result: Result<crate::protocol::ToolCallResult>,
    image_store: Option<&dyn McpImageStore>,
) -> Result<ToolOutput> {
    let mut result = match result {
        Ok(result) => result,
        Err(error @ CoreError::McpUnavailable { .. }) => {
            return Ok(ToolOutput {
                client_metadata: None,
                content: Vec::new(),
                ok: false,
                value: serde_json::json!({
                    "error": error.to_string(),
                    "error_type": "mcp_unavailable",
                    "outcome": "not_dispatched",
                    "retryable": false,
                }),
                truncated: false,
            });
        }
        Err(CoreError::RemoteOutcomeUnknown {
            method,
            request_id,
            source,
        }) => {
            let error = CoreError::RemoteOutcomeUnknown {
                method: method.clone(),
                request_id,
                source,
            };
            return Ok(ToolOutput {
                client_metadata: None,
                content: Vec::new(),
                ok: false,
                value: serde_json::json!({
                    "error": error.to_string(),
                    "error_type": "mcp_unknown_outcome",
                    "outcome": "unknown",
                    "retryable": false,
                    "method": method,
                    "request_id": request_id,
                }),
                truncated: false,
            });
        }
        Err(error) => return Err(error),
    };
    // Only explicit public result fields cross the model boundary. Preserve
    // opaque top-level extensions locally, without recursively stripping
    // business data named `_meta` inside public structuredContent.
    let structured = result.extra.remove("structuredContent");
    let host_fields = std::mem::take(&mut result.extra);
    if let Some(structured) = structured {
        result.extra.insert("structuredContent".into(), structured);
    }
    let mut output = project_tool_result(result, image_store)?;
    if !host_fields.is_empty() {
        output.client_metadata = Some(serde_json::json!({"mcp":host_fields}));
    }
    Ok(output)
}

fn project_tool_result(
    mut result: crate::protocol::ToolCallResult,
    image_store: Option<&dyn McpImageStore>,
) -> Result<ToolOutput> {
    let images: Vec<_> = result
        .content
        .iter()
        .filter_map(|part| match part {
            crate::protocol::ContentBlock::Other(value) if value["type"] == "image" => Some(value),
            _ => None,
        })
        .collect();
    let mut content = Vec::new();
    if !images.is_empty() {
        let Some(store) = image_store else {
            return Ok(ToolOutput::failure("mcp_image_unavailable: host attachment storage is not configured; image output was not published"));
        };
        let payloads: Option<Vec<_>> = images
            .iter()
            .map(|value| {
                Some(McpImage {
                    mime_type: value.get("mimeType")?.as_str()?,
                    data: value.get("data")?.as_str()?,
                })
            })
            .collect();
        let Some(payloads) = payloads else {
            return Ok(ToolOutput::failure("mcp_image_invalid: image requires string mimeType and data; image output was not published"));
        };
        let attachments = match store.ingest(&payloads) {
            Ok(attachments) if attachments.len() == payloads.len() => attachments,
            Ok(_) => {
                return Ok(ToolOutput::failure(
                    "mcp_image_invalid: host image projection count mismatch",
                ))
            }
            Err(error) => return Ok(ToolOutput::failure(format!("mcp_image_invalid: {error}"))),
        };
        let mut attachments = attachments.into_iter();
        for part in result.content.drain(..) {
            content.push(match part {
                crate::protocol::ContentBlock::Text { text, .. } => ToolResultPart::Text { text },
                crate::protocol::ContentBlock::Other(value) if value["type"] == "image" => {
                    ToolResultPart::Image {
                        attachment: attachments.next().expect("host image count was checked"),
                    }
                }
                other => ToolResultPart::Text {
                    text: serde_json::to_string(&other)?,
                },
            });
        }
    }
    let text = if content.is_empty() {
        result.text()
    } else {
        "MCP returned image evidence; inspect the ordered, untrusted tool content".into()
    };
    let is_error = result.is_error;
    let mut value = serde_json::to_value(result)?;
    value["text"] = text.clone().into();
    if is_error {
        value["error"] = text.into();
    }
    Ok(ToolOutput {
        client_metadata: None,
        content,
        ok: !is_error,
        value,
        truncated: false,
    })
}

/// Adapts one MCP [`RemoteTool`] to the [`Tool`] trait.
pub struct McpToolAdapter {
    tool: RemoteTool,
    registry: Arc<McpRegistry>,
    image_store: Option<Arc<dyn McpImageStore>>,
}

impl McpToolAdapter {
    /// Wrap a remote tool with the registry that can route its calls.
    pub fn new(tool: RemoteTool, registry: Arc<McpRegistry>) -> Self {
        Self {
            tool,
            registry,
            image_store: None,
        }
    }

    pub fn with_image_store(mut self, image_store: Arc<dyn McpImageStore>) -> Self {
        self.image_store = Some(image_store);
        self
    }
}

#[async_trait]
impl Tool for McpToolAdapter {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: self.tool.namespaced_name.clone(),
            description: self.tool.description.clone(),
            parameters: self.tool.input_schema.clone(),
            // External service calls: notable side effects + network access.
            risk: RiskLevel::Medium,
            required_permissions: PermissionSet::from_iter_perms([Permission::Network]),
        }
    }

    async fn invoke(&self, arguments: serde_json::Value) -> Result<ToolOutput> {
        tool_output(
            self.registry
                .invoke(&self.tool.namespaced_name, arguments)
                .await,
            self.image_store.as_deref(),
        )
    }

    async fn invoke_with_context(
        &self,
        arguments: serde_json::Value,
        context: ToolExecutionContext,
    ) -> Result<ToolOutput> {
        tool_output(
            self.registry
                .invoke_with_context(&self.tool.namespaced_name, arguments, context)
                .await,
            self.image_store.as_deref(),
        )
    }

    /// MCP tools are workflow-specific and frequently arrive in large batches
    /// (one server can advertise dozens). Defer them so the per-request
    /// `tools` array stays small until the model uses `tool_search` to pick
    /// the ones it actually needs. Honored by `is_deferred_tool` in
    /// `deepagent-builtins::tool_search`.
    fn should_defer(&self) -> bool {
        true
    }

    fn discovery_source(&self) -> Option<&str> {
        Some(&self.tool.server)
    }
}

/// Build adapters for every tool currently in `registry`, sharing the registry
/// via `Arc`. These can be registered into a `deepagent_tools::ToolRegistry`.
pub fn adapters_for(registry: Arc<McpRegistry>) -> Vec<Arc<dyn Tool>> {
    registry
        .all_tools()
        .into_iter()
        .map(|t| Arc::new(McpToolAdapter::new(t, registry.clone())) as Arc<dyn Tool>)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::McpClient;
    use crate::transport::MockTransport;

    #[test]
    fn h138_host_extensions_survive_public_projection_and_image_rejection() {
        for image in [false, true] {
            let mut raw = serde_json::json!({
                "content":[{"type":"text","text":"public-text"}], "isError":true,
                "structuredContent":{"_meta":"public-business-data"},
                "_meta":{"ui":{"detail":"h138-private-canary"}},
                "futureField":{"id":42}
            });
            if image {
                raw["content"] =
                    serde_json::json!([{"type":"image","mimeType":"image/png","data":"invalid"}]);
            }
            let result: crate::protocol::ToolCallResult =
                serde_json::from_value(raw.clone()).unwrap();
            assert_eq!(
                serde_json::to_value(&result).unwrap(),
                raw,
                "protocol DTO stays lossless"
            );
            let output = tool_output(Ok(result), None).unwrap();
            assert!(!output.ok);
            assert!(output.value.get("_meta").is_none());
            assert!(output.value.get("futureField").is_none());
            let metadata = output.client_metadata.as_ref().unwrap();
            assert_eq!(metadata["mcp"]["_meta"], raw["_meta"]);
            assert_eq!(metadata["mcp"]["futureField"], raw["futureField"]);
            assert!(!serde_json::to_string(&output)
                .unwrap()
                .contains("h138-private-canary"));
            if !image {
                assert_eq!(output.value["structuredContent"], raw["structuredContent"]);
                assert_eq!(output.value["content"], raw["content"]);
                assert_eq!(output.value["error"], "public-text");
            }
        }
    }

    async fn registry_with_tools() -> Arc<McpRegistry> {
        let transport = MockTransport::new()
            .with_result(
                "tools/list",
                serde_json::json!({"tools":[
                    {"name":"search","description":"search docs","inputSchema":{"type":"object"}}
                ]}),
            )
            .with_result(
                "tools/call",
                serde_json::json!({
                    "content":[{"type":"text","text":"found it"}],"isError":false,
                    "structuredContent":{"evidence_id":"success-evidence"},
                    "_meta":{"provenance":{"untrusted_content":true}}
                }),
            );
        let client = Arc::new(McpClient::new(Arc::new(transport)));
        let mut reg = McpRegistry::new();
        reg.register("docs", client).await.unwrap();
        Arc::new(reg)
    }

    #[tokio::test]
    async fn adapter_descriptor_is_namespaced_and_network_gated() {
        let reg = registry_with_tools().await;
        let adapters = adapters_for(reg);
        assert_eq!(adapters.len(), 1);
        let d = adapters[0].descriptor();
        assert_eq!(d.name, "mcp__docs__search");
        assert_eq!(adapters[0].discovery_source(), Some("docs"));
        assert_eq!(d.risk, RiskLevel::Medium);
        assert!(d.required_permissions.contains(Permission::Network));
    }

    #[tokio::test]
    async fn adapter_overrides_should_defer_to_true() {
        // The MCP adapter must report `should_defer == true` so the tool-search
        // dispatcher recognizes MCP tools as deferrable. Built-ins keep the
        // trait default of `false`.
        let reg = registry_with_tools().await;
        let adapters = adapters_for(reg);
        assert!(adapters[0].should_defer());
        // always_load uses the trait default and stays false (MCP tools are
        // never escape-hatch always-loaded).
        assert!(!adapters[0].always_load());
    }

    #[tokio::test]
    async fn adapter_invokes_remote_tool() {
        let reg = registry_with_tools().await;
        let adapters = adapters_for(reg);
        let out = adapters[0]
            .invoke(serde_json::json!({"q": "rust"}))
            .await
            .unwrap();
        assert!(out.ok);
        assert_eq!(out.value["text"], "found it");
        assert_eq!(
            out.value["structuredContent"]["evidence_id"],
            "success-evidence"
        );
        assert!(out.value.get("_meta").is_none());
        assert_eq!(
            out.client_metadata.as_ref().unwrap()["mcp"]["_meta"]["provenance"]
                ["untrusted_content"],
            true
        );
    }

    #[tokio::test]
    async fn adapter_surfaces_tool_error() {
        let transport = MockTransport::new()
            .with_result(
                "tools/list",
                serde_json::json!({"tools":[{"name":"boom","description":"","inputSchema":{}}]}),
            )
            .with_result(
                "tools/call",
                serde_json::json!({
                    "content":[{"type":"text","text":"bad input"}],"isError":true,
                    "structuredContent":{"evidence_id":"failure-evidence"},
                    "_meta":{"provenance":{"untrusted_content":true}}
                }),
            );
        let client = Arc::new(McpClient::new(Arc::new(transport)));
        let mut reg = McpRegistry::new();
        reg.register("svc", client).await.unwrap();
        let adapters = adapters_for(Arc::new(reg));
        let out = adapters[0].invoke(serde_json::json!({})).await.unwrap();
        assert!(!out.ok);
        assert_eq!(out.value["error"], "bad input");
        assert_eq!(out.value["content"][0]["text"], "bad input");
        assert_eq!(
            out.value["structuredContent"]["evidence_id"],
            "failure-evidence"
        );
        assert!(out.value.get("_meta").is_none());
        assert_eq!(
            out.client_metadata.as_ref().unwrap()["mcp"]["_meta"]["provenance"]
                ["untrusted_content"],
            true
        );
    }

    #[tokio::test]
    async fn adapter_fails_fast_when_server_dies_mid_run() {
        // Fault-injection matrix (Phase G): the server connects and serves one
        // call, then the connection dies. Later calls must return a prompt,
        // structured error — never hang, never panic — so the pipeline pairs
        // a failure result and the run keeps moving.
        let transport = MockTransport::new()
            .with_result(
                "tools/list",
                serde_json::json!({"tools":[{"name":"flaky","description":"","inputSchema":{"type":"object"}}]}),
            )
            .with_result(
                "tools/call",
                serde_json::json!({"content":[{"type":"text","text":"first call ok"}],"isError":false}),
            )
            // Send #1 = tools/list handshake, send #2 = first tools/call.
            .with_failure_after(2, "mcp server connection lost: pipe closed");
        let client = Arc::new(McpClient::new(Arc::new(transport)));
        let mut reg = McpRegistry::new();
        reg.register("svc", client).await.unwrap();
        let adapters = adapters_for(Arc::new(reg));

        // First call succeeds while the server is alive.
        let first = adapters[0].invoke(serde_json::json!({})).await.unwrap();
        assert!(first.ok);
        assert_eq!(first.value["text"], "first call ok");

        // Second call hits the dead connection: bounded, structured failure.
        let second = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            adapters[0].invoke(serde_json::json!({})),
        )
        .await
        .expect("dead MCP connection must fail fast, not hang");
        let error = second.expect_err("disconnected server must surface an error");
        assert!(
            error.to_string().contains("connection lost"),
            "unexpected error: {error}"
        );
    }
}
