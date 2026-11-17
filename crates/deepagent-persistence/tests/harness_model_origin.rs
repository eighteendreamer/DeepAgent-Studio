use deepagent_core::clock::Timestamp;
use deepagent_core::event::EventPayload;
use deepagent_core::id::SessionId;
use deepagent_persistence::event_store::EventStore;
use deepagent_persistence::Database;
use serde_json::json;

#[test]
fn legacy_and_cross_provider_items_replay_from_the_same_compressed_session_file() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("state.sqlite");
    let session = SessionId::new();
    let records = vec![
        json!({"type":"response_item_appended","item":{
            "type":"reasoning","content":"legacy thought"}}),
        json!({"type":"response_item_appended","item":{
            "type":"reasoning","content":"alpha thought","signature":"synthetic-alpha-signature"},
            "origin":{"provider_id":"alpha","model_name":"model-a","protocol":"messages"}}),
        json!({"type":"response_item_appended","item":{
            "type":"message","role":"assistant","content":"alpha answer"},
            "origin":{"provider_id":"alpha","model_name":"model-a","protocol":"messages"}}),
        json!({"type":"response_item_appended","item":{
            "type":"message","role":"user","content":"next user turn"}}),
        json!({"type":"response_item_appended","item":{
            "type":"reasoning","id":"beta-reasoning","content":"beta thought"},
            "origin":{"provider_id":"beta","model_name":"model-b","protocol":"responses"}}),
        json!({"type":"response_item_appended","item":{
            "type":"message","role":"assistant","content":"beta answer"},
            "origin":{"provider_id":"beta","model_name":"model-b","protocol":"responses"}}),
    ];
    {
        let db = Database::open(&path).unwrap();
        let store = EventStore::new(&db);
        store
            .create_session(session, None, Timestamp::from_millis(1))
            .unwrap();
        for record in &records {
            let payload: EventPayload = serde_json::from_value(record.clone()).unwrap();
            store
                .append(session, payload, Timestamp::from_millis(2))
                .unwrap();
        }
        let files: Vec<_> = std::fs::read_dir(db.session_transcript_dir(session))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .ends_with(".v1.jsonl.zst")
            })
            .collect();
        assert_eq!(
            files.len(),
            1,
            "both providers use one existing session format"
        );
        assert!(std::fs::metadata(&files[0]).unwrap().len() > 0);
    }
    let cold = Database::open(&path).unwrap();
    let events = EventStore::new(&cold).load_session(session).unwrap();
    assert_eq!(events.len(), records.len());
    for (index, (event, record)) in events.iter().zip(&records).enumerate() {
        assert_eq!(event.sequence, index as u64);
        assert_eq!(serde_json::to_value(&event.payload).unwrap(), *record);
    }
}
