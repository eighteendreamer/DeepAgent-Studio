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
use deepagent_persistence::event_store::{EventStore, SessionSearchScope};
use deepagent_persistence::run_store::RunStore;
use deepagent_persistence::runtime_log_store::RuntimeLogStore;
use deepagent_persistence::Database;
use deepagent_skills::SkillsRoots;

fn deepseek_key() -> Option<String> {
    std::env::var("DEEPSEEK_API_KEY")
        .ok()
        .filter(|key| !key.trim().is_empty())
        .or_else(|| {
            KeychainStore::new(deepagent_app_core::KEYCHAIN_SERVICE)
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

/// Records every request body so the run can be inspected for prompt-cache
/// stability: DeepSeek caches implicitly on the whole prefix, so the first
/// differing offset between two consecutive turns is exactly the depth at which
/// the cached prefix stopped paying for this session.
struct RecordingTransport {
    inner: Arc<ReqwestTransport>,
    bodies: Arc<Mutex<Vec<String>>>,
}

#[async_trait::async_trait]
impl deepagent_models::transport::HttpTransport for RecordingTransport {
    async fn stream(
        &self,
        request: deepagent_models::transport::TransportRequest,
        sink: &mut dyn deepagent_models::transport::EventSink,
    ) -> deepagent_core::error::Result<()> {
        self.bodies.lock().unwrap().push(request.body.clone());
        self.inner.stream(request, sink).await
    }

    async fn get_json(&self, url: &str, api_key: &str) -> deepagent_core::error::Result<String> {
        self.inner.get_json(url, api_key).await
    }
}

/// The cache-shape summary for consecutive recorded requests.
///
/// Measured live (five runs, same fixture): turns 1 and 3 reuse 98% and 95% of
/// their input, while the first history-bearing turn reports exactly 2816 cached
/// tokens every time. This probe is what shows the cause is NOT our request
/// shape: between turn 1 and turn 2 the tools list is identical (34 names, same
/// order), every earlier history item re-serializes unchanged, and the two
/// bodies share a 16874-character prefix. Draining the background note pass and
/// waiting eight quiet seconds before turn 2 changed nothing (still 2816), so
/// concurrent provider traffic is ruled out too. What remains is DeepSeek's own
/// accounting for the first request that extends a cached entry.
fn cache_divergence(bodies: &[String]) -> Vec<String> {
    let tool_names = |body: &str| -> Vec<String> {
        serde_json::from_str::<serde_json::Value>(body)
            .ok()
            .and_then(|value| value["tools"].as_array().cloned())
            .map(|tools| {
                tools
                    .iter()
                    .filter_map(|tool| tool["name"].as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };
    let input_items = |body: &str| -> Vec<String> {
        serde_json::from_str::<serde_json::Value>(body)
            .ok()
            .and_then(|value| value["input"].as_array().cloned())
            .map(|items| {
                items
                    .iter()
                    .map(|item| item.to_string())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    };
    bodies
        .windows(2)
        .map(|pair| {
            let (left, right) = (&pair[0], &pair[1]);
            let diverged = left
                .chars()
                .zip(right.chars())
                .take_while(|(a, b)| a == b)
                .count();
            let (before, after) = (tool_names(left), tool_names(right));
            let change = if before == after {
                "same list".to_string()
            } else {
                let added: Vec<&String> =
                    after.iter().filter(|name| !before.contains(name)).collect();
                let removed: Vec<&String> =
                    before.iter().filter(|name| !after.contains(name)).collect();
                format!("added {added:?} removed {removed:?}")
            };
            // Item-level view of the history: an agent turn that re-renders an
            // EARLIER history item (a reminder folded into old content, a
            // re-serialized tool result) breaks the provider prefix cache far
            // sooner than simply appending a new turn, and the raw char offset
            // alone cannot tell those two apart.
            let (left_items, right_items) = (input_items(left), input_items(right));
            let first_changed = left_items
                .iter()
                .zip(right_items.iter())
                .position(|(a, b)| a != b);
            let history = match first_changed {
                Some(index) => format!(
                    "history item {index} re-rendered ({} items -> {}), left={:?} right={:?}",
                    left_items.len(),
                    right_items.len(),
                    left_items[index].chars().take(180).collect::<String>(),
                    right_items[index].chars().take(180).collect::<String>(),
                ),
                None => format!(
                    "history prefix unchanged ({} items -> {})",
                    left_items.len(),
                    right_items.len()
                ),
            };
            format!(
                "diverged_at_char={diverged} of {}/{} | tools {} -> {} ({change}) | {history}",
                left.len(),
                right.len(),
                before.len(),
                after.len()
            )
        })
        .collect()
}

#[tokio::test]
#[ignore = "hits the real DeepSeek API; run explicitly with --ignored"]
async fn real_responses_session_replays_tools_skills_files_and_logs() {
    run_real_responses_session(ThinkingDepth::Simple).await;
}

#[tokio::test]
#[ignore = "hits the real DeepSeek API; run explicitly with --ignored"]
async fn real_responses_thinking_session_replays_tools_skills_files_and_logs() {
    run_real_responses_session(ThinkingDepth::Medium).await;
}

async fn run_real_responses_session(thinking_depth: ThinkingDepth) {
    let Some(key) = deepseek_key() else {
        eprintln!("[skip] no DeepSeek key in env or keychain");
        return;
    };
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
    let bodies: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let transport: Arc<dyn deepagent_models::transport::HttpTransport> =
        Arc::new(RecordingTransport {
            inner: Arc::new(ReqwestTransport::new()),
            bodies: bodies.clone(),
        });
    let secrets = Arc::new(MemorySecretStore::new());
    let settings = Arc::new(SettingsService::new(db.clone(), transport.clone(), secrets));
    settings
        .initialize(&key)
        .await
        .expect("live model discovery");
    settings
        .set_thinking_depth(thinking_depth)
        .expect("set requested thinking profile");
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
    if thinking_depth == ThinkingDepth::Medium {
        let reasoning_ids: Vec<&str> = events
            .iter()
            .filter_map(|event| match &event.payload {
                EventPayload::ResponseItemAppended {
                    item:
                        deepagent_core::response_item::ResponseItem::Reasoning {
                            id: Some(id),
                            content,
                        },
                } if !content.trim().is_empty() => Some(id.as_str()),
                _ => None,
            })
            .collect();
        assert!(
            !reasoning_ids.is_empty(),
            "thinking-mode run must durably retain non-empty reasoning for Responses replay"
        );
        assert_eq!(
            reasoning_ids.len(),
            reasoning_ids
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len(),
            "each provider reasoning item must be persisted once"
        );
    }
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
    eprintln!(
        "[real-session] prompt prefix divergence between consecutive requests: {:?}",
        cache_divergence(&bodies.lock().unwrap())
    );
    // The plan's second live claim: one session must not pay for a fresh summary
    // on every turn. Counted after the pass is drained, below.

    // Live memory line. The note pass is a background kernel run, so draining it
    // is what a one-shot host has to do before exiting — and here it also makes
    // the assertions below deterministic instead of racing a spawned task.
    let drained = chat
        .wait_for_session_memory_passes(std::time::Duration::from_secs(120))
        .await;
    assert!(
        drained,
        "the live session-note pass must finish, not be cancelled at shutdown"
    );

    // The note pass is the only caller whose request names the note file. One
    // extraction can use a variable number of model/tool turns, so validate its
    // bounded spend and persisted cursor below instead of assuming one call per
    // read/edit/answer phase.
    let final_bodies = bodies.lock().unwrap();
    let note_requests = final_bodies
        .iter()
        .filter(|body| body.contains("session-memory.md"))
        .count();
    let agent_requests = final_bodies.len() - note_requests;
    eprintln!(
        "[real-session] model requests: {agent_requests} agent + {note_requests} note-pass calls"
    );
    assert!(
        note_requests <= 6,
        "three turns must not buy three summaries: {note_requests} note calls across \
         {agent_requests} non-note calls"
    );

    let largest_context = usage.iter().map(|entry| entry.0 as u64).max().unwrap_or(0);
    let note_path = db.session_memory_path(session);
    if largest_context < 10_000 {
        eprintln!(
            "[real-session] largest live context was {largest_context} tokens, below the \
             10k extraction gate; the note pass was not exercised"
        );
    } else {
        let note = std::fs::read_to_string(&note_path)
            .expect("the gate opened, so the note beside the session log must exist");
        let unchanged = note.contains("(nothing recorded yet)");
        eprintln!(
            "[real-session] note {} at {}",
            if unchanged { "UNCHANGED" } else { "UPDATED" },
            note_path.display()
        );

        // `query_row` on an absent row is a rusqlite error that is not nameable
        // from this test, so existence is probed with a count that always rows.
        let cursors: Option<String> = db
            .with_conn(|conn| {
                let present: i64 = conn
                    .query_row(
                        "SELECT COUNT(*) FROM documents WHERE collection = 'session_memory' AND id = ?1",
                        [&session_id],
                        |row| row.get(0),
                    )
                    .map_err(|error| {
                        deepagent_core::error::CoreError::Persistence(error.to_string())
                    })?;
                if present == 0 {
                    return Ok(None);
                }
                conn.query_row(
                    "SELECT body FROM documents WHERE collection = 'session_memory' AND id = ?1",
                    [&session_id],
                    |row| row.get(0),
                )
                .map_err(|error| {
                    deepagent_core::error::CoreError::Persistence(error.to_string())
                })
            })
            .unwrap();
        let state: serde_json::Value =
            serde_json::from_str(&cursors.expect("a pass that ran must record its cursors"))
                .expect("the cursors are JSON");
        // The invariant the offline suite pins, verified against real model
        // output: a note that gained nothing must not be trusted as coverage,
        // because compaction cuts the model view at exactly that sequence.
        if unchanged {
            assert_eq!(
                state["last_summarized_sequence"],
                serde_json::Value::Null,
                "an untouched note may not claim a cut point: {state}"
            );
        } else {
            assert!(
                state["last_summarized_sequence"].is_number(),
                "an edited note must record the cut point it covers: {state}"
            );
        }
        assert!(
            state["tokens_at_last_extraction"].as_u64().unwrap_or(0) > 0,
            "the paid window must be remembered so the next turn does not re-buy it: {state}"
        );
    }

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
    // The marker is asserted from the read_file RESULT, which is deterministic:
    // the fixture file contains it. The previous form required the model to echo
    // the marker into its summary, so this live gate went red on runs where the
    // model phrased the recap differently — a flake with no connection to the code
    // under test.
    assert!(
        events.iter().any(|event| matches!(
            &event.payload,
            EventPayload::ToolCallCompleted { output, .. }
                if output.to_string().contains("DEEPAGENT_FILE_SESSION_90210")
        )),
        "no tool result ever carried the fixture marker"
    );

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
        .search("DEEPAGENT_FILE_SESSION_90210", SessionSearchScope::All, 10)
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
