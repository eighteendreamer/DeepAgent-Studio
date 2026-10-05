use std::collections::HashSet;
use std::str::FromStr;
use std::sync::Arc;

use deepagent_core::clock::Clock;
use deepagent_core::error::Result;
use deepagent_core::event::EventPayload;
use deepagent_models::ModelClient;
use deepagent_persistence::Database;
use deepagent_runtime::{agent::RunUsage, events::RuntimeEvent, RuntimeEventSink};
use deepagent_session::Session;

use crate::cost_service::{CostRecordRequest, CostService};
use crate::knowledge_service::KnowledgeService;
use crate::session_memory::{
    pending_tool_call_at_tail, SessionMemoryExtractor, SessionMemoryPasses,
};
use crate::tool_manifest::DiscoveredToolSet;

use crate::run_coordinator::CancellationMap;

#[derive(Clone)]
pub(crate) struct AppRunFinalizer {
    db: Arc<Database>,
    cost: Option<Arc<CostService>>,
    knowledge: Option<Arc<KnowledgeService>>,
    cancellations: CancellationMap,
    session_memory: SessionMemoryPasses,
}

pub(crate) struct AppRunFinalizerRequest<'a> {
    pub(crate) session_id: &'a str,
    pub(crate) run_id: &'a str,
    pub(crate) discovered_before_run: &'a HashSet<String>,
    pub(crate) discovered_tools: &'a DiscoveredToolSet,
    pub(crate) usage: Option<RunUsage>,
    pub(crate) model_name: &'a str,
    pub(crate) sink: &'a dyn RuntimeEventSink,
    pub(crate) run_succeeded: bool,
    pub(crate) capture_client: Option<Arc<ModelClient>>,
    pub(crate) capture_model: Option<String>,
}

/// Put one measured model call into the session's cost ledger and tell the UI.
///
/// Shared by the run itself and by side calls (pre-run compaction, session-note
/// extraction) so "what did this session cost" has exactly one answer. A call
/// that reported no tokens is skipped, not recorded as zero.
pub(crate) fn record_usage_cost(
    cost: &CostService,
    session_id: &str,
    model_name: &str,
    usage: RunUsage,
    sink: &dyn RuntimeEventSink,
) {
    match cost.record(CostRecordRequest {
        session_id: session_id.to_string(),
        model: model_name.to_string(),
        input_tokens: usage.prompt_tokens,
        output_tokens: usage.completion_tokens,
        cache_hit_tokens: usage.prompt_cache_hit_tokens,
        cache_miss_tokens: usage.prompt_cache_miss_tokens,
        total_tokens: usage.total_tokens,
    }) {
        Ok(Some(cny)) => {
            tracing::info!(cost_yuan = cny, "recorded run cost");
            sink.emit(RuntimeEvent::CostRecorded {
                model: model_name.to_string(),
                cost_yuan: Some(cny),
            });
        }
        Ok(None) => {
            tracing::warn!(
                model = model_name,
                "token usage saved but cost unavailable (no pricing for model)"
            );
            sink.emit(RuntimeEvent::CostRecorded {
                model: model_name.to_string(),
                cost_yuan: None,
            });
        }
        Err(error) => tracing::warn!(error = %error, "failed to record run cost"),
    }
}

impl AppRunFinalizer {
    pub(crate) fn new(
        db: Arc<Database>,
        cost: Option<Arc<CostService>>,
        knowledge: Option<Arc<KnowledgeService>>,
        cancellations: CancellationMap,
        session_memory: SessionMemoryPasses,
    ) -> Self {
        Self {
            db,
            cost,
            knowledge,
            cancellations,
            session_memory,
        }
    }

    pub(crate) fn finalize_after_kernel<C: Clock>(
        &self,
        session: &mut Session<'_, C>,
        request: AppRunFinalizerRequest<'_>,
    ) -> Result<()> {
        self.persist_discovered_tools_delta(
            session,
            request.discovered_before_run,
            request.discovered_tools,
        )?;
        let context_tokens = request
            .usage
            .map_or(0u64, |usage| usage.total_tokens as u64);
        self.record_run_cost(
            request.session_id,
            request.model_name,
            request.usage,
            request.sink,
        );
        self.clear_run_cancellation(request.session_id, request.run_id);
        self.spawn_session_memory(
            request.run_succeeded,
            request.session_id,
            context_tokens,
            request.capture_client.clone(),
            request.capture_model.clone(),
        );
        self.spawn_auto_capture(
            request.run_succeeded,
            request.session_id,
            request.capture_client,
            request.capture_model,
        );
        Ok(())
    }

