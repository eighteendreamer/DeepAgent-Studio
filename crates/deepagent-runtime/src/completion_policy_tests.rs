//! H-294: action history is not an implicit final-diff contract.
use super::*;
use crate::{
    agent::testing::ScriptedAgent,
    checkpoint::{CheckpointManager, MutationKind},
    completion::{CompletionPolicy, RequiredEffect, RequiredPathEffect},
};
use async_trait::async_trait;
use deepagent_core::{clock::FixedClock, error::Result, message::Message};
use deepagent_persistence::Database;
use deepagent_tools::{RiskLevel, Tool, ToolDescriptor, ToolInvocation, ToolOutput};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};

struct SameWrite {
    calls: Arc<AtomicUsize>,
    fail: bool,
    cancel: Option<Arc<AtomicBool>>,
}

#[async_trait]
impl Tool for SameWrite {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "write_file".into(),
            description: "fixture".into(),
            parameters: serde_json::json!({"type":"object","required":["path"],"properties":{"path":{"type":"string"}}}),
            risk: RiskLevel::Medium,
            required_permissions: PermissionSet::developer(),
        }
    }

    async fn invoke(&self, arguments: serde_json::Value) -> Result<ToolOutput> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.fail {
            return Ok(ToolOutput::failure("synthetic write failure"));
        }
        std::fs::write(arguments["path"].as_str().unwrap(), "unchanged bytes")
            .map_err(|error| deepagent_core::CoreError::other(error.to_string()))?;
        if let Some(cancel) = &self.cancel {
            cancel.store(true, Ordering::Release);
        }
        Ok(ToolOutput::success(serde_json::json!({"written":true})))
    }
}

async fn exercise(
    explicit: bool,
    batch: bool,
    message: bool,
    denied: bool,
    fail: bool,
    cancel_after: bool,
) {
    let temporary = tempfile::tempdir().unwrap();
    let target = temporary.path().join("same.txt");
    std::fs::write(&target, "unchanged bytes").unwrap();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let checkpoint = Arc::new(
        CheckpointManager::new(
            db.clone(),
            "h294",
            0,
            temporary.path(),
            temporary.path().join("checkpoints"),
        )
        .unwrap(),
    );
    let clock = FixedClock::new(1);
    let mut session = Session::create(&db, &clock, None).unwrap();
    let task = session.create_task("write the same bytes").unwrap();
    let cancel = Arc::new(AtomicBool::new(false));
    let calls = Arc::new(AtomicUsize::new(0));
    let mut registry = ToolRegistry::new();
    registry
        .register(Arc::new(SameWrite {
            calls: calls.clone(),
            fail,
            cancel: cancel_after.then(|| cancel.clone()),
        }))
        .unwrap();
    let invocation =
        ToolInvocation::new("write_file", serde_json::json!({"path":target})).with_id("same-write");
    let final_decision = if message {
        AgentDecision::CompleteMessage(Message::assistant("done"))
    } else {
        AgentDecision::Complete("done".into())
    };
    let mut agent = ScriptedAgent::new([
        if batch {
            AgentDecision::CallTools(vec![invocation])
        } else {
            AgentDecision::CallTool(invocation)
        },
        final_decision.clone(),
        final_decision.clone(),
        final_decision,
    ]);
    let policy = if explicit {
        CompletionPolicy {
            require_create_or_modify: true,
            required_paths: vec![RequiredPathEffect {
                path: target.clone(),
                effect: RequiredEffect::CreateOrModify,
            }],
            ..CompletionPolicy::default()
        }
    } else {
        CompletionPolicy::default()
    };
    let (sink, mut events) = crate::events::ChannelSink::new();
    let engine = RuntimeEngine::new(
        &registry,
        Metrics::new(),
        RuntimeConfig {
            completion_policy: policy,
            checkpoint: Some(checkpoint.clone()),
            max_completion_retries: 1,
            permissions: if denied {
                PermissionSet::read_only()
            } else {
                PermissionSet::developer()
            },
            auto_approve: true,
            ..RuntimeConfig::default()
        },
    )
    .with_events(Arc::new(sink))
    .with_cancel(cancel);
    let outcome = engine.run(&mut session, task, &mut agent).await.unwrap();
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "unchanged bytes");
    assert_eq!(calls.load(Ordering::SeqCst), usize::from(!denied));
    let mut evidence = Vec::new();
    let mut failures = 0;
    while let Ok(event) = events.try_recv() {
        match event {
            RuntimeEvent::CompletionEvidence { mutations } => evidence.push(mutations),
            RuntimeEvent::Verification {
                passed: false,
                detail,
            } if detail.contains("filesystem effect") => failures += 1,
            _ => {}
        }
    }
    if cancel_after {
        assert_eq!(outcome, RunOutcome::Cancelled);
        assert!(evidence.is_empty());
    } else {
        assert!(matches!(outcome, RunOutcome::Completed(_)));
        let gate_observations = agent
            .observations
            .iter()
            .filter(|obs| obs.output["completion_blocked"] == true)
            .count();
        assert_eq!(gate_observations, usize::from(explicit));
        assert_eq!(failures > 0, explicit);
        assert_eq!(evidence.len(), if explicit { 2 } else { 1 });
        if !denied && !fail {
            assert!(evidence
                .iter()
                .all(|items| items.len() == 1 && items[0].kind == MutationKind::Unchanged));
        }
        let actual = agent
            .observations
            .iter()
            .find(|obs| obs.tool == "write_file")
            .unwrap();
        assert_eq!(
            actual.ok,
            !denied && !fail,
            "failure/denial must remain a failed tool result"
        );
    }
    assert!(checkpoint
        .mutation_evidence()
        .unwrap()
        .iter()
        .all(|item| item.kind == MutationKind::Unchanged));
}

