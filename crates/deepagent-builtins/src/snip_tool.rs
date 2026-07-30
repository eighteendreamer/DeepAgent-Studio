//! `snip_history` — model-driven history trimming, modeled on Claude Code's
//! HISTORY_SNIP SnipTool (`query.ts` L396-410, `messages.ts` L2345-2364).
//!
//! When the conversation carries clearly-finished earlier segments, the model
//! may call this tool with the `[id:uN]` tags of the user turns that begin
//! those segments to remove them from the live context and free space. Like
//! Claude Code's SnipTool, the tool itself only **validates and echoes** the
//! requested ids: the actual removal (and the tool-call pairing safety walk)
//! is applied by the runtime (`ModelAgent::apply_snip_from_output`) against the
//! live conversation, so the transcript / event log stay the source of truth.
//!
//! The tool is deliberately advisory and reversible in spirit — if the model
//! later needs the removed detail it can re-run the relevant tool. It is a
//! read-only capability from the registry's perspective (it mutates only the
//! model-facing message window, never the workspace).

use async_trait::async_trait;

use deepagent_core::error::Result;
use deepagent_tools::permission::{PermissionSet, RiskLevel};
use deepagent_tools::{Tool, ToolDescriptor, ToolOutput};

/// The tool name advertised to the model.
pub const SNIP_HISTORY_TOOL_NAME: &str = "snip_history";

/// The `snip_history` tool. Stateless: it validates the requested ids and
/// returns them for the runtime to apply.
#[derive(Debug, Default, Clone, Copy)]
pub struct SnipHistoryTool;

impl SnipHistoryTool {
    /// Build the tool.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for SnipHistoryTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: SNIP_HISTORY_TOOL_NAME.into(),
            description: "Free context space by removing earlier conversation segments that are \
                clearly finished and no longer needed for the remaining work. Pass the `[id:uN]` \
                tags (the `uN` part, e.g. \"u3\") of the user turns that START the segments to \
                drop; each removes that turn through to just before the next tagged user turn. \
                Only snip segments you are confident are done — when unsure, keep them. Recent \
                turns are always protected and cannot be snipped. If you later need removed \
                detail, re-run the relevant tool."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "ids": {
                        "type": "array",
                        "minItems": 1,
                        "description": "Message ids to snip, e.g. [\"u3\", \"u5\"]. The leading '[id:' and trailing ']' are optional.",
                        "items": { "type": "string" }
                    },
                    "reason": {
                        "type": "string",
                        "description": "Optional short note on why these segments are safe to drop."
                    }
                },
                "required": ["ids"]
            }),
            // Read-only from the registry's perspective: it only trims the
            // model-facing window, never the workspace or the event log.
            risk: RiskLevel::Safe,
            required_permissions: PermissionSet::read_only(),
        }
    }

    async fn invoke(&self, args: serde_json::Value) -> Result<ToolOutput> {
        let Some(raw_ids) = args.get("ids").and_then(|value| value.as_array()) else {
            return Ok(ToolOutput::failure("missing 'ids' array"));
        };
        // Normalize each id to its bare `uN` form so the runtime can match the
        // `[id:uN]` tags regardless of whether the model included the brackets.
        let mut ids: Vec<String> = Vec::new();
        for value in raw_ids {
            let Some(text) = value.as_str() else {
                continue;
            };
            let normalized = normalize_id(text);
            if !normalized.is_empty() && !ids.contains(&normalized) {
                ids.push(normalized);
            }
        }
        if ids.is_empty() {
            return Ok(ToolOutput::failure(
                "no valid ids provided; pass tags like \"u3\"",
            ));
        }
        Ok(ToolOutput::success(serde_json::json!({
            "snipped": true,
            "ids": ids,
            "note": "Requested segments will be removed from the live context before the next model turn.",
        })))
    }
}

/// Strip an optional `[id:` prefix / `]` suffix and surrounding whitespace,
/// leaving the bare `uN` token.
fn normalize_id(raw: &str) -> String {
    raw.trim()
        .trim_start_matches("[id:")
        .trim_end_matches(']')
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn echoes_normalized_ids() {
        let tool = SnipHistoryTool::new();
        let out = tool
            .invoke(serde_json::json!({ "ids": ["u3", "[id:u5]", " u3 "] }))
            .await
            .unwrap();
        assert!(out.ok);
        assert_eq!(out.value["snipped"], true);
        // Duplicate "u3" collapsed; "[id:u5]" normalized to "u5".
        assert_eq!(out.value["ids"], serde_json::json!(["u3", "u5"]));
    }

    #[tokio::test]
    async fn rejects_missing_ids() {
        let tool = SnipHistoryTool::new();
        let out = tool.invoke(serde_json::json!({})).await.unwrap();
        assert!(!out.ok);
    }

    #[tokio::test]
    async fn rejects_empty_after_normalization() {
        let tool = SnipHistoryTool::new();
        let out = tool
            .invoke(serde_json::json!({ "ids": ["", "  "] }))
            .await
            .unwrap();
        assert!(!out.ok);
    }

    #[test]
    fn descriptor_is_read_only_safe() {
        let d = SnipHistoryTool::new().descriptor();
        assert_eq!(d.name, SNIP_HISTORY_TOOL_NAME);
        assert_eq!(d.risk, RiskLevel::Safe);
    }
}