    fn persist_discovered_tools_delta<C: Clock>(
        &self,
        session: &mut Session<'_, C>,
        discovered_before_run: &HashSet<String>,
        discovered_tools: &DiscoveredToolSet,
    ) -> Result<()> {
        let new_discovered: Vec<String> = {
            let now = discovered_tools.lock().unwrap_or_else(|p| p.into_inner());
            let mut out: Vec<String> = now
                .iter()
                .filter(|n| !discovered_before_run.contains(n.as_str()))
                .cloned()
                .collect();
            out.sort();
            out
        };
        if !new_discovered.is_empty() {
            session.append(EventPayload::ToolsDiscovered {
                names: new_discovered,
            })?;
        }
        Ok(())
    }

    fn record_run_cost(
        &self,
        session_id: &str,
        model_name: &str,
        usage: Option<RunUsage>,
        sink: &dyn RuntimeEventSink,
    ) {
        let Some(cost) = &self.cost else {
            return;
        };
        let Some(u) = usage else {
            return;
        };
        if u.total_tokens == 0 {
            return;
        }
        record_usage_cost(cost, session_id, model_name, u, sink);
    }

    fn clear_run_cancellation(&self, session_id: &str, run_id: &str) {
        let mut map = self.cancellations.lock().unwrap_or_else(|p| p.into_inner());
        map.remove(session_id);
        map.remove(run_id);
    }

    fn spawn_session_memory(
        &self,
        run_succeeded: bool,
        session_id: &str,
        context_tokens: u64,
        client: Option<Arc<ModelClient>>,
        model: Option<String>,
    ) {
        // A failed turn leaves the transcript mid-flight; the next successful
        // turn folds it in. Without a reported context size there is no honest
        // way to apply the token gate, so the pass is skipped rather than guessed.
        if !run_succeeded || context_tokens == 0 {
            return;
        }
        let (Some(client), Some(model)) = (client, model) else {
            return;
        };
        let Ok(session_key) = deepagent_core::id::SessionId::from_str(session_id) else {
            tracing::warn!(session_id, "session-memory: bad session id");
            return;
        };
        let Some(guard) = self.session_memory.try_begin(session_key) else {
            tracing::debug!(session_id, "session-memory: a pass is already running");
            return;
        };
        let this = self.clone();
        tokio::spawn(async move {
            this.run_session_memory_pass(guard, session_key, context_tokens, client, model)
                .await
        });
    }

