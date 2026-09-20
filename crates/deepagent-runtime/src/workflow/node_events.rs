use std::collections::BTreeMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::events::{NullEventSink, RuntimeEvent, RuntimeEventSink};
use crate::redaction::{scrub_secret_literals, scrub_secrets_value};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeExecutionStatus {
    Pending,
    Running,
    Completed,
    Skipped,
    Failed,
    Cancelled,
}

impl NodeExecutionStatus {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Skipped | Self::Failed | Self::Cancelled
        )
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeExecutionEvent {
    pub revision: String,
    pub node_id: String,
    pub status: NodeExecutionStatus,
    #[serde(default)]
    pub scope: Vec<String>,
    pub attempt: u32,
    pub elapsed_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outputs: Option<Value>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub updates: BTreeMap<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Routed operation, reported so a machine consumer does not have to read
    /// the UI output payload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
    /// `artifact://<id>` references produced by this node. Media bytes, file
    /// system paths and credentials never belong in an event.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<String>,
    /// Provider-side job id for asynchronous media work, so a stopped node can
    /// be resumed without guessing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_id: Option<String>,
}

/// An event reports which artifacts a node produced, never their bytes.
const MAX_EVENT_ARTIFACTS: usize = 8;
/// Output payloads are user-shaped; bound the walk that lifts provenance out.
const MAX_PROVENANCE_DEPTH: usize = 8;

fn artifact_uri(value: &Value, out: &mut Vec<String>, depth: usize) {
    if depth > MAX_PROVENANCE_DEPTH {
        return;
    }
    match value {
        Value::String(text) => {
            if text.starts_with("artifact://") {
                out.push(text.clone());
            }
        }
        Value::Array(items) => {
            for item in items {
                artifact_uri(item, out, depth + 1);
            }
        }
        Value::Object(map) => {
            for item in map.values() {
                artifact_uri(item, out, depth + 1);
            }
        }
        _ => {}
    }
}

/// The output payload a node returns stays the report of record for the UI;
/// these typed fields are a derived view of the same facts for machine
/// consumers, which is why the publisher computes them instead of each node.
fn annotate_provenance(event: &mut NodeExecutionEvent) {
    let Some(outputs) = event.outputs.as_ref() else {
        return;
    };
    let text = |key: &str| outputs.get(key).and_then(Value::as_str).map(str::to_string);
    event.operation = text("operation");
    event.provider_id = text("providerId");
    event.model_id = text("modelId");
    event.job_id = text("videoTaskId");
    let mut refs = Vec::new();
    artifact_uri(outputs, &mut refs, 0);
    // Object iteration order is not stable, and replayed events must be.
    refs.sort();
    refs.dedup();
    refs.truncate(MAX_EVENT_ARTIFACTS);
    event.artifacts = refs;
}

#[derive(Clone)]
pub struct NodeEventPublisher {
    sink: Arc<dyn RuntimeEventSink>,
}

impl Default for NodeEventPublisher {
    fn default() -> Self {
        Self {
            sink: Arc::new(NullEventSink),
        }
    }
}

impl NodeEventPublisher {
    pub fn new(sink: Arc<dyn RuntimeEventSink>) -> Self {
        Self { sink }
    }

