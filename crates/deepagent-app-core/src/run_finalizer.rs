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
    /// Main-loop and workflow calls can select different routes. These are
    /// projections of the same canonical usage, not additional spend.
    pub(crate) model_usage: Option<&'a [deepagent_runtime::usage::ModelUsage]>,
    /// Production request-owned calls are already in the one durable ledger.
    /// Finalization projects their identities; it must not charge them twice.
    pub(crate) booked_calls: Option<&'a crate::call_usage::OwnedCallUsage>,
    pub(crate) multi_model_label: &'a str,
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
pub(crate) fn cost_request_for(
    session_id: &str,
    model_name: &str,
    usage: &RunUsage,
) -> CostRecordRequest {
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
    match write_usage_cost(cost, session_id, model_name, &usage) {
        Ok((_, cost_yuan)) => sink.emit(RuntimeEvent::CostRecorded {
            model: model_name.to_string(),
            cost_yuan,
            origin,
        }),
        Err(error) => tracing::warn!(error = %error, "failed to record run cost"),
    }
}

/// The single cost-write boundary, shared by aggregate workflow receipts and
/// the existing single-model/side-call notification wrapper above.
fn write_usage_cost(
    cost: &CostService,
    session_id: &str,
    model_name: &str,
    usage: &RunUsage,
) -> Result<(i64, Option<f64>)> {
    let result = cost.record_with_id(cost_request_for(session_id, model_name, usage))?;
    if let Some(cny) = result.1 {
        tracing::info!(model = model_name, cost_yuan = cny, "recorded run cost");
    } else {
        tracing::warn!(
            model = model_name,
            "token usage saved but cost unavailable (no pricing for model)"
        );
    }
    Ok(result)
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
            knowledge: knowledge.map(|knowledge| knowledge.bind_current()),
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
        if let Some(models) = request.model_usage {
            self.record_model_cost(
                session,
                request.run_id,
                models,
                request.multi_model_label,
                request.sink,
                request.booked_calls,
            )?;
        } else {
            self.record_run_cost(
                request.session_id,
                request.model_name,
                request.usage,
                request.sink,
            );
        }
        if let (None, Some(cost), Some(side)) =
            (request.booked_calls, &self.cost, request.side_usage)
        {
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

    fn record_model_cost<C: Clock>(
        &self,
        session: &mut Session<'_, C>,
        run_id: &str,
        models: &[deepagent_runtime::usage::ModelUsage],
        multi_model_label: &str,
        sink: &dyn RuntimeEventSink,
        booked_calls: Option<&crate::call_usage::OwnedCallUsage>,
    ) -> Result<()> {
        let session_id = session.id().to_string();
        let mut total = models
            .iter()
            .all(|model| model.usage_complete)
            .then_some(0.0);
        let mut ledger_ids = Vec::new();
        if let Some(booked) = booked_calls {
            booked.checkpoint()?;
            for receipt in booked.main_receipts()? {
                if let Some(id) = receipt.ledger_id {
                    ledger_ids.push(id);
                }
                total = total.zip(receipt.cost_yuan).map(|(sum, value)| sum + value);
            }
        } else if let Some(cost) = &self.cost {
            for measured in models {
                if measured.usage.billed_prompt_tokens == 0 && measured.usage.completion_tokens == 0
                {
                    continue;
                }
                match write_usage_cost(cost, &session_id, &measured.model_id, &measured.usage) {
                    Ok((id, value)) => {
                        ledger_ids.push(id);
                        total = total.zip(value).map(|(sum, cost)| sum + cost);
                    }
                    Err(error) => {
                        total = None;
                        tracing::warn!(model = %measured.model_id, error = %error, "failed to record model cost");
                    }
                }
            }
        } else {
            total = None;
        }
        if ledger_ids.is_empty() {
            total = None;
        }
        let task_id = deepagent_persistence::run_store::RunStore::new(&self.db)
            .get(run_id)?
            .and_then(|run| run.task_id)
            .map(|id| id.parse::<deepagent_core::id::TaskId>())
            .transpose()?;
        let booked = !ledger_ids.is_empty();
        session.append(EventPayload::RunCostRecorded {
            version: 1,
            run_id: run_id.into(),
            task_id,
            ledger_session_id: session_id,
            ledger_ids,
            cost_yuan: total,
        })?;
        if booked {
            // Existing UI replaces the turn's cost and settles its live token
            // overlay on this event. Notify once, only AFTER all model rows.
            sink.emit(RuntimeEvent::CostRecorded {
                model: if models.len() == 1 {
                    models[0].model_id.clone()
                } else {
                    multi_model_label.into()
                },
                cost_yuan: total,
                origin: CostOrigin::Run,
            });
        }
        Ok(())
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
        // The pre-filter validates cursor provenance against history; an old
        // branch's token watermark cannot postpone extraction on a new branch.
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
    use deepagent_runtime::{CancellationTree, NullEventSink};
    use std::collections::HashMap;
    use std::sync::Mutex;

    #[test]
    fn finalizer_consolidates_its_bound_project_after_ui_activation_changes() {
        let tmp = tempfile::tempdir().unwrap();
        let alpha = tmp.path().join("alpha");
        let beta = tmp.path().join("beta");
        let global = tmp.path().join("global");
        let knowledge = Arc::new(KnowledgeService::open(&alpha, &global).unwrap());
        let seed = |title: &str| -> crate::KnowledgeDraftDto {
            serde_json::from_value(serde_json::json!({"title":title,
                "body":"project deployment evidence repeated verbatim in two notes",
                "scope":"project", "kind":"memory"}))
            .unwrap()
        };
        for title in ["Alpha one", "Alpha two"] {
            knowledge.save(seed(title)).unwrap();
        }
        let db = Arc::new(Database::open_in_memory().unwrap());
        let finalizer = AppRunFinalizer::new(
            db.clone(),
            None,
            Some(knowledge.clone()),
            Arc::new(Mutex::new(HashMap::new())),
            SessionMemoryPasses::default(),
        )
        .with_consolidation_gate(crate::memory_consolidation::ConsolidationGate {
            min_entries: 2,
            min_interval: std::time::Duration::ZERO,
            min_runs_between: 1,
        });
        knowledge.activate_project(&beta).unwrap();
        for title in ["Beta one", "Beta two"] {
            knowledge.save(seed(title)).unwrap();
        }
        let mut session = Session::create(&db, &SystemClock, Some("late finalize")).unwrap();
        let id = session.id().to_string();
        finalizer.consolidate_memories(&mut session, &id).unwrap();
        assert_eq!(
            KnowledgeService::open(&alpha, &global)
                .unwrap()
                .entry_count(),
            1
        );
        assert_eq!(
            KnowledgeService::open(&beta, &global)
                .unwrap()
                .entry_count(),
            2
        );
        assert_eq!(knowledge.entry_count(), 2);
    }

    #[test]
    fn clear_run_cancellation_removes_session_and_run_aliases() {
        let map: CancellationMap = Arc::new(Mutex::new(HashMap::new()));
        let cancellation = Arc::new(CancellationTree::new());
        map.lock()
            .unwrap()
            .insert("ses_test".to_string(), cancellation.clone());
        map.lock()
            .unwrap()
            .insert("run_test".to_string(), cancellation);

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
                    model_usage: None,
                    booked_calls: None,
                    multi_model_label: "multi-model",
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
                        Arc::new(crate::test_sequence::mock([
                            r#"{"type":"response.completed","response":{"id":"fixture-response","object":"response","created_at":1,"model":"deepseek-flash","output":[],"status":"completed"}}"#
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
                    model_usage: None,
                    booked_calls: None,
                    multi_model_label: "multi-model",
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
                    model_usage: None,
                    booked_calls: None,
                    multi_model_label: "multi-model",
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

    #[test]
    fn route_cost_preserves_side_model_and_unknown_aggregate() {
        use crate::cost_service::ModelPricing;
        use deepagent_persistence::cost_store::CostStore;
        use deepagent_persistence::event_store::EventStore;
        use deepagent_runtime::usage::ModelUsage;

        for (missing_price, missing_usage) in [(false, false), (true, false), (false, true)] {
            let db = Arc::new(Database::open_in_memory().unwrap());
            let cost = Arc::new(CostService::new(db.clone()));
            let mut pricing = HashMap::from([(
                "primary".into(),
                ModelPricing {
                    input_cache_hit_per_million: 1.0,
                    input_cache_miss_per_million: 1.0,
                    output_per_million: 1.0,
                },
            )]);
            if !missing_price {
                pricing.insert(
                    "fallback".into(),
                    ModelPricing {
                        input_cache_hit_per_million: 10.0,
                        input_cache_miss_per_million: 10.0,
                        output_per_million: 10.0,
                    },
                );
            }
            cost.set_pricing(pricing);
            let mut session = Session::create(&db, &SystemClock, Some("routes and side")).unwrap();
            let session_id = session.id().to_string();
            let usage = RunUsage::from(provider_call(1000, 100, 800, 200));
            let models: Vec<_> = ["primary", "fallback"]
                .into_iter()
                .map(|model| ModelUsage {
                    provider_id: None,
                    model_id: model.into(),
                    usage,
                    usage_complete: model != "fallback" || !missing_usage,
                })
                .collect();
            let finalizer = AppRunFinalizer::new(
                db.clone(),
                Some(cost),
                None,
                Arc::new(Mutex::new(HashMap::new())),
                SessionMemoryPasses::default(),
            );
            let empty = Arc::new(Mutex::new(HashSet::new()));
            let before = HashSet::new();
            let (sink, mut events) = deepagent_runtime::ChannelSink::new();
            finalizer
                .finalize_after_kernel(
                    &mut session,
                    AppRunFinalizerRequest {
                        session_id: &session_id,
                        run_id: "route-side-test",
                        discovered_before_run: &before,
                        discovered_tools: &empty,
                        usage: Some(run_of(&[provider_call(1000, 100, 800, 200); 2])),
                        model_usage: Some(&models),
                        booked_calls: None,
                        multi_model_label: "multi-model",
                        side_usage: Some(RunUsage::from(provider_call(900, 60, 700, 200))),
                        model_name: "primary",
                        sink: &sink,
                        run_succeeded: false,
                        capture_client: None,
                        capture_model: None,
                    },
                )
                .unwrap();
            let rows = CostStore::new(&db)
                .session_costs_detailed(&session_id)
                .unwrap();
            assert_eq!(rows.len(), 3);
            assert_eq!(rows[0].cost_yuan, Some(0.0011));
            assert_eq!(rows[1].cost_yuan, (!missing_price).then_some(0.011));
            assert_eq!(
                rows[2].cost_yuan,
                Some(0.00096),
                "side call retains primary pricing"
            );
            let costs: Vec<_> = std::iter::from_fn(|| events.try_recv().ok())
                .filter_map(|event| match event {
                    RuntimeEvent::CostRecorded {
                        model,
                        cost_yuan,
                        origin,
                    } => Some((model, cost_yuan, origin)),
                    _ => None,
                })
                .collect();
            assert_eq!(costs.len(), 2);
            assert_eq!(costs[0].0, "multi-model");
            assert_eq!(costs[0].2, CostOrigin::Run);
            let expected = (!(missing_price || missing_usage)).then_some(0.0121);
            match (costs[0].1, expected) {
                (Some(actual), Some(expected)) => assert!((actual - expected).abs() < 1e-12),
                (actual, expected) => assert_eq!(actual, expected),
            }
            assert_eq!(
                costs[1],
                ("primary".into(), Some(0.00096), CostOrigin::SideCall)
            );
            let receipts: Vec<_> = EventStore::new(&db)
                .load_session(session.id())
                .unwrap()
                .into_iter()
                .filter_map(|event| match event.payload {
                    EventPayload::RunCostRecorded {
                        ledger_ids,
                        cost_yuan,
                        ..
                    } => Some((ledger_ids, cost_yuan)),
                    _ => None,
                })
                .collect();
            assert_eq!(receipts.len(), 1);
            assert_eq!(receipts[0].0, vec![rows[0].id, rows[1].id]);
            assert_eq!(receipts[0].1, costs[0].1);
        }
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
