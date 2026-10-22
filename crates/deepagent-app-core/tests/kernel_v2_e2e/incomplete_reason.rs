//! Provider incomplete reasons must retain their meaning through the real run lifecycle.

use super::*;
use deepagent_models::ModelRole;

struct IncompleteModel {
    reason: &'static str,
    requests: Mutex<Vec<serde_json::Value>>,
}

#[async_trait]
impl HttpTransport for IncompleteModel {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let attempt = {
            let mut requests = self.requests.lock().unwrap();
            requests.push(serde_json::from_str(&request.body).unwrap());
            requests.len()
        };
        // Two incomplete responses exercise escalation and continuation separately.
        // The final response closes the bounded fixture even when the client retries incorrectly.
        assert!(attempt <= 3, "unexpected extra model request");
        let incomplete = attempt < 3;
        let text = if incomplete {
            "AUDIT_PARTIAL"
        } else {
            "AUDIT_COMPLETE"
        };
        let id = format!("audit-response-{attempt}");
        let item_id = format!("audit-message-{attempt}");
        let status = if incomplete {
            "incomplete"
        } else {
            "completed"
        };
        let item = serde_json::json!({
            "type":"message", "id":item_id, "role":"assistant", "status":status,
            "content":[{"type":"output_text","text":text,"annotations":[]}]
        });
        let mut response = serde_json::json!({
            "id":id,"object":"response","status":status,"output":[item],
            "usage":{"input_tokens":12,"output_tokens":6,"total_tokens":18}
        });
        if incomplete {
            response["incomplete_details"] = serde_json::json!({"reason":self.reason});
        }
        let events = [
            serde_json::json!({"type":"response.created","sequence_number":0,
                "response":{"id":id,"status":"in_progress"}}),
            serde_json::json!({"type":"response.output_item.added","sequence_number":1,
                "output_index":0,"item":{"type":"message","id":item_id,
                    "role":"assistant","status":"in_progress","content":[]}}),
            serde_json::json!({"type":"response.output_text.delta","sequence_number":2,
                "item_id":item_id,"output_index":0,"content_index":0,"delta":text}),
            serde_json::json!({"type":"response.output_item.done","sequence_number":3,
                "output_index":0,"item":item}),
            serde_json::json!({"type":if incomplete {"response.incomplete"} else {"response.completed"},
                "sequence_number":4,"response":response}),
        ];
        emit_responses_events(
            sink,
            &events.iter().map(ToString::to_string).collect::<Vec<_>>(),
        )
    }
}

async fn observe_incomplete_reason(reason: &'static str) -> (bool, usize, String, bool) {
    let workspace = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open(workspace.path().join("state.sqlite")).unwrap());
    let settings = Arc::new(SettingsService::new(
        db.clone(),
        Arc::new(MockTransport::with_get_json(
            r#"{"data":[{"id":"deepseek-flash","object":"model","owned_by":"deepseek"}]}"#,
        )),
        Arc::new(MemorySecretStore::new()),
    ));
    settings
        .initialize("offline-audit-placeholder")
        .await
        .unwrap();
    for role in [ModelRole::Chat, ModelRole::Reasoner] {
        settings.set_model(role, "deepseek-flash").unwrap();
    }
    let model = Arc::new(IncompleteModel {
        reason,
        requests: Mutex::new(Vec::new()),
    });
    let chat = ChatService::new(db, settings, model.clone(), workspace.path());
    let result = chat
        .run_in_session(
            "Audit incomplete response classification.",
            None,
            None,
            None,
            Vec::new(),
            None,
            false,
            Some("audit-incomplete-reason"),
            |_| {},
            |_| {},
        )
        .await;
    let events = chat.run_events("audit-incomplete-reason", None).unwrap();
    let terminal = events
        .iter()
        .filter(|e| e.event_type == "run_terminal")
        .collect::<Vec<_>>();
    assert_eq!(terminal.len(), 1, "the run must terminate exactly once");
    let terminal_kind = terminal[0].data["kind"].as_str().unwrap().to_string();
    let requests = model.requests.lock().unwrap();
    let ceilings: Vec<_> = requests
        .iter()
        .map(|r| r["max_output_tokens"].clone())
        .collect();
    let continued_with_partial = requests.get(2).is_some_and(|r| {
        r["input"].as_array().unwrap().iter().any(|item| {
            item["role"] == "assistant" && item["content"].as_str() == Some("AUDIT_PARTIAL")
        })
    });
    let injected_output_limit_prompt = requests.get(2).is_some_and(|r| {
        r["input"].as_array().unwrap().iter().any(|item| {
            item["role"] == "user"
                && item["content"]
                    .as_str()
                    .is_some_and(|text| text.starts_with("Output token limit hit. Resume directly"))
        })
    });
    eprintln!("incomplete-reason audit: reason={reason} requests={} ceilings={ceilings:?} continued_with_partial={continued_with_partial} injected_output_limit_prompt={injected_output_limit_prompt} succeeded={} terminal={terminal_kind}", requests.len(), result.is_ok());
    if reason == "max_output_tokens" && requests.len() == 3 {
        assert!(
            continued_with_partial,
            "the actual third wire must contain the partial reply"
        );
        assert!(
            injected_output_limit_prompt,
            "the real continuation uses output-limit guidance"
        );
        assert!(
            ceilings[1].as_u64().unwrap() > ceilings[0].as_u64().unwrap(),
            "the second request really escalated its output ceiling"
        );
    }
    let escalated_output = ceilings.first().is_some_and(|first| {
        ceilings
            .iter()
            .skip(1)
            .any(|later| later.as_u64() > first.as_u64())
    });
    (
        result.is_ok(),
        requests.len(),
        terminal_kind,
        injected_output_limit_prompt || escalated_output,
    )
}

#[tokio::test]
async fn output_limit_can_recover_with_escalation_and_continuation() {
    let (succeeded, requests, terminal, used_length_recovery) =
        observe_incomplete_reason("max_output_tokens").await;
    assert!(succeeded);
    assert_eq!(requests, 3);
    assert_eq!(terminal, "succeeded");
    assert!(used_length_recovery);
}

#[tokio::test]
#[ignore = "audit: content_filter must not trigger max-output escalation or an automatic continue request"]
async fn audit_content_filter_does_not_retry_as_output_limit() {
    let (_, _, _, used_length_recovery) = observe_incomplete_reason("content_filter").await;
    assert!(
        !used_length_recovery,
        "content filtering must not use output-limit recovery"
    );
}
