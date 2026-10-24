//! Resume must not use a bounded status preview as the child's recovery state.
//! Provider decisions are fixtures; file reads and both parent turns use the real host.

use super::responses_fixture::{call, completed, text};
use super::*;
use deepagent_models::ModelRole;
use deepagent_persistence::run_store::RunStore;
use deepagent_persistence::subagent_store::SubagentRunStore;
use serde_json::{json, Value};
use std::time::Duration;

const MODEL: &str = "deepseek-flash";
const ORIGINAL: &str = "AUDIT_RESUME_ORIGINAL: inspect the supplied fixture and return a report.";
const CONTINUATION: &str = "AUDIT_RESUME_CONTINUE: continue from the previous investigation.";
const FACT: &str = "SYNTHETIC_OBSERVATION_7F3A";
const TAIL: &str = "SYNTHETIC_REPORT_TAIL_8D2B";
const FIRST_RUN: &str = "audit-child-context-first";
const NEXT_RUN: &str = "audit-child-context-next";

struct ResumeModel {
    report: String,
    read: bool,
    child_id: Mutex<String>,
    requests: Mutex<Vec<Value>>,
}

impl ResumeModel {
    fn child_final(&self) -> usize {
        1 + usize::from(self.read)
    }
    fn parent_final(&self) -> usize {
        self.child_final() + 1
    }
    fn resumed(&self) -> usize {
        self.parent_final() + 2
    }
}

#[async_trait]
impl HttpTransport for ResumeModel {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let body: Value = serde_json::from_str(&request.body)?;
        assert_eq!(body["model"], MODEL);
        let index = {
            let mut requests = self.requests.lock().unwrap();
            requests.push(body);
            requests.len() - 1
        };
        let item = if index == 0 {
            call(
                "context-launch",
                "task",
                json!({"description":"context worker","prompt":ORIGINAL}),
            )
        } else if self.read && index == 1 {
            call("context-read", "read_file", json!({"path":"evidence.txt"}))
        } else if index == self.child_final() {
            text("context-original-report", &self.report)
        } else if index == self.parent_final() {
            text(
                "context-first-parent-final",
                "Synthetic delegation completed.",
            )
        } else if index == self.resumed() - 1 {
            call(
                "context-resume",
                "task",
                json!({"operation":"resume",
                "subagent_id":self.child_id.lock().unwrap().clone(),"prompt":CONTINUATION}),
            )
        } else if index == self.resumed() {
            text(
                "context-resumed-result",
                "Synthetic continuation completed.",
            )
        } else if index == self.resumed() + 1 {
            text("context-next-parent-final", "Synthetic parent completed.")
        } else {
            panic!("unexpected resume audit request {index}");
        };
        emit_responses_events(
            sink,
            &completed(
                &format!("context-response-{index}"),
                MODEL,
                item,
                json!({"input_tokens":12,"output_tokens":6,"total_tokens":18}),
            ),
        )
    }
}

