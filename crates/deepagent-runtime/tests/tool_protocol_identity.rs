//! H-156: programmatic extension -> registry/pipeline -> native history -> next request.
//! Synthetic SSE and a no-op tool only: no real patching, network or credentials.
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use deepagent_core::{clock::FixedClock, error::Result, event::EventPayload};
use deepagent_models::{
    transport::EventSink, HttpTransport, ModelClient, ModelConfig, ToolSchema, TransportRequest,
};
use deepagent_persistence::{event_store::EventStore, Database};
use deepagent_runtime::{ModelAgent, RuntimeConfig, RuntimeEngine};
use deepagent_session::Session;
use deepagent_tools::{PermissionSet, RiskLevel, Tool, ToolDescriptor, ToolOutput, ToolRegistry};
use deepagent_tracing::metrics::Metrics;
use serde_json::{json, Value};

struct SameNameExtension(Arc<Mutex<Vec<Value>>>);

#[async_trait]
impl Tool for SameNameExtension {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "apply_patch".into(),
            description: "synthetic no-op extension".into(),
            parameters: json!({"type":"object"}),
            risk: RiskLevel::Safe,
            required_permissions: PermissionSet::read_only(),
        }
    }

    async fn invoke(&self, args: Value) -> Result<ToolOutput> {
        self.0.lock().unwrap().push(args);
        Ok(ToolOutput::success(json!({"synthetic":8})))
    }
}

struct ObservedTransport {
    replay: Mutex<VecDeque<Vec<String>>>,
    requests: Mutex<Vec<Value>>,
}

#[async_trait]
impl HttpTransport for ObservedTransport {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        self.requests
            .lock()
            .unwrap()
            .push(serde_json::from_str(&request.body)?);
        let events = self
            .replay
            .lock()
            .unwrap()
            .pop_front()
            .expect("one synthetic response per request");
        for event in events {
            if sink.on_event(&event)? {
                break;
            }
        }
        Ok(())
    }
}

fn completed(id: &str, output: Value) -> Vec<String> {
    vec![
        json!({"type":"response.completed","sequence_number":0,"response":{
            "id":id,"object":"response","created_at":1,"model":"deepseek-flash",
            "status":"completed","output":output
        }})
        .to_string(),
    ]
}

#[tokio::test]
async fn h156_same_name_function_and_explicit_custom_execute_once_and_recover_native_pairs() {
    for custom in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("native.sqlite");
        let clock = FixedClock::new(1);
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut registry = ToolRegistry::new();
        let extension = SameNameExtension(calls.clone());
        let descriptor = extension.descriptor();
        registry.register(Arc::new(extension)).unwrap();
        let mut schema = ToolSchema::function(
            descriptor.name,
            descriptor.description,
            descriptor.parameters,
        );
        if custom {
            schema.kind = "custom".into();
        }
        let call = if custom {
            json!({"type":"custom_tool_call","id":"item-call","call_id":"same-name","name":"apply_patch","input":"synthetic patch"})
        } else {
            json!({"type":"function_call","id":"item-call","call_id":"same-name","name":"apply_patch","arguments":"{\"value\":7}"})
        };
        let transport = Arc::new(ObservedTransport {
            replay: Mutex::new(VecDeque::from([
                completed("first", json!([call])),
                completed(
                    "second",
                    json!([{"type":"message","id":"item-text","role":"assistant","content":[{"type":"output_text","text":"done","annotations":[]}]}]),
                ),
                completed(
                    "resumed",
                    json!([{"type":"message","id":"item-next","role":"assistant","content":[{"type":"output_text","text":"resumed","annotations":[]}]}]),
                ),
            ])),
            requests: Mutex::new(Vec::new()),
        });
        let client = Arc::new(ModelClient::new(
            transport.clone(),
            ModelConfig::deepseek("synthetic"),
        ));
        let id = {
            let db = Database::open(&path).unwrap();
            let mut session = Session::create(&db, &clock, None).unwrap();
            let task = session.create_task("synthetic").unwrap();
            let mut agent = ModelAgent::new(
                client.clone(),
                "deepseek-flash",
                "fixture",
                "synthetic",
                vec![schema.clone()],
            );
            RuntimeEngine::new(&registry, Metrics::new(), RuntimeConfig::default())
                .run(&mut session, task, &mut agent)
                .await
                .unwrap();
            assert_eq!(
                *calls.lock().unwrap(),
                vec![if custom {
                    json!({"patch":"synthetic patch"})
                } else {
                    json!({"value":7})
                }]
            );
            session.id()
        };
        let db = Database::open(&path).unwrap();
        Session::recover(&db, &clock, id).unwrap();
        let events = EventStore::new(&db).load_session(id).unwrap();
        let items: Vec<_> = events
            .iter()
            .filter_map(|event| match &event.payload {
                EventPayload::ResponseItemAppended { item } => Some(item.clone()),
                _ => None,
            })
            .collect();
        let call_items: Vec<_> = items
            .iter()
            .filter(|item| item.client_tool_call_id() == Some("same-name"))
            .collect();
        let output_items: Vec<_> = items
            .iter()
            .filter(|item| item.client_tool_output_call_id() == Some("same-name"))
            .collect();
        assert_eq!(call_items.len(), 1);
        assert_eq!(output_items.len(), 1);
        assert_eq!(
            matches!(
                call_items[0],
                deepagent_core::response_item::ResponseItem::CustomToolCall { .. }
            ),
            custom
        );
        assert_eq!(
            matches!(
                output_items[0],
                deepagent_core::response_item::ResponseItem::CustomToolCallOutput { .. }
            ),
            custom
        );
        let mut resumed = ModelAgent::new(
            client,
            "deepseek-flash",
            "fixture",
            "continue",
            vec![schema],
        )
        .with_response_history(items);
        use deepagent_runtime::Agent;
        resumed.think(0, &[]).await.unwrap();
        let requests = transport.requests.lock().unwrap();
        assert_eq!(requests.len(), 3);
        for request in &requests[1..] {
            let input = request["input"].as_array().unwrap();
            let call = input
                .iter()
                .find(|item| item["call_id"] == "same-name" && item.get("name").is_some())
                .unwrap();
            assert_eq!(
                call["type"],
                if custom {
                    "custom_tool_call"
                } else {
                    "function_call"
                }
            );
            let result = input
                .iter()
                .find(|item| item["call_id"] == "same-name" && item.get("output").is_some())
                .unwrap();
            assert_eq!(
                result["type"],
                if custom {
                    "custom_tool_call_output"
                } else {
                    "function_call_output"
                }
            );
            assert!(result["output"].as_str().unwrap().contains("synthetic"));
            assert_eq!(
                request["tools"][0]["type"],
                if custom { "custom" } else { "function" }
            );
        }
        assert_eq!(
            calls.lock().unwrap().len(),
            1,
            "history recovery must not redispatch the tool"
        );
    }
}
