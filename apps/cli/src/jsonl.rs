use deepagent_harness_protocol::{
    project_runtime_event, EventContext, HarnessEvent, PROTOCOL_VERSION,
};
use deepagent_runtime::RuntimeEvent;

pub fn event_line(event: &HarnessEvent) -> Result<String, String> {
    event
        .to_wire_value()
        .and_then(|value| serde_json::to_string(&value))
        .map_err(|error| format!("serialize harness event: {error}"))
}

pub fn project_runtime_event_line(
    event: &RuntimeEvent,
    context: &EventContext,
) -> Result<Option<String>, String> {
    project_runtime_event(event, context)
        .as_ref()
        .map(event_line)
        .transpose()
}

pub fn error_line(code: impl Into<String>, message: impl Into<String>) -> String {
    event_line(&HarnessEvent::Error {
        code: code.into(),
        message: message.into(),
        data: Some(serde_json::json!({ "protocolVersion": PROTOCOL_VERSION })),
    })
    .expect("HarnessEvent serialization is infallible for JSON values")
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepagent_harness_protocol::{EventContext, HarnessEvent, ItemPayload};

    #[test]
    fn serializes_one_machine_event_as_one_json_line() {
        let event = HarnessEvent::ItemUpdated {
            thread_id: Some("thread-1".into()),
            turn_id: Some("turn-1".into()),
            item_id: None,
            item: ItemPayload::ContentDelta {
                text: "hello\nworld".into(),
            },
        };

        let line = event_line(&event).unwrap();
        assert!(!line.contains('\n'));
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&line).unwrap()["type"],
            "item.updated"
        );
    }

    #[test]
    fn projects_runtime_events_with_thread_and_turn_correlation() {
        let line = project_runtime_event_line(
            &deepagent_runtime::RuntimeEvent::ContentDelta {
                text: "hello".into(),
            },
            &EventContext::new(Some("thread-1".into()), Some("turn-1".into())),
        )
        .unwrap()
        .unwrap();

        let value: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(value["protocolVersion"], PROTOCOL_VERSION);
        assert_eq!(value["type"], "item.updated");
        assert_eq!(value["threadId"], "thread-1");
        assert_eq!(value["turnId"], "turn-1");
    }

    #[test]
    fn h84_jsonl_keeps_pre_gate_diagnostic_separate_from_actionable_approval() {
        let context = EventContext::new(Some("thread-1".into()), Some("turn-1".into()));
        let diagnostic = project_runtime_event_line(
            &RuntimeEvent::ToolBlocked {
                name: "delete_path".into(),
                reason: "requires approval".into(),
                needs_approval: true,
            },
            &context,
        )
        .unwrap()
        .unwrap();
        let approval = event_line(&HarnessEvent::ApprovalRequested {
            approval_id: Some("registered-gate-id".into()),
            thread_id: context.thread_id,
            turn_id: context.turn_id,
            tool_name: Some("delete_path".into()),
            reason: "requires approval".into(),
            scope: Some("tool".into()),
        })
        .unwrap();
        let lines: Vec<HarnessEvent> = format!("{diagnostic}\n{approval}\n")
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert!(matches!(&lines[0], HarnessEvent::ItemUpdated {
            item: ItemPayload::Runtime { event_type, data }, ..
        } if event_type == "tool_blocked" && data["needs_approval"] == true));
        assert!(matches!(&lines[1], HarnessEvent::ApprovalRequested {
            approval_id: Some(id), ..
        } if id == "registered-gate-id"));
    }
}