async fn settings(db: Arc<Database>) -> Arc<SettingsService> {
    let settings = Arc::new(SettingsService::new(
        db,
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
        settings.set_model(role, MODEL).unwrap();
    }
    settings
        .set_active_permission_preset(PermissionPreset::Default)
        .unwrap();
    settings
}

async fn run(chat: &ChatService, prompt: &str, session: Option<&str>, run: &str) -> String {
    tokio::time::timeout(
        Duration::from_secs(8),
        chat.run_in_session(
            prompt,
            session,
            None,
            None,
            Vec::new(),
            None,
            false,
            Some(run),
            |_| {},
            |_| {},
        ),
    )
    .await
    .expect("resume workflow must settle")
    .unwrap()
}

fn output(body: &Value, id: &str) -> Value {
    let items = body["input"].as_array().unwrap();
    assert_eq!(
        items
            .iter()
            .filter(|item| item["type"] == "function_call" && item["call_id"] == id)
            .count(),
        1
    );
    let outputs: Vec<_> = items
        .iter()
        .filter(|item| item["type"] == "function_call_output" && item["call_id"] == id)
        .collect();
    assert_eq!(outputs.len(), 1);
    let value: Value = serde_json::from_str(outputs[0]["output"].as_str().unwrap()).unwrap();
    assert_eq!(value["status"], "ok");
    value["result"].clone()
}

#[derive(Debug)]
struct Observation {
    report_chars: usize,
    report_bytes: usize,
    status_chars: usize,
    full_report_in_resume: bool,
    tail_in_resume: bool,
    fact_in_resume: bool,
    read_pair_in_resume: bool,
    resumed_input_items: usize,
}

async fn observe(chars: usize, unicode: bool, read: bool, rebuild: bool) -> Observation {
    let temporary = tempfile::tempdir().unwrap();
    let workspace = temporary.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    std::fs::write(workspace.join("evidence.txt"), FACT).unwrap();
    assert!(chars >= TAIL.chars().count());
    let report = format!(
        "{}{TAIL}",
        if unicode { "研" } else { "x" }.repeat(chars - TAIL.chars().count())
    );
    assert_eq!(report.chars().count(), chars);
    let db = Arc::new(Database::open(temporary.path().join("state.sqlite")).unwrap());
    let settings = settings(db.clone()).await;
    let model = Arc::new(ResumeModel {
        report: report.clone(),
        read,
        child_id: Mutex::new(String::new()),
        requests: Mutex::new(Vec::new()),
    });
    let chat = ChatService::new(db.clone(), settings.clone(), model.clone(), &workspace);
    let session = run(&chat, "Run the controlled delegation.", None, FIRST_RUN).await;
    let original = chat.subagent_runs(FIRST_RUN).unwrap().remove(0);
    assert_eq!(original.state, "succeeded");
    let saved: Value =
        serde_json::from_slice(&std::fs::read(original.transcript_path.as_ref().unwrap()).unwrap())
            .unwrap();
    assert_eq!(saved["summary"], report);
    assert_eq!(saved["attempts"][0]["summary"], report);
    assert_eq!(saved["request"]["prompt"], ORIGINAL);
    let summary = original.summary.as_ref().unwrap();
    assert_eq!(summary.chars().count(), chars.min(2000));
    if chars <= 2000 {
        assert_eq!(summary, &report);
    } else {
        assert_eq!(
            summary,
            &format!("{}…", report.chars().take(1999).collect::<String>())
        );
    }
    *model.child_id.lock().unwrap() = original.id.clone();
    // This is service reconstruction with the same open SQLite, not a process restart.
    let chat = if rebuild {
        drop(chat);
        ChatService::new(db.clone(), settings, model.clone(), &workspace)
    } else {
        chat
    };
    assert_eq!(
        run(
            &chat,
            "Continue the controlled delegation.",
            Some(&session),
            NEXT_RUN
        )
        .await,
        session
    );
    let resumed = SubagentRunStore::new(&db)
        .get(&original.id)
        .unwrap()
        .unwrap();
    assert_eq!(resumed.state, "succeeded");
    assert_eq!(resumed.resume_count, 1);
    assert_eq!(resumed.origin_parent_run_id, FIRST_RUN);
    assert_eq!(resumed.parent_run_id, NEXT_RUN);
    let after: Value =
        serde_json::from_slice(&std::fs::read(resumed.transcript_path.as_ref().unwrap()).unwrap())
            .unwrap();
    assert_eq!(after["attempts"].as_array().unwrap().len(), 2);
    assert_eq!(after["attempts"][0]["summary"], report);
    assert_eq!(after["state"], "succeeded");
    let requests = model.requests.lock().unwrap();
    assert_eq!(requests.len(), model.resumed() + 2);
    // The parent received the entire initial answer, so provider/output truncation is excluded.
    assert_eq!(
        output(&requests[model.parent_final()], "context-launch")["result"],
        report
    );
    assert_eq!(
        output(&requests[model.resumed() + 1], "context-resume")["resumed"],
        true
    );
    if read {
        let read_output = output(&requests[model.child_final()], "context-read");
        assert!(read_output.to_string().contains(FACT));
        assert!(!report.contains(FACT));
    }
    let resumed_body = &requests[model.resumed()];
    let resumed_input = resumed_body["input"].to_string();
    assert!(resumed_input.contains(ORIGINAL));
    assert!(resumed_input.contains(CONTINUATION));
    let observed = Observation {
        report_chars: chars,
        report_bytes: report.len(),
        status_chars: summary.chars().count(),
        full_report_in_resume: resumed_input.contains(&report),
        tail_in_resume: resumed_input.contains(TAIL),
        fact_in_resume: resumed_input.contains(FACT),
        read_pair_in_resume: resumed_body["input"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| {
                item["type"] == "function_call_output" && item["call_id"] == "context-read"
            }),
        resumed_input_items: resumed_body["input"].as_array().unwrap().len(),
    };
    for run in [FIRST_RUN, NEXT_RUN] {
        assert_eq!(
            RunStore::new(&db)
                .get(run)
                .unwrap()
                .unwrap()
                .terminal_kind
                .as_deref(),
            Some("succeeded")
        );
        let events = chat.run_events(run, None).unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event_type == "run_terminal")
                .count(),
            1
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event_type == "subagent_started")
                .count(),
            1
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event_type == "subagent_completed")
                .count(),
            1
        );
    }
    assert_eq!(
        std::fs::read_to_string(workspace.join("evidence.txt")).unwrap(),
        FACT
    );
    eprintln!(
        "Subagent resume context: unicode={unicode} read={read} rebuild={rebuild} {observed:?}"
    );
    observed
}

