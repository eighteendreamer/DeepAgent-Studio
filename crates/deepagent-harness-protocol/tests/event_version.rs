use deepagent_harness_protocol::{
    project_runtime_event, EventContext, HarnessEvent, PROTOCOL_VERSION,
};
use deepagent_runtime::RuntimeEvent;

#[test]
fn projected_events_all_have_the_same_wire_version_and_legacy_dtos_stay_readable() {
    let context = EventContext::new(Some("thread".into()), Some("run".into()));
    let events = [
        RuntimeEvent::ContentDelta {
            text: "hello".into(),
        },
        RuntimeEvent::RunFailed {
            reason: "synthetic failure".into(),
        },
        RuntimeEvent::RunCancelled {
            edited_files: vec![],
        },
    ];
    for runtime in events {
        let event = project_runtime_event(&runtime, &context).unwrap();
        let wire = event.to_wire_value().unwrap();
        assert_eq!(wire["protocolVersion"], PROTOCOL_VERSION);
        assert_eq!(serde_json::from_value::<HarnessEvent>(wire).unwrap(), event);
        let legacy = serde_json::to_value(&event).unwrap();
        assert_eq!(
            serde_json::from_value::<HarnessEvent>(legacy).unwrap(),
            event
        );
    }
}
