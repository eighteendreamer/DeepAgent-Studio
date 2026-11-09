//! H-116: real command hooks, the ordinary kernel, and cold SQLite recovery.
#![cfg(windows)]

use async_trait::async_trait;
use deepagent_core::{
    clock::FixedClock,
    error::{CoreError, Result},
    message::Message,
    response_item::ResponseItem,
    task::TaskState,
};
use deepagent_hooks::{
    Hook, HookContext, HookDefinitions, HookOutcome, HookPoint, HookRegistry, SystemHookRunner,
};
use deepagent_persistence::{event_store::EventStore, run_store::RunStore, Database};
use deepagent_runtime::{
    agent::RunUsage, Agent, AgentDecision, AgentKernel, CancellationTree, Observation, RunRequest,
    RuntimeConfig, RuntimeEvent, RuntimeEventSink, TerminalKind,
};
use deepagent_session::Session;
use deepagent_tools::ToolRegistry;
use deepagent_tracing::metrics::Metrics;
use serde_json::Value;
use std::{path::Path, sync::Arc, sync::Mutex};

const ANSWER: &str = "H116 synthetic answer";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Case {
    Text,
    Message,
    Native,
    ZeroSteps,
    OneStep,
    Approval,
    TokenBudget,
    PlanDenied,
    Cancelled,
    AgentError,
    FinalHookError,
}

impl Case {
    fn terminal(self) -> TerminalKind {
        match self {
            Self::Text | Self::Message | Self::Native => TerminalKind::Succeeded,
            Self::ZeroSteps | Self::OneStep => TerminalKind::MaxTurns,
            Self::Approval => TerminalKind::Blocked,
            Self::TokenBudget => TerminalKind::BudgetExceeded,
            Self::Cancelled => TerminalKind::Cancelled,
            _ => TerminalKind::Failed,
        }
    }

    fn task_state(self) -> TaskState {
        match self {
            // A final hook error occurs after the engine's Completed transition;
            // the existing kernel still commits a failed run, not a success.
            Self::Text | Self::Message | Self::Native | Self::FinalHookError => {
                TaskState::Completed
            }
            Self::Approval => TaskState::WaitingApproval,
            _ => TaskState::Failed,
        }
    }

    fn completion_calls(self) -> usize {
        usize::from(matches!(
            self,
            Self::Text | Self::Message | Self::Native | Self::FinalHookError
        ))
    }
}

struct CaseAgent {
    case: Case,
    cancellation: CancellationTree,
    thinks: usize,
}

#[async_trait]
impl Agent for CaseAgent {
    async fn think(&mut self, _: usize, _: &[Observation]) -> Result<AgentDecision> {
        self.thinks += 1;
        Ok(match self.case {
            Case::ZeroSteps | Case::OneStep => AgentDecision::Continue,
            Case::Approval => AgentDecision::NeedsApproval("H116 synthetic input".into()),
            Case::Message => AgentDecision::CompleteMessage(Message::assistant(ANSWER)),
            Case::Native => AgentDecision::CompleteItems {
                message: Message::assistant(ANSWER),
                items: vec![ResponseItem::Message {
                    role: "assistant".into(),
                    content: ANSWER.into(),
                }],
            },
            Case::Cancelled => {
                self.cancellation.cancel();
                AgentDecision::Complete(ANSWER.into())
            }
            Case::AgentError => return Err(CoreError::other("H116 synthetic agent error")),
            _ => AgentDecision::Complete(ANSWER.into()),
        })
    }

    fn cumulative_usage(&self) -> Option<RunUsage> {
        (self.case == Case::TokenBudget && self.thinks > 0).then_some(RunUsage {
            prompt_tokens: 2,
            total_tokens: 2,
            billed_prompt_tokens: 2,
            ..RunUsage::default()
        })
    }
}

struct PolicyHook {
    deny_plan: bool,
}

