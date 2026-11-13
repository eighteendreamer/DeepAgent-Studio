//! H-240: an unrelated dispatch cannot claim a one-shot action.
#[path = "once/lifecycle.rs"]
mod lifecycle;
#[path = "once/real_actions.rs"]
mod real_actions;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use async_trait::async_trait;
use deepagent_core::{error::Result, id::SessionId};
use deepagent_hooks::{
    HookAction, HookActionExecutor, HookContext, HookData, HookDefinitions, HookOutcome, HookPoint,
    HookRegistry, SystemHookRunner,
};
use serde_json::{json, Value};

fn context(name: &str, path: &str) -> HookContext {
    HookContext::new(
        SessionId::nil(),
        HookPoint::BeforeToolUse,
        HookData::Tool {
            name: name.into(),
            arguments: json!({"path":path}),
            ok: None,
            call_id: None,
            result: None,
            error: None,
        },
    )
}

fn definitions(action: Value, filter: &str) -> HookDefinitions {
    let mut group = json!({"once":true,"hooks":[action]});
    if filter != "condition" {
        group["matcher"] = json!("write_file");
    }
    if filter != "matcher" {
        group["if"] = json!("write_file(allowed.txt)");
    }
    HookDefinitions::parse(&json!({"hooks":{"PreToolUse":[group]}}).to_string()).unwrap()
}

struct Host {
    calls: AtomicUsize,
    outcome: Option<HookOutcome>,
}

#[async_trait]
impl HookActionExecutor for Host {
    async fn execute(&self, _: &HookAction, payload: Value) -> Result<HookOutcome> {
        assert_eq!(payload["tool_name"], "write_file");
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.outcome
            .clone()
            .ok_or_else(|| deepagent_core::error::CoreError::other("controlled once failure"))
    }
}

fn register(definitions: &HookDefinitions, host: Arc<Host>) -> HookRegistry {
    let mut registry = HookRegistry::new();
    assert_eq!(
        definitions.register_into_with_host_wrapped(
            &mut registry,
            Arc::new(SystemHookRunner),
            host,
            |hook| {
                assert!(
                    !hook.applies(&context("read_file", "primer")),
                    "{definitions:?}"
                );
                assert!(
                    hook.applies(&context("write_file", "allowed.txt")),
                    "{definitions:?}"
                );
                hook
            }
        ),
        1
    );
    registry
}

fn action(kind: &str) -> Value {
    json!({"type":kind,"command":"mcp__synthetic__gate","prompt":"synthetic gate","timeout":1})
}

#[tokio::test]
async fn h240_host_actions_share_matching_and_once_owner_lifetime() {
    for kind in ["prompt", "agent", "mcp_tool"] {
        for filter in ["matcher", "condition", "both"] {
            let definitions = definitions(action(kind), filter);
            let host = Arc::new(Host {
                calls: AtomicUsize::new(0),
                outcome: Some(HookOutcome::deny("synthetic gate")),
            });
            let registry = register(&definitions, host.clone());
            let cloned = registry.clone();
            for _ in 0..3 {
                assert_eq!(
                    cloned
                        .dispatch(&context("read_file", "primer"))
                        .await
                        .unwrap(),
                    HookOutcome::Continue
                );
            }
            if filter != "matcher" {
                cloned
                    .dispatch(&context("write_file", "other"))
                    .await
                    .unwrap();
            }
            assert_eq!(host.calls.load(Ordering::SeqCst), 0);
            let matching = context("write_file", "allowed.txt");
            assert!(registry.dispatch(&matching).await.unwrap().is_deny());
            assert_eq!(
                cloned.dispatch(&matching).await.unwrap(),
                HookOutcome::Continue
            );
            assert_eq!(host.calls.load(Ordering::SeqCst), 1, "{kind}/{filter}");
            let fresh = register(&definitions, host.clone());
            assert!(fresh.dispatch(&matching).await.unwrap().is_deny());
            assert_eq!(host.calls.load(Ordering::SeqCst), 2);
        }
    }
}

#[tokio::test]
async fn h240_admitted_allow_deny_async_and_error_all_consume_once() {
    for outcome in [
        Some(HookOutcome::Continue),
        Some(HookOutcome::deny("synthetic gate")),
        Some(HookOutcome::AsyncPending {
            timeout: std::time::Duration::from_secs(1),
        }),
        None,
    ] {
        let host = Arc::new(Host {
            calls: AtomicUsize::new(0),
            outcome: outcome.clone(),
        });
        let registry = register(&definitions(action("mcp_tool"), "both"), host.clone());
        registry
            .dispatch(&context("read_file", "primer"))
            .await
            .unwrap();
        let matching = context("write_file", "allowed.txt");
        let first = registry.dispatch(&matching).await;
        if let Some(outcome) = outcome {
            assert_eq!(
                first.unwrap(),
                if matches!(outcome, HookOutcome::AsyncPending { .. }) {
                    HookOutcome::Continue
                } else {
                    outcome
                }
            );
        } else {
            assert!(first
                .unwrap_err()
                .to_string()
                .contains("controlled once failure"));
        }
        assert_eq!(
            registry.dispatch(&matching).await.unwrap(),
            HookOutcome::Continue
        );
        assert_eq!(host.calls.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn h240_parallel_misses_never_claim_and_matching_clones_admit_at_most_once() {
    let host = Arc::new(Host {
        calls: AtomicUsize::new(0),
        outcome: Some(HookOutcome::Continue),
    });
    let registry = Arc::new(register(
        &definitions(action("mcp_tool"), "both"),
        host.clone(),
    ));
    let barrier = Arc::new(tokio::sync::Barrier::new(33));
    let contexts = (0..32)
        .map(|index| {
            context(
                if index % 2 == 0 {
                    "read_file"
                } else {
                    "write_file"
                },
                "allowed.txt",
            )
        })
        .collect::<Vec<_>>();
    let tasks = contexts
        .into_iter()
        .map(|ctx| {
            let registry = registry.clone();
            let barrier = barrier.clone();
            tokio::spawn(async move {
                barrier.wait().await;
                registry.dispatch(&ctx).await
            })
        })
        .collect::<Vec<_>>();
    barrier.wait().await;
    let results = futures::future::join_all(tasks).await;
    assert!(results
        .into_iter()
        .all(|result| result.unwrap().unwrap() == HookOutcome::Continue));
    assert_eq!(host.calls.load(Ordering::SeqCst), 1);
}
