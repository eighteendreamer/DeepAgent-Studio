//! Real engine/tool/model adoption regression for H-260. No direct cache clear.
use super::*;
use crate::session_memory::testing::{scripted_client_with_transport, ScriptedTransport};
use deepagent_core::clock::SystemClock;
use deepagent_core::message::Role;
use deepagent_runtime::{ModelAgent, RuntimeConfig, RuntimeEngine};
use deepagent_session::Session;
use deepagent_tools::{PermissionSet, ToolRegistry};
use serde_json::{json, Value};
use std::sync::atomic::AtomicUsize;

#[derive(Clone, Copy)]
pub(super) enum Replacement {
    Native,
    Legacy,
    Failed,
    Unchanged,
}

struct DropRead {
    kind: Replacement,
    attempts: AtomicUsize,
    adopted: AtomicBool,
}

#[async_trait]
impl ReactiveContextCompactor for DropRead {
    async fn compact(
        &self,
        messages: &[Message],
        _trigger: CompactionTrigger,
    ) -> Result<Option<ReactiveCompaction>> {
        if self.adopted.load(Ordering::Acquire)
            || !messages.iter().any(|message| {
                message.role == Role::Tool
                    && message.content.contains("H260 original observed bytes")
            })
        {
            return Ok(None);
        }
        self.attempts.fetch_add(1, Ordering::AcqRel);
        if matches!(self.kind, Replacement::Failed) {
            return Err(CoreError::other(
                "synthetic compaction failure; keep the original Surface",
            ));
        }
        if matches!(self.kind, Replacement::Unchanged) {
            return Ok(None);
        }
        self.adopted.store(true, Ordering::Release);
        let summary = "The old file observation was discarded. Read again before editing.";
        Ok(Some(ReactiveCompaction {
            messages: vec![Message::system("synthetic rules"), Message::user(summary)],
            response_items: matches!(self.kind, Replacement::Native).then(|| {
                vec![ResponseInputItem::Message {
                    role: "user".into(),
                    content: summary.into(),
                }]
            }),
            tokens_before: 1000,
            tokens_after: 20,
            summary: summary.into(),
            usage: Default::default(),
        }))
    }
}

fn mutation(tool: &str, old: &str, new: &str) -> Value {
    let old = format!("value={old}");
    let new = format!("value={new}");
    match tool {
        "write_file" => json!({"path":"existing.txt","content":format!("{new}\n")}),
        "edit_file" => json!({"path":"existing.txt","old":old,"new":new}),
        "multi_edit" => json!({"path":"existing.txt","edits":[{"old":old,"new":new}]}),
        _ => unreachable!(),
    }
}

fn output<'a>(request: &'a Value, id: &str) -> &'a str {
    let items: Vec<_> = request["input"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["type"] == "function_call_output" && item["call_id"] == id)
        .collect();
    assert_eq!(items.len(), 1, "missing or duplicated {id}");
    items[0]["output"].as_str().unwrap()
}

