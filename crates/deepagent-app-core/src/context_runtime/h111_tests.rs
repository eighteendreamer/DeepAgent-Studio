//! Real PreCompact command output through legacy/native/prefire summary requests.
use super::*;
use crate::session_memory::testing::{scripted_client_with_transport, ScriptedTransport};
use deepagent_core::clock::SystemClock;
use deepagent_hooks::{HookDefinitions, SystemHookRunner};
use serde_json::{json, Value};

const FIRST: &str = "H111_RETAIN_FIRST_INSTRUCTION";
const SECOND: &str = "H111_RETAIN_SECOND_INSTRUCTION";

struct Fixture {
    temp: tempfile::TempDir,
    path: std::path::PathBuf,
    db: Arc<Database>,
    sid: SessionId,
    messages: Vec<Message>,
    items: Vec<ResponseInputItem>,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("state.sqlite");
        let db = Arc::new(Database::open(&path).unwrap());
        let mut session = deepagent_session::Session::create(&db, &SystemClock, None).unwrap();
        let messages = (0..24)
            .map(|index| {
                let text = format!(
                    "history-{index}: {}",
                    "synthetic retained progress ".repeat(600)
                );
                if index % 2 == 0 {
                    Message::user(text)
                } else {
                    Message::assistant(text)
                }
            })
            .collect::<Vec<_>>();
        for message in &messages {
            session
                .append(EventPayload::MessageAppended {
                    message: message.clone(),
                })
                .unwrap();
        }
        let sid = session.id();
        let events = deepagent_persistence::event_store::EventStore::new(&db)
            .load_session(sid)
            .unwrap();
        let items = crate::input_runtime::response_items_from_events(&events);
        Self {
            temp,
            path,
            db,
            sid,
            messages,
            items,
        }
    }

    fn compactor(
        &self,
        outputs: &[(&str, i32)],
        answers: Vec<Vec<String>>,
    ) -> (HookedReactiveContextCompactor, Arc<ScriptedTransport>) {
        let actions=outputs.iter().enumerate().map(|(index,(text,code))| {
            let marker=self.temp.path().join(format!("hook-{index}.txt")).to_string_lossy().replace('\'',"''");
            json!({"type":"command","shell":"powershell","timeout":5,
                "command":format!("[IO.File]::AppendAllText('{marker}','called\n'); [Console]::Out.Write('{}'); exit {code}",text.replace('\'',"''"))})
        }).collect::<Vec<_>>();
        self.with_definitions(json!({"hooks":{"PreCompact":[{"hooks":actions}]}}), answers)
    }

    fn with_definitions(
        &self,
        definitions: Value,
        answers: Vec<Vec<String>>,
    ) -> (HookedReactiveContextCompactor, Arc<ScriptedTransport>) {
        let defs = HookDefinitions::parse(&definitions.to_string()).unwrap();
        let mut hooks = HookRegistry::new();
        assert!(defs.register_into(&mut hooks, Arc::new(SystemHookRunner)) > 0);
        let (client, wire) = scripted_client_with_transport(answers);
        (
            HookedReactiveContextCompactor::new(
                client,
                "deepseek-flash".into(),
                Arc::new(hooks),
                self.sid,
                self.db.clone(),
            ),
            wire,
        )
    }

    fn assert_once(&self, count: usize) {
        for index in 0..count {
            assert_eq!(
                std::fs::read_to_string(self.temp.path().join(format!("hook-{index}.txt")))
                    .unwrap(),
                "called\n"
            );
        }
    }
}

#[derive(Default)]
struct PhysicalUsage(std::sync::Mutex<Vec<deepagent_models::usage::CallUsageReport>>);

impl deepagent_models::usage::CallUsageSink for PhysicalUsage {
    fn record(&self, report: deepagent_models::usage::CallUsageReport) {
        self.0.lock().unwrap().push(report);
    }
}

fn answer(label: &str) -> Vec<String> {
    ScriptedTransport::text(&json!({"goal":"synthetic task","completed":[label]}).to_string())
}

fn assert_instructions(wire: &ScriptedTransport) {
    let body: Value = serde_json::from_str(&wire.last_prompt()).unwrap();
    assert!(
        !body["instructions"].to_string().contains(FIRST),
        "hook text must not overwrite system authority"
    );
    let text = body["input"].to_string();
    assert_eq!(
        text.matches(FIRST).count(),
        1,
        "each instruction must reach actual user-side request once"
    );
    assert_eq!(text.matches(SECOND).count(), 1);
    assert!(text.find(FIRST).unwrap() < text.find(SECOND).unwrap());
}