    /// Fold the newest part of this session's transcript into its rolling note.
    ///
    /// Kept separate from the spawn so the pass can be awaited in tests; the
    /// guard is held for the whole body, which is what keeps two passes for the
    /// same session from overwriting each other's note.
    pub(crate) async fn run_session_memory_pass(
        &self,
        guard: crate::session_memory::SessionMemoryPassGuard,
        session_key: deepagent_core::id::SessionId,
        context_tokens: u64,
        client: Arc<ModelClient>,
        model: String,
    ) {
        let _guard = guard;
        let session_id = session_key.to_string();
        let extractor = SessionMemoryExtractor::new(self.db.clone(), client, model.clone());
        // Reading the session log means decompressing the whole transcript, so
        // do it only when the token prerequisite could actually open the gate.
        let might_be_due = match extractor.might_be_due(session_key, context_tokens) {
            Ok(might_be_due) => might_be_due,
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    session_id = %session_id,
                    "session-memory: cursor read failed"
                );
                return;
            }
        };
        if !might_be_due {
            tracing::debug!(session_id, "session-memory: token gate not open");
            return;
        }
        let store = deepagent_persistence::event_store::EventStore::new(&self.db);
        let Ok(events) = store.load_session(session_key) else {
            tracing::warn!(session_id, "session-memory: load session failed");
            return;
        };
        // The same model-facing view every other projection uses, so the note
        // folds in exactly what the model was about to see.
        let history = crate::input_runtime::model_history_from_events(&events);
        let unsafe_tail = pending_tool_call_at_tail(&events);
        let outcome = match extractor
            .run_if_due(session_key, &history, &events, context_tokens, unsafe_tail)
            .await
        {
            Ok(outcome) => outcome,
            Err(error) => {
                tracing::warn!(error = %error, session_id = %session_id, "session-memory: pass failed");
                return;
            }
        };
        if !outcome.extracted {
            tracing::debug!(
                session_id,
                reason = outcome.reason,
                "session-memory: pass deferred"
            );
            return;
        }
        let Some(usage) = outcome.usage else {
            tracing::warn!(session_id, "session-memory: pass reported no usage");
            return;
        };
        tracing::info!(
            session_id,
            prompt_tokens = usage.prompt_tokens,
            completion_tokens = usage.completion_tokens,
            "session-memory: note updated"
        );
        // Cost of the internal pass joins the ordinary ledger for the same
        // session; it is never a private accounting path.
        let Some(cost) = &self.cost else {
            return;
        };
        if let Err(error) = cost.record(CostRecordRequest {
            session_id: session_id.clone(),
            model: model.clone(),
            input_tokens: usage.prompt_tokens,
            output_tokens: usage.completion_tokens,
            cache_hit_tokens: usage.prompt_cache_hit_tokens,
            cache_miss_tokens: usage.prompt_cache_miss_tokens,
            total_tokens: usage.total_tokens,
        }) {
            tracing::warn!(error = %error, "session-memory: failed to record pass cost");
        }
    }

    fn spawn_auto_capture(
        &self,
        run_succeeded: bool,
        session_id: &str,
        capture_client: Option<Arc<ModelClient>>,
        capture_model: Option<String>,
    ) {
        if !run_succeeded {
            return;
        }
        let Some(knowledge) = &self.knowledge else {
            return;
        };
        if !knowledge.auto_capture_enabled() {
            return;
        }
        let Some(client) = capture_client else {
            return;
        };
        let Some(model) = capture_model else {
            return;
        };

        let knowledge = knowledge.clone();
        let db = self.db.clone();
        let sid = session_id.to_string();
        tokio::spawn(async move {
            let events = match deepagent_core::id::SessionId::from_str(&sid) {
                Ok(id) => {
                    let store = deepagent_persistence::event_store::EventStore::new(&db);
                    match store.load_session(id) {
                        Ok(evs) => evs,
                        Err(error) => {
                            tracing::warn!(error = %error, "auto-capture: load session failed");
                            return;
                        }
                    }
                }
                Err(error) => {
                    tracing::warn!(error = %error, "auto-capture: bad session id");
                    return;
                }
            };
            if let Some(dto) = knowledge
                .capture_from_session(client, model, &events, &sid)
                .await
            {
                tracing::info!(id = %dto.id, "auto-captured knowledge");
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepagent_core::clock::SystemClock;
    use deepagent_runtime::NullEventSink;
    use std::collections::HashMap;
    use std::sync::atomic::AtomicBool;
    use std::sync::Mutex;

    #[test]
    fn clear_run_cancellation_removes_session_and_run_aliases() {
        let map: CancellationMap = Arc::new(Mutex::new(HashMap::new()));
        let flag = Arc::new(AtomicBool::new(false));
        map.lock()
            .unwrap()
            .insert("ses_test".to_string(), flag.clone());
        map.lock().unwrap().insert("run_test".to_string(), flag);

        let finalizer = AppRunFinalizer::new(
            Arc::new(Database::open_in_memory().unwrap()),
            None,
            None,
            map.clone(),
            SessionMemoryPasses::default(),
        );
        finalizer.clear_run_cancellation("ses_test", "run_test");

        assert!(map.lock().unwrap().is_empty());
    }

    #[test]
    fn discovered_tool_delta_is_persisted_once() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let clock = SystemClock;
        let mut session = Session::create(&db, &clock, Some("delta")).unwrap();
        let session_id = session.id().to_string();
        let discovered: DiscoveredToolSet = Arc::new(Mutex::new(HashSet::from([
            "old_tool".to_string(),
            "new_tool".to_string(),
        ])));
        let before = HashSet::from(["old_tool".to_string()]);
        let finalizer = AppRunFinalizer::new(
            db.clone(),
            None,
            None,
            Arc::new(Mutex::new(HashMap::new())),
            SessionMemoryPasses::default(),
        );

        finalizer
            .finalize_after_kernel(
                &mut session,
                AppRunFinalizerRequest {
                    session_id: &session_id,
                    run_id: "run_test",
                    discovered_before_run: &before,
                    discovered_tools: &discovered,
                    usage: None,
                    model_name: "model",
                    sink: &NullEventSink,
                    run_succeeded: false,
                    capture_client: Some(Arc::new(ModelClient::new(
                        Arc::new(deepagent_models::MockTransport::new([
                            r#"{"type":"response.completed","response":{"status":"completed"}}"#
                                .to_string(),
                        ])),
                        deepagent_models::ModelConfig::deepseek("test"),
                    ))),
                    capture_model: Some("model".to_string()),
                },
            )
            .unwrap();

        let events = deepagent_persistence::event_store::EventStore::new(&db)
            .load_session(session.id())
            .unwrap();
        assert!(events.iter().any(|event| matches!(
            &event.payload,
            EventPayload::ToolsDiscovered { names } if names == &vec!["new_tool".to_string()]
        )));
    }

    fn finalizer_with(
        db: Arc<Database>,
        cost: Option<Arc<CostService>>,
        passes: &SessionMemoryPasses,
    ) -> AppRunFinalizer {
        AppRunFinalizer::new(
            db,
            cost,
            None,
            Arc::new(Mutex::new(HashMap::new())),
            passes.clone(),
        )
    }

    fn transcript_session(db: &Arc<Database>) -> deepagent_core::id::SessionId {
        let clock = SystemClock;
        let mut session = Session::create(db, &clock, Some("note")).unwrap();
        session
            .append(deepagent_core::event::EventPayload::MessageAppended {
                message: deepagent_core::message::Message::user("decide the storage layout"),
            })
            .unwrap();
        session
            .append(deepagent_core::event::EventPayload::MessageAppended {
                message: deepagent_core::message::Message::assistant(
                    "one vault, files hold bodies",
                ),
            })
            .unwrap();
        session.id()
    }

    #[tokio::test]
    async fn session_memory_pass_folds_the_transcript_and_pays_into_the_ledger() {
        use crate::session_memory::testing::{scripted_client, ScriptedTransport};

        let db = Arc::new(Database::open_in_memory().unwrap());
        let session_key = transcript_session(&db);
        let cost = Arc::new(CostService::new(db.clone()));
        let passes = SessionMemoryPasses::default();
        let guard = passes.try_begin(session_key).expect("slot is free");
        let finalizer = finalizer_with(db.clone(), Some(cost.clone()), &passes);

        finalizer
            .run_session_memory_pass(
                guard,
                session_key,
                20_000,
                scripted_client(vec![
                    ScriptedTransport::call(
                        "c1",
                        "read_file",
                        serde_json::json!({"path": "session-memory.md"}),
                    ),
                    ScriptedTransport::call(
                        "c2",
                        "edit_file",
                        serde_json::json!({
                            "path": "session-memory.md",
                            "old": "(nothing recorded yet)",
                            "new": "- decided: one vault, bodies in files",
                        }),
                    ),
                    ScriptedTransport::text("note updated"),
                ]),
                "deepseek-chat".to_string(),
            )
            .await;

        let note_path = db.session_memory_path(session_key);
        let note = std::fs::read_to_string(&note_path).expect("the pass writes the note");
        assert!(note.contains("decided: one vault"), "{note}");

        // The pass is not free: its tokens land in the same session ledger the
        // user's run used, so the panel cannot under-report what happened.
        let summary = cost.summary(&session_key.to_string()).unwrap();
        assert_eq!(summary.input_tokens, 120);
        assert_eq!(summary.output_tokens, 54);

        assert!(
            passes.try_begin(session_key).is_some(),
            "the session slot is released when the pass ends"
        );
    }

    #[tokio::test]
    async fn small_context_sessions_get_no_note_and_no_cost() {
        use crate::session_memory::testing::scripted_client;

        let db = Arc::new(Database::open_in_memory().unwrap());
        let session_key = transcript_session(&db);
        let cost = Arc::new(CostService::new(db.clone()));
        let passes = SessionMemoryPasses::default();
        let guard = passes.try_begin(session_key).unwrap();
        let finalizer = finalizer_with(db.clone(), Some(cost.clone()), &passes);

        // Below the initialization watermark: no model call, no note, no cost.
        finalizer
            .run_session_memory_pass(guard, session_key, 500, scripted_client(vec![]), "m".into())
            .await;

        assert!(
            !db.session_memory_path(session_key).exists(),
            "the note must not be created before the gate fires"
        );
        let summary = cost.summary(&session_key.to_string()).unwrap();
        assert_eq!(summary.input_tokens, 0);
        assert_eq!(summary.output_tokens, 0);
    }
}
