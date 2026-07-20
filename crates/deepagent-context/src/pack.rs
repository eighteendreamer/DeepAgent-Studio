//! Observable context pack and usage snapshots.

use serde::{Deserialize, Serialize};

use crate::policy::ContextPolicy;

/// Logical context block kind. These names map directly to the compact UI
/// labels shown in the input-box capacity popover.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextBlockKind {
    StablePrefix,
    DynamicRuntime,
    TaskSummary,
    RecentConversation,
    RetrievedKnowledge,
    RetrievedCode,
    ToolSchemas,
    ToolResultRefs,
    UserGoal,
    Other,
}

/// Whether a block belongs to the cache-stable prefix or the volatile suffix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheScope {
    StablePrefix,
    Dynamic,
}

/// One model-visible context block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextBlock {
    pub kind: ContextBlockKind,
    pub source: String,
    pub priority: u8,
    pub cache_scope: CacheScope,
    pub estimated_tokens: usize,
    pub content: String,
}

/// A structured context package assembled before a model call.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextPack {
    pub blocks: Vec<ContextBlock>,
}

impl ContextPack {
    pub fn new(blocks: Vec<ContextBlock>) -> Self {
        Self { blocks }
    }

    pub fn estimated_prompt_tokens(&self) -> usize {
        self.blocks.iter().map(|b| b.estimated_tokens).sum()
    }

    pub fn block_usages(&self) -> Vec<ContextBlockUsage> {
        self.blocks
            .iter()
            .map(|block| ContextBlockUsage {
                name: block.kind.label().to_string(),
                kind: block.kind,
                tokens: block.estimated_tokens,
                cache_scope: block.cache_scope,
                source: block.source.clone(),
            })
            .collect()
    }
}

impl ContextBlockKind {
    pub const fn label(self) -> &'static str {
        match self {
            ContextBlockKind::StablePrefix => "稳定前缀",
            ContextBlockKind::DynamicRuntime => "动态环境",
            ContextBlockKind::TaskSummary => "任务摘要",
            ContextBlockKind::RecentConversation => "最近对话",
            ContextBlockKind::RetrievedKnowledge => "知识库检索",
            ContextBlockKind::RetrievedCode => "代码片段",
            ContextBlockKind::ToolSchemas => "工具定义",
            ContextBlockKind::ToolResultRefs => "工具结果摘要",
            ContextBlockKind::UserGoal => "当前目标",
            ContextBlockKind::Other => "其他上下文",
        }
    }
}

/// UI/log projection for a single block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextBlockUsage {
    pub name: String,
    pub kind: ContextBlockKind,
    pub tokens: usize,
    pub cache_scope: CacheScope,
    pub source: String,
}

/// Per-turn context usage snapshot, emitted before a model call and enriched
/// after provider usage is known.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextUsageSnapshot {
    pub model_id: String,
    pub context_window: usize,
    pub prompt_budget: usize,
    pub estimated_prompt_tokens: usize,
    pub used_ratio: f32,
    pub reserved_output_tokens: usize,
    pub reserved_tool_tokens: usize,
    pub cache_hit_tokens: usize,
    pub cache_miss_tokens: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_hit_ratio: Option<f32>,
    pub compacted: bool,
    pub blocks: Vec<ContextBlockUsage>,
}

impl ContextUsageSnapshot {
    pub fn from_pack(policy: &ContextPolicy, pack: &ContextPack, compacted: bool) -> Self {
        let estimated_prompt_tokens = pack.estimated_prompt_tokens();
        let used_ratio = if policy.context_window == 0 {
            0.0
        } else {
            estimated_prompt_tokens as f32 / policy.context_window as f32
        };
        Self {
            model_id: policy.model_id.clone(),
            context_window: policy.context_window,
            prompt_budget: policy.prompt_budget,
            estimated_prompt_tokens,
            used_ratio,
            reserved_output_tokens: policy.reserved_output_tokens,
            reserved_tool_tokens: policy.reserved_tool_tokens,
            cache_hit_tokens: 0,
            cache_miss_tokens: 0,
            cache_hit_ratio: None,
            compacted,
            blocks: pack.block_usages(),
        }
    }

    pub fn with_cache_usage(mut self, cache_hit_tokens: usize, cache_miss_tokens: usize) -> Self {
        self.cache_hit_tokens = cache_hit_tokens;
        self.cache_miss_tokens = cache_miss_tokens;
        let total = cache_hit_tokens + cache_miss_tokens;
        self.cache_hit_ratio = (total > 0).then_some(cache_hit_tokens as f32 / total as f32);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::ContextPolicy;
    use deepagent_models::{CapabilitySource, ModelCapability, ThinkingDepth};

    #[test]
    fn snapshot_sums_blocks_and_keeps_cache_separate() {
        let cap = ModelCapability {
            model_id: "deepseek-v4-flash".into(),
            context_window: 1_000_000,
            max_output_tokens: 384_000,
            supports_tools: true,
            supports_thinking: true,
            supports_json_output: true,
            capability_source: CapabilitySource::BundledOfficialSnapshot,
            fallback_reason: None,
        };
        let policy = ContextPolicy::for_capability(&cap, ThinkingDepth::Simple);
        let pack = ContextPack::new(vec![
            ContextBlock {
                kind: ContextBlockKind::StablePrefix,
                source: "system".into(),
                priority: u8::MAX,
                cache_scope: CacheScope::StablePrefix,
                estimated_tokens: 10,
                content: "system".into(),
            },
            ContextBlock {
                kind: ContextBlockKind::RecentConversation,
                source: "history".into(),
                priority: 130,
                cache_scope: CacheScope::Dynamic,
                estimated_tokens: 90,
                content: "history".into(),
            },
        ]);

        let snapshot =
            ContextUsageSnapshot::from_pack(&policy, &pack, false).with_cache_usage(80, 20);
        assert_eq!(snapshot.estimated_prompt_tokens, 100);
        assert_eq!(snapshot.cache_hit_ratio, Some(0.8));
        assert_eq!(snapshot.blocks.len(), 2);
    }
}
