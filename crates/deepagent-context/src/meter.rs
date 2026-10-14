//! Provider-facing context measurement over the canonical response-item Surface.

use deepagent_core::response_item::ResponseInputItem;
use deepagent_models::ToolSchema;

use crate::{CacheScope, ContextBlockKind, ContextBlockUsage, HeuristicTokenizer, TokenCounter};
use std::hash::{Hash, Hasher};

/// Shared assembly boundary; the separator itself remains model-visible.
pub const DYNAMIC_INSTRUCTIONS_BOUNDARY: &str = "\n\n<<<DYNAMIC>>>\n\n";

/// Token categories used by the context panel and compaction decisions.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SurfaceTokenBreakdown {
    pub instructions: usize,
    pub dynamic_runtime: usize,
    pub messages: usize,
    pub reasoning: usize,
    pub tool_calls: usize,
    pub tool_outputs: usize,
    pub tools: usize,
    pub attachments: usize,
}

impl SurfaceTokenBreakdown {
    /// Total estimated tokens in the provider-facing request surface.
    pub const fn total(self) -> usize {
        self.instructions
            + self.dynamic_runtime
            + self.messages
            + self.reasoning
            + self.tool_calls
            + self.tool_outputs
            + self.tools
            + self.attachments
    }
}

/// Measure the same native items that the Responses request serializer sees.
///
/// This deliberately counts tool schemas and provider-native call/output items;
/// a chat-only rendering is not a valid pressure estimate for a stateless model.
pub fn measure_response_surface(
    instructions: Option<&str>,
    items: &[ResponseInputItem],
    tools: &[ToolSchema],
) -> SurfaceTokenBreakdown {
    measure_response_surface_with(&HeuristicTokenizer::new(), instructions, items, tools)
}

/// Measure the routed representation of attachments using the same pricing
/// supplied by request serialization. Non-image file text is already in items.
pub fn measure_response_surface_for_route(
    instructions: Option<&str>,
    items: &[ResponseInputItem],
    tools: &[ToolSchema],
    mut image_price: impl FnMut(&deepagent_core::message::MessageAttachment) -> usize,
) -> SurfaceTokenBreakdown {
    let mut measured = measure_response_surface(instructions, items, tools);
    measured.attachments = items
        .iter()
        .filter_map(|item| match item {
            ResponseInputItem::InputMessage {
                role, attachments, ..
            } if role == "user" => Some(attachments),
            _ => None,
        })
        .flatten()
        .filter(|attachment| attachment.kind == "image")
        .map(&mut image_price)
        .sum();
    measured
}

/// Generic form used by tests and future model-specific token counters.
pub fn measure_response_surface_with<C: TokenCounter + ?Sized>(
    counter: &C,
    instructions: Option<&str>,
    items: &[ResponseInputItem],
    tools: &[ToolSchema],
) -> SurfaceTokenBreakdown {
    let (stable, dynamic) = instructions
        .unwrap_or("")
        .split_once(DYNAMIC_INSTRUCTIONS_BOUNDARY)
        .unwrap_or((instructions.unwrap_or(""), ""));
    let mut out = SurfaceTokenBreakdown {
        instructions: counter.count(stable) + if stable.is_empty() { 0 } else { 4 },
        dynamic_runtime: counter.count(dynamic)
            + if dynamic.is_empty() {
                0
            } else {
                counter.count(DYNAMIC_INSTRUCTIONS_BOUNDARY)
            },
        tools: if tools.is_empty() {
            0
        } else {
            serde_json::to_string(tools)
                .ok()
                .map_or(0, |text| counter.count(&text))
        },
        ..SurfaceTokenBreakdown::default()
    };

    for item in items {
        match item {
            ResponseInputItem::Message { content, .. } => {
                out.messages += counter.count(content) + 8;
            }
            ResponseInputItem::InputMessage {
                content,
                attachments,
                ..
            } => {
                out.messages += counter.count(content) + 8;
                // Attachment references are not wire text. Their routed cost is
                // supplied by the request projector instead of counting paths.
                let _ = attachments;
            }
            ResponseInputItem::Reasoning { content, .. } => {
                out.reasoning += counter.count(content) + 4;
            }
            ResponseInputItem::FunctionCall {
                name, arguments, ..
            } => {
                out.tool_calls += counter.count(name) + counter.count(arguments) + 4;
            }
            ResponseInputItem::CustomToolCall { name, input, .. } => {
                out.tool_calls += counter.count(name) + counter.count(input) + 4;
            }
            ResponseInputItem::FunctionCallOutput { output, .. }
            | ResponseInputItem::CustomToolCallOutput { output, .. } => {
                out.tool_outputs += counter.count(output) + 4;
            }
            ResponseInputItem::WebSearchCall { action, status, .. } => {
                out.tool_calls += 4
                    + counter.count(status)
                    + action
                        .as_ref()
                        .and_then(|value| serde_json::to_string(value).ok())
                        .map_or(0, |text| counter.count(&text));
            }
        }
    }
    out
}