fn verify_report(observed: &Observation, chars: usize, unicode: bool) {
    assert_eq!(observed.report_chars, chars);
    assert_eq!(observed.status_chars, chars.min(2000));
    assert_eq!(
        observed.report_bytes,
        if unicode {
            (chars - TAIL.len()) * 3 + TAIL.len()
        } else {
            chars
        }
    );
}

#[tokio::test]
async fn subagent_resume_context_short_result_roundtrips() {
    for rebuild in [false, true] {
        let observed = observe(128, false, false, rebuild).await;
        verify_report(&observed, 128, false);
        assert!(observed.full_report_in_resume && observed.tail_in_resume);
    }
}

#[tokio::test]
async fn subagent_resume_context_exact_character_limit_roundtrips() {
    for unicode in [false, true] {
        let observed = observe(2000, unicode, false, false).await;
        verify_report(&observed, 2000, unicode);
        assert!(observed.full_report_in_resume && observed.tail_in_resume);
    }
}

async fn verify_long_report(rebuild: bool) {
    let mut observations = Vec::new();
    for chars in [2001, 2400] {
        for unicode in [false, true] {
            let observed = observe(chars, unicode, false, rebuild).await;
            verify_report(&observed, chars, unicode);
            observations.push(observed);
        }
    }
    assert!(
        observations
            .iter()
            .all(|observed| observed.full_report_in_resume && observed.tail_in_resume),
        "complete preserved report must not be replaced by a status preview: {observations:?}"
    );
}

#[tokio::test]
#[ignore = "audit reproducer: child resume reads a truncated status preview instead of the saved result"]
async fn subagent_resume_context_long_result_must_survive() {
    verify_long_report(false).await;
}

#[tokio::test]
#[ignore = "audit reproducer: service reconstruction must not lose a completed child's full result"]
async fn subagent_resume_context_long_result_must_survive_rebuilt_service() {
    verify_long_report(true).await;
}

#[tokio::test]
#[ignore = "audit reproducer: a resumed child has no recoverable record of its prior tool observation"]
async fn subagent_resume_context_prior_observation_must_survive() {
    let mut observations = Vec::new();
    for rebuild in [false, true] {
        observations.push(observe(128, false, true, rebuild).await);
    }
    assert!(observations.iter().all(|observed| observed.fact_in_resume && observed.read_pair_in_resume && observed.resumed_input_items > 1),
        "a resumed child must recover its actual observation with a paired tool history: {observations:?}");
}
