use super::*;
use crate::workflow::{KnowledgeDocument, KnowledgeQuery, KnowledgeScope, KnowledgeSearch};
use std::sync::Mutex;

#[derive(Default)]
struct Probe {
    calls: Mutex<Vec<(String, usize, KnowledgeScope)>>,
    fail: bool,
}

#[async_trait]
impl KnowledgeRetriever for Probe {
    async fn search(&self, query: KnowledgeQuery<'_>) -> Result<KnowledgeSearch> {
        self.calls
            .lock()
            .unwrap()
            .push((query.query.into(), query.limit, query.scope));
        if self.fail {
            return Err(CoreError::other("synthetic knowledge backend failure"));
        }
        Ok(KnowledgeSearch {
            revision: "frozen-revision".into(),
            documents: [0.9, 0.1]
                .into_iter()
                .map(|score| KnowledgeDocument {
                    id: "project:synthetic".into(),
                    title: "Synthetic".into(),
                    content: "synthetic excerpt".into(),
                    score,
                    scope: KnowledgeScope::Project,
                    updated_at: 123,
                })
                .collect(),
        })
    }
}

fn agent(probe: Arc<Probe>) -> WorkflowAgent {
    let definition = serde_json::from_value(json!({"version":1,"nodes":[
        {"id":"start","kind":"start"},
        {"id":"retrieve","kind":"knowledge-retrieval","config":{"knowledgeBaseId":"project","knowledgeTopK":999,"queryVariable":"needle","scoreThreshold":0.5}},
        {"id":"end","kind":"end"}
    ],"edges":[{"id":"a","source":"start","target":"retrieve"},{"id":"b","source":"retrieve","target":"end"}]})).unwrap();
    WorkflowAgent::new(
        crate::workflow::compile(definition).unwrap(),
        Map::new(),
        None,
        NodeEventPublisher::default(),
    )
    .unwrap()
    .with_knowledge_retriever(probe)
}

#[tokio::test]
async fn h176_typed_request_keeps_limit_threshold_and_source_metadata() {
    let probe = Arc::new(Probe::default());
    let mut agent = agent(probe.clone());
    agent.think(0, &[]).await.unwrap();
    agent.think(1, &[]).await.unwrap();
    assert_eq!(
        *probe.calls.lock().unwrap(),
        vec![("needle".into(), 20, KnowledgeScope::Project)]
    );
    let output = &agent.outputs["retrieve"];
    assert_eq!(output["documents"].as_array().unwrap().len(), 1);
    assert_eq!(output["documents"][0]["scope"], "project");
    assert_eq!(output["documents"][0]["updatedAt"], 123);
    assert_eq!(output["revision"], "frozen-revision");
    assert!(output["content"]
        .as_str()
        .unwrap()
        .contains("synthetic excerpt"));
}

#[tokio::test]
async fn h176_backend_error_fails_node_without_success_shaped_output() {
    let probe = Arc::new(Probe {
        fail: true,
        ..Default::default()
    });
    let mut agent = agent(probe.clone());
    agent.think(0, &[]).await.unwrap();
    let error = agent.think(1, &[]).await.unwrap_err();
    assert!(error
        .to_string()
        .contains("synthetic knowledge backend failure"));
    assert!(!agent.outputs.contains_key("retrieve"));
    assert_eq!(probe.calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn h176_cancellation_before_retrieval_never_dispatches_backend() {
    let probe = Arc::new(Probe::default());
    let cancel = Arc::new(AtomicBool::new(false));
    let mut agent = agent(probe.clone()).with_cancel(cancel.clone());
    agent.think(0, &[]).await.unwrap();
    cancel.store(true, Ordering::SeqCst);
    assert!(agent.think(1, &[]).await.is_err());
    assert!(probe.calls.lock().unwrap().is_empty());
    assert!(!agent.outputs.contains_key("retrieve"));
}