#[tokio::test]
async fn no_automatic_net_change_contract_on_single_batch_or_either_completion_arm() {
    for batch in [false, true] {
        for message in [false, true] {
            exercise(false, batch, message, false, false, false).await;
        }
    }
}

#[tokio::test]
async fn explicit_net_change_policy_still_rejects_unchanged_on_both_completion_arms() {
    for message in [false, true] {
        exercise(true, false, message, false, false, false).await;
    }
}

#[tokio::test]
async fn failed_denied_and_cancelled_tools_keep_their_original_semantics() {
    exercise(false, false, false, true, false, false).await;
    exercise(false, false, false, false, true, false).await;
    exercise(false, false, false, false, false, true).await;
}

#[tokio::test]
async fn explicit_satisfied_policy_still_accepts_factual_final_change() {
    for message in [false, true] {
        let temporary = tempfile::tempdir().unwrap();
        let target = temporary.path().join("created.txt");
        let db = Arc::new(Database::open_in_memory().unwrap());
        let checkpoint = Arc::new(
            CheckpointManager::new(
                db.clone(),
                "explicit",
                0,
                temporary.path(),
                temporary.path().join("checkpoints"),
            )
            .unwrap(),
        );
        checkpoint.capture_before(&target).unwrap();
        std::fs::write(&target, "real bytes").unwrap();
        let clock = FixedClock::new(1);
        let mut session = Session::create(&db, &clock, None).unwrap();
        let task = session.create_task("explicit final change").unwrap();
        let mut agent = ScriptedAgent::new([if message {
            AgentDecision::CompleteMessage(Message::assistant("done"))
        } else {
            AgentDecision::Complete("done".into())
        }]);
        let registry = ToolRegistry::new();
        let engine = RuntimeEngine::new(
            &registry,
            Metrics::new(),
            RuntimeConfig {
                checkpoint: Some(checkpoint),
                completion_policy: CompletionPolicy {
                    require_create_or_modify: true,
                    required_paths: vec![RequiredPathEffect {
                        path: target,
                        effect: RequiredEffect::CreateOrModify,
                    }],
                    ..CompletionPolicy::default()
                },
                ..RuntimeConfig::default()
            },
        );
        assert_eq!(
            engine.run(&mut session, task, &mut agent).await.unwrap(),
            RunOutcome::Completed("done".into())
        );
        assert!(agent.observations.is_empty());
    }
}
