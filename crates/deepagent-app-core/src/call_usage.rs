//! Run-owned accounting for physical model calls. Reuses the models'
//! physical-request observer, the host's pricing and the existing run ledger.
//! Results/decisions are deliberately not a prerequisite for booking usage.

use std::sync::{Arc, Mutex, Weak};

use deepagent_core::clock::{Clock, SystemClock};
use deepagent_core::error::{CoreError, Result};
use deepagent_core::id::EventId;
use deepagent_persistence::cost_store::{CostStore, RunCallUsage};
use deepagent_persistence::run_store::RunStore;
use deepagent_persistence::Database;
use deepagent_runtime::agent::RunUsage;
use deepagent_runtime::events::{CostOrigin, RuntimeEvent};
use deepagent_runtime::usage::{CallUsageReport, CallUsageSink};
use deepagent_runtime::RuntimeEventSink;

use crate::cost_service::CostService;
use crate::run_finalizer::cost_request_for;

#[derive(Clone)]
pub(crate) struct RunCallUsageOwner {
    pub(crate) db: Arc<Database>,
    pub(crate) cost: Option<Arc<CostService>>,
    pub(crate) session_id: String,
    pub(crate) run_id: String,
    pub(crate) events: Weak<dyn RuntimeEventSink>,
}

impl RunCallUsageOwner {
    pub(crate) fn invocation(
        &self,
        invocation_id: String,
        purpose: &'static str,
    ) -> Arc<OwnedCallUsage> {
        self.owned_invocation(invocation_id, purpose, CostOrigin::SideCall)
    }

    pub(crate) fn main_loop(&self) -> Arc<OwnedCallUsage> {
        self.owned_invocation(self.run_id.clone(), "main_loop", CostOrigin::Run)
    }

    fn owned_invocation(
        &self,
        invocation_id: String,
        purpose: &'static str,
        origin: CostOrigin,
    ) -> Arc<OwnedCallUsage> {
        Arc::new(OwnedCallUsage {
            owner: self.clone(),
            invocation_id,
            purpose,
            origin,
            error: Mutex::new(None),
        })
    }
}

pub(crate) struct OwnedCallUsage {
    owner: RunCallUsageOwner,
    invocation_id: String,
    purpose: &'static str,
    origin: CostOrigin,
    error: Mutex<Option<String>>,
}

impl OwnedCallUsage {
    pub(crate) fn main_receipts(
        &self,
    ) -> Result<Vec<deepagent_persistence::cost_store::RunCallReceipt>> {
        Ok(CostStore::new(&self.owner.db)
            .run_call_receipts(&self.owner.session_id, &self.owner.run_id)?
            .into_iter()
            .filter(|receipt| receipt.report["origin"] == "run")
            .collect())
    }

    pub(crate) fn checkpoint(&self) -> Result<()> {
        match self
            .error
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
        {
            Some(error) => Err(CoreError::Persistence(format!(
                "model call accounting failed: {error}"
            ))),
            None => Ok(()),
        }
    }
}

impl CallUsageSink for OwnedCallUsage {
    fn checkpoint(&self) -> Result<()> {
        OwnedCallUsage::checkpoint(self)
    }

    fn before_call(&self) -> Result<()> {
        self.checkpoint()?;
        let Some(cost) = &self.owner.cost else {
            return Ok(());
        };
        let receipts = CostStore::new(&self.owner.db)
            .run_call_receipts(&self.owner.session_id, &self.owner.run_id)?;
        let accounting_complete = !receipts.iter().any(|receipt| {
            receipt.report["ledger_available"] == true
                && (receipt.ledger_id.is_none() || receipt.cost_yuan.is_none())
        });
        cost.check_budget_accounting(accounting_complete)
    }

    fn record(&self, report: CallUsageReport) {
        // Stable receipt identity for this physical attempt; never use the
        // provider response id (which may be absent or repeated on retries).
        let call_id = EventId::new().to_string();
        let data = serde_json::json!({
            "version":1,
            "purpose":self.purpose,
            "origin":self.origin,
            "invocation_id":self.invocation_id,
            "provider_id":report.provider_id,
            "model_id":report.model_id,
            "usage":report.usage,
            "usage_complete":report.usage.is_some(),
            "raw_usage":report.raw_usage.as_ref().map(deepagent_models::usage::sanitized_provider_usage),
            "ledger_available":self.owner.cost.is_some(),
        });
        let call = RunCallUsage {
            session_id: &self.owner.session_id,
            run_id: &self.owner.run_id,
            call_id: &call_id,
            timestamp: SystemClock.now().as_millis(),
            report: &data,
        };
        let result = match &self.owner.cost {
            Some(cost) => cost.record_run_call(
                &call,
                report.usage.map(|usage| {
                    cost_request_for(
                        &self.owner.session_id,
                        &report.model_id,
                        &RunUsage::from(usage),
                    )
                }),
            ),
            None => CostStore::new(&self.owner.db).record_run_call(&call, None),
        };
        match result {
            Ok(recorded) => {
                // The main turn keeps its existing one final aggregate notice.
                // Its rows are already durable; finalization must not book again.
                if self.origin == CostOrigin::SideCall
                    && recorded.inserted
                    && recorded.ledger_id.is_some()
                {
                    if let Some(events) = self.owner.events.upgrade() {
                        events.emit(RuntimeEvent::CostRecorded {
                            model: report.model_id,
                            cost_yuan: recorded.cost_yuan,
                            origin: CostOrigin::SideCall,
                        });
                    }
                }
            }
            Err(error) => {
                // No successful CostRecorded, zero charge or silent fallback.
                // Retain an explicit failed receipt when the run ledger is
                // still writable (e.g. a cost insert alone was rejected).
                tracing::error!(run_id = %self.owner.run_id, %call_id, %error, "model call usage could not be booked");
                *self.error.lock().unwrap_or_else(|p| p.into_inner()) = Some(error.to_string());
                let store = RunStore::new(&self.owner.db);
                let failed_receipt = store.get(&self.owner.run_id).and_then(|run| {
                    let run = run.ok_or_else(|| CoreError::not_found("model call parent run"))?;
                    store.append_event(
                        &self.owner.run_id,
                        call.timestamp,
                        &run.state,
                        "failed",
                        "model_call_usage_failed",
                        &serde_json::json!({"version":1,"session_id":self.owner.session_id,
                            "call_id":call_id,"report":data,"error_kind":"accounting_failed"}),
                    )
                });
                if let Err(receipt_error) = failed_receipt {
                    tracing::error!(run_id = %self.owner.run_id, %call_id, %receipt_error, "failed model usage receipt could not be persisted");
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "call_usage_tests.rs"]
mod tests;
