//! H-104: a real hook reads the approved native view while it is executing.
use super::*;
use deepagent_core::event::Event;
use deepagent_persistence::event_store::EventStore;
use serde_json::Value;

fn capture(path: &std::path::Path) -> Value {
    let path = path.to_string_lossy().replace('\'', "''");
    serde_json::json!({"hooks":[{
        "type":"command", "shell":"powershell", "timeout":5,
        "command":format!(
            "$ErrorActionPreference = 'Stop'; $payload = [Console]::In.ReadToEnd(); $inputData = $payload | ConvertFrom-Json; \
             $snapshot = $null; $readOnly = $false; \
             if ($inputData.transcript_path -and [IO.File]::Exists($inputData.transcript_path)) {{ \
               $snapshot = [IO.File]::ReadAllText($inputData.transcript_path); \
               $readOnly = ([IO.File]::GetAttributes($inputData.transcript_path) -band [IO.FileAttributes]::ReadOnly) -ne 0 \
             }}; \
             $record = @{{payload=$inputData; snapshot=$snapshot; read_only=$readOnly; actual_cwd=(Get-Location).ProviderPath}} | ConvertTo-Json -Compress -Depth 30; \
             [IO.File]::AppendAllText('{path}', $record + [Environment]::NewLine)"
        )
    }]})
}

