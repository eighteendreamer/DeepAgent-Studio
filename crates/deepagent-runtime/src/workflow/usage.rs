//! Workflow projections of the existing canonical call accounting rule.

use serde_json::{json, Value};

use super::canvas::{CanvasUsageReport, CanvasUsageSink};
use super::node_events::NodeEventPublisher;
use crate::agent::RunUsage;
use crate::events::RuntimeEvent;
use crate::usage::RunUsageTracker;

pub(super) use crate::usage::CallUsageObserver as WorkflowUsageObserver;
pub use crate::usage::ModelUsage as WorkflowModelUsage;

pub(super) struct WorkflowUsage {
    usage: RunUsageTracker,
    publisher: NodeEventPublisher,
}

impl WorkflowUsage {
    pub(super) fn new(publisher: NodeEventPublisher) -> Self {
        Self {
            usage: RunUsageTracker::default(),
            publisher,
        }
    }

    pub(super) fn total(&self) -> RunUsage {
        self.usage.total()
    }

    pub(super) fn models(&self) -> Vec<WorkflowModelUsage> {
        self.usage.models()
    }

    pub(super) fn take_raw(&self) -> Vec<Value> {
        self.usage.take_raw()
    }
}

impl CanvasUsageSink for WorkflowUsage {
    fn record(&self, mut report: CanvasUsageReport) {
        let mut raw = report
            .raw_usage
            .as_ref()
            .map(deepagent_models::usage::sanitized_provider_usage)
            .unwrap_or_else(|| json!({}));
        raw["_deepagent"] = json!({
            "version": 1,
            "providerId": report.provider_id,
            "modelId": report.model_id,
            "usageReported": report.usage.is_some(),
        });
        report.raw_usage = Some(raw.clone());
        self.usage.record(&report);
        if let Some(usage) = report.usage {
            self.publisher.emit_runtime(RuntimeEvent::Usage {
                prompt_tokens: usage.prompt_tokens,
                completion_tokens: usage.completion_tokens,
                reasoning_tokens: usage.reasoning_tokens,
                total_tokens: usage.total_tokens,
                prompt_cache_hit_tokens: usage.prompt_cache_hit_tokens,
                prompt_cache_miss_tokens: usage.prompt_cache_miss_tokens,
                cost_yuan: None,
                raw_responses_usage: Some(raw),
            });
        }
    }
}