#[tokio::test]
async fn h111_legacy_and_native_commands_reach_summary_and_durable_reopen() {
    for native in [false, true] {
        let f = Fixture::new();
        let (c, wire) = f.compactor(
            &[(FIRST, 0), (SECOND, 0)],
            vec![answer("synchronous-summary")],
        );
        let result = if native {
            c.compact_response_items(None, &f.items, CompactionTrigger::ContextOverflow)
                .await
        } else {
            c.compact(&f.messages, CompactionTrigger::ContextOverflow)
                .await
        }
        .unwrap()
        .unwrap();
        assert_instructions(&wire);
        f.assert_once(2);
        assert_eq!(result.usage.prompt_tokens, 120);
        assert!(result.summary.contains("synchronous-summary"));
        let expected = deepagent_persistence::event_store::EventStore::new(&f.db)
            .load_session(f.sid)
            .unwrap();
        assert_eq!(
            expected
                .iter()
                .filter(|event| matches!(event.payload, EventPayload::ContextCompacted { .. }))
                .count(),
            1
        );
        let sid = f.sid;
        let path = f.path.clone();
        drop(c);
        drop(f.db);
        let db = Database::open(&path).unwrap();
        assert_eq!(
            deepagent_persistence::event_store::EventStore::new(&db)
                .load_session(sid)
                .unwrap(),
            expected
        );
    }
}

#[tokio::test]
async fn h111_prefire_must_regenerate_uninstructed_note_without_rerunning_hooks() {
    for native in [false, true] {
        let f = Fixture::new();
        let (c, wire) = f.compactor(
            &[(FIRST, 0), (SECOND, 0)],
            vec![answer("uninstructed-note"), answer("instructed-summary")],
        );
        let note = if native {
            c.prefire_pass1_response_items(None, &f.items).await
        } else {
            c.prefire_pass1(&f.messages).await
        }
        .unwrap()
        .unwrap();
        assert!(
            !wire.last_prompt().contains(FIRST),
            "speculation must not execute side-effectful hooks"
        );
        let result = if native {
            c.apply_prefire_response_items(&note, None, &f.items).await
        } else {
            c.apply_prefire(&note, &f.messages).await
        }
        .unwrap()
        .unwrap();
        assert_instructions(&wire);
        f.assert_once(2);
        assert!(
            result.summary.contains("instructed-summary"),
            "cached note cannot retroactively honor instructions"
        );
        assert_eq!(
            result.usage.prompt_tokens, 120,
            "Usage::absorb preserves latest prompt context"
        );
        assert_eq!(
            result.usage.completion_tokens, 36,
            "discarded speculation plus regeneration both retain output spend"
        );
    }
}

#[tokio::test]
async fn h111_empty_nonzero_and_veto_controls_preserve_existing_behavior() {
    for (output, code, allowed) in [
        ("", 0, true),
        ("not an instruction", 7, true),
        ("", 2, false),
        (
            r#"{"permissionDecision":"ask","permissionDecisionReason":"synthetic"}"#,
            0,
            false,
        ),
    ] {
        let f = Fixture::new();
        let (c, wire) = f.compactor(&[(output, code)], vec![answer("normal-summary")]);
        let result = c
            .compact_response_items(None, &f.items, CompactionTrigger::ContextOverflow)
            .await
            .unwrap();
        assert_eq!(result.is_some(), allowed);
        f.assert_once(1);
        if allowed {
            assert!(!wire.last_prompt().contains("not an instruction"));
        } else {
            assert!(wire.last_prompt().is_empty());
        }
    }
}

#[tokio::test]
async fn h111_oversized_prefire_regeneration_keeps_both_physical_calls_and_no_boundary() {
    let f = Fixture::new();
    let (c, wire) = f.compactor(
        &[(FIRST, 0)],
        vec![
            answer("speculative"),
            answer(&"oversized regenerated note ".repeat(25_000)),
        ],
    );
    let ledger = Arc::new(PhysicalUsage::default());
    let c = c.with_call_usage_sink(ledger.clone());
    let note = c
        .prefire_pass1_response_items(None, &f.items)
        .await
        .unwrap()
        .unwrap();
    let before = deepagent_persistence::event_store::EventStore::new(&f.db)
        .load_session(f.sid)
        .unwrap();
    let result = c
        .apply_prefire_response_items_accounted(&note, None, &f.items, None)
        .await
        .unwrap();
    assert!(wire.last_prompt().contains(FIRST));
    assert!(result.compaction.is_none());
    assert_eq!(result.observed_usage.completion_tokens, 36);
    assert_eq!(
        deepagent_persistence::event_store::EventStore::new(&f.db)
            .load_session(f.sid)
            .unwrap(),
        before
    );
    let calls = ledger.0.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(
        calls
            .iter()
            .map(|call| call.usage.unwrap().prompt_tokens)
            .sum::<u32>(),
        240
    );
    assert_eq!(
        calls
            .iter()
            .map(|call| call.usage.unwrap().completion_tokens)
            .sum::<u32>(),
        36
    );
}

