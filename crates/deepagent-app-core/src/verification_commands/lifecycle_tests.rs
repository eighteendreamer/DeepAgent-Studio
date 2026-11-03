//! H-64 production assembly boundary. Only this fixture shortens task_timeout;
//! ChatService's public API and the production timeout defaults are unchanged.

use super::*;
use crate::kernel_runtime::{build_kernel_runtime_config, KernelRuntimeConfigRequest};
use crate::settings::VerificationPolicy;
use deepagent_builtins::{BashTool, PlanMode, SystemExecutor, TodoStore};
use deepagent_core::clock::FixedClock;
use deepagent_models::transport::MockTransport;
use deepagent_models::{ModelClient, ModelConfig};
use deepagent_persistence::run_control::{RunActionState, RunControlStore};
use deepagent_persistence::{run_store::RunStore, Database};
use deepagent_runtime::{AgentKernel, AutoApproveGate, ModelAgent, RunRequest, TerminalKind};
use deepagent_session::Session;
use deepagent_tracing::metrics::Metrics;
use deepagent_verification::VerificationStep;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

#[path = "../../tests/fixtures/verification_build.rs"]
mod fixture;
use fixture::BuildFixture;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stop {
    None,
    Cancel,
    Deadline,
}

async fn observe(stop: Stop, parent_exits: bool) {
    observe_fixture(BuildFixture::new().await, stop, parent_exits)
        .await
        .unwrap();
}

