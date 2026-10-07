use std::collections::HashSet;
use std::str::FromStr;
use std::sync::Arc;

use deepagent_core::clock::{Clock, SystemClock};
use deepagent_core::error::Result;
use deepagent_core::event::EventPayload;
use deepagent_models::ModelClient;
use deepagent_persistence::Database;
use deepagent_runtime::{
    agent::RunUsage,
    events::{CostOrigin, RuntimeEvent},
    RuntimeEventSink,
};
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
    /// When the unattended memory pass is allowed to fold duplicates. Production
    /// uses the defaults; a test that wants to watch the trigger fire lowers it.
    consolidation_gate: crate::memory_consolidation::ConsolidationGate,
}

pub(crate) struct AppRunFinalizerRequest<'a> {
    pub(crate) session_id: &'a str,
    pub(crate) run_id: &'a str,
    pub(crate) discovered_before_run: &'a HashSet<String>,
    pub(crate) discovered_tools: &'a DiscoveredToolSet,
    pub(crate) usage: Option<RunUsage>,
    /// Tokens of model calls the run loop did not make itself (compaction
    /// summaries). Booked as its own ledger row, never folded into `usage`, so
    /// the per-turn token display keeps describing only the sampling loop.
    pub(crate) side_usage: Option<RunUsage>,
    pub(crate) model_name: &'a str,
    pub(crate) sink: &'a dyn RuntimeEventSink,
    pub(crate) run_succeeded: bool,
    pub(crate) capture_client: Option<Arc<ModelClient>>,
    pub(crate) capture_model: Option<String>,
}

/// The ledger row one set of measured calls turns into.
///
/// One mapping for every booking site (run, compaction summary, session-note
/// pass), so "what did this cost the session" cannot be answered two ways.
/// It reads the billed totals: a pass that answered six requests is billed for
/// six, not for the last request's context size.
fn cost_request_for(session_id: &str, model_name: &str, usage: &RunUsage) -> CostRecordRequest {
    CostRecordRequest {
        session_id: session_id.to_string(),
        model: model_name.to_string(),
        input_tokens: usage.billed_prompt_tokens,
        output_tokens: usage.completion_tokens,
        cache_hit_tokens: usage.billed_cache_hit_tokens,
        cache_miss_tokens: usage.billed_cache_miss_tokens,
        total_tokens: usage.billed_prompt_tokens + usage.completion_tokens,
    }
}

