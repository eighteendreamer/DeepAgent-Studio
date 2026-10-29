//! H-139: execution facts must precede every fallible post-tool extension.

use super::*;
use async_trait::async_trait;
use deepagent_core::{clock::FixedClock, error::CoreError, id::SessionId};
use deepagent_hooks::Hook;
use deepagent_persistence::{
    event_store::EventStore,
    run_control::{RunActionState, RunControlStore},
    run_store::RunStore,
    Database,
};
use deepagent_tools::{RiskLevel, Tool, ToolDescriptor, ToolInvocation, ToolOutput};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};

const RUN: &str = "post-tool-commit";

struct FileTool {
    write: bool,
    calls: Arc<AtomicUsize>,
    barrier: Option<Arc<tokio::sync::Barrier>>,
}

#[async_trait]
impl Tool for FileTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: if self.write {
                "write_file"
            } else {
                "read_file"
            }
            .into(),
            description: "temporary file regression fixture".into(),
            parameters: serde_json::json!({"type":"object","required":["path"],
                "properties":{"path":{"type":"string"}}}),
            risk: if self.write {
                RiskLevel::Medium
            } else {
                RiskLevel::Safe
            },
            required_permissions: if self.write {
                PermissionSet::developer()
            } else {
                PermissionSet::read_only()
            },
        }
    }

    async fn invoke(&self, arguments: serde_json::Value) -> Result<ToolOutput> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some(barrier) = &self.barrier {
            barrier.wait().await;
        }
        let path = arguments["path"].as_str().unwrap();
        let value = if self.write {
            std::fs::write(path, "real committed bytes")
                .map_err(|error| CoreError::other(error.to_string()))?;
            "real committed bytes".into()
        } else {
            std::fs::read_to_string(path).map_err(|error| CoreError::other(error.to_string()))?
        };
        Ok(ToolOutput::success(serde_json::json!({"content":value})))
    }
}

enum HookEffect {
    Continue,
    Error,
    Cancel(Arc<AtomicBool>),
    Pending(Arc<tokio::sync::Notify>),
}

struct CommitHook {
    db: Arc<Database>,
    expected: Vec<(String, bool)>,
    host_generated: bool,
    calls: Arc<AtomicUsize>,
    effect: HookEffect,
}

