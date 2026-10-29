//! Verify every dispatch path and reuse of a shared engine/registry.
use super::*;
use async_trait::async_trait;
use deepagent_core::clock::FixedClock;
use deepagent_persistence::Database;
use deepagent_tools::{
    Tool, ToolContextReset, ToolDescriptor, ToolInvocation, ToolObservation, ToolOutput,
};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Probe {
    seen: Mutex<Vec<(String, ToolObservation)>>,
    barrier: Option<Arc<tokio::sync::Barrier>>,
}

#[async_trait]
impl Tool for Probe {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "observe_context".into(),
            description: "test-only invocation observation identity".into(),
            parameters: serde_json::json!({"type":"object","properties":{"owner":{"type":"string"},"barrier":{"type":"boolean"}},"required":["owner"]}),
            risk: deepagent_tools::RiskLevel::Safe,
            required_permissions: PermissionSet::read_only(),
        }
    }

    async fn invoke(&self, _: serde_json::Value) -> Result<ToolOutput> {
        panic!("runtime must supply its execution context")
    }

    async fn invoke_with_context(
        &self,
        args: serde_json::Value,
        context: ToolExecutionContext,
    ) -> Result<ToolOutput> {
        let observation = context
            .observation()
            .expect("missing runtime observation scope")
            .clone();
        assert!(observation.is_current());
        self.seen
            .lock()
            .unwrap()
            .push((args["owner"].as_str().unwrap().into(), observation));
        if args["barrier"] == true {
            self.barrier.as_ref().unwrap().wait().await;
        }
        Ok(ToolOutput::success(serde_json::json!({"observed":true})))
    }
}

fn call(owner: &str, id: &str, barrier: bool) -> ToolInvocation {
    ToolInvocation::new(
        "observe_context",
        serde_json::json!({"owner":owner,"barrier":barrier}),
    )
    .with_id(id)
}

struct ProbeAgent {
    owner: &'static str,
    barrier: bool,
    reset: Option<ToolContextReset>,
}

#[async_trait]
impl Agent for ProbeAgent {
    fn set_tool_context_reset(&mut self, reset: ToolContextReset) {
        self.reset = Some(reset);
    }

    async fn think(&mut self, step: usize, last: &[Observation]) -> Result<AgentDecision> {
        assert!(last.iter().all(|observation| observation.ok));
        Ok(match step {
            0 => AgentDecision::CallTools(vec![
                call(self.owner, "one", self.barrier),
                call(self.owner, "two", self.barrier),
            ]),
            1 => AgentDecision::CallTool(call(self.owner, "three", false)),
            _ => AgentDecision::Complete("done".into()),
        })
    }
}

#[tokio::test]
async fn concurrent_and_reused_engine_runs_neither_share_nor_revoke_observations() {
    let db = Database::open_in_memory().unwrap();
    let clock = FixedClock::new(1);
    let probe = Arc::new(Probe {
        barrier: Some(Arc::new(tokio::sync::Barrier::new(4))),
        ..Default::default()
    });
    let mut registry = ToolRegistry::new();
    registry.register(probe.clone()).unwrap();
    let engine = RuntimeEngine::new(&registry, Metrics::new(), RuntimeConfig::default());
    let mut first = Session::create(&db, &clock, Some("first")).unwrap();
    let mut second = Session::create(&db, &clock, Some("second")).unwrap();
    let first_task = first.create_task("observe first").unwrap();
    let second_task = second.create_task("observe second").unwrap();
    let mut a = ProbeAgent {
        owner: "a",
        barrier: true,
        reset: None,
    };
    let mut b = ProbeAgent {
        owner: "b",
        barrier: true,
        reset: None,
    };
    let (one, two) = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        tokio::join!(
            engine.run(&mut first, first_task, &mut a),
            engine.run(&mut second, second_task, &mut b)
        )
    })
    .await
    .expect("both runs and their parallel invocations must overlap");
    assert_eq!(one.unwrap(), RunOutcome::Completed("done".into()));
    assert_eq!(two.unwrap(), RunOutcome::Completed("done".into()));
    let mut resumed = ProbeAgent {
        owner: "resumed",
        barrier: false,
        reset: None,
    };
    let task = first
        .create_task("new run on same session and engine")
        .unwrap();
    assert_eq!(
        engine.run(&mut first, task, &mut resumed).await.unwrap(),
        RunOutcome::Completed("done".into())
    );
    let seen = probe.seen.lock().unwrap();
    let tokens = |owner: &str| {
        seen.iter()
            .filter(|(label, _)| label == owner)
            .map(|(_, token)| token)
            .collect::<Vec<_>>()
    };
    let a_tokens = tokens("a");
    let b_tokens = tokens("b");
    let resumed_tokens = tokens("resumed");
    for group in [&a_tokens, &b_tokens, &resumed_tokens] {
        assert_eq!(group.len(), 3);
        assert!(group.iter().all(|token| group[0].same_context(token)));
    }
    assert!(!a_tokens[0].same_context(b_tokens[0]));
    assert!(!a_tokens[0].same_context(resumed_tokens[0]));
    a.reset.as_ref().unwrap()();
    assert!(a_tokens.iter().all(|token| !token.is_current()));
    assert!(b_tokens
        .iter()
        .chain(&resumed_tokens)
        .all(|token| token.is_current()));
}

#[tokio::test]
async fn normal_speculative_and_legacy_dispatch_capture_the_owner_generation() {
    let db = Database::open_in_memory().unwrap();
    let clock = FixedClock::new(1);
    let mut session = Session::create(&db, &clock, Some("paths")).unwrap();
    let session_id = session.id();
    let probe = Arc::new(Probe::default());
    let mut registry = ToolRegistry::new();
    registry.register(probe.clone()).unwrap();
    let engine =
        RuntimeEngine::<FixedClock>::new(&registry, Metrics::new(), RuntimeConfig::default());
    assert!(
        engine
            .tool_pipeline(session_id)
            .execute(call("pipeline", "p", false))
            .await
            .unwrap()
            .output
            .ok
    );
    assert!(
        engine
            .execute_tool_with_before_legacy(
                &mut session,
                session_id,
                call("legacy", "l", false),
                None
            )
            .await
            .unwrap()
            .ok
    );
    let mut attempt = StreamingToolAttempt::new(
        &registry,
        PermissionSet::read_only(),
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
        None,
        engine.observations.clone(),
        std::time::Duration::from_secs(10),
        true,
    );
    let invocation = call("speculative", "s", false);
    attempt.begin(1);
    attempt.prepare(invocation.clone());
    attempt.commit(1);
    let speculative = attempt
        .take_speculative(&invocation)
        .expect("safe read should speculate");
    assert!(speculative.handle.await.unwrap().0.unwrap().ok);
    let old = engine.observations.capture();
    {
        let seen = probe.seen.lock().unwrap();
        assert_eq!(seen.len(), 3);
        assert!(seen.iter().all(|(_, token)| old.same_context(token)));
    }
    engine.observations.reset();
    assert!(!old.is_current());
    assert!(
        engine
            .tool_pipeline(session_id)
            .execute(call("new", "n", false))
            .await
            .unwrap()
            .output
            .ok
    );
    let seen = probe.seen.lock().unwrap();
    assert!(seen[..3].iter().all(|(_, token)| !token.is_current()));
    assert!(seen[3].1.is_current());
}