/// Provider prompt usage calibrated against exactly the surface sent with it.
/// A header/tool change invalidates calibration, while appends and replacements
/// use a signed surface delta. No independent history or persistence lives here.
#[derive(Debug, Default)]
pub struct ContextMeter {
    anchor: Option<(u64, usize, usize)>, // header fingerprint, local cost, provider cost
}

impl ContextMeter {
    pub fn request_fingerprint(
        model: &str,
        instructions: Option<&str>,
        tools: &[ToolSchema],
    ) -> u64 {
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        model.hash(&mut hash);
        instructions.hash(&mut hash);
        serde_json::to_string(tools)
            .unwrap_or_default()
            .hash(&mut hash);
        hash.finish()
    }

    pub fn anchor(&mut self, fingerprint: u64, surface_tokens: usize, prompt_tokens: usize) {
        self.anchor = Some((fingerprint, surface_tokens, prompt_tokens));
    }

    pub fn invalidate(&mut self) {
        self.anchor = None;
    }

    pub fn projected_tokens(&self, fingerprint: u64, surface: SurfaceTokenBreakdown) -> usize {
        let current = surface.total();
        match self.anchor {
            Some((key, local, measured)) if key == fingerprint => {
                (measured as i128 + current as i128 - local as i128).max(0) as usize
            }
            _ => current,
        }
    }
}