async fn observe_fixture(
    fixture: BuildFixture,
    stop: Stop,
    parent_exits: bool,
) -> Result<(), String> {
    let db_path = fixture.root.join("run.sqlite");
    let db = Arc::new(Database::open(&db_path).unwrap());
    let clock = FixedClock::new(1);
    let mut session = Session::create(&db, &clock, Some("build lifecycle")).unwrap();
    let task = session.create_task("create answer.txt").unwrap();
    let run_id = "verification-kernel-lifecycle";
    RunStore::new(&db)
        .create(
            run_id,
            &session.id().to_string(),
            Some(&task.to_string()),
            1,
        )
        .unwrap();
    let mut registry = ToolRegistry::new();
    registry
        .register(Arc::new(
            BashTool::new(SystemExecutor, fixture.root.to_string_lossy(), vec![])
                .with_external_safety_gate(true),
        ))
        .unwrap();
    let registry = Arc::new(registry);
    let hooks = Arc::new(HookRegistry::new());
    let cancellation = CancellationTree::new();
    let approvals = Arc::new(AutoApproveGate);
    let runtime = build_kernel_runtime_config(KernelRuntimeConfigRequest {
        db: db.clone(),
        run_id,
        session_sequence: 0,
        root: &fixture.root,
        tool_results_dir: &fixture.root.join("tool-results"),
        plan: PlanMode::new(),
        todo_store: TodoStore::new(),
        verification_policy: VerificationPolicy::Enabled,
        fire_session_start: false,
        granted: deepagent_tools::PermissionSet::developer(),
        verification_context: VerificationCommandContext {
            registry: registry.clone(),
            session_id: session.id(),
            hooks: hooks.clone(),
            approvals: approvals.clone(),
            cancellation: cancellation.clone(),
        },
        nested_instructions: None,
    })
    .unwrap();
    let mut config = runtime.config;
    config.task_timeout = (stop == Stop::Deadline).then_some(Duration::from_secs(5));
    let answer = fixture.root.join("answer.txt");
    runtime.checkpoint.capture_before(&answer).unwrap();
    std::fs::write(&answer, "real mutation evidence").unwrap();
    let mut plan = crate::completion_plan::discover_verification_plan(
        &fixture.root,
        runtime.verification_commands.clone(),
    )
    .unwrap();
    // A real second step must run normally, but not after interruption.
    plan.steps.push(VerificationStep::new(
        "second step",
        Command::new("cargo", ["--version".into()])
            .in_dir(fixture.root.to_string_lossy().into_owned()),
        true,
    ));
    let kernel =
        AgentKernel::<FixedClock>::new(db.clone(), &registry, Metrics::new(), config, run_id)
            .with_hooks(&hooks)
            .with_approvals(approvals)
            .with_cancellation(cancellation)
            .with_verification(&plan);
    runtime
        .verification_commands
        .bind_events(kernel.event_sink())
        .unwrap();
    let mut agent = ModelAgent::new(
        Arc::new(ModelClient::new(
            Arc::new(MockTransport::new([
                r#"{"type":"response.output_text.delta","delta":"Created answer.txt."}"#.into(),
                r#"{"type":"response.completed","response":{"id":"fixture-response","object":"response","created_at":1,"model":"deepseek-flash","output":[],"status":"completed"}}"#.into(),
            ])),
            ModelConfig::deepseek("offline-placeholder"),
        )),
        "deepseek-flash",
        "system",
        "create answer.txt",
        vec![],
    );
    let exited = AtomicBool::new(false);
    let run = async {
        let result = kernel
            .start(RunRequest::new(&mut session, task, &mut agent))
            .await;
        exited.store(true, Ordering::Release);
        result
    };
    let observe = async {
        let processes = match fixture.try_started().await {
            Ok(processes) => processes,
            Err(error) => {
                // Preserve the failing observation, then settle the SAME run.
                // Panicking here used to drop join! before its result/actions
                // could distinguish command failure from compiler readiness.
                let before = RunStore::new(&db).get(run_id).unwrap();
                let actions = RunControlStore::new(&db).list_actions(run_id).unwrap();
                let event_types: Vec<_> = RunStore::new(&db)
                    .events_after(run_id, None)
                    .unwrap()
                    .into_iter()
                    .map(|event| event.event_type)
                    .collect();
                let accepted = kernel.handle().cancel();
                let settled = tokio::time::timeout(Duration::from_secs(2), async {
                    while !exited.load(Ordering::Acquire) {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                })
                .await
                .is_ok();
                fixture.release();
                return Err(format!(
                    "{error}; stop={stop:?} parent_exits={parent_exits} before={before:?} \
                     actions={actions:?} event_types={event_types:?} \
                     cleanup_cancel_accepted={accepted} cleanup_settled={settled}"
                ));
            }
        };
        if parent_exits {
            fixture.return_parent(&processes).await;
        }
        assert!(RunStore::new(&db)
            .get(run_id)
            .unwrap()
            .unwrap()
            .terminal_kind
            .is_none());
        if stop == Stop::Cancel {
            assert!(kernel.handle().cancel());
        }
        let stopped = if stop == Stop::None {
            tokio::time::sleep(Duration::from_millis(100)).await;
            exited.load(Ordering::Acquire)
        } else {
            tokio::time::timeout(
                Duration::from_secs(if stop == Stop::Deadline { 6 } else { 2 }),
                async {
                    while !exited.load(Ordering::Acquire) {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                },
            )
            .await
            .is_ok()
        };
        let native_exited = processes.exited();
        let before = RunStore::new(&db)
            .get(run_id)
            .unwrap()
            .unwrap()
            .terminal_kind;
        fixture.release();
        processes.settle().await;
        Ok((stopped, native_exited, before))
    };
    let (terminal, observed) = tokio::time::timeout(Duration::from_secs(12), async {
        tokio::join!(run, observe)
    })
    .await
    .expect("finite kernel and real processes must settle");
    let (stopped, native_exited, before) = match observed {
        Ok(observed) => observed,
        Err(error) => {
            let reopened = Database::open(&db_path).unwrap();
            let record = RunStore::new(&reopened).get(run_id).unwrap();
            let actions = RunControlStore::new(&reopened)
                .list_actions(run_id)
                .unwrap();
            let actions_terminal = actions.iter().all(|action| action.state.is_terminal());
            return Err(format!(
                "{error}; settled_run_result={terminal:?} settled_record={record:?} \
                 settled_actions_terminal={actions_terminal}"
            ));
        }
    };
    let terminal = terminal.unwrap();
    let expected = match stop {
        Stop::None => TerminalKind::Succeeded,
        Stop::Cancel => TerminalKind::Cancelled,
        Stop::Deadline => TerminalKind::BudgetExceeded,
    };
    assert_eq!(terminal.kind, expected);
    // Re-open the actual SQLite file, not just the live connection.
    let reopened = Database::open(&db_path).unwrap();
    let store = RunStore::new(&reopened);
    let record = store.get(run_id).unwrap().unwrap();
    let expected_label = match stop {
        Stop::None => "succeeded",
        Stop::Cancel => "cancelled",
        Stop::Deadline => "budget_exceeded",
    };
    assert_eq!(record.terminal_kind.as_deref(), Some(expected_label));
    assert_eq!(
        store
            .events_after(run_id, None)
            .unwrap()
            .iter()
            .filter(|e| e.event_type == "run_terminal")
            .count(),
        1
    );
    let actions = RunControlStore::new(&reopened)
        .list_actions(run_id)
        .unwrap();
    assert!(actions.iter().all(|a| a.state.is_terminal()), "{actions:?}");
    // Verifier may record an already-cancelled second check, but it cannot run.
    let executed = actions.iter().filter(|a| a.started_at.is_some()).count();
    assert_eq!(executed, if stop == Stop::None { 2 } else { 1 });
    let first = &actions[0];
    assert_eq!(
        first.state,
        if stop == Stop::None {
            RunActionState::Completed
        } else {
            RunActionState::Cancelled
        }
    );
    fixture.assert_compiler_effects(stop != Stop::None);
    eprintln!("verification-kernel stop={stop:?} parent_exits={parent_exits} stopped_before_release={stopped} native_exited_before_release={native_exited} before={before:?} wrote={} executed={executed} actions={}", fixture.wrote(), actions.len());
    if stop == Stop::None {
        assert!(!stopped && !native_exited && before.is_none());
        assert!(fixture.wrote());
    } else {
        assert!(
            stopped && native_exited,
            "run must own both real process exits"
        );
        assert_eq!(before.as_deref(), Some(expected_label));
        assert!(!fixture.wrote(), "descendant side effect survived terminal");
    }
    Ok(())
}

#[tokio::test]
async fn production_completion_build_normal_cancel_and_deadline() {
    for stop in [Stop::None, Stop::Cancel, Stop::Deadline] {
        observe(stop, false).await;
    }
}

#[tokio::test]
async fn production_build_parent_exit_with_inherited_pipes_still_owns_worker() {
    for stop in [Stop::None, Stop::Cancel, Stop::Deadline] {
        observe(stop, true).await;
    }
}

#[tokio::test]
async fn production_cold_compiler_gate_normal_cancel_and_deadline() {
    for stop in [Stop::None, Stop::Cancel, Stop::Deadline] {
        observe_fixture(BuildFixture::cold().await, stop, false)
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn readiness_failure_retains_kernel_result_and_settles_actions() {
    let fixture = BuildFixture::cold().await;
    // A deterministic failure BEFORE rustc-wrapper dispatch; unlike the
    // historical timeout, this injected cause is known and must stay an error.
    std::fs::write(fixture.root.join("Cargo.toml"), "not a valid manifest\n").unwrap();
    let error = observe_fixture(fixture, Stop::None, false)
        .await
        .expect_err("a build that never reaches the gate must not pass readiness");
    assert!(
        error.contains("real build must reach compiler-started"),
        "{error}"
    );
    assert!(error.contains("compiler_stages=[]"), "{error}");
    assert!(error.contains("tool_completed"), "{error}");
    assert!(error.contains("cleanup_settled=true"), "{error}");
    assert!(error.contains("settled_run_result=Ok("), "{error}");
    assert!(error.contains("settled_actions_terminal=true"), "{error}");
}

#[tokio::test]
async fn cold_query_progress_remains_visible_when_compiler_gate_is_not_reached() {
    let fixture = BuildFixture::cold().await;
    let wrapper = fixture.root.parent().unwrap().join("compiler-wrapper.exe");
    let command = format!(
        "& {} rustc --version",
        quote_argument(wrapper.to_str().unwrap())
    );
    let output = SystemExecutor
        .run(&command, fixture.root.to_str().unwrap())
        .await
        .unwrap();
    assert_eq!(output.exit_code, Some(0), "{output:?}");
    assert!(!fixture.root.join("target").exists());
    assert!(!fixture.root.join("compiler-started").exists());
    let error = match fixture.try_started().await {
        Ok(_) => panic!("a version query must not reach the build-script gate"),
        Err(error) => error,
    };
    assert!(error.contains("real build must reach compiler-started"));
    assert!(error.contains("query completed exit=Some(0)"), "{error}");
    assert!(error.contains("cargo_lock=false"), "{error}");
    assert!(!fixture.wrote());
}

#[tokio::test]
async fn dropping_active_verification_future_stops_real_descendant() {
    for build in [BuildFixture::new().await, BuildFixture::cold().await] {
        drop_active_fixture(build).await;
    }
}

async fn drop_active_fixture(build: BuildFixture) {
    let commands = super::tests::fixture(
        &build.root,
        Arc::new(SystemExecutor),
        deepagent_hooks::HookOutcome::Continue,
    );
    let command = Command::new(
        "cargo",
        ["check".into(), "--offline".into(), "--quiet".into()],
    )
    .in_dir(build.root.to_string_lossy().into_owned());
    let mut run = Box::pin(CommandRunner::run(&commands, &command));
    let processes = tokio::select! {
        result = &mut run => panic!("real build returned before release: {result:?}"),
        processes = build.try_started() => processes.unwrap(),
    };
    // Drop the owned future itself, not a Pin<&mut Future> reference.
    drop(run);
    let stopped = tokio::time::timeout(Duration::from_secs(2), async {
        while !processes.exited() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .is_ok();
    build.release();
    processes.settle().await;
    eprintln!(
        "verification-drop native_exited_before_release={stopped} wrote={}",
        build.wrote()
    );
    assert!(
        stopped,
        "dropping the adapter must not detach real build descendants"
    );
    assert!(!build.wrote());
    build.assert_compiler_effects(true);
}

#[tokio::test]
async fn post_edit_default_timeout_stops_real_descendant() {
    for build in [BuildFixture::new().await, BuildFixture::cold().await] {
        timeout_fixture(build).await;
    }
}

async fn timeout_fixture(build: BuildFixture) {
    let commands = super::tests::fixture(
        &build.root,
        Arc::new(SystemExecutor),
        deepagent_hooks::HookOutcome::Continue,
    );
    let dispatcher = crate::verification_dispatcher::VerificationDispatcher::standard_with_executor(
        Arc::new(commands),
    );
    let file = build.root.join("src/lib.rs");
    let mut run = Box::pin(dispatcher.verify_file(&file));
    let processes = tokio::select! {
        result = &mut run => panic!("real post-edit build returned before release: {result:?}"),
        processes = build.try_started() => processes.unwrap(),
    };
    let result = tokio::time::timeout(Duration::from_secs(6), &mut run).await;
    let stopped = tokio::time::timeout(Duration::from_secs(2), async {
        while !processes.exited() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .is_ok();
    build.release();
    processes.settle().await;
    assert_eq!(
        result.unwrap(),
        crate::verification_dispatcher::VerificationOutcome::TimedOut
    );
    eprintln!(
        "verification-post-edit-timeout native_exited_before_release={stopped} wrote={}",
        build.wrote()
    );
    assert!(
        stopped,
        "the dispatcher's unchanged default timeout must own subprocess cleanup"
    );
    assert!(!build.wrote());
    build.assert_compiler_effects(true);
}