/// Put one measured model call into the session's cost ledger and tell the UI.
///
/// Shared by the run itself and by side calls (pre-run compaction, session-note
/// extraction) so "what did this session cost" has exactly one answer. A call
/// that reported no tokens is skipped, not recorded as zero.
///
/// The ledger books [`RunUsage::billed_prompt_tokens`] and the billed cache
/// totals, never the `prompt_tokens` gauge: a run that took 60 steps was billed
/// for 60 prompts, and booking the last request's context size understated
/// session spend by orders of magnitude.
pub(crate) fn record_usage_cost(
    cost: &CostService,
    session_id: &str,
    model_name: &str,
    usage: RunUsage,
    origin: CostOrigin,
    sink: &dyn RuntimeEventSink,
) {
    match cost.record(cost_request_for(session_id, model_name, &usage)) {
        Ok(Some(cny)) => {
            tracing::info!(cost_yuan = cny, "recorded run cost");
            sink.emit(RuntimeEvent::CostRecorded {
                model: model_name.to_string(),
                cost_yuan: Some(cny),
                origin,
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
                origin,
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
            consolidation_gate: crate::memory_consolidation::ConsolidationGate::default(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_consolidation_gate(
        mut self,
        gate: crate::memory_consolidation::ConsolidationGate,
    ) -> Self {
        self.consolidation_gate = gate;
        self
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
        if let (Some(cost), Some(side)) = (&self.cost, request.side_usage) {
            // Compaction spend is booked as its own row, never folded into the
            // turn's usage (that would move the numbers the panel shows).
            record_usage_cost(
                cost,
                request.session_id,
                request.model_name,
                side,
                CostOrigin::SideCall,
                request.sink,
            );
        }
        self.clear_run_cancellation(request.session_id, request.run_id);
        if request.run_succeeded {
            self.consolidate_memories(session, request.session_id)?;
        }
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

    /// Fold duplicate long-term entries when the gate allows, and record what the
    /// pass changed in this run's own event log, so a merge is still auditable
    /// after a refresh instead of only in the vault's file layout.
    fn consolidate_memories<C: Clock>(
        &self,
        session: &mut Session<'_, C>,
        session_id: &str,
    ) -> Result<()> {
        let Some(knowledge) = &self.knowledge else {
            return Ok(());
        };
        let gate = self.consolidation_gate;
        let Some(note) = crate::memory_consolidation::consolidate_if_due(
            &self.db,
            knowledge,
            session_id,
            &SystemClock,
            &gate,
        )?
        else {
            return Ok(());
        };
        session.append(EventPayload::Note { text: note })?;
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
        // A provider that never reported usage produces an all-zero record; the
        // ledger speaks billed totals, so those are what decide "nothing to book".
        if u.billed_prompt_tokens == 0 && u.completion_tokens == 0 {
            return;
        }
        record_usage_cost(cost, session_id, model_name, u, CostOrigin::Run, sink);
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
        }
        let Some(usage) = outcome.usage else {
            if outcome.extracted {
                tracing::warn!(session_id, "session-memory: pass reported no usage");
            }
            return;
        };
        tracing::info!(
            session_id,
            extracted = outcome.extracted,
            reason = outcome.reason,
            prompt_tokens = usage.prompt_tokens,
            completion_tokens = usage.completion_tokens,
            "session-memory: note pass billed"
        );
        // Cost of the internal pass joins the ordinary ledger for the same
        // session; it is never a private accounting path. A pass that produced no
        // note still answered model calls, and the ledger describes spend.
        let Some(cost) = &self.cost else {
            return;
        };
        if let Err(error) = cost.record(cost_request_for(&session_id, &model, &usage)) {
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
                    side_usage: None,
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

    /// The wiring, not just the algorithm: a successful run must actually open
    /// the vault, fold the duplicates, keep them recoverable, and leave a durable
    /// note in that run's own event log.
    #[test]
    fn a_finished_run_consolidates_the_vault_and_notes_it_in_the_event_log() {
        use deepagent_knowledge::{
            EntryKind, KnowledgeBase, KnowledgeConfig, KnowledgeDraft, Scope, Vault,
        };
        use deepagent_memory::HashingEmbedder;

        let tmp = tempfile::tempdir().unwrap();
        let db = Arc::new(Database::open_in_memory().unwrap());
        let mut base = KnowledgeBase::new(
            vec![Vault::new(tmp.path().join("proj"), Scope::Project)],
            HashingEmbedder::default(),
            KnowledgeConfig::default(),
        );
        let note = |title: &str, body: &str| KnowledgeDraft {
            title: title.to_string(),
            body: body.to_string(),
            kind: EntryKind::Pitfall,
            tags: vec![],
            source_session: Some("ses-other".to_string()),
            scope: Scope::Project,
            tier: None,
            importance: None,
        };
        // Two decisions, each written down twice under reworded titles.
        for (first, second, body) in [
            (
                "cargo 并行构建产生假错误",
                "cargo 并行构建会假报错",
                "本机 cargo 必须 -j1，并行构建会报出成百 prelude 假错误。",
            ),
            (
                "pnpm 构建脚本白名单",
                "pnpm allowBuilds 需要显式赋值",
                "新增依赖后要写 allowBuilds，否则 install 以退出码 1 失败。",
            ),
        ] {
            for title in [first, second] {
                base.write(note(title, body), 1_000).unwrap();
            }
        }
        let knowledge = Arc::new(KnowledgeService::from_base(base));
        assert_eq!(knowledge.entry_count(), 4);

        let mut session = Session::create(&db, &SystemClock, Some("consol")).unwrap();
        let session_id = session.id().to_string();
        let finalizer = AppRunFinalizer::new(
            db.clone(),
            None,
            Some(knowledge.clone()),
            Arc::new(Mutex::new(HashMap::new())),
            SessionMemoryPasses::default(),
        )
        .with_consolidation_gate(crate::memory_consolidation::ConsolidationGate {
            min_entries: 4,
            min_interval: std::time::Duration::from_secs(0),
            min_runs_between: 1,
        });
        let empty: DiscoveredToolSet = Arc::new(Mutex::new(HashSet::new()));
        let before = HashSet::new();
        finalizer
            .finalize_after_kernel(
                &mut session,
                AppRunFinalizerRequest {
                    session_id: &session_id,
                    run_id: "run_consolidate",
                    discovered_before_run: &before,
                    discovered_tools: &empty,
                    usage: None,
                    side_usage: None,
                    model_name: "deepseek-chat",
                    sink: &NullEventSink,
                    run_succeeded: true,
                    capture_client: None,
                    capture_model: None,
                },
            )
            .unwrap();

        assert_eq!(
            knowledge.entry_count(),
            2,
            "one entry per decision must remain"
        );
        let archived = std::fs::read_dir(tmp.path().join("proj").join(".superseded"))
            .expect("the archive directory exists")
            .filter_map(|entry| entry.ok())
            .count();
        assert_eq!(archived, 2, "the duplicates are archived, not deleted");

        let events = deepagent_persistence::event_store::EventStore::new(&db)
            .load_session(session.id())
            .unwrap();
        assert!(
            events.iter().any(|event| {
                matches!(&event.payload, EventPayload::Note { text } if text.contains("2 merged"))
            }),
            "the pass has to be visible in the run's durable event log"
        );
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

    /// One answered provider request, as the model layer reports it.
    fn provider_call(prompt: u32, completion: u32, hit: u32, miss: u32) -> deepagent_models::Usage {
        deepagent_models::Usage {
            prompt_tokens: prompt,
            completion_tokens: completion,
            total_tokens: prompt + completion,
            prompt_cache_hit_tokens: hit,
            prompt_cache_miss_tokens: miss,
            ..Default::default()
        }
    }

    /// A run's usage, folded the way the loop folds it: one `record_call` per
    /// answered request.
    fn run_of(calls: &[deepagent_models::Usage]) -> RunUsage {
        let mut usage = RunUsage::default();
        for call in calls {
            usage.record_call(call);
        }
        usage
    }

    #[test]
    fn compaction_side_usage_is_booked_as_its_own_row() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let cost = Arc::new(CostService::new(db.clone()));
        let clock = SystemClock;
        let mut session = Session::create(&db, &clock, Some("rows")).unwrap();
        let session_id = session.id().to_string();
        let finalizer = AppRunFinalizer::new(
            db.clone(),
            Some(cost.clone()),
            None,
            Arc::new(Mutex::new(HashMap::new())),
            SessionMemoryPasses::default(),
        );
        let empty: DiscoveredToolSet = Arc::new(Mutex::new(HashSet::new()));
        let before = HashSet::new();
        // The bar needs to tell these two rows apart: only the run's own row
        // settles the in-flight totals it has been holding.
        let (sink, mut events) = deepagent_runtime::ChannelSink::new();
        let sink: &dyn RuntimeEventSink = &sink;

        finalizer
            .finalize_after_kernel(
                &mut session,
                AppRunFinalizerRequest {
                    session_id: &session_id,
                    run_id: "run_rows",
                    discovered_before_run: &before,
                    discovered_tools: &empty,
                    usage: Some(run_of(&[
                        provider_call(500, 20, 400, 100),
                        provider_call(450, 15, 300, 150),
                        provider_call(550, 15, 400, 150),
                    ])),
                    side_usage: Some(RunUsage::from(provider_call(900, 60, 700, 200))),
                    model_name: "deepseek-chat",
                    sink,
                    run_succeeded: false,
                    capture_client: None,
                    capture_model: None,
                },
            )
            .unwrap();

        // Two rows, not one merged figure: the turn's own tokens and the
        // compaction tokens both reach the ledger. The turn's input is the sum
        // of its three requests (1500), not the last request's context size
        // (550): every step resends the conversation and is billed again.
        let summary = cost.summary(&session_id).unwrap();
        assert_eq!(summary.input_tokens, 1500 + 900);
        assert_eq!(summary.output_tokens, 50 + 60);
        assert_eq!(summary.cache_hit_tokens, 1100 + 700);
        assert_eq!(summary.cache_miss_tokens, 400 + 200);

        let origins: Vec<CostOrigin> = std::iter::from_fn(|| events.try_recv().ok())
            .filter_map(|event| match event {
                RuntimeEvent::CostRecorded { origin, .. } => Some(origin),
                _ => None,
            })
            .collect();
        assert_eq!(
            origins,
            vec![CostOrigin::Run, CostOrigin::SideCall],
            "the run's own row must be told apart from the compaction row"
        );
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
        // user's run used, so the panel cannot under-report what happened. Each
        // of the three calls re-sent the conversation and reported 120 input
        // tokens, so the session owes 360 — not the last call's 120.
        let summary = cost.summary(&session_key.to_string()).unwrap();
        assert_eq!(summary.input_tokens, 360);
        assert_eq!(summary.output_tokens, 54);

        assert!(
            passes.try_begin(session_key).is_some(),
            "the session slot is released when the pass ends"
        );
    }

    /// A pass that leaves the note as it found it claims no coverage, but it did
    /// answer model calls: the ledger has to describe spend, not outcomes.
    #[tokio::test]
    async fn a_deferred_pass_still_pays_into_the_ledger() {
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
                    ScriptedTransport::text("nothing worth recording"),
                ]),
                "deepseek-chat".to_string(),
            )
            .await;

        let note = std::fs::read_to_string(db.session_memory_path(session_key)).unwrap();
        assert!(
            note.contains("(nothing recorded yet)"),
            "an untouched note must stay the template: {note}"
        );
        let summary = cost.summary(&session_key.to_string()).unwrap();
        // Both calls reported 120 input tokens for the context they were sent.
        assert_eq!(summary.input_tokens, 240, "the attempt was billed");
        // Two answered calls in this script, not three: completion tokens
        // accumulate per call, so the deferred pass is cheaper but not free.
        assert_eq!(summary.output_tokens, 36);
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