#[tokio::test]
async fn h111_existing_session_note_cannot_bypass_new_summary_instructions() {
    use crate::session_memory::{SessionMemoryState, SessionMemoryStore};
    let f = Fixture::new();
    let events = deepagent_persistence::event_store::EventStore::new(&f.db)
        .load_session(f.sid)
        .unwrap();
    SessionMemoryStore::new(f.db.clone())
        .save(
            f.sid,
            &SessionMemoryState {
                summary: "cached note without custom instructions".into(),
                initialized: true,
                last_summarized_sequence: events.last().map(|event| event.sequence),
                ..Default::default()
            },
            &events,
        )
        .unwrap();
    let (c, wire) = f.compactor(&[(FIRST, 0)], vec![answer("new instructed summary")]);
    let (cached, usage) = c.summarize_response_item_zone(&f.items, None).await;
    assert!(cached.contains("cached note"));
    assert_eq!(usage, deepagent_models::Usage::default());
    assert!(wire.last_prompt().is_empty());
    let (summary, usage) = c
        .summarize_response_item_zone_cancelled(&f.items, None, Some(FIRST))
        .await
        .unwrap();
    assert!(summary.contains("new instructed summary"));
    assert!(wire.last_prompt().contains(FIRST));
    assert_eq!(usage.completion_tokens, 18);
}

#[tokio::test]
async fn h111_real_hook_cancellation_stops_compaction_and_timeout_remains_nonblocking() {
    for timeout in [false, true] {
        let f = Fixture::new();
        let pid_path = f.temp.path().join("started-pid.txt");
        let late_path = f.temp.path().join("late.txt");
        let quote = |path: &std::path::Path| path.to_string_lossy().replace('\'', "''");
        let command = format!("[IO.File]::WriteAllText('{}',[string]$PID); Start-Sleep -Seconds 30; [IO.File]::WriteAllText('{}','late'); [Console]::Out.Write('{FIRST}')", quote(&pid_path), quote(&late_path));
        let (c, wire) = f.with_definitions(json!({"hooks":{"PreCompact":[{"hooks":[{
            "type":"command","shell":"powershell","command":command,"timeout":if timeout {1} else {10}
        }]}]}}), vec![answer("normal timeout fallback")]);
        let cancel = Arc::new(AtomicBool::new(false));
        let before = deepagent_persistence::event_store::EventStore::new(&f.db)
            .load_session(f.sid)
            .unwrap();
        let execution = c.compact_response_items_accounted(
            None,
            &f.items,
            CompactionTrigger::ContextOverflow,
            Some(cancel.clone()),
        );
        let signal = async {
            tokio::time::timeout(std::time::Duration::from_secs(5), async {
                while !pid_path.exists() {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("real hook must start");
            if !timeout {
                cancel.store(true, Ordering::Release);
            }
        };
        let (result, ()) = tokio::time::timeout(std::time::Duration::from_secs(8), async {
            tokio::join!(execution, signal)
        })
        .await
        .unwrap();
        if timeout {
            // SystemHookRunner represents timeout as nonzero exit 124; the
            // existing nonblocking policy must not turn it into instructions.
            let outcome = result.unwrap();
            assert!(outcome.compaction.is_some());
            assert!(!wire.last_prompt().is_empty());
            assert!(!wire.last_prompt().contains(FIRST));
        } else {
            let Err(error) = result else {
                panic!("cancelled hook must not compact")
            };
            assert!(error.error.to_string().contains("cancelled"));
            assert_eq!(error.usage, deepagent_models::Usage::default());
            assert!(wire.last_prompt().is_empty());
            assert_eq!(
                deepagent_persistence::event_store::EventStore::new(&f.db)
                    .load_session(f.sid)
                    .unwrap(),
                before
            );
        }
        assert!(!late_path.exists());
        let pid: u32 = std::fs::read_to_string(&pid_path).unwrap().parse().unwrap();
        // Query the exact synthetic hook process, not the user's processes.
        let status = std::process::Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                &format!("if (Get-Process -Id {pid} -ErrorAction SilentlyContinue) {{ exit 1 }}"),
            ])
            .status()
            .unwrap();
        assert!(
            status.success(),
            "cancel/timeout must reap the actual command hook"
        );
    }
}
