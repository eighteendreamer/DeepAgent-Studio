//! The hook registry / dispatcher.
//!
//! Hooks are registered against one or more [`HookPoint`]s. When the runtime
//! reaches a point it calls [`HookRegistry::dispatch`], which runs every hook
//! registered there in registration order.
//!
//! Veto / decision semantics (mirroring the IDE hook rules and the §13
//! permission protocol):
//! - At a **vetoable** point, hooks are combined by precedence
//!   `Deny > Ask > Modify > Continue`. A [`HookOutcome::Deny`] short-circuits
//!   dispatch immediately (terminal). `Ask` and `Modify` are accumulated: the
//!   highest-precedence outcome seen so far is kept, and a later `Deny` still
//!   overrides it.
//! - At a **non-vetoable** (observational) point, any non-`Continue` outcome is
//!   downgraded to a logged warning and dispatch continues — observers cannot
//!   halt or alter the loop.

use std::collections::BTreeMap;
use std::sync::Arc;

use deepagent_core::error::Result;

use crate::hook::{Hook, HookOutcome};
use crate::lifecycle::{HookContext, HookPoint};

/// Registers hooks per lifecycle point and dispatches them.
#[derive(Default, Clone)]
pub struct HookRegistry {
    hooks: BTreeMap<HookPoint, Vec<Arc<dyn Hook>>>,
}

impl HookRegistry {
    /// New empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register `hook` to fire at `point`. The same hook instance can be
    /// registered at multiple points by calling this repeatedly.
    pub fn register(&mut self, point: HookPoint, hook: Arc<dyn Hook>) -> &mut Self {
        self.hooks.entry(point).or_default().push(hook);
        self
    }

    /// Number of hooks registered at `point`.
    pub fn count_at(&self, point: HookPoint) -> usize {
        self.hooks.get(&point).map(|v| v.len()).unwrap_or(0)
    }

    /// Whether any hooks are registered at `point`.
    pub fn has_hooks(&self, point: HookPoint) -> bool {
        self.count_at(point) > 0
    }

    /// Dispatch all hooks registered at `ctx.point`.
    ///
    /// Returns the effective [`HookOutcome`] after combining every hook's
    /// result by precedence `Deny > Ask > Modify > Continue`:
    /// - `Deny` short-circuits immediately (terminal).
    /// - `Ask` / `Modify` are accumulated; the highest-precedence outcome wins,
    ///   and a later `Deny` overrides them.
    ///
    /// At non-vetoable points every non-`Continue` outcome is downgraded to a
    /// warning and `Continue` is returned.
    pub async fn dispatch(&self, ctx: &HookContext) -> Result<HookOutcome> {
        let Some(hooks) = self.hooks.get(&ctx.point) else {
            return Ok(HookOutcome::Continue);
        };

        let vetoable = ctx.point.is_vetoable();
        // Accumulator: the highest-precedence non-terminal outcome so far.
        let mut effective = HookOutcome::Continue;

        for hook in hooks {
            let outcome = hook.run(ctx).await?;
            if matches!(outcome, HookOutcome::Continue) {
                continue;
            }

            if !vetoable {
                tracing::warn!(
                    hook = hook.name(),
                    point = ctx.point.label(),
                    "hook returned a non-Continue outcome at a non-vetoable point; ignoring"
                );
                continue;
            }

            match &outcome {
                HookOutcome::Deny { reason, source } => {
                    tracing::info!(
                        hook = hook.name(),
                        point = ctx.point.label(),
                        reason = reason.as_str(),
                        source = source.label(),
                        "hook denied operation"
                    );
                    return Ok(outcome); // terminal
                }
                HookOutcome::Ask { reason, source } => {
                    tracing::info!(
                        hook = hook.name(),
                        point = ctx.point.label(),
                        reason = reason.as_str(),
                        source = source.label(),
                        "hook requested approval"
                    );
                    // Ask outranks Modify/Continue.
                    if !effective.is_ask() {
                        effective = outcome;
                    }
                }
                HookOutcome::Modify { source, .. } => {
                    tracing::info!(
                        hook = hook.name(),
                        point = ctx.point.label(),
                        source = source.label(),
                        "hook rewrote operation input"
                    );
                    // Modify only upgrades over Continue (Ask outranks it).
                    if matches!(effective, HookOutcome::Continue) {
                        effective = outcome;
                    }
                }
                HookOutcome::Continue => unreachable!("filtered above"),
            }
        }
        Ok(effective)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lifecycle::{HookData, HookPoint};
    use async_trait::async_trait;
    use deepagent_core::id::SessionId;

    struct RecordingHook {
        name: String,
        outcome: HookOutcome,
        calls: Arc<std::sync::atomic::AtomicUsize>,
    }

    #[async_trait]
    impl Hook for RecordingHook {
        fn name(&self) -> &str {
            &self.name
        }
        async fn run(&self, _ctx: &HookContext) -> Result<HookOutcome> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(self.outcome.clone())
        }
    }

    fn ctx(point: HookPoint) -> HookContext {
        HookContext::new(SessionId::nil(), point, HookData::None)
    }

    #[tokio::test]
    async fn empty_registry_continues() {
        let reg = HookRegistry::new();
        let out = reg.dispatch(&ctx(HookPoint::BeforeToolUse)).await.unwrap();
        assert_eq!(out, HookOutcome::Continue);
    }

