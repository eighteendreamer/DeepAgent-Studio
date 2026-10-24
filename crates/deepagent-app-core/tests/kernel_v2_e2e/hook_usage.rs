//! Model-backed lifecycle work must reach the existing session cost owner.
//! All usage is synthetic; model parsing, hooks, tools, ledger and admission are real.

use super::responses_fixture::{call, completed, text};
use super::*;
use deepagent_app_core::{BudgetConfig, CostSummary};
use deepagent_core::event::EventPayload;
use deepagent_core::id::SessionId;
use deepagent_models::ModelRole;
use deepagent_persistence::cost_store::CostStore;
use deepagent_persistence::event_store::EventStore;
use deepagent_runtime::events::RuntimeEvent;
use serde_json::{json, Value};
use std::str::FromStr;
use std::time::Duration;

const MODEL: &str = "deepseek-flash";
const MARKER: &str = "AUDIT_MODEL_HOOK_USAGE";
const RUN: &str = "audit-hook-usage-first";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HookKind {
    None,
    UnmatchedAgent,
    Prompt,
    Agent,
    PromptDeny,
    AgentDeny,
}

impl HookKind {
    fn is_agent(self) -> bool {
        matches!(self, Self::Agent | Self::AgentDeny | Self::UnmatchedAgent)
    }

    fn denies(self) -> bool {
        matches!(self, Self::PromptDeny | Self::AgentDeny)
    }
}

struct UsageModel {
    kind: HookKind,
    parent: Mutex<Vec<Value>>,
    hooks: Mutex<Vec<Value>>,
}

#[async_trait]
impl HttpTransport for UsageModel {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let body: Value = serde_json::from_str(&request.body)?;
        assert_eq!(body["model"], MODEL); // No fallback or distinct-model pricing ambiguity.
        let hook = body["input"].to_string().contains(MARKER);
        let index = {
            let mut requests = if hook {
                self.hooks.lock().unwrap()
            } else {
                self.parent.lock().unwrap()
            };
            requests.push(body);
            requests.len() - 1
        };
        let item = if hook {
            assert!(index < if self.kind.is_agent() { 2 } else { 1 });
            match index {
                0 if self.kind.is_agent() => call("hook-usage-list", "task_list", json!({})),
                0 | 1 => text(
                    "hook-decision",
                    if self.kind.denies() {
                        r#"{"ok":false,"reason":"controlled audit denial"}"#
                    } else {
                        r#"{"ok":true}"#
                    },
                ),
                _ => panic!("unexpected hook request: {:?}/{index}", self.kind),
            }
        } else if index == 0 {
            call(
                "parent-usage-read",
                "read_file",
                json!({"path":"fixture.txt"}),
            )
        } else {
            assert!(
                index < 3,
                "one continuation and at most one follow-up request"
            );
            text(
                &format!("parent-answer-{index}"),
                "Offline hook usage audit complete.",
            )
        };
        emit_responses_events(
            sink,
            &completed(
                &format!("usage-response-{hook}-{index}"),
                MODEL,
                item,
                json!({"input_tokens":1000,"output_tokens":100,"total_tokens":1100,
                "input_tokens_details":{"cached_tokens":800},
                "output_tokens_details":{"reasoning_tokens":0}}),
            ),
        )
    }
}

#[derive(Debug)]
struct Observation {
    answered: usize,
    hook_calls: usize,
    summary: CostSummary,
    ledger_cost: f64,
    event_cost: f64,
    live_input: u64,
    durable_input: u64,
    raw_calls: usize,
    expected_cost: f64,
    limit: f64,
    followup_error: Option<String>,
    followup_dispatches: usize,
}

async fn hook_settings(db: Arc<Database>, kind: HookKind) -> Arc<SettingsService> {
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
        settings.set_model(role, MODEL).unwrap();
    }
    if kind != HookKind::None {
        settings
            .set_hooks_json(
                &json!({"hooks":{"PreToolUse":[{
                    "matcher":if kind == HookKind::UnmatchedAgent {"write_file"} else {"read_file"},
                    "hooks":[{"type":if kind.is_agent() {"agent"} else {"prompt"},
                        "prompt":MARKER,"timeout":5}]
                }]}})
                .to_string(),
            )
            .unwrap();
    }
    settings
}

fn tool_output(request: &Value, call_id: &str) -> Value {
    let outputs: Vec<_> = request["input"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["type"] == "function_call_output" && item["call_id"] == call_id)
        .collect();
    assert_eq!(outputs.len(), 1);
    serde_json::from_str(outputs[0]["output"].as_str().unwrap()).unwrap()
}

