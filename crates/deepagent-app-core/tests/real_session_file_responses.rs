#![cfg(all(feature = "keychain", feature = "web"))]

//! Opt-in live DeepSeek validation for the complete conversation path.
//!
//! This is intentionally ignored in normal CI: it spends real API quota and
//! reads the developer key from the environment or the desktop keychain. The
//! assertions are on durable runtime facts (tool events, replay, file-backed
//! storage, FTS projection, and diagnostics shape), not on exact model prose.

use std::path::Path;
use std::str::FromStr;
use std::sync::{Arc, Mutex};

use deepagent_app_core::secret_store::KeychainStore;
use deepagent_app_core::{
    ApprovalPolicy, ChatService, MemorySecretStore, SecretStore, SettingsService, SkillsService,
};
use deepagent_core::event::EventPayload;
use deepagent_core::id::SessionId;
use deepagent_models::{ReqwestTransport, ThinkingDepth};
use deepagent_persistence::event_store::EventStore;
use deepagent_persistence::run_store::RunStore;
use deepagent_persistence::runtime_log_store::RuntimeLogStore;
use deepagent_persistence::Database;
use deepagent_skills::SkillsRoots;

fn deepseek_key() -> Option<String> {
    std::env::var("DEEPSEEK_API_KEY")
        .ok()
        .filter(|key| !key.trim().is_empty())
        .or_else(|| {
            KeychainStore::new("deepagent-studio")
                .get("deepseek_api_key")
                .ok()
                .flatten()
        })
}

fn collect_session_files(root: &Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            collect_session_files(&path, out);
        } else if path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().ends_with(".jsonl.zst"))
        {
            out.push(path);
        }
    }
}