#[async_trait]
impl Hook for PolicyHook {
    fn name(&self) -> &str {
        "h116-policy"
    }

    async fn run(&self, _: &HookContext) -> Result<HookOutcome> {
        if self.deny_plan {
            Ok(HookOutcome::deny("H116 synthetic plan denial"))
        } else {
            Err(CoreError::other("H116 synthetic final hook error"))
        }
    }
}

struct Live {
    db: Arc<Database>,
    run: String,
    events: Mutex<Vec<RuntimeEvent>>,
}

impl RuntimeEventSink for Live {
    fn emit(&self, event: RuntimeEvent) {
        if event.is_terminal() {
            let store = RunStore::new(&self.db);
            assert!(store.get(&self.run).unwrap().unwrap().finished_at.is_some());
            assert_eq!(
                store
                    .events_after(&self.run, None)
                    .unwrap()
                    .iter()
                    .filter(|event| event.event_type == "run_terminal")
                    .count(),
                1,
                "live delivery must follow the canonical terminal commit"
            );
        }
        self.events.lock().unwrap().push(event);
    }
}

fn recorded(path: &Path) -> Vec<Value> {
    if !path.exists() {
        return vec![];
    }
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

async fn exercise(case: Case) -> usize {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("h116.sqlite");
    let created = root.path().join("created.jsonl");
    let completed = root.path().join("completed.jsonl");
    let command = |path: &Path| {
        let quoted = path.to_string_lossy().replace('\'', "''");
        serde_json::json!({"hooks":[{"type":"command","shell":"powershell","timeout":5,
            "command":format!("$payload = [Console]::In.ReadToEnd(); [IO.File]::AppendAllText('{quoted}', $payload + [Environment]::NewLine)")}]})
    };
    let definitions = HookDefinitions::parse(
        &serde_json::json!({"hooks":{
            "TaskCreated":[command(&created)], "TaskCompleted":[command(&completed)]
        }})
        .to_string(),
    )
    .unwrap();
    let mut hooks = HookRegistry::new();
    assert_eq!(
        definitions.register_into(&mut hooks, Arc::new(SystemHookRunner)),
        2
    );
    if matches!(case, Case::PlanDenied | Case::FinalHookError) {
        hooks.register(
            if case == Case::PlanDenied {
                HookPoint::BeforePlan
            } else {
                HookPoint::TaskCompleted
            },
            Arc::new(PolicyHook {
                deny_plan: case == Case::PlanDenied,
            }),
        );
    }
    let clock = FixedClock::new(1);
    let run = format!("h116-{case:?}");
    let (id, task, record, durable, history) = {
        let db = Arc::new(Database::open(&path).unwrap());
        let mut session = Session::create(&db, &clock, Some(&run)).unwrap();
        let task = session.create_task(&run).unwrap();
        let registry = ToolRegistry::new();
        let cancellation = CancellationTree::new();
        let live = Arc::new(Live {
            db: db.clone(),
            run: run.clone(),
            events: Mutex::new(Vec::new()),
        });
        let config = RuntimeConfig {
            max_steps: match case {
                Case::ZeroSteps => Some(0),
                Case::OneStep => Some(1),
                _ => None,
            },
            max_total_tokens: (case == Case::TokenBudget).then_some(1),
            ..RuntimeConfig::default()
        };
        let mut agent = CaseAgent {
            case,
            cancellation: cancellation.clone(),
            thinks: 0,
        };
        let kernel = AgentKernel::new(db.clone(), &registry, Metrics::new(), config, &run)
            .with_hooks(&hooks)
            .with_cancellation(cancellation)
            .with_events(live.clone());
        let terminal = kernel
            .start(RunRequest::new(&mut session, task, &mut agent))
            .await
            .unwrap();
        assert_eq!(terminal.kind, case.terminal(), "{case:?}");
        assert_eq!(session.state().task(task).unwrap().state, case.task_state());
        let record = RunStore::new(&db).get(&run).unwrap().unwrap();
        assert_eq!(
            record.terminal_kind.as_deref(),
            Some(case.terminal().label())
        );
        let durable = RunStore::new(&db).events_after(&run, None).unwrap();
        assert!(durable.windows(2).all(|p| p[0].sequence < p[1].sequence));
        assert_eq!(
            durable
                .iter()
                .filter(|e| e.event_type == "run_terminal")
                .count(),
            1
        );
        let live = live.events.lock().unwrap();
        let terminals: Vec<_> = live.iter().filter(|event| event.is_terminal()).collect();
        assert_eq!(terminals.len(), 1);
        let notification = match case.terminal() {
            TerminalKind::Succeeded => "run_completed",
            TerminalKind::Cancelled => "run_cancelled",
            TerminalKind::Blocked => "run_awaiting_approval",
            _ => "run_failed",
        };
        assert_eq!(terminals[0].label(), notification);
        assert_eq!(
            durable
                .iter()
                .filter(|e| e.event_type == notification)
                .count(),
            1
        );
        if case.terminal() != TerminalKind::Succeeded {
            assert!(!durable.iter().any(|e| e.event_type == "run_completed"));
        }
        let history = EventStore::new(&db).load_session(session.id()).unwrap();
        (session.id(), task, record, durable, history)
    }; // All kernel/session/live/database handles are released before reopening.
    let before = (recorded(&created), recorded(&completed));
    assert_eq!(
        before.0.len(),
        1,
        "prove the actual command runner: {case:?}"
    );
    for (payloads, event) in [(&before.0, "TaskCreated"), (&before.1, "TaskCompleted")] {
        for payload in payloads {
            assert_eq!(payload["hook_event_name"], event);
            assert_eq!(payload["session_id"], id.to_string());
            assert_eq!(payload["task_id"], task.to_string());
            assert_eq!(payload["task_subject"], run);
        }
    }
    let reopened = Database::open(&path).unwrap();
    assert_eq!(RunStore::new(&reopened).get(&run).unwrap().unwrap(), record);
    assert_eq!(
        RunStore::new(&reopened).events_after(&run, None).unwrap(),
        durable
    );
    assert_eq!(
        EventStore::new(&reopened).load_session(id).unwrap(),
        history
    );
    let session = Session::recover(&reopened, &clock, id).unwrap();
    assert_eq!(session.state().task(task).unwrap().state, case.task_state());
    assert_eq!((recorded(&created), recorded(&completed)), before);
    eprintln!(
        "{case:?}: terminal={:?}, command_completions={}",
        case.terminal(),
        before.1.len()
    );
    before.1.len()
}

#[tokio::test]
async fn h116_successful_completion_variants_fire_once_and_survive_reopen() {
    for case in [Case::Text, Case::Message, Case::Native] {
        assert_eq!(exercise(case).await, case.completion_calls());
    }
}

#[tokio::test]
async fn h116_unsuccessful_exits_do_not_announce_completion() {
    let mut unexpected = vec![];
    for case in [
        Case::ZeroSteps,
        Case::OneStep,
        Case::Approval,
        Case::TokenBudget,
        Case::PlanDenied,
    ] {
        if exercise(case).await != case.completion_calls() {
            unexpected.push(case);
        }
    }
    assert!(
        unexpected.is_empty(),
        "invalid TaskCompleted dispatch: {unexpected:?}"
    );
}

#[tokio::test]
async fn h116_true_cancellation_and_agent_errors_preserve_their_terminal_without_completion() {
    for case in [Case::Cancelled, Case::AgentError] {
        assert_eq!(exercise(case).await, case.completion_calls());
    }
}

#[tokio::test]
async fn h116_final_hook_error_still_commits_one_failure_not_a_success() {
    let case = Case::FinalHookError;
    assert_eq!(exercise(case).await, case.completion_calls());
}