impl SurfaceTokenBreakdown {
    /// Partition the calibrated total using the same measurement as pressure
    /// management. Integer remainder is assigned to the final nonempty block.
    pub fn blocks(self, projected: usize) -> Vec<ContextBlockUsage> {
        let groups = [
            (ContextBlockKind::StablePrefix, self.instructions),
            (ContextBlockKind::DynamicRuntime, self.dynamic_runtime),
            (ContextBlockKind::ToolSchemas, self.tools),
            (
                ContextBlockKind::RecentConversation,
                self.messages + self.reasoning + self.tool_calls,
            ),
            (ContextBlockKind::ToolResultRefs, self.tool_outputs),
            (ContextBlockKind::Attachments, self.attachments),
        ];
        let total = self.total();
        if total == 0 {
            return if projected == 0 {
                Vec::new()
            } else {
                vec![ContextBlockUsage {
                    name: ContextBlockKind::Other.label().into(),
                    kind: ContextBlockKind::Other,
                    tokens: projected,
                    cache_scope: CacheScope::Dynamic,
                    source: "surface_meter".into(),
                }]
            };
        }
        let mut blocks: Vec<_> = groups
            .into_iter()
            .filter(|(_, tokens)| *tokens > 0)
            .map(|(kind, tokens)| ContextBlockUsage {
                name: kind.label().into(),
                kind,
                tokens: ((tokens as u128 * projected as u128) / total as u128) as usize,
                cache_scope: if matches!(
                    kind,
                    ContextBlockKind::StablePrefix | ContextBlockKind::ToolSchemas
                ) {
                    CacheScope::StablePrefix
                } else {
                    CacheScope::Dynamic
                },
                source: "surface_meter".into(),
            })
            .collect();
        let remainder = projected.saturating_sub(blocks.iter().map(|block| block.tokens).sum());
        if let Some(last) = blocks.last_mut() {
            last.tokens += remainder;
        }
        blocks
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepagent_core::response_item::ResponseInputItem;

    #[test]
    fn provider_anchor_tracks_appends_replacements_and_header_changes() {
        let mut meter = ContextMeter::default();
        let initial = SurfaceTokenBreakdown {
            messages: 100,
            tools: 50,
            ..Default::default()
        };
        meter.anchor(1, initial.total(), 200);
        let grown = SurfaceTokenBreakdown {
            messages: 140,
            tools: 50,
            tool_outputs: 80,
            ..Default::default()
        };
        assert_eq!(meter.projected_tokens(1, grown), 320);
        let pruned = SurfaceTokenBreakdown {
            tool_outputs: 10,
            ..grown
        };
        assert_eq!(meter.projected_tokens(1, pruned), 250);
        assert_eq!(
            meter.projected_tokens(2, pruned),
            pruned.total(),
            "changed tools/header invalidate the anchor"
        );
        assert_eq!(
            pruned
                .blocks(250)
                .iter()
                .map(|block| block.tokens)
                .sum::<usize>(),
            250
        );
        assert_eq!(
            pruned
                .blocks(3)
                .iter()
                .map(|block| block.tokens)
                .sum::<usize>(),
            3
        );
    }

    #[test]
    fn native_arguments_alone_can_exceed_a_pressure_threshold() {
        let items = vec![ResponseInputItem::FunctionCall {
            call_id: "c1".into(),
            name: "execute".into(),
            arguments: "x".repeat(20_000),
        }];
        assert!(measure_response_surface(None, &items, &[]).total() > 4_000);
    }

    #[test]
    fn routed_attachment_costs_replace_reference_metadata() {
        let items = vec![ResponseInputItem::InputMessage {
            role: "user".into(),
            content: "look".into(),
            attachments: vec![deepagent_core::message::MessageAttachment {
                id: "image-1".into(),
                kind: "image".into(),
                media_type: Some("image/png".into()),
                path: Some("ignored-path".repeat(100)),
            }],
        }];
        let measured = measure_response_surface_for_route(None, &items, &[], |_| 1024);
        assert_eq!(measured.attachments, 1024);
        let text_only = measure_response_surface_for_route(None, &items, &[], |_| 0);
        assert_eq!(measured.total() - text_only.total(), 1024);
    }

    #[test]
    fn native_calls_outputs_and_tools_are_counted() {
        let items = vec![
            ResponseInputItem::Message {
                role: "user".into(),
                content: "goal".into(),
            },
            ResponseInputItem::FunctionCall {
                call_id: "c1".into(),
                name: "read_file".into(),
                arguments: "{\"path\":\"large\"}".into(),
            },
            ResponseInputItem::FunctionCallOutput {
                call_id: "c1".into(),
                output: "tool output".into(),
            },
        ];
        let tools = vec![ToolSchema::function(
            "read_file",
            "Read a file",
            serde_json::json!({"type":"object"}),
        )];
        let breakdown = measure_response_surface(Some("rules"), &items, &tools);
        assert!(breakdown.instructions > 0);
        assert!(breakdown.messages > 0);
        assert!(breakdown.tool_calls > 0);
        assert!(breakdown.tool_outputs > 0);
        assert!(breakdown.tools > 0);
        assert_eq!(
            breakdown.total(),
            breakdown.instructions
                + breakdown.dynamic_runtime
                + breakdown.messages
                + breakdown.reasoning
                + breakdown.tool_calls
                + breakdown.tool_outputs
                + breakdown.tools
                + breakdown.attachments
        );
    }
}
