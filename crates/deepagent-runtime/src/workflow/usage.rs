//! Workflow projections of the existing canonical call accounting rule.

use std::sync::{Arc, Mutex};

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
    external: Mutex<Option<Arc<dyn CanvasUsageSink>>>,
    /// Canonical host admission/accounting errors stop the run, not just one
    /// item. Retain their source message without guessing from error text.
    control_error: Mutex<Option<String>>,
}

impl WorkflowUsage {
    pub(super) fn new(publisher: NodeEventPublisher) -> Self {
        Self {
            usage: RunUsageTracker::default(),
            publisher,
            external: Mutex::new(None),
            control_error: Mutex::new(None),
        }
    }

    pub(super) fn bind_sink(&self, sink: Arc<dyn CanvasUsageSink>) {
        *self.external.lock().unwrap_or_else(|p| p.into_inner()) = Some(sink);
    }

    fn external(&self) -> Option<Arc<dyn CanvasUsageSink>> {
        self.external
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
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

    pub(super) fn check_control(&self) -> deepagent_core::error::Result<()> {
        match self
            .control_error
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
        {
            Some(error) => Err(deepagent_core::error::CoreError::other(error)),
            None => Ok(()),
        }
    }

    fn control_result(
        &self,
        result: deepagent_core::error::Result<()>,
    ) -> deepagent_core::error::Result<()> {
        if let Err(error) = &result {
            self.control_error
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .get_or_insert_with(|| error.to_string());
        }
        result
    }
}

impl CanvasUsageSink for WorkflowUsage {
    fn before_call(&self) -> deepagent_core::error::Result<()> {
        self.check_control()?;
        self.control_result(self.external().map_or(Ok(()), |sink| sink.before_call()))
    }

    fn checkpoint(&self) -> deepagent_core::error::Result<()> {
        self.check_control()?;
        self.control_result(self.external().map_or(Ok(()), |sink| sink.checkpoint()))
    }

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
        if let Some(sink) = self.external() {
            sink.record(report.clone());
        }
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