    pub fn emit(&self, mut event: NodeExecutionEvent) {
        event.outputs = event.outputs.map(scrub_secrets_value);
        event.updates = event
            .updates
            .into_iter()
            .map(|(key, value)| (key, scrub_secrets_value(value)))
            .collect();
        event.error = event.error.map(|error| scrub_secret_literals(&error));
        annotate_provenance(&mut event);
        self.sink.emit(RuntimeEvent::WorkflowNode { event });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Mutex;

    #[derive(Default)]
    struct LastEvent {
        last: Arc<Mutex<Option<NodeExecutionEvent>>>,
    }

    impl RuntimeEventSink for LastEvent {
        fn emit(&self, event: RuntimeEvent) {
            if let RuntimeEvent::WorkflowNode { event } = event {
                *self.last.lock().unwrap() = Some(event);
            }
        }
    }

    fn publish(outputs: Option<Value>) -> NodeExecutionEvent {
        let sink = Arc::new(LastEvent::default());
        let captured = sink.last.clone();
        let publisher = NodeEventPublisher::new(sink);
        publisher.emit(NodeExecutionEvent {
            revision: "rev-1".to_string(),
            node_id: "video-1".to_string(),
            status: NodeExecutionStatus::Completed,
            scope: Vec::new(),
            attempt: 1,
            elapsed_ms: 12,
            outputs,
            updates: BTreeMap::new(),
            error: None,
            operation: None,
            provider_id: None,
            model_id: None,
            artifacts: Vec::new(),
            job_id: None,
        });
        let emitted = captured.lock().unwrap().clone();
        emitted.expect("event emitted")
    }

    #[test]
    fn completed_media_event_reports_provenance_and_artifact_refs() {
        let event = publish(Some(json!({
            "videoUrl": "artifact://art_77",
            "storyboards": ["artifact://art_78", { "nested": "artifact://art_79" }],
            "reference": "C:/Users/dev/Downloads/clip.mp4",
            "inline": "data:image/png;base64,AAAA",
            "mime": "video/mp4",
            "operation": "video_generate",
            "providerId": "cvp-1",
            "modelId": "Wan-AI/Wan2.2-T2V-A14B",
            "videoTaskId": "job-77",
        })));
        assert_eq!(event.operation.as_deref(), Some("video_generate"));
        assert_eq!(event.provider_id.as_deref(), Some("cvp-1"));
        assert_eq!(event.model_id.as_deref(), Some("Wan-AI/Wan2.2-T2V-A14B"));
        assert_eq!(event.job_id.as_deref(), Some("job-77"));
        assert_eq!(
            event.artifacts,
            vec![
                "artifact://art_77",
                "artifact://art_78",
                "artifact://art_79"
            ],
            "only artifact refs are reportable: no path, no inline bytes"
        );
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["artifacts"].as_array().map(Vec::len), Some(3));
        assert_eq!(json["model_id"], "Wan-AI/Wan2.2-T2V-A14B");
    }

    #[test]
    fn artifact_refs_are_deduplicated_and_bounded() {
        let many: Vec<String> = (0..(MAX_EVENT_ARTIFACTS + 6))
            .map(|index| format!("artifact://art_{index}"))
            .collect();
        let mut payload = serde_json::Map::new();
        for uri in &many {
            payload.insert(uri.clone(), json!(uri));
        }
        payload.insert("again".to_string(), json!(many[0]));
        let event = publish(Some(Value::Object(payload)));
        assert_eq!(event.artifacts.len(), MAX_EVENT_ARTIFACTS);
        assert_eq!(event.artifacts[0], many[0], "duplicates collapse");
    }

    #[test]
    fn events_without_outputs_carry_no_provenance() {
        let running = publish(None);
        assert!(running.artifacts.is_empty());
        assert_eq!(running.operation, None);
        assert_eq!(running.provider_id, None);
        assert_eq!(running.model_id, None);
        assert_eq!(running.job_id, None);
        let json = serde_json::to_value(&running).unwrap();
        for key in [
            "operation",
            "provider_id",
            "model_id",
            "artifacts",
            "job_id",
        ] {
            assert!(json.get(key).is_none(), "{key} must stay absent");
        }
    }

    #[test]
    fn pre_upgrade_event_body_still_deserializes() {
        let stored = json!({
            "revision": "rev-1",
            "node_id": "llm-1",
            "status": "completed",
            "scope": [],
            "attempt": 1,
            "elapsed_ms": 4,
            "outputs": { "text": "hello" }
        });
        let event: NodeExecutionEvent = serde_json::from_value(stored).expect("replayable");
        assert_eq!(event.node_id, "llm-1");
        assert!(event.artifacts.is_empty());
        assert_eq!(event.job_id, None);
    }
}
