use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use deepagent_core::clock::Clock;
use deepagent_core::error::Result;
use deepagent_core::event::EventPayload;
use deepagent_models::ModelClient;
use deepagent_persistence::Database;
use deepagent_runtime::{agent::RunUsage, RuntimeEvent, RuntimeEventSink};
use deepagent_session::Session;

use crate::cost_service::{CostRecordRequest, CostService};
use crate::knowledge_service::KnowledgeService;
use crate::tool_manifest::DiscoveredToolSet;

pub(crate) type CancellationMap = Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>;

#[derive(Clone)]
pub(crate) struct AppRunFinalizer {
    db: Arc<Database>,
    cost: Option<Arc<CostService>>,
    knowledge: Option<Arc<KnowledgeService>>,
    cancellations: CancellationMap,
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
    pub(crate) capture_client: Arc<ModelClient>,
    pub(crate) capture_model: String,
}

impl AppRunFinalizer {
    pub(crate) fn new(
        db: Arc<Database>,
        cost: Option<Arc<CostService>>,
        knowledge: Option<Arc<KnowledgeService>>,
        cancellations: CancellationMap,
    ) -> Self {
        Self {
            db,
            cost,
            knowledge,
            cancellations,
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
        self.record_run_cost(
            request.session_id,
            request.model_name,
            request.usage,
            request.sink,
        );
        self.clear_run_cancellation(request.session_id, request.run_id);
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
        match cost.record(CostRecordRequest {
            session_id: session_id.to_string(),
            model: model_name.to_string(),
            input_tokens: u.prompt_tokens,
            output_tokens: u.completion_tokens,
            cache_hit_tokens: u.prompt_cache_hit_tokens,
            cache_miss_tokens: u.prompt_cache_miss_tokens,
            total_tokens: u.total_tokens,
        }) {
            Ok(cny) => {
                tracing::info!(cost_yuan = cny, "recorded run cost");
                sink.emit(RuntimeEvent::Usage {
                    prompt_tokens: 0,
                    completion_tokens: 0,
                    reasoning_tokens: 0,
                    total_tokens: 0,
                    prompt_cache_hit_tokens: 0,
                    prompt_cache_miss_tokens: 0,
                    cost_yuan: Some(cny),
                });
            }
            Err(error) => tracing::warn!(error = %error, "failed to record run cost"),
        }
    }

    fn clear_run_cancellation(&self, session_id: &str, run_id: &str) {
        let mut map = self.cancellations.lock().unwrap_or_else(|p| p.into_inner());
        map.remove(session_id);
        map.remove(run_id);
    }

    fn spawn_auto_capture(
        &self,
        run_succeeded: bool,
        session_id: &str,
        capture_client: Arc<ModelClient>,
        capture_model: String,
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
                .capture_from_session(capture_client, capture_model, &events, &sid)
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
        let finalizer =
            AppRunFinalizer::new(db.clone(), None, None, Arc::new(Mutex::new(HashMap::new())));

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
                    capture_client: Arc::new(ModelClient::new(
                        Arc::new(deepagent_models::MockTransport::new([
                            r#"{"type":"response.completed","response":{"status":"completed"}}"#
                                .to_string(),
                        ])),
                        deepagent_models::ModelConfig::deepseek("test"),
                    )),
                    capture_model: "model".to_string(),
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
}
