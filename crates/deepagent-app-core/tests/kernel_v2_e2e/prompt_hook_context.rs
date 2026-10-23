//! Successful input-hook output must reach the model independently of its decision.

use super::*;
use deepagent_core::event::EventPayload;
use deepagent_core::id::SessionId;
use deepagent_models::ModelRole;
use deepagent_persistence::event_store::EventStore;
use deepagent_persistence::run_store::RunStore;
use deepagent_runtime::RuntimeEvent;
use std::str::FromStr;
use std::time::Duration;

const CONTEXT: &str = "AUDIT_PROMPT_HOOK_CONTEXT_6E20: use the audit fixture conventions.";
const PROMPT: &str = "Review the offline audit fixture.";
const RUN_ID: &str = "audit-prompt-hook-context";

#[derive(Clone, Copy, Debug)]
enum OutputMode {
    DirectInputControl,
    BlockControl,
    PlainText,
    Structured,
}

#[derive(Default)]
struct RecordingModel(Mutex<Vec<serde_json::Value>>);

#[async_trait]
impl HttpTransport for RecordingModel {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        self.0
            .lock()
            .unwrap()
            .push(serde_json::from_str(&request.body).unwrap());
        emit_responses_events(sink, &response_text("Offline audit completed."))
    }
}

struct Observation {
    requests: usize,
    wire_has_context: bool,
    history_has_context: bool,
}

async fn observe(mode: OutputMode) -> Observation {
    let temporary = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open_in_memory().unwrap());
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
    let output = match mode {
        OutputMode::DirectInputControl => String::new(),
        OutputMode::BlockControl => {
            serde_json::json!({"decision":"block", "reason":"Audit control blocked input."})
                .to_string()
        }
        OutputMode::PlainText => CONTEXT.to_string(),
        OutputMode::Structured => serde_json::json!({"hookSpecificOutput":{
            "hookEventName":"UserPromptSubmit", "additionalContext":CONTEXT
        }})
        .to_string(),
    };
    let marker = temporary.path().join("hook-input.json");
    let quoted_marker = marker.to_string_lossy().replace('\'', "''");
    let quoted_output = output.replace('\'', "''");
    let command = format!(
        "$auditPayload = [Console]::In.ReadToEnd(); \
         [System.IO.File]::WriteAllText('{quoted_marker}', $auditPayload); \
         [Console]::Out.WriteLine('{quoted_output}')"
    );
    settings
        .set_hooks_json(
            &serde_json::json!({"hooks":{"UserPromptSubmit":[{"hooks":[{
                "type":"command", "shell":"powershell", "command":command, "timeout":5
            }]}]}})
            .to_string(),
        )
        .unwrap();
    let model = Arc::new(RecordingModel::default());
    let chat = ChatService::new(db.clone(), settings, model.clone(), temporary.path());
    let live = Arc::new(Mutex::new(Vec::new()));
    let captured = live.clone();
    let prompt = if matches!(mode, OutputMode::DirectInputControl) {
        format!("{PROMPT}\n{CONTEXT}")
    } else {
        PROMPT.to_string()
    };
    let session_id = tokio::time::timeout(
        Duration::from_secs(10),
        chat.run_in_session(
            &prompt,
            None,
            None,
            None,
            Vec::new(),
            None,
            false,
            Some(RUN_ID),
            move |event| captured.lock().unwrap().push(event),
            |_| {},
        ),
    )
    .await
    .expect("the real command hook and offline model must finish")
    .unwrap();
    let payload: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(marker).unwrap()).unwrap();
    assert_eq!(payload["hook_event_name"], "UserPromptSubmit");
    assert_eq!(payload["session_id"], session_id);
    assert_eq!(payload["prompt"], prompt);
    let live = live.lock().unwrap();
    let completions: Vec<_> = live
        .iter()
        .filter_map(|event| match event {
            RuntimeEvent::HookCompleted {
                event,
                exit_code,
                stdout,
                outcome,
                ..
            } if event == "UserPromptSubmit" => Some((exit_code, stdout, outcome)),
            _ => None,
        })
        .collect();
    assert_eq!(completions.len(), 1, "the configured hook must run once");
    assert_eq!(*completions[0].0, 0);
    assert_eq!(completions[0].1.trim(), output);
    assert_eq!(
        completions[0].2,
        if matches!(mode, OutputMode::BlockControl) {
            "blocked"
        } else {
            "continued"
        }
    );
    let run = RunStore::new(&db).get(RUN_ID).unwrap().unwrap();
    assert!(run.terminal_kind.is_some());
    assert_eq!(
        chat.run_events(RUN_ID, None)
            .unwrap()
            .iter()
            .filter(|event| event.event_type == "run_terminal")
            .count(),
        1
    );
    let history_has_context = EventStore::new(&db)
        .load_session(SessionId::from_str(&session_id).unwrap())
        .unwrap()
        .iter()
        .any(|event| match &event.payload {
            EventPayload::MessageAppended { message } => message.content.contains(CONTEXT),
            EventPayload::ResponseItemAppended { item } => {
                serde_json::to_string(item).unwrap().contains(CONTEXT)
            }
            _ => false,
        });
    let requests = model.0.lock().unwrap();
    let wire_has_context = requests
        .iter()
        .any(|request| request.to_string().contains(CONTEXT));
    eprintln!(
        "mode={mode:?} hook_exit=0 outcome={} requests={} wire_context={wire_has_context} \
         history_context={history_has_context} terminal={:?}",
        completions[0].2,
        requests.len(),
        run.terminal_kind
    );
    Observation {
        requests: requests.len(),
        wire_has_context,
        history_has_context,
    }
}

#[tokio::test]
async fn prompt_hook_context_controls_preserve_direct_input_and_block_main_request() {
    let direct = observe(OutputMode::DirectInputControl).await;
    assert_eq!(direct.requests, 1);
    assert!(direct.wire_has_context);
    assert!(direct.history_has_context);
    let blocked = observe(OutputMode::BlockControl).await;
    assert_eq!(blocked.requests, 0);
    assert!(!blocked.wire_has_context);
}

#[tokio::test]
#[ignore = "audit: successful UserPromptSubmit context must enter the actual model request"]
async fn audit_successful_prompt_hook_context_reaches_model_request() {
    let mut missing = Vec::new();
    for mode in [OutputMode::PlainText, OutputMode::Structured] {
        let observation = observe(mode).await;
        assert_eq!(observation.requests, 1);
        if !observation.wire_has_context {
            missing.push(format!("{mode:?}"));
        }
    }
    assert!(
        missing.is_empty(),
        "successful hook context lost: {missing:?}"
    );
}