    #[tokio::test]
    async fn deny_at_vetoable_point_short_circuits() {
        let calls_a = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let calls_b = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut reg = HookRegistry::new();
        reg.register(
            HookPoint::BeforeToolUse,
            Arc::new(RecordingHook {
                name: "denier".into(),
                outcome: HookOutcome::deny("blocked"),
                calls: calls_a.clone(),
            }),
        );
        reg.register(
            HookPoint::BeforeToolUse,
            Arc::new(RecordingHook {
                name: "after".into(),
                outcome: HookOutcome::Continue,
                calls: calls_b.clone(),
            }),
        );

        let out = reg.dispatch(&ctx(HookPoint::BeforeToolUse)).await.unwrap();
        assert_eq!(out, HookOutcome::deny("blocked"));
        // First hook ran, second was short-circuited.
        assert_eq!(calls_a.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(calls_b.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn deny_at_observational_point_is_ignored() {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut reg = HookRegistry::new();
        reg.register(
            HookPoint::AfterToolUse,
            Arc::new(RecordingHook {
                name: "noisy".into(),
                outcome: HookOutcome::deny("cannot stop me"),
                calls: calls.clone(),
            }),
        );
        let out = reg.dispatch(&ctx(HookPoint::AfterToolUse)).await.unwrap();
        // Downgraded to Continue.
        assert_eq!(out, HookOutcome::Continue);
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn hooks_run_in_registration_order_until_veto() {
        let mut reg = HookRegistry::new();
        let order = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));

        struct OrderHook {
            name: String,
            order: Arc<std::sync::Mutex<Vec<String>>>,
        }
        #[async_trait]
        impl Hook for OrderHook {
            fn name(&self) -> &str {
                &self.name
            }
            async fn run(&self, _ctx: &HookContext) -> Result<HookOutcome> {
                self.order.lock().unwrap().push(self.name.clone());
                Ok(HookOutcome::Continue)
            }
        }

        for n in ["h1", "h2", "h3"] {
            reg.register(
                HookPoint::SessionStart,
                Arc::new(OrderHook {
                    name: n.into(),
                    order: order.clone(),
                }),
            );
        }
        reg.dispatch(&ctx(HookPoint::SessionStart)).await.unwrap();
        assert_eq!(*order.lock().unwrap(), vec!["h1", "h2", "h3"]);
    }

    #[test]
    fn registration_counts() {
        let mut reg = HookRegistry::new();
        assert!(!reg.has_hooks(HookPoint::SessionStart));
        reg.register(
            HookPoint::SessionStart,
            Arc::new(RecordingHook {
                name: "x".into(),
                outcome: HookOutcome::Continue,
                calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            }),
        );
        assert_eq!(reg.count_at(HookPoint::SessionStart), 1);
        assert!(reg.has_hooks(HookPoint::SessionStart));
    }

    fn noop_calls() -> Arc<std::sync::atomic::AtomicUsize> {
        Arc::new(std::sync::atomic::AtomicUsize::new(0))
    }

    #[tokio::test]
    async fn ask_outranks_modify_and_continue() {
        let mut reg = HookRegistry::new();
        reg.register(
            HookPoint::BeforeToolUse,
            Arc::new(RecordingHook {
                name: "modifier".into(),
                outcome: HookOutcome::modify(serde_json::json!({"safe": true})),
                calls: noop_calls(),
            }),
        );
        reg.register(
            HookPoint::BeforeToolUse,
            Arc::new(RecordingHook {
                name: "asker".into(),
                outcome: HookOutcome::ask("please confirm"),
                calls: noop_calls(),
            }),
        );
        let out = reg.dispatch(&ctx(HookPoint::BeforeToolUse)).await.unwrap();
        assert!(out.is_ask());
    }

    #[tokio::test]
    async fn deny_overrides_a_prior_ask() {
        let mut reg = HookRegistry::new();
        reg.register(
            HookPoint::BeforeToolUse,
            Arc::new(RecordingHook {
                name: "asker".into(),
                outcome: HookOutcome::ask("confirm?"),
                calls: noop_calls(),
            }),
        );
        reg.register(
            HookPoint::BeforeToolUse,
            Arc::new(RecordingHook {
                name: "denier".into(),
                outcome: HookOutcome::deny("hard no"),
                calls: noop_calls(),
            }),
        );
        let out = reg.dispatch(&ctx(HookPoint::BeforeToolUse)).await.unwrap();
        assert!(out.is_deny());
        assert_eq!(out.deny_reason(), Some("hard no"));
    }

    #[tokio::test]
    async fn lone_modify_is_returned_at_vetoable_point() {
        let mut reg = HookRegistry::new();
        reg.register(
            HookPoint::BeforeToolUse,
            Arc::new(RecordingHook {
                name: "modifier".into(),
                outcome: HookOutcome::modify(serde_json::json!({"x": 2})),
                calls: noop_calls(),
            }),
        );
        let out = reg.dispatch(&ctx(HookPoint::BeforeToolUse)).await.unwrap();
        assert!(out.is_modify());
    }

    #[tokio::test]
    async fn ask_at_observational_point_is_downgraded() {
        let mut reg = HookRegistry::new();
        reg.register(
            HookPoint::AfterToolUse,
            Arc::new(RecordingHook {
                name: "asker".into(),
                outcome: HookOutcome::ask("too late"),
                calls: noop_calls(),
            }),
        );
        let out = reg.dispatch(&ctx(HookPoint::AfterToolUse)).await.unwrap();
        assert_eq!(out, HookOutcome::Continue);
    }
}
