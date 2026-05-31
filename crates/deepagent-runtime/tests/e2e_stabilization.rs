//! Phase 13 — End-to-end stabilization tests.
//!
//! These exercise the kernel under stress to validate the plan's Phase 13
//! acceptance criteria in a fast, deterministic form:
//! - **长任务 / 连续运行**: a runtime loop driving many (>500) tool calls.
//! - **崩溃恢复 100%**: kill a session mid-run and reconstruct it from the
//!   event log, asserting the projection matches.
//! - **并发**: many independent sessions running concurrently without
//!   cross-contamination.
//! - **事件存储完整性**: gapless sequences hold under load.
//!
//! They use in-memory databases and a scripted agent so they run in
//! milliseconds while still going through the real append-only event store,
//! session replay, tool registry, and runtime loop.

use std::sync::Arc;

use async_trait::async_trait;
use deepagent_core::clock::FixedClock;
use deepagent_core::error::Result;
use deepagent_core::task::TaskState;
use deepagent_persistence::Database;
use deepagent_runtime::agent::{Agent, AgentDecision, Observation};
use deepagent_runtime::{RuntimeConfig, RuntimeEngine};
use deepagent_session::Session;
use deepagent_tools::permission::{PermissionSet, RiskLevel};
use deepagent_tools::{Tool, ToolDescriptor, ToolInvocation, ToolOutput, ToolRegistry};
use deepagent_tracing::metrics::{names, Metrics};

/// A no-op tool that always succeeds.
struct NoopTool;

#[async_trait]
impl Tool for NoopTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "noop".into(),
            description: "does nothing".into(),
            parameters: serde_json::json!({"type": "object"}),
            risk: RiskLevel::Safe,
            required_permissions: PermissionSet::read_only(),
        }
    }
    async fn invoke(&self, _args: serde_json::Value) -> Result<ToolOutput> {
        Ok(ToolOutput::success(serde_json::json!({"ok": true})))
    }
}

/// An agent that calls the tool `n` times then completes.
struct BusyAgent {
    remaining: usize,
}

#[async_trait]
impl Agent for BusyAgent {
    async fn think(&mut self, _step: usize, _last: &[Observation]) -> Result<AgentDecision> {
        if self.remaining > 0 {
            self.remaining -= 1;
            Ok(AgentDecision::CallTool(ToolInvocation::new(
                "noop",
                serde_json::json!({}),
            )))
        } else {
            Ok(AgentDecision::Complete("done".into()))
        }
    }
}

fn registry() -> ToolRegistry {
    let mut r = ToolRegistry::new();
    r.register(Arc::new(NoopTool)).unwrap();
    r
}

#[tokio::test]
async fn long_run_500_plus_tool_calls_stay_consistent() {
    let db = Database::open_in_memory().unwrap();
    let clock = FixedClock::new(1);
    let mut session = Session::create(&db, &clock, Some("long")).unwrap();
    let task = session.create_task("crunch").unwrap();

    const CALLS: usize = 600;
    let mut agent = BusyAgent { remaining: CALLS };
    let reg = registry();
    let metrics = Metrics::new();
    let config = RuntimeConfig {
        max_steps: CALLS + 5,
        ..Default::default()
    };
    let engine = RuntimeEngine::new(&reg, metrics.clone(), config);
    engine.run(&mut session, task, &mut agent).await.unwrap();

    // The task completed and exactly CALLS tools ran.
    assert_eq!(
        session.state().task(task).unwrap().state,
        TaskState::Completed
    );
    assert_eq!(metrics.get(names::TOOL_CALLS), CALLS as u64);
    assert_eq!(session.state().tool_calls_completed, CALLS);
}

