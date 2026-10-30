//! Run-owned projections of the canonical call accounting rule, shared by chat
//! and workflow. These are usage receipts, not another cost ledger.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use deepagent_models::{DeltaObserver, Usage};
use serde_json::Value;

use crate::agent::RunUsage;

/// Attribution for the existing finalizer. The exact model id is the pricing
/// key; an unavailable provider identity must not be invented.
#[derive(Debug, Clone)]
pub struct ModelUsage {
    pub provider_id: Option<String>,
    pub model_id: String,
    pub usage: RunUsage,
    pub usage_complete: bool,
}

/// Receipt for one physical provider request, before output validation.
#[derive(Debug, Clone)]
pub struct CallUsageReport {
    pub provider_id: Option<String>,
    pub model_id: String,
    pub usage: Option<Usage>,
    pub raw_usage: Option<Value>,
}

pub trait CallUsageSink: Send + Sync {
    fn record(&self, report: CallUsageReport);
}

#[derive(Default)]
struct RouteUsage {
    usage: RunUsage,
    unreported: bool,
}

#[derive(Default)]
struct UsageState {
    total: RunUsage,
    models: BTreeMap<(Option<String>, String), RouteUsage>,
    pending_raw: Vec<Value>,
}

#[derive(Default)]
pub(crate) struct RunUsageTracker {
    state: Mutex<UsageState>,
}

impl RunUsageTracker {
    pub(crate) fn record(&self, report: &CallUsageReport) {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(raw) = &report.raw_usage {
            state.pending_raw.push(raw.clone());
        }
        if let Some(usage) = report.usage {
            state.total.record_call(&usage);
        }
        let route = state
            .models
            .entry((report.provider_id.clone(), report.model_id.clone()))
            .or_default();
        if let Some(usage) = report.usage {
            route.usage.record_call(&usage);
        } else {
            route.unreported = true;
        }
    }

    pub(crate) fn total(&self) -> RunUsage {
        self.state.lock().unwrap_or_else(|p| p.into_inner()).total
    }

    pub(crate) fn models(&self) -> Vec<ModelUsage> {
        self.state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .models
            .iter()
            .map(|((provider_id, model_id), route)| ModelUsage {
                provider_id: provider_id.clone(),
                model_id: model_id.clone(),
                usage: route.usage,
                usage_complete: !route.unreported,
            })
            .collect()
    }

    pub(crate) fn take_raw(&self) -> Vec<Value> {
        std::mem::take(
            &mut self
                .state
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .pending_raw,
        )
    }
}

/// Retain the latest SSE usage snapshot, then settle exactly once, including
/// when a provider error or cancellation drops the waiting future. The request
/// route is captured before dispatch, never read from a later fallback model.
pub(crate) struct CallUsageObserver {
    sink: Arc<dyn CallUsageSink>,
    model_id: String,
    usage: Option<Usage>,
    raw: Option<Value>,
    record_unreported: bool,
}

impl CallUsageObserver {
    pub(crate) fn new(
        sink: Arc<dyn CallUsageSink>,
        model_id: String,
        record_unreported: bool,
    ) -> Self {
        Self {
            sink,
            model_id,
            usage: None,
            raw: None,
            record_unreported,
        }
    }

    pub(crate) fn completed(&mut self, usage: Option<Usage>, raw: Option<&Value>) {
        self.record_unreported = true;
        if usage.is_some() {
            self.usage = usage;
        }
        if let Some(raw) = raw {
            self.raw = Some(raw.clone());
        }
    }
}

impl DeltaObserver for CallUsageObserver {
    fn on_usage(&mut self, usage: Usage, raw: &Value) {
        self.usage = Some(usage);
        self.raw = Some(raw.clone());
    }

    fn on_event(&mut self, event: deepagent_models::ModelStreamEvent) {
        if let deepagent_models::ModelStreamEvent::Usage { usage } = event {
            self.usage = Some(usage);
        }
    }
}

impl Drop for CallUsageObserver {
    fn drop(&mut self) {
        // No fabricated charge or unknown-cost row for an unmeasured failed
        // request. A successful unmeasured response is explicitly incomplete.
        if self.usage.is_some() || self.record_unreported {
            self.sink.record(CallUsageReport {
                provider_id: None,
                model_id: self.model_id.clone(),
                usage: self.usage,
                raw_usage: self.raw.take(),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    impl CallUsageSink for RunUsageTracker {
        fn record(&self, report: CallUsageReport) {
            RunUsageTracker::record(self, &report);
        }
    }

    #[test]
    fn attempt_snapshots_settle_once_on_drop_and_keep_routes_and_raw() {
        let tracker = Arc::new(RunUsageTracker::default());
        let first = Usage {
            prompt_tokens: 1000,
            completion_tokens: 100,
            total_tokens: 1100,
            prompt_cache_hit_tokens: 800,
            prompt_cache_miss_tokens: 200,
            ..Default::default()
        };
        let raw = serde_json::json!({"input_tokens":1000,"output_tokens":100});
        {
            let mut observer = CallUsageObserver::new(tracker.clone(), "flash".into(), false);
            observer.on_usage(first, &raw);
            observer.on_usage(first, &raw);
            observer.completed(Some(first), Some(&raw));
        }
        {
            let mut observer = CallUsageObserver::new(tracker.clone(), "pro".into(), false);
            observer.on_usage(first, &raw);
            let last = Usage {
                prompt_tokens: 2000,
                ..first
            };
            observer.on_usage(last, &raw);
            // Provider failure/cancellation: no completed() call.
        }
        let total = tracker.total();
        assert_eq!(total.billed_prompt_tokens, 3000);
        assert_eq!(total.prompt_tokens, 2000);
        assert_eq!(total.completion_tokens, 200);
        let models = tracker.models();
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].model_id, "flash");
        assert_eq!(models[0].usage.billed_prompt_tokens, 1000);
        assert_eq!(models[1].usage.billed_prompt_tokens, 2000);
        assert!(models
            .iter()
            .all(|route| route.provider_id.is_none() && route.usage_complete));
        assert_eq!(tracker.take_raw(), vec![raw.clone(), raw]);
        assert!(tracker.take_raw().is_empty());
    }

    #[test]
    fn unmeasured_failure_is_skipped_but_success_marks_route_incomplete() {
        let tracker = Arc::new(RunUsageTracker::default());
        drop(CallUsageObserver::new(
            tracker.clone(),
            "failed".into(),
            false,
        ));
        assert!(tracker.models().is_empty());
        let mut observer = CallUsageObserver::new(tracker.clone(), "answered".into(), false);
        observer.completed(None, None);
        drop(observer);
        assert_eq!(tracker.models().len(), 1);
        assert!(!tracker.models()[0].usage_complete);
        assert_eq!(tracker.total(), RunUsage::default());
    }
}