fn verify_first_run(chat: &ChatService, model: &UsageModel, events: &[RuntimeEvent]) -> usize {
    let kind = model.kind;
    let hook_calls = model.hooks.lock().unwrap().len();
    assert_eq!(
        hook_calls,
        match kind {
            HookKind::Prompt | HookKind::PromptDeny => 1,
            HookKind::Agent | HookKind::AgentDeny => 2,
            _ => 0,
        }
    );
    assert_eq!(model.parent.lock().unwrap().len(), 2);
    if matches!(kind, HookKind::Agent | HookKind::AgentDeny) {
        let hooks = model.hooks.lock().unwrap();
        assert_eq!(tool_output(&hooks[1], "hook-usage-list")["status"], "ok");
    }
    let run_events = chat.run_events(RUN, None).unwrap();
    let terminals: Vec<_> = run_events
        .iter()
        .filter(|event| event.event_type == "run_terminal")
        .collect();
    assert_eq!(terminals.len(), 1);
    assert_eq!(terminals[0].data["kind"], "succeeded");
    let parent_result = {
        let requests = model.parent.lock().unwrap();
        tool_output(&requests[1], "parent-usage-read")
    };
    assert_eq!(
        parent_result["status"],
        if kind.denies() { "error" } else { "ok" }
    );
    if kind.denies() {
        assert!(parent_result
            .to_string()
            .contains("controlled audit denial"));
        assert!(!parent_result.to_string().contains("AUDIT_HOOK_USAGE_FILE"));
    } else {
        assert!(parent_result.to_string().contains("AUDIT_HOOK_USAGE_FILE"));
    }
    assert_eq!(events.iter().filter(|event| matches!(event,
        RuntimeEvent::HookCompleted { event, outcome, .. }
            if event == "PreToolUse" && outcome == if kind.denies() {"blocked"} else {"continued"}
    )).count(), usize::from(hook_calls > 0));
    hook_calls
}

fn live_totals(events: &[RuntimeEvent]) -> (f64, u64) {
    let event_cost = events
        .iter()
        .filter_map(|event| match event {
            RuntimeEvent::CostRecorded { cost_yuan, .. } => {
                Some(cost_yuan.expect("active official pricing"))
            }
            _ => None,
        })
        .sum();
    let live_input = events
        .iter()
        .filter_map(|event| match event {
            RuntimeEvent::Usage { prompt_tokens, .. } => Some(u64::from(*prompt_tokens)),
            _ => None,
        })
        .sum();
    (event_cost, live_input)
}

fn durable_totals(db: &Database, session: &str) -> (u64, usize) {
    let history = EventStore::new(db)
        .load_session(SessionId::from_str(session).unwrap())
        .unwrap();
    let durable_input = history
        .iter()
        .filter_map(|event| match &event.payload {
            EventPayload::UsageRecorded { prompt_tokens, .. } => Some(u64::from(*prompt_tokens)),
            _ => None,
        })
        .sum();
    let raw_calls = history
        .iter()
        .filter_map(|event| match &event.payload {
            EventPayload::UsageRecorded {
                raw_responses_usage: Some(raw),
                ..
            } => Some(raw.as_array().unwrap().len()),
            _ => None,
        })
        .sum();
    (durable_input, raw_calls)
}