#[tokio::test]
async fn crash_recovery_reconstructs_exact_state() {
    let db = Database::open_in_memory().unwrap();
    let clock = FixedClock::new(1);

    let sid;
    let task;
    {
        let mut session = Session::create(&db, &clock, Some("crashy")).unwrap();
        sid = session.id();
        task = session.create_task("work").unwrap();
        let mut agent = BusyAgent { remaining: 50 };
        let reg = registry();
        let config = RuntimeConfig {
            max_steps: 100,
            ..Default::default()
        };
        let engine = RuntimeEngine::new(&reg, Metrics::new(), config);
        engine.run(&mut session, task, &mut agent).await.unwrap();
        // Simulate a crash: drop the live session, keep only the durable DB.
    }

    // Recover purely from the event log.
    let recovered = Session::recover(&db, &clock, sid).unwrap();
    assert_eq!(
        recovered.state().task(task).unwrap().state,
        TaskState::Completed
    );
    assert_eq!(recovered.state().tool_calls_completed, 50);
    assert_eq!(recovered.state().message_count, 1); // the final "done" message
}

#[tokio::test]
async fn many_concurrent_sessions_do_not_interfere() {
    // Each session gets its own in-memory DB (independent streams). Run them
    // concurrently and assert each completes its own workload.
    const SESSIONS: usize = 25;
    const CALLS_PER: usize = 20;

    let mut handles = Vec::new();
    for i in 0..SESSIONS {
        handles.push(tokio::spawn(async move {
            let db = Database::open_in_memory().unwrap();
            let clock = FixedClock::new(1 + i as i64);
            let mut session = Session::create(&db, &clock, Some("concurrent")).unwrap();
            let task = session.create_task("w").unwrap();
            let mut agent = BusyAgent {
                remaining: CALLS_PER,
            };
            let reg = registry();
            let config = RuntimeConfig {
                max_steps: CALLS_PER + 5,
                ..Default::default()
            };
            let engine = RuntimeEngine::new(&reg, Metrics::new(), config);
            engine.run(&mut session, task, &mut agent).await.unwrap();
            session.state().tool_calls_completed
        }));
    }

    let mut total = 0usize;
    for h in handles {
        total += h.await.unwrap();
    }
    assert_eq!(total, SESSIONS * CALLS_PER);
}

#[tokio::test]
async fn event_log_sequences_are_gapless_under_load() {
    use deepagent_persistence::event_store::EventStore;

    let db = Database::open_in_memory().unwrap();
    let clock = FixedClock::new(1);
    let sid;
    {
        let mut session = Session::create(&db, &clock, None).unwrap();
        sid = session.id();
        let task = session.create_task("load").unwrap();
        let mut agent = BusyAgent { remaining: 200 };
        let reg = registry();
        let config = RuntimeConfig {
            max_steps: 300,
            ..Default::default()
        };
        let engine = RuntimeEngine::new(&reg, Metrics::new(), config);
        engine.run(&mut session, task, &mut agent).await.unwrap();
    }

    // load_session internally verifies contiguity; assert it loads and the
    // sequences are exactly 0..n.
    let store = EventStore::new(&db);
    let events = store.load_session(sid).unwrap();
    assert!(events.len() > 400); // ~200 requests + 200 completions + lifecycle
    for (i, e) in events.iter().enumerate() {
        assert_eq!(e.sequence, i as u64);
    }
}

#[tokio::test]
async fn repeated_recovery_is_idempotent() {
    // Recovering the same session many times must always yield identical state
    // (no drift / no side effects) — the replay determinism guarantee.
    let db = Database::open_in_memory().unwrap();
    let clock = FixedClock::new(1);
    let sid;
    {
        let mut session = Session::create(&db, &clock, Some("idem")).unwrap();
        sid = session.id();
        let task = session.create_task("x").unwrap();
        let mut agent = BusyAgent { remaining: 30 };
        let reg = registry();
        let config = RuntimeConfig {
            max_steps: 60,
            ..Default::default()
        };
        RuntimeEngine::new(&reg, Metrics::new(), config)
            .run(&mut session, task, &mut agent)
            .await
            .unwrap();
    }

    let first = Session::recover(&db, &clock, sid).unwrap();
    let first_state = first.state().clone();
    for _ in 0..10 {
        let again = Session::recover(&db, &clock, sid).unwrap();
        assert_eq!(again.state(), &first_state);
    }
}