impl CommitHook {
    fn assert_committed(&self, session_id: SessionId) {
        let history = EventStore::new(&self.db).load_session(session_id).unwrap();
        let completed = history
            .iter()
            .filter_map(|event| match &event.payload {
                EventPayload::ToolCallCompleted {
                    call_id,
                    ok,
                    output,
                    ..
                } => {
                    assert!(!output.to_string().contains("post hook regression error"));
                    Some((call_id.clone(), *ok))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        if self.host_generated {
            assert!(completed.is_empty(), "host checks are not model tool calls");
        } else {
            assert_eq!(
                completed, self.expected,
                "all executed peers must commit before hooks"
            );
        }
        let actions = RunControlStore::new(&self.db).list_actions(RUN).unwrap();
        assert_eq!(actions.len(), self.expected.len());
        for (action, (call_id, ok)) in actions.iter().zip(&self.expected) {
            assert_eq!(&action.call_id, call_id);
            assert_eq!(
                action.state,
                if *ok {
                    RunActionState::Completed
                } else {
                    RunActionState::Failed
                }
            );
            assert!(!action
                .blocked_reason
                .as_deref()
                .unwrap_or_default()
                .contains("post hook"));
        }
    }
}

#[async_trait]
impl Hook for CommitHook {
    fn name(&self) -> &str {
        "commit-before-hook"
    }

    async fn run(&self, context: &HookContext) -> Result<HookOutcome> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.assert_committed(context.session_id);
        match &self.effect {
            HookEffect::Continue => return Ok(HookOutcome::Continue),
            HookEffect::Error => {}
            HookEffect::Cancel(cancel) => cancel.store(true, Ordering::Release),
            HookEffect::Pending(entered) => {
                entered.notify_one();
                std::future::pending::<()>().await;
            }
        }
        Err(CoreError::invalid("post hook regression error"))
    }
}

#[derive(Default)]
struct Case {
    parallel: bool,
    speculative: bool,
    failed_tool: bool,
    file_changed: bool,
    cancel_in_hook: bool,
    drop_in_hook: bool,
    host_generated: bool,
    valid_hook: bool,
}

async fn exercise(case: Case) {
    let temporary = tempfile::tempdir().unwrap();
    let db_path = temporary.path().join("state.sqlite");
    let db = Arc::new(Database::open(&db_path).unwrap());
    let clock = FixedClock::new(1);
    let mut session = Session::create(&db, &clock, None).unwrap();
    let session_id = session.id();
    RunStore::new(&db)
        .create(RUN, &session_id.to_string(), None, 1)
        .unwrap();
    let count = if case.parallel { 2 } else { 1 };
    let writes = case.file_changed || case.host_generated;
    let calls = Arc::new(AtomicUsize::new(0));
    let mut registry = ToolRegistry::new();
    registry
        .register(Arc::new(FileTool {
            write: writes,
            calls: calls.clone(),
            barrier: case
                .parallel
                .then(|| Arc::new(tokio::sync::Barrier::new(count))),
        }))
        .unwrap();
    let mut invocations = (0..count)
        .map(|index| {
            let path = temporary.path().join(format!("file-{index}.txt"));
            if !writes && !case.failed_tool {
                std::fs::write(&path, "real read bytes").unwrap();
            }
            ToolInvocation::new(
                if writes { "write_file" } else { "read_file" },
                serde_json::json!({"path":path}),
            )
            .with_id(format!("call-{index}"))
        })
        .collect::<Vec<_>>();
    let persistence =
        crate::tool_pipeline::ToolActionPersistence::new(db.clone(), RUN, "turn").unwrap();
    if !case.host_generated {
        persistence
            .register_invocations(&mut invocations, &registry)
            .unwrap();
    }
    let cancel = Arc::new(AtomicBool::new(false));
    let entered = Arc::new(tokio::sync::Notify::new());
    let hook_calls = Arc::new(AtomicUsize::new(0));
    let hook = Arc::new(CommitHook {
        db: db.clone(),
        expected: invocations
            .iter()
            .map(|call| (call.id.clone().unwrap(), !case.failed_tool))
            .collect(),
        host_generated: case.host_generated,
        calls: hook_calls.clone(),
        effect: if case.drop_in_hook {
            HookEffect::Pending(entered.clone())
        } else if case.cancel_in_hook {
            HookEffect::Cancel(cancel.clone())
        } else if case.valid_hook {
            HookEffect::Continue
        } else {
            HookEffect::Error
        },
    });
    let mut hooks = HookRegistry::new();
    hooks.register(
        if case.file_changed {
            HookPoint::FileChanged
        } else if case.failed_tool {
            HookPoint::PostToolUseFailure
        } else {
            HookPoint::AfterToolUse
        },
        hook.clone(),
    );
    if case.host_generated {
        let result = crate::tool_pipeline::ToolExecutionPipeline::new(
            &registry,
            session_id,
            PermissionSet::developer(),
        )
        .with_action_persistence(Some(persistence))
        .with_hooks(Some(&hooks))
        .with_auto_approve(true)
        .execute_registered(invocations.remove(0))
        .await;
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("post hook regression error"));
    } else {
        let config = RuntimeConfig {
            action_persistence: Some(persistence),
            auto_approve: true,
            tool_timeout: std::time::Duration::from_secs(3),
            checkpoint: case.file_changed.then(|| {
                Arc::new(
                    crate::checkpoint::CheckpointManager::new(
                        db.clone(),
                        RUN,
                        0,
                        temporary.path(),
                        temporary.path().join("checkpoints"),
                    )
                    .unwrap(),
                )
            }),
            ..RuntimeConfig::default()
        };
        let engine: RuntimeEngine<'_, FixedClock> =
            RuntimeEngine::new(&registry, Metrics::default(), config)
                .with_hooks(&hooks)
                .with_cancel(cancel);
        let mut attempt = StreamingToolAttempt::new(
            &registry,
            PermissionSet::developer(),
            Arc::new(AtomicBool::new(false)),
            None,
            engine.observations.clone(),
            std::time::Duration::from_secs(3),
            true,
        );
        let speculative = if case.speculative {
            attempt.begin(1);
            for call in &invocations {
                attempt.prepare(call.clone());
            }
            attempt.commit(1);
            let prepared = attempt.take_speculative_batch(&invocations);
            assert_eq!(
                prepared.len(),
                count,
                "exercise real speculative completion"
            );
            prepared
        } else {
            Default::default()
        };
        {
            let execution =
                engine.execute_tools(&mut session, session_id, invocations, speculative, false);
            if case.drop_in_hook {
                tokio::select! {
                    result = execution => panic!("hook must still be pending: {result:?}"),
                    _ = entered.notified() => {}
                }
            } else {
                let result = execution.await;
                if case.valid_hook {
                    assert_eq!(result.unwrap().len(), count);
                } else {
                    assert!(result
                        .unwrap_err()
                        .to_string()
                        .contains("post hook regression error"));
                }
            }
        }
    }
    hook.assert_committed(session_id);
    assert_eq!(calls.load(Ordering::SeqCst), count, "no repeated execution");
    assert_eq!(
        hook_calls.load(Ordering::SeqCst),
        if case.valid_hook { count } else { 1 }
    );
    if writes {
        assert_eq!(
            std::fs::read_to_string(temporary.path().join("file-0.txt")).unwrap(),
            "real committed bytes"
        );
    }
    // Reopen the real database after releasing all first-run owners. The action
    // must not be rewritten as an interrupted execution by startup recovery.
    let expected = hook.expected.clone();
    drop(session);
    drop(hook);
    drop(hooks);
    drop(db);
    let reopened = Database::open(&db_path).unwrap();
    let actions = RunControlStore::new(&reopened).list_actions(RUN).unwrap();
    for (action, (_, ok)) in actions.iter().zip(expected) {
        assert_eq!(
            action.state,
            if ok {
                RunActionState::Completed
            } else {
                RunActionState::Failed
            }
        );
        assert_ne!(
            action.blocked_reason.as_deref(),
            Some("process_restarted_while_running")
        );
    }
}

#[tokio::test]
async fn failed_tool_commits_before_failure_hook_error() {
    exercise(Case {
        failed_tool: true,
        ..Case::default()
    })
    .await;
}
#[tokio::test]
async fn parallel_peers_commit_before_first_hook_error() {
    exercise(Case {
        parallel: true,
        ..Case::default()
    })
    .await;
}
#[tokio::test]
async fn valid_parallel_hooks_preserve_observations() {
    exercise(Case {
        parallel: true,
        valid_hook: true,
        ..Case::default()
    })
    .await;
}
#[tokio::test]
async fn speculative_result_commits_before_hook_error() {
    exercise(Case {
        speculative: true,
        ..Case::default()
    })
    .await;
}
#[tokio::test]
async fn speculative_parallel_peers_commit_before_hook_error() {
    exercise(Case {
        parallel: true,
        speculative: true,
        ..Case::default()
    })
    .await;
}
#[tokio::test]
async fn file_change_hook_error_preserves_executed_write() {
    exercise(Case {
        file_changed: true,
        ..Case::default()
    })
    .await;
}
#[tokio::test]
async fn cancellation_in_post_hook_cannot_cancel_completed_action() {
    exercise(Case {
        cancel_in_hook: true,
        ..Case::default()
    })
    .await;
}
#[tokio::test]
async fn dropped_post_hook_future_cannot_erase_execution() {
    exercise(Case {
        drop_in_hook: true,
        ..Case::default()
    })
    .await;
}
#[tokio::test]
async fn host_generated_action_commits_before_hook_error() {
    exercise(Case {
        host_generated: true,
        ..Case::default()
    })
    .await;
}