async fn observe(kind: HookKind, budget_calls: f64) -> Observation {
    let temporary = tempfile::tempdir().unwrap();
    let workspace = temporary.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    std::fs::write(workspace.join("fixture.txt"), "AUDIT_HOOK_USAGE_FILE").unwrap();
    let db = Arc::new(Database::open(temporary.path().join("state.sqlite")).unwrap());
    let settings = hook_settings(db.clone(), kind).await;
    let pricing = settings.load().unwrap().unwrap().pricing_catalog.models;
    let unit_cost = pricing[MODEL].calculate(1000, 100, 800, 200);
    assert!(unit_cost > 0.0);
    let limit = unit_cost * budget_calls;
    let cost = Arc::new(CostService::new(db.clone()));
    cost.set_pricing(pricing); // Exclude the already documented H-225 activation defect.
    cost.set_budget(BudgetConfig {
        daily_limit: Some(limit),
        monthly_limit: None,
    });
    let model = Arc::new(UsageModel {
        kind,
        parent: Mutex::new(Vec::new()),
        hooks: Mutex::new(Vec::new()),
    });
    let chat =
        ChatService::new(db.clone(), settings, model.clone(), &workspace).with_cost(cost.clone());
    let events = Arc::new(Mutex::new(Vec::new()));
    let captured = events.clone();
    let session = tokio::time::timeout(
        Duration::from_secs(8),
        chat.run_in_session(
            "Read fixture.txt and reply briefly.",
            None,
            None,
            None,
            Vec::new(),
            None,
            false,
            Some(RUN),
            move |event| captured.lock().unwrap().push(event),
            |_| {},
        ),
    )
    .await
    .expect("formal hook workflow must finish")
    .unwrap();
    let (hook_calls, event_cost, live_input) = {
        let events = events.lock().unwrap();
        let hook_calls = verify_first_run(&chat, &model, &events);
        let (event_cost, live_input) = live_totals(&events);
        (hook_calls, event_cost, live_input)
    };
    let (durable_input, raw_calls) = durable_totals(&db, &session);
    let summary = cost.summary(&session).unwrap();
    let rows = CostStore::new(&db)
        .session_costs_detailed(&session)
        .unwrap();
    let ledger_cost = rows
        .iter()
        .map(|row| row.cost_yuan.expect("active pricing in historical ledger"))
        .sum();
    let answered = 2 + hook_calls;
    let followup = tokio::time::timeout(
        Duration::from_secs(8),
        chat.run_in_session(
            "Reply briefly without reading any files.",
            Some(&session),
            None,
            None,
            Vec::new(),
            None,
            false,
            Some("audit-hook-usage-next"),
            |_| {},
            |_| {},
        ),
    )
    .await
    .expect("follow-up budget admission must finish");
    let observed = Observation {
        answered,
        hook_calls,
        summary,
        ledger_cost,
        event_cost,
        live_input,
        durable_input,
        raw_calls,
        expected_cost: unit_cost * answered as f64,
        limit,
        followup_error: followup.err().map(|error| error.to_string()),
        followup_dispatches: model.parent.lock().unwrap().len() - 2,
    };
    eprintln!("hook usage audit: kind={kind:?}, observed={observed:?}");
    observed
}

fn assert_all_answered_usage(observed: Observation) {
    let expected_input = observed.answered as u64 * 1000;
    let blocked = observed
        .followup_error
        .as_deref()
        .is_some_and(|error| error.contains("daily budget exhausted"));
    let expected_blocked = observed.expected_cost >= observed.limit;
    assert!(
        observed.summary.input_tokens == expected_input
            && observed.summary.output_tokens == observed.answered as u64 * 100
            && observed.summary.cache_hit_tokens == observed.answered as u64 * 800
            && observed.summary.cache_miss_tokens == observed.answered as u64 * 200
            && (observed.summary.session_cost - observed.expected_cost).abs() < 1e-12
            && (observed.ledger_cost - observed.expected_cost).abs() < 1e-12
            && (observed.event_cost - observed.expected_cost).abs() < 1e-12
            && blocked == expected_blocked
            && observed.followup_dispatches == usize::from(!expected_blocked),
        "all answered hook requests must own ledger usage and next admission: {observed:?}"
    );
    // Capture observability separately; a correct side-call design need not fold
    // child usage into the parent's context-size gauge or main-only raw history.
    assert!(observed.live_input > 0 && observed.durable_input > 0 && observed.raw_calls > 0);
    assert!(observed.hook_calls <= observed.answered);
}

#[tokio::test]
async fn no_model_hook_records_answered_usage() {
    assert_all_answered_usage(observe(HookKind::None, 2.5).await);
}

#[tokio::test]
async fn unmatched_model_hook_does_not_create_usage() {
    assert_all_answered_usage(observe(HookKind::UnmatchedAgent, 2.5).await);
}

#[tokio::test]
async fn counted_main_usage_blocks_the_next_admission() {
    assert_all_answered_usage(observe(HookKind::None, 1.5).await);
}

#[tokio::test]
#[ignore = "audit: prompt hook usage must reach the existing ledger and budget admission"]
async fn audit_prompt_hook_usage_reaches_ledger_and_budget() {
    assert_all_answered_usage(observe(HookKind::Prompt, 2.5).await);
}

#[tokio::test]
#[ignore = "audit: agent hook calls must reach the existing ledger and budget admission"]
async fn audit_agent_hook_usage_reaches_ledger_and_budget() {
    assert_all_answered_usage(observe(HookKind::Agent, 2.5).await);
}

#[tokio::test]
#[ignore = "audit: a denied parent tool must not erase prompt-hook spend"]
async fn audit_prompt_hook_denial_still_accounts_usage() {
    assert_all_answered_usage(observe(HookKind::PromptDeny, 2.5).await);
}

#[tokio::test]
#[ignore = "audit: a denied parent tool must not erase agent-hook spend"]
async fn audit_agent_hook_denial_still_accounts_usage() {
    assert_all_answered_usage(observe(HookKind::AgentDeny, 2.5).await);
}
