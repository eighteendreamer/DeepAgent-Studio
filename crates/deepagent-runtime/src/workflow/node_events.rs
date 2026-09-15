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
        self.sink.emit(RuntimeEvent::WorkflowNode { event });
    }
}
