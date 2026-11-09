use super::*;
use crate::{DecisionSource, HookAdditionalContext, HookData};
use async_trait::async_trait;
use deepagent_core::id::SessionId;
use std::sync::atomic::{AtomicUsize, Ordering};

struct ContextHook {
    name: String,
    decision: HookOutcome,
    text: String,
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl Hook for ContextHook {
    fn name(&self) -> &str {
        &self.name
    }
    async fn run(&self, _ctx: &HookContext) -> Result<HookOutcome> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.decision.clone())
    }
    async fn run_report(
        &self,
        ctx: &HookContext,
        _execution: ToolExecutionContext,
    ) -> Result<HookReport> {
        let outcome = self.run(ctx).await?;
        // A custom hook is not trusted to assign its own provenance or bounds.
        Ok(HookReport {
            outcome,
            additional_context: vec![HookAdditionalContext {
                hook: "forged_system_authority".into(),
                ordinal: usize::MAX,
                text: self.text.clone(),
                truncated: false,
            }],
            context_truncated: false,
        })
    }
}

#[tokio::test]
async fn h140_report_bounds_deduplicates_and_preserves_modify_without_authority_from_output() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut registry = HookRegistry::new();
    for index in 0..20 {
        let hook = Arc::new(ContextHook {
            name: format!("source-{index}"),
            decision: if index == 0 {
                HookOutcome::Modify {
                    updated_input: serde_json::json!({"text":"modified source"}),
                    source: DecisionSource::Policy,
                }
            } else {
                HookOutcome::Continue
            },
            text: "界🦀".repeat(10_000),
            calls: calls.clone(),
        });
        registry.register(HookPoint::UserPromptSubmit, hook.clone());
        registry.register(HookPoint::UserPromptSubmit, hook);
    }
    let ctx = HookContext::new(
        SessionId::nil(),
        HookPoint::UserPromptSubmit,
        HookData::prompt("original"),
    );
    let report = registry
        .dispatch_report(&ctx, ToolExecutionContext::default())
        .await
        .unwrap();
    assert!(report.outcome.is_modify());
    assert_eq!(calls.load(Ordering::SeqCst), 20);
    assert!(report.context_truncated);
    assert!(report.additional_context.len() <= MAX_HOOK_CONTEXTS);
    assert!(
        report
            .additional_context
            .iter()
            .map(|entry| entry.text.len())
            .sum::<usize>()
            <= MAX_HOOK_CONTEXT_TOTAL_BYTES
    );
    for (index, entry) in report.additional_context.iter().enumerate() {
        assert_eq!(entry.hook, format!("source-{index}"));
        assert_eq!(entry.ordinal, index);
        assert!(entry.text.len() <= MAX_HOOK_CONTEXT_BYTES);
        assert!(entry.truncated);
        assert!(entry.text.is_char_boundary(entry.text.len()));
    }
}

#[tokio::test]
async fn h140_reports_cannot_admit_context_for_vetoes_observers_or_cancelled_owners() {
    for point in [HookPoint::UserPromptSubmit, HookPoint::AfterToolUse] {
        for decision in [HookOutcome::deny("blocked"), HookOutcome::ask("approval")] {
            let mut registry = HookRegistry::new();
            let calls = Arc::new(AtomicUsize::new(0));
            for (name, outcome) in [("text", HookOutcome::Continue), ("gate", decision.clone())] {
                registry.register(
                    point,
                    Arc::new(ContextHook {
                        name: name.into(),
                        decision: outcome,
                        text: "not admitted".into(),
                        calls: calls.clone(),
                    }),
                );
            }
            let ctx = HookContext::new(SessionId::nil(), point, HookData::prompt("input"));
            let report = registry
                .dispatch_report(&ctx, ToolExecutionContext::default())
                .await
                .unwrap();
            assert!(report.additional_context.is_empty());
            assert_eq!(calls.load(Ordering::SeqCst), 2);
            let execution = ToolExecutionContext::default();
            execution.cancellation_token().cancel();
            assert!(registry.dispatch_report(&ctx, execution).await.is_err());
            assert_eq!(
                calls.load(Ordering::SeqCst),
                2,
                "cancel before admission cannot execute handlers"
            );
        }
    }
}

#[tokio::test]
async fn h140_short_context_count_limit_and_once_forward_the_same_handoff() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut registry = HookRegistry::new();
    for index in 0..20 {
        registry.register(
            HookPoint::UserPromptSubmit,
            crate::OnceHook::new(Arc::new(ContextHook {
                name: format!("once-{index}"),
                decision: HookOutcome::Continue,
                text: "PRIVATE_H140_REPORT_BODY".into(),
                calls: calls.clone(),
            })),
        );
    }
    let ctx = HookContext::new(
        SessionId::nil(),
        HookPoint::UserPromptSubmit,
        HookData::prompt("input"),
    );
    let report = registry
        .dispatch_report(&ctx, ToolExecutionContext::default())
        .await
        .unwrap();
    assert_eq!(report.additional_context.len(), MAX_HOOK_CONTEXTS);
    assert!(report.context_truncated);
    assert!(!format!("{report:?}").contains("PRIVATE_H140_REPORT_BODY"));
    let second = registry
        .dispatch_report(&ctx, ToolExecutionContext::default())
        .await
        .unwrap();
    assert!(second.additional_context.is_empty());
    assert_eq!(
        calls.load(Ordering::SeqCst),
        20,
        "forwarding a report cannot run a one-shot twice"
    );
}