#[tokio::test]
#[ignore = "hits the real DeepSeek API; run explicitly with --ignored"]
async fn real_responses_session_replays_tools_skills_files_and_logs() {
    let Some(key) = deepseek_key() else {
        eprintln!("[skip] no DeepSeek key in env or keychain");
        return;
    };
    eprintln!("[real-session] key resolved (len={})", key.len());

    let temp = tempfile::tempdir().expect("temporary test root");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(
        workspace.join("fixture.txt"),
        "project=DeepAgent Studio\nprotocol=Responses\nmarker=DEEPAGENT_FILE_SESSION_90210\n",
    )
    .unwrap();

    let db_path = temp.path().join("state.sqlite3");
    let db = Arc::new(Database::open(&db_path).unwrap());
    let transport = Arc::new(ReqwestTransport::new());
    let secrets = Arc::new(MemorySecretStore::new());
    let settings = Arc::new(SettingsService::new(db.clone(), transport.clone(), secrets));
    settings
        .initialize(&key)
        .await
        .expect("live model discovery");
    settings
        .set_thinking_depth(ThinkingDepth::Simple)
        .expect("set deterministic low-cost thinking profile");
    settings
        .set_approval_policy(ApprovalPolicy::AutoReview)
        .expect("allow the live fixture to finish without UI approval");

    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let skills = SkillsService::open_v2(SkillsRoots {
        builtin: repo_root.join(".deepagent/skills"),
        user: temp.path().join("skills/user"),
        marketplace: temp.path().join("skills/marketplace"),
        workspace: Some(workspace.join(".deepagent/skills")),
    })
    .expect("load shipped skills");
    assert!(
        skills
            .manager()
            .registry()
            .get("deepagent-code-review")
            .is_some(),
        "the real shipped review skill must be discoverable"
    );

    let runtime_logs = Arc::new(
        RuntimeLogStore::open(temp.path().join("runtime-logs.sqlite3")).expect("runtime log store"),
    );
    let chat = ChatService::new(db.clone(), settings, transport, &workspace)
        .with_skills(Arc::new(Mutex::new(skills)))
        .with_runtime_logs(runtime_logs.clone());

    let streamed = Arc::new(Mutex::new(Vec::<String>::new()));
    let streamed_first = streamed.clone();
    let session_id = chat
        .run(
            "执行这个只读多步骤任务，必须按顺序完成且不能省略工具调用：\n\
             1. 调用 skill 工具加载 deepagent-code-review；\n\
             2. 调用 read_file 读取 workspace 根目录的 fixture.txt；\n\
             3. 用三条简短事实总结文件，并原样包含 marker 值。\n\
             不要修改任何文件，也不要调用 shell/bash。",
            move |event| {
                streamed_first
                    .lock()
                    .unwrap()
                    .push(event.label().to_string())
            },
            |approval| {
                panic!("read-only live fixture unexpectedly requested approval: {approval:?}")
            },
        )
        .await
        .expect("first live agent turn");
    eprintln!("[real-session] first turn session={session_id}");

    let streamed_second = streamed.clone();
    let continued_id = chat
        .run_in_session(
            "继续上一轮，不再读取文件。仅列出上一轮实际使用的工具名，并复述 marker 值。",
            Some(&session_id),
            None,
            None,
            Vec::new(),
            None,
            false,
            None,
            move |event| {
                streamed_second
                    .lock()
                    .unwrap()
                    .push(event.label().to_string())
            },
            |approval| panic!("continuation unexpectedly requested approval: {approval:?}"),
        )
        .await
        .expect("continued live agent turn");
    assert_eq!(
        continued_id, session_id,
        "continuation must reuse the session"
    );

    let streamed_third = streamed.clone();
    let searched_id = chat
        .run_in_session(
            "继续当前会话。必须调用 session_search 工具搜索精确文本 \
             DEEPAGENT_FILE_SESSION_90210，然后只回答搜索结果中是否包含当前会话。",
            Some(&session_id),
            None,
            None,
            Vec::new(),
            None,
            false,
            None,
            move |event| {
                streamed_third
                    .lock()
                    .unwrap()
                    .push(event.label().to_string())
            },
            |approval| panic!("session search unexpectedly requested approval: {approval:?}"),
        )
        .await
        .expect("session-search live agent turn");
    assert_eq!(
        searched_id, session_id,
        "search turn must reuse the session"
    );

    let session = SessionId::from_str(&session_id).unwrap();
    let store = EventStore::new(&db);
    let events = store.load_session(session).expect("replay file source");
    let usage: Vec<_> = events
        .iter()
        .filter_map(|event| match &event.payload {
            EventPayload::UsageRecorded {
                prompt_tokens,
                prompt_cache_hit_tokens,
                prompt_cache_miss_tokens,
                raw_responses_usage,
                ..
            } => Some((
                *prompt_tokens,
                *prompt_cache_hit_tokens,
                *prompt_cache_miss_tokens,
                raw_responses_usage.is_some(),
            )),
            _ => None,
        })
        .collect();
    assert_eq!(usage.len(), 3, "each live turn must persist provider usage");
    assert!(usage.iter().all(|entry| entry.3));
    assert!(
        usage.iter().all(|entry| entry.1 + entry.2 == entry.0),
        "Responses cached and uncached input tokens must account for every input token"
    );
    eprintln!("[real-session] provider usage (prompt, hit, miss, raw)={usage:?}");
    let requested_tools: Vec<&str> = events
        .iter()
        .filter_map(|event| match &event.payload {
            EventPayload::ToolCallRequested { call } => Some(call.name.as_str()),
            _ => None,
        })
        .collect();
    eprintln!("[real-session] requested tools={requested_tools:?}");
    assert!(
        requested_tools.contains(&"skill"),
        "skill tool was not invoked"
    );
    assert!(
        requested_tools.contains(&"read_file"),
        "read_file tool was not invoked"
    );
    assert!(
        requested_tools.contains(&"session_search"),
        "session_search tool was not invoked"
    );
    assert!(events.iter().any(|event| matches!(
        &event.payload,
        EventPayload::MessageAppended { message }
            if message.content.contains("DEEPAGENT_FILE_SESSION_90210")
    )));

    let sqlite_body_rows: i64 = db
        .with_conn(|conn| {
            conn.query_row("SELECT COUNT(*) FROM events", [], |row| row.get(0))
                .map_err(|error| deepagent_core::error::CoreError::Persistence(error.to_string()))
        })
        .unwrap();
    assert_eq!(
        sqlite_body_rows, 0,
        "event bodies must not return to SQLite"
    );

    let mut session_files = Vec::new();
    collect_session_files(&temp.path().join("files/sessions"), &mut session_files);
    assert_eq!(
        session_files.len(),
        1,
        "one session must map to one active file"
    );
    eprintln!(
        "[real-session] source={} events={} bytes={}",
        session_files[0].display(),
        events.len(),
        std::fs::metadata(&session_files[0]).unwrap().len()
    );
    let legacy_payload_bytes: usize = events
        .iter()
        .map(|event| serde_json::to_vec(&event.payload).unwrap().len())
        .sum();
    let (run_rows, run_data_bytes): (i64, i64) = db
        .with_conn(|conn| {
            conn.query_row(
                "SELECT COUNT(*), COALESCE(SUM(length(data)), 0) FROM run_events",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|error| deepagent_core::error::CoreError::Persistence(error.to_string()))
        })
        .unwrap();
    let main_wal_bytes = std::fs::metadata(temp.path().join("state.sqlite3-wal"))
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    eprintln!(
        "[real-session] storage legacy_payload_estimate={legacy_payload_bytes} run_rows={run_rows} run_data_bytes={run_data_bytes} main_wal_bytes={main_wal_bytes}"
    );

    let hits = store
        .search("DEEPAGENT_FILE_SESSION_90210", None, 10)
        .expect("search projection");
    assert!(hits.iter().any(|hit| hit.session.id == session));

    let logs = runtime_logs
        .recent_for_session(&session_id, 1000)
        .expect("runtime diagnostics");
    assert!(!logs.is_empty(), "the full run must emit diagnostics");
    let runtime_data_bytes: usize = logs
        .iter()
        .map(|entry| serde_json::to_vec(&entry.data).unwrap().len())
        .sum();
    let runtime_wal_bytes = std::fs::metadata(temp.path().join("runtime-logs.sqlite3-wal"))
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    eprintln!(
        "[real-session] storage runtime_rows={} runtime_data_bytes={runtime_data_bytes} runtime_wal_bytes={runtime_wal_bytes}",
        logs.len()
    );
    assert!(
        logs.iter().all(|entry| entry.event != "content_delta"),
        "raw content-delta rows must not be persisted"
    );
    let delta_batches: Vec<_> = logs
        .iter()
        .filter(|entry| entry.event.ends_with("_delta_batch"))
        .collect();
    assert!(
        !delta_batches.is_empty(),
        "streamed text must emit batch metrics"
    );
    assert!(
        delta_batches.iter().all(|entry| {
            entry
                .data
                .get("bytes")
                .and_then(|value| value.as_u64())
                .is_some()
                && entry
                    .data
                    .get("chunks")
                    .and_then(|value| value.as_u64())
                    .is_some()
                && entry.data.get("text").is_none()
                && entry.data.get("content").is_none()
                && !entry
                    .data
                    .to_string()
                    .contains("DEEPAGENT_FILE_SESSION_90210")
        }),
        "delta diagnostic rows must store metrics, never streamed text"
    );
    let mut runtime_counts = std::collections::BTreeMap::<&str, usize>::new();
    for entry in &logs {
        *runtime_counts.entry(entry.event.as_str()).or_default() += 1;
    }
    assert!(!logs.iter().any(|entry| {
        entry.event == "responses_stream_event"
            && entry
                .data
                .get("delta_chars")
                .is_some_and(|value| !value.is_null())
    }));
    eprintln!("[real-session] runtime event counts={runtime_counts:?}");

    let run_store = RunStore::new(&db);
    let mut run_ids: Vec<_> = logs
        .iter()
        .filter_map(|entry| entry.run_id.clone())
        .collect();
    run_ids.sort();
    run_ids.dedup();
    assert_eq!(
        run_ids.len(),
        3,
        "three chat turns must create three durable runs"
    );
    for run_id in run_ids {
        let persisted = run_store.events_after(&run_id, None).unwrap();
        let delta_rows = persisted
            .iter()
            .filter(|event| event.event_type.ends_with("_delta_batch"))
            .count();
        assert!(!persisted.iter().any(|event| {
            event.event_type == "responses_stream_event"
                && event
                    .data
                    .get("delta_chars")
                    .is_some_and(|value| !value.is_null())
        }));
        eprintln!(
            "[real-session] run={run_id} persisted_rows={} delta_batches={delta_rows}",
            persisted.len()
        );
    }

    let labels = streamed.lock().unwrap();
    assert_eq!(
        labels
            .iter()
            .filter(|label| label.as_str() == "run_completed")
            .count(),
        3,
        "all three turns must reach a terminal completed event"
    );
}