pub(super) async fn assert_dropped_read_authority(kind: Replacement) {
    for tool in ["write_file", "edit_file", "multi_edit"] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("existing.txt");
        let initial = format!(
            "H260 original observed bytes\n{}\nvalue=old\n",
            "padding ".repeat(200)
        );
        std::fs::write(&path, &initial).unwrap();
        let mut registry = ToolRegistry::new();
        for tool in
            deepagent_builtins::file_tools(deepagent_builtins::WorkspaceRoot::new(dir.path()))
        {
            registry.register(tool).unwrap();
        }
        let failed = matches!(kind, Replacement::Failed | Replacement::Unchanged);
        let (client, transport) = scripted_client_with_transport(vec![
            ScriptedTransport::call("first-read", "read_file", json!({"path":"existing.txt"})),
            ScriptedTransport::call("without-reread", tool, mutation(tool, "old", "new")),
            ScriptedTransport::call("fresh-read", "read_file", json!({"path":"existing.txt"})),
            ScriptedTransport::call(
                "after-reread",
                tool,
                mutation(tool, if failed { "new" } else { "old" }, "final"),
            ),
            ScriptedTransport::text("Controlled test complete."),
        ]);
        let compactor = Arc::new(DropRead {
            kind,
            attempts: AtomicUsize::new(0),
            adopted: AtomicBool::new(false),
        });
        let permissions = PermissionSet::developer();
        let schemas = registry
            .visible_to(&permissions)
            .into_iter()
            .map(|d| ToolSchema::function(d.name, d.description, d.parameters))
            .collect();
        let mut agent = ModelAgent::new(
            client,
            "deepseek-flash",
            "synthetic rules",
            "Execute the supplied sequence.",
            schemas,
        )
        .with_reactive_compactor(compactor.clone())
        .with_proactive_compaction(1);
        let db = Arc::new(Database::open_in_memory().unwrap());
        let mut session = Session::create(&db, &SystemClock, Some("h260 synthetic")).unwrap();
        let task = session
            .create_task("verify context and read authority")
            .unwrap();
        let engine: RuntimeEngine<'_, SystemClock> = RuntimeEngine::new(
            &registry,
            Default::default(),
            RuntimeConfig {
                permissions,
                auto_approve: true,
                max_steps: Some(8),
                checkpoint: Some(Arc::new(
                    deepagent_runtime::checkpoint::CheckpointManager::new(
                        db.clone(),
                        "h260-synthetic",
                        0,
                        dir.path(),
                        dir.path().join("checkpoints"),
                    )
                    .unwrap(),
                )),
                ..Default::default()
            },
        );
        let outcome = engine.run(&mut session, task, &mut agent).await.unwrap();
        assert!(
            matches!(outcome, deepagent_runtime::RunOutcome::Completed(_)),
            "{outcome:?}"
        );
        assert!(compactor.attempts.load(Ordering::Acquire) > 0);
        let request: Value = serde_json::from_str(&transport.last_prompt()).unwrap();
        let first_write: Value = serde_json::from_str(output(&request, "without-reread")).unwrap();
        assert_eq!(
            first_write["status"] == "ok",
            failed,
            "{tool}: {first_write}"
        );
        assert_eq!(compactor.adopted.load(Ordering::Acquire), !failed);
        if !failed {
            assert!(
                first_write.to_string().contains("must call read_file"),
                "{first_write}"
            );
            assert!(!request["input"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["call_id"] == "first-read"));
            let reread = output(&request, "fresh-read");
            assert!(reread.contains("H260 original observed bytes"));
            assert!(!reread.contains("FILE_UNCHANGED_STUB"));
        }
        let final_write: Value = serde_json::from_str(output(&request, "after-reread")).unwrap();
        assert_eq!(final_write["status"], "ok", "{tool}: {final_write}");
        assert!(std::fs::read_to_string(&path)
            .unwrap()
            .contains("value=final"));
    }
}

#[tokio::test]
async fn legacy_compaction_revokes_three_mutations_in_the_real_engine() {
    assert_dropped_read_authority(Replacement::Legacy).await;
}

#[tokio::test]
async fn failed_compaction_keeps_the_three_original_read_authorities() {
    assert_dropped_read_authority(Replacement::Failed).await;
}

#[tokio::test]
async fn unchanged_compaction_keeps_the_three_original_read_authorities() {
    assert_dropped_read_authority(Replacement::Unchanged).await;
}

/// Grows actual tool history without reading another file or resetting caches.
struct PaddingTool;

#[async_trait]
impl deepagent_tools::Tool for PaddingTool {
    fn descriptor(&self) -> deepagent_tools::ToolDescriptor {
        deepagent_tools::ToolDescriptor {
            name: "padding".into(),
            description: "Controlled context-pressure fixture".into(),
            parameters: json!({"type":"object","properties":{}}),
            risk: deepagent_tools::RiskLevel::Safe,
            required_permissions: PermissionSet::read_only(),
        }
    }

    async fn invoke(&self, _: Value) -> Result<deepagent_tools::ToolOutput> {
        Ok(deepagent_tools::ToolOutput::success(json!({
            "text": "bounded synthetic context padding. ".repeat(400),
        })))
    }
}

fn engine_config(db: Arc<Database>, root: &Path, run: &str) -> RuntimeConfig {
    RuntimeConfig {
        permissions: PermissionSet::developer(),
        auto_approve: true,
        max_steps: Some(18),
        checkpoint: Some(Arc::new(
            deepagent_runtime::checkpoint::CheckpointManager::new(
                db,
                run,
                0,
                root,
                root.join("checkpoints"),
            )
            .unwrap(),
        )),
        tool_result_budget: deepagent_runtime::tool_budget::ToolResultBudgetConfig {
            max_tokens: 100_000,
            output_dir: root.join("tool-results"),
            ..Default::default()
        },
        ..Default::default()
    }
}

#[tokio::test]
async fn production_prune_revokes_mutations_and_cold_resume_does_not_inherit_read_authority() {
    production_replacement_and_cold_resume(false).await;
}

#[tokio::test]
async fn production_full_summary_after_prune_rejection_revokes_file_authority() {
    production_replacement_and_cold_resume(true).await;
}

struct RejectPrune;

