//! Workflow projections of the existing canonical call accounting rule.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use deepagent_models::{DeltaObserver, Usage};
use serde_json::{json, Value};

use super::canvas::{CanvasUsageReport, CanvasUsageSink};
use super::node_events::NodeEventPublisher;
use crate::agent::RunUsage;
use crate::events::RuntimeEvent;

/// Attribution for the existing finalizer/ledger. The provider is retained for
/// diagnostics; `model_id` stays the exact pricing key, not `provider::model`.
#[derive(Debug, Clone)]
pub struct WorkflowModelUsage {
    pub provider_id: Option<String>,
    pub model_id: String,
    pub usage: RunUsage,
    /// False if any request on this route did not report usable usage.
    pub usage_complete: bool,
}

#[derive(Default)]
struct ModelUsage {
    usage: RunUsage,
    unreported: bool,
}

#[derive(Default)]
struct UsageState {
    total: RunUsage,
    models: BTreeMap<(Option<String>, String), ModelUsage>,
    pending_raw: Vec<Value>,
}

pub(super) struct WorkflowUsage {
    state: Mutex<UsageState>,
    publisher: NodeEventPublisher,
}

impl WorkflowUsage {
    pub(super) fn new(publisher: NodeEventPublisher) -> Self {
        Self {
            state: Mutex::new(UsageState::default()),
            publisher,
        }
    }

    pub(super) fn total(&self) -> RunUsage {
        self.state.lock().unwrap_or_else(|p| p.into_inner()).total
    }

    pub(super) fn models(&self) -> Vec<WorkflowModelUsage> {
        self.state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .models
            .iter()
            .map(|((provider_id, model_id), usage)| WorkflowModelUsage {
                provider_id: provider_id.clone(),
                model_id: model_id.clone(),
                usage: usage.usage,
                usage_complete: !usage.unreported,
            })
            .collect()
    }

    pub(super) fn take_raw(&self) -> Vec<Value> {
        std::mem::take(
            &mut self
                .state
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .pending_raw,
        )
    }
}

impl CanvasUsageSink for WorkflowUsage {
    fn record(&self, report: CanvasUsageReport) {
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
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        state.pending_raw.push(raw.clone());
        if let Some(usage) = report.usage {
            state.total.record_call(&usage);
        }
        let model = state
            .models
            .entry((report.provider_id, report.model_id))
            .or_default();
        if let Some(usage) = report.usage {
            model.usage.record_call(&usage);
        } else {
            model.unreported = true;
        }
        drop(state);
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

/// A Chat stream can report usage before returning a truncated/error result.
/// Keep its latest snapshot (not a sum of repeated SSE snapshots), then settle
/// exactly once even when the waiting future is dropped by cancellation.
pub(super) struct WorkflowUsageObserver {
    sink: Arc<dyn CanvasUsageSink>,
    model_id: String,
    usage: Option<Usage>,
    raw: Option<Value>,
}

impl WorkflowUsageObserver {
    pub(super) fn new(sink: Arc<dyn CanvasUsageSink>, model_id: String) -> Self {
        Self {
            sink,
            model_id,
            usage: None,
            raw: None,
        }
    }
}

impl DeltaObserver for WorkflowUsageObserver {
    fn on_usage(&mut self, usage: Usage, raw: &Value) {
        self.usage = Some(usage);
        self.raw = Some(deepagent_models::usage::sanitized_provider_usage(raw));
    }
}

impl Drop for WorkflowUsageObserver {
    fn drop(&mut self) {
        self.sink.record(CanvasUsageReport {
            provider_id: None,
            model_id: self.model_id.clone(),
            usage: self.usage,
            raw_usage: self.raw.take(),
        });
    }
}