fn read(path: &std::path::Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn validate(record: &Value, session_id: &str, root: &std::path::Path) -> Vec<Event> {
    let payload = &record["payload"];
    assert_eq!(payload["session_id"], session_id);
    let cwd = payload["cwd"]
        .as_str()
        .expect("real workspace cwd is required");
    assert_eq!(
        std::fs::canonicalize(cwd).unwrap(),
        std::fs::canonicalize(root).unwrap()
    );
    assert_eq!(
        std::fs::canonicalize(record["actual_cwd"].as_str().unwrap()).unwrap(),
        std::fs::canonicalize(root).unwrap(),
        "declared cwd must agree with actual command execution"
    );
    let path = payload["transcript_path"]
        .as_str()
        .expect("readable file path is required");
    assert!(std::path::Path::new(path).is_absolute());
    assert!(
        !std::path::Path::new(path).exists(),
        "callback lease must be cleaned after dispatch"
    );
    assert_eq!(payload["transcript_format"], "deepagent.events.v1");
    assert_eq!(record["read_only"], true);
    let events: Vec<Event> = serde_json::from_str(
        record["snapshot"]
            .as_str()
            .expect("hook must read the file during execution"),
    )
    .unwrap();
    assert!(
        !events.is_empty(),
        "never substitute an empty/missing transcript"
    );
    assert!(events
        .iter()
        .all(|event| event.session_id.to_string() == session_id));
    assert!(events
        .windows(2)
        .all(|pair| pair[1].sequence == pair[0].sequence + 1));
    events
}

#[tokio::test]
async fn h104_real_hook_reads_native_snapshot_and_current_workspace_at_each_lifecycle() {
    let (db, settings, dir) = super::tests::seeded().await;
    let root = dir.path().join("workspace");
    std::fs::create_dir_all(&root).unwrap();
    let marker = dir.path().join("hook-base.jsonl");
    let group = capture(&marker);
    settings
        .set_hooks_json(
            &serde_json::json!({"hooks":{
                "SessionStart":[group.clone()], "Stop":[group.clone()], "SessionEnd":[group]
            }})
            .to_string(),
        )
        .unwrap();
    let chat = ChatService::new(db.clone(), settings, super::tests::chat_transport(), &root);
    let id = chat
        .run("H104 synthetic native transcript", |_| {}, |_| {})
        .await
        .unwrap();
    let before_end = EventStore::new(&db)
        .load_session(id.parse().unwrap())
        .unwrap();
    chat.end_session(&id).await.unwrap();
    let records = read(&marker);
    assert_eq!(records.len(), 3);
    let mut snapshots = Vec::new();
    for (record, name) in records.iter().zip(["SessionStart", "Stop", "SessionEnd"]) {
        assert_eq!(record["payload"]["hook_event_name"], name);
        snapshots.push(validate(record, &id, &root));
    }
    assert!(
        snapshots[1].len() > snapshots[0].len(),
        "Stop sees new durable history"
    );
    assert_eq!(&before_end[..snapshots[0].len()], snapshots[0].as_slice());
    assert_eq!(&before_end[..snapshots[1].len()], snapshots[1].as_slice());
    assert!(snapshots[2].iter().any(|event|
        matches!(&event.payload, EventPayload::SessionEnded { reason } if reason.as_deref() == Some("explicit_end"))));
}

#[tokio::test]
async fn h104_two_workspaces_and_session_bound_end_never_share_transcript_or_cwd() {
    let (db, settings, dir) = super::tests::seeded().await;
    let first = dir.path().join("first");
    let second = dir.path().join("second");
    for root in [&first, &second] {
        std::fs::create_dir_all(root).unwrap();
    }
    let marker = dir.path().join("two-workspaces.jsonl");
    let group = capture(&marker);
    settings
        .set_hooks_json(
            &serde_json::json!({"hooks":{
                "SessionStart":[group.clone()], "SessionEnd":[group]
            }})
            .to_string(),
        )
        .unwrap();
    let chat = ChatService::new(db, settings, super::tests::chat_transport(), &first);
    let other = chat.for_workspace(&second);
    let first_id = chat
        .run("first synthetic workspace", |_| {}, |_| {})
        .await
        .unwrap();
    let second_id = other
        .run("second synthetic workspace", |_| {}, |_| {})
        .await
        .unwrap();
    chat.end_session(&first_id).await.unwrap();
    // A different host default cannot replace the persisted session's project.
    chat.end_session(&second_id).await.unwrap();
    let records = read(&marker);
    assert_eq!(records.len(), 4);
    let mut paths = Vec::new();
    for (record, (id, root)) in records.iter().zip([
        (&first_id, &first),
        (&second_id, &second),
        (&first_id, &first),
        (&second_id, &second),
    ]) {
        validate(record, id, root);
        paths.push(record["payload"]["transcript_path"].as_str().unwrap());
    }
    assert_ne!(
        paths[0], paths[1],
        "different sessions cannot reuse the same snapshot"
    );
    assert_ne!(paths[2], paths[3]);
}

#[tokio::test]
async fn h104_settings_command_reads_an_isolated_native_view_without_a_model() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("settings-base.jsonl");
    let action = serde_json::from_value(capture(&path)["hooks"][0].clone()).unwrap();
    let result = crate::hook_runtime::test_hook_action("SessionStart", None, action, None, None)
        .await
        .unwrap();
    assert_eq!(result.outcome, "continued", "{}", result.detail);
    let records = read(&path);
    assert_eq!(records.len(), 1);
    let id = records[0]["payload"]["session_id"].as_str().unwrap();
    assert_ne!(id, deepagent_core::id::SessionId::nil().to_string());
    let events = validate(&records[0], id, &std::env::current_dir().unwrap());
    assert_eq!(
        events.len(),
        1,
        "settings tests expose only their synthetic history"
    );
    assert!(
        matches!(&events[0].payload, EventPayload::SessionStarted{title:Some(title), ..} if title == "Synthetic hook settings test")
    );
}

#[tokio::test]
async fn h103_settings_tool_hook_samples_include_identity_and_event_results() {
    let directory = tempfile::tempdir().unwrap();
    for (event, expected_result, expected_error) in [
        ("PreToolUse", false, false),
        ("PostToolUse", true, false),
        ("PostToolUseFailure", false, true),
    ] {
        let path = directory.path().join(format!("{event}.jsonl"));
        let action = serde_json::from_value(capture(&path)["hooks"][0].clone()).unwrap();
        let result = crate::hook_runtime::test_hook_action(event, None, action, None, None)
            .await
            .unwrap();
        assert_eq!(result.outcome, "continued", "{event}: {}", result.detail);
        let payload = &read(&path)[0]["payload"];
        assert_eq!(payload["hook_event_name"], event);
        assert_eq!(payload["tool_use_id"], "call_test");
        assert_eq!(payload.get("tool_response").is_some(), expected_result);
        assert_eq!(payload.get("error").is_some(), expected_error);
        if expected_result {
            assert_eq!(
                payload["tool_response"],
                serde_json::json!({"output":"synthetic hook test result"})
            );
        }
        if expected_error {
            assert_eq!(payload["error"], "synthetic hook failure");
        }
    }
}
