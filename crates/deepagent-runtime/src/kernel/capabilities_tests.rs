//! H-219: consume the admitted registry evidence, never append a second copy.
use super::*;
use crate::agent::{AgentDecision, Observation};
use async_trait::async_trait;
use deepagent_core::clock::FixedClock;
use deepagent_tools::{PermissionSet, RiskLevel, Tool, ToolDescriptor, ToolOutput};

struct NamedTool(&'static str);

#[async_trait]
impl Tool for NamedTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: self.0.into(),
            description: "capability probe".into(),
            parameters: serde_json::json!({"type":"object"}),
            risk: RiskLevel::Safe,
            required_permissions: PermissionSet::read_only(),
        }
    }

    async fn invoke(&self, _: serde_json::Value) -> Result<ToolOutput> {
        panic!("the capability check must never execute a tool")
    }
}

#[derive(Default)]
struct ProbeAgent(usize);

#[async_trait]
impl Agent for ProbeAgent {
    async fn think(&mut self, _: usize, _: &[Observation]) -> Result<AgentDecision> {
        self.0 += 1;
        Ok(AgentDecision::Complete("done".into()))
    }
}

fn build_registry(name: &'static str) -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(NamedTool(name))).unwrap();
    registry
}

#[tokio::test]
async fn h219_kernel_reuses_matching_admitted_snapshot_and_legacy_writes_exactly_one() {
    for prebound in [false, true] {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let clock = FixedClock::new(1);
        let mut session = Session::create(&db, &clock, None).unwrap();
        let task = session.create_task("capability probe").unwrap();
        let registry = build_registry("expected");
        let store = RunStore::new(&db);
        if prebound {
            store
                .create("h219-kernel", &session.id().to_string(), None, 1)
                .unwrap();
            let mut snapshot =
                serde_json::to_value(registry_capability_fingerprint(&registry).unwrap()).unwrap();
            snapshot["version"] = serde_json::json!(1);
            snapshot["runId"] = serde_json::json!("h219-kernel");
            snapshot["threadId"] = serde_json::json!(session.id().to_string());
            snapshot["snapshotId"] = serde_json::json!("admitted-identity");
            store
                .append_event(
                    "h219-kernel",
                    1,
                    "preparing",
                    "completed",
                    "capability.snapshot",
                    &snapshot,
                )
                .unwrap();
        }
        let mut agent = ProbeAgent::default();
        let kernel = AgentKernel::<FixedClock>::new(
            db.clone(),
            &registry,
            Metrics::new(),
            RuntimeConfig::default(),
            "h219-kernel",
        );
        assert_eq!(
            kernel
                .start_prepared(&mut session, task, &mut agent)
                .await
                .unwrap()
                .kind,
            TerminalKind::Succeeded
        );
        assert_eq!(agent.0, 1);
        let events = store.events_after("h219-kernel", None).unwrap();
        let snapshots: Vec<_> = events
            .iter()
            .filter(|e| e.event_type == "capability.snapshot")
            .collect();
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].data["toolCount"], 1);
        if prebound {
            assert_eq!(snapshots[0].data["snapshotId"], "admitted-identity");
        } else {
            assert!(snapshots[0].data.get("snapshotId").is_none());
        }
    }
}

#[tokio::test]
async fn h219_kernel_rejects_mismatched_duplicate_and_malformed_snapshots_before_agent() {
    for case in [
        "schema",
        "duplicate",
        "malformed",
        "foreign-run",
        "foreign-thread",
    ] {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let clock = FixedClock::new(1);
        let mut session = Session::create(&db, &clock, None).unwrap();
        let task = session.create_task("must not execute").unwrap();
        let registry = build_registry("expected");
        let store = RunStore::new(&db);
        store
            .create("h219-rejected", &session.id().to_string(), None, 1)
            .unwrap();
        let mut snapshot = match case {
            // Same tool count: rejecting a changed schema requires the actual hash.
            "schema" => serde_json::to_value(
                registry_capability_fingerprint(&build_registry("other")).unwrap(),
            )
            .unwrap(),
            "malformed" => serde_json::json!({"toolCount":1}),
            _ => serde_json::to_value(registry_capability_fingerprint(&registry).unwrap()).unwrap(),
        };
        if case.starts_with("foreign-") {
            snapshot["version"] = serde_json::json!(1);
            snapshot["runId"] = serde_json::json!(if case == "foreign-run" {
                "other-run"
            } else {
                "h219-rejected"
            });
            snapshot["threadId"] = serde_json::json!(if case == "foreign-thread" {
                "other-thread".into()
            } else {
                session.id().to_string()
            });
        }
        for _ in 0..if case == "duplicate" { 2 } else { 1 } {
            store
                .append_event(
                    "h219-rejected",
                    1,
                    "preparing",
                    "completed",
                    "capability.snapshot",
                    &snapshot,
                )
                .unwrap();
        }
        let mut agent = ProbeAgent::default();
        let kernel = AgentKernel::<FixedClock>::new(
            db.clone(),
            &registry,
            Metrics::new(),
            RuntimeConfig::default(),
            "h219-rejected",
        );
        assert!(
            kernel
                .start_prepared(&mut session, task, &mut agent)
                .await
                .is_err(),
            "{case}"
        );
        assert_eq!(agent.0, 0, "{case}");
        assert_eq!(session.state().task(task).unwrap().state, TaskState::Queued);
    }
}