impl deepagent_runtime::ContextSurfacePersistence for RejectPrune {
    fn replace_tool_results(&self, _: &str, _: &[(String, String)]) -> Result<()> {
        Err(CoreError::other("controlled prune persistence refusal"))
    }

    fn record_snip(&self, _: &[String]) -> Result<()> {
        Err(CoreError::other(
            "unexpected snip in controlled full-summary fixture",
        ))
    }
}

async fn production_replacement_and_cold_resume(reject_prune: bool) {
    for tool in ["write_file", "edit_file", "multi_edit"] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("workspace");
        std::fs::create_dir(&root).unwrap();
        let path = root.join("existing.txt");
        std::fs::write(
            &path,
            format!(
                "H260 real production read\n{}\nvalue=old\n",
                "source ".repeat(200)
            ),
        )
        .unwrap();
        let db_path = dir.path().join("surface.sqlite3");
        let db = Arc::new(Database::open(&db_path).unwrap());
        let mut session = Session::create(&db, &SystemClock, Some("H260 durable fixture")).unwrap();
        let sid = session.id();
        let mut registry = ToolRegistry::new();
        for file_tool in
            deepagent_builtins::file_tools(deepagent_builtins::WorkspaceRoot::new(&root))
        {
            registry.register(file_tool).unwrap();
        }
        registry.register(Arc::new(PaddingTool)).unwrap();
        let schemas: Vec<_> = registry
            .visible_to(&PermissionSet::developer())
            .into_iter()
            .map(|d| ToolSchema::function(d.name, d.description, d.parameters))
            .collect();
        let mut turns = vec![ScriptedTransport::call(
            "first-read",
            "read_file",
            json!({"path":"existing.txt"}),
        )];
        turns.extend((0..8).map(|index| {
            let mut turn =
                ScriptedTransport::call(&format!("padding-{index}"), "padding", json!({}));
            if reject_prune {
                // Full compaction retains at least five narrative blocks, not
                // five tool dumps. Supply real assistant text in those turns.
                turn.insert(
                    0,
                    ScriptedTransport::text("Controlled earlier narrative block.")[0].clone(),
                );
            }
            // ScriptedTransport::call normally reports a constant 120 input
            // tokens. That would hide growing history from the calibrated
            // meter. Model a provider reporting the growing synthetic input.
            *turn.last_mut().unwrap() =
                ScriptedTransport::text_with_context("", (index + 1) * 4_000)
                    .pop()
                    .unwrap();
            turn
        }));
        turns.extend([
            ScriptedTransport::call("without-reread", tool, mutation(tool, "old", "new")),
            ScriptedTransport::call("fresh-read", "read_file", json!({"path":"existing.txt"})),
            ScriptedTransport::call("after-reread", tool, mutation(tool, "old", "final")),
            ScriptedTransport::call("terminal-read", "read_file", json!({"path":"existing.txt"})),
            ScriptedTransport::text("Controlled production test complete."),
        ]);
        let (client, transport) = scripted_client_with_transport(turns);
        let summary_client =
            crate::session_memory::testing::scripted_client(vec![ScriptedTransport::text(
                r#"{"goal":"controlled sequence","completed":["earlier padding"]}"#,
            )]);
        let persistence: Arc<dyn deepagent_runtime::ContextSurfacePersistence> = if reject_prune {
            Arc::new(RejectPrune)
        } else {
            Arc::new(SessionSurfaceStore {
                db: db.clone(),
                session_id: sid,
            })
        };
        let mut agent = ModelAgent::new(
            client,
            "deepseek-flash",
            "rules",
            "Execute the controlled sequence.",
            schemas.clone(),
        )
        .with_surface_persistence(persistence)
        .with_reactive_compactor(Arc::new(HookedReactiveContextCompactor::new(
            summary_client,
            "deepseek-flash".into(),
            Arc::new(HookRegistry::new()),
            sid,
            db.clone(),
        )))
        .with_proactive_compaction(if reject_prune { 35_000 } else { 18_000 });
        let task = session.create_task("H260 production prune").unwrap();
        let engine = RuntimeEngine::<SystemClock>::new(
            &registry,
            Default::default(),
            engine_config(db.clone(), &root, "production"),
        );
        assert!(matches!(
            engine.run(&mut session, task, &mut agent).await.unwrap(),
            deepagent_runtime::RunOutcome::Completed(_)
        ));
        let request: Value = serde_json::from_str(&transport.last_prompt()).unwrap();
        assert!(output(&request, "terminal-read").contains("value=final"));
        let rejected: Value = serde_json::from_str(output(&request, "without-reread")).unwrap();
        assert_eq!(rejected["status"], "error", "{tool}: {rejected}");
        assert!(rejected.to_string().contains("must call read_file"));
        assert!(output(&request, "fresh-read").contains("H260 real production read"));
        assert!(!output(&request, "fresh-read").contains("FILE_UNCHANGED_STUB"));
        assert_eq!(
            serde_json::from_str::<Value>(output(&request, "after-reread")).unwrap()["status"],
            "ok"
        );
        assert!(std::fs::read_to_string(&path)
            .unwrap()
            .contains("value=final"));
        drop(engine);
        drop(agent);
        drop(session);
        drop(db);

        // Close/reopen the actual database, but deliberately reuse the tools:
        // even an embedder retaining its registry must not carry read authority.
        let db = Arc::new(Database::open(&db_path).unwrap());
        let events = deepagent_persistence::event_store::EventStore::new(&db)
            .load_session(sid)
            .unwrap();
        if reject_prune {
            assert!(events.iter().any(|event| matches!(
                &event.payload,
                EventPayload::ContextCompacted {
                    first_kept_sequence: Some(_),
                    ..
                }
            )));
            assert!(!events
                .iter()
                .any(|event| matches!(&event.payload, EventPayload::ContextSurfacePatched { .. })));
        } else {
            assert!(
                events.iter().any(|event| matches!(&event.payload,
                EventPayload::ContextSurfacePatched { replacements, .. }
                    if replacements.iter().any(|replacement| replacement.call_id == "first-read")))
            );
        }
        assert!(events.iter().any(|event| matches!(&event.payload,
            EventPayload::ResponseItemAppended { item: ResponseInputItem::FunctionCallOutput { call_id, output } }
                if call_id == "first-read" && output.contains("H260 real production read"))));
        let replay = crate::input_runtime::response_items_from_events(&events);
        assert!(!replay.iter().any(|item| matches!(item,
            ResponseInputItem::FunctionCallOutput { call_id, output }
                if call_id == "first-read" && output.contains("H260 real production read"))));
        let mut session = Session::recover(&db, &SystemClock, sid).unwrap();
        match tool {
            "edit_file" => {
                session =
                    Session::fork(&db, &SystemClock, sid, events.last().unwrap().sequence).unwrap();
                assert_ne!(session.id(), sid);
            }
            "multi_edit" => {
                let before_terminal_read = events.iter().rev().find(|event| matches!(&event.payload,
                    EventPayload::ResponseItemAppended { item: ResponseInputItem::FunctionCallOutput { call_id, .. } }
                        if call_id == "after-reread")).unwrap().sequence;
                assert!(session.rewind(before_terminal_read).unwrap() > 0);
            }
            _ => {}
        }
        let restored_events = deepagent_persistence::event_store::EventStore::new(&db)
            .load_session(session.id())
            .unwrap();
        let replay = crate::input_runtime::response_items_from_events(&restored_events);
        let (client, transport) = scripted_client_with_transport(vec![
            ScriptedTransport::call(
                "resume-without-reread",
                tool,
                mutation(tool, "final", "resumed"),
            ),
            ScriptedTransport::call("resume-read", "read_file", json!({"path":"existing.txt"})),
            ScriptedTransport::call(
                "resume-after-read",
                tool,
                mutation(tool, "final", "resumed"),
            ),
            ScriptedTransport::text("Controlled resume complete."),
        ]);
        let mut agent = ModelAgent::new(
            client,
            "deepseek-flash",
            "rules",
            "Resume controlled sequence.",
            schemas,
        )
        .with_response_history(replay);
        let task = session.create_task("H260 cold resume").unwrap();
        let engine = RuntimeEngine::<SystemClock>::new(
            &registry,
            Default::default(),
            engine_config(db.clone(), &root, "resume"),
        );
        assert!(matches!(
            engine.run(&mut session, task, &mut agent).await.unwrap(),
            deepagent_runtime::RunOutcome::Completed(_)
        ));
        let request: Value = serde_json::from_str(&transport.last_prompt()).unwrap();
        let rejected: Value =
            serde_json::from_str(output(&request, "resume-without-reread")).unwrap();
        assert_eq!(rejected["status"], "error");
        assert!(rejected.to_string().contains("must call read_file"));
        assert!(!output(&request, "resume-read").contains("FILE_UNCHANGED_STUB"));
        assert_eq!(
            serde_json::from_str::<Value>(output(&request, "resume-after-read")).unwrap()["status"],
            "ok"
        );
        assert!(std::fs::read_to_string(path)
            .unwrap()
            .contains("value=resumed"));
    }
}
