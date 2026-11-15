//! H-113: production file tools -> checkpoint evidence -> external command wire.
//! Each case uses a fresh run checkpoint; this does not redefine run-wide diff semantics.
#![cfg(windows)]

use std::sync::Arc;

use deepagent_builtins::{file_tools, WorkspaceRoot};
use deepagent_core::{clock::Timestamp, event::EventPayload, id::SessionId};
use deepagent_hooks::{HookDefinitions, HookRegistry, SystemHookRunner};
use deepagent_persistence::{
    checkpoint_store::CheckpointStore,
    event_store::EventStore,
    run_control::{RunActionState, RunControlStore},
    run_store::RunStore,
    Database,
};
use deepagent_runtime::{CheckpointManager, ToolActionPersistence, ToolExecutionPipeline};
use deepagent_tools::{PermissionSet, ToolInvocation, ToolRegistry};
use serde_json::{json, Value};

#[derive(Clone, Copy, Debug)]
enum Case {
    Create,
    Modify,
    Unchanged,
    Delete,
    Move,
    FailedEdit,
    ReadOnly,
    PermissionDenied,
}

async fn observe(case: Case) {
    let temporary = tempfile::tempdir().unwrap();
    let workspace = temporary.path().join("工作区");
    std::fs::create_dir(&workspace).unwrap();
    let source = workspace.join("文件.txt");
    let destination = workspace.join("移动目标.txt");
    let seeded = !matches!(case, Case::Create | Case::PermissionDenied);
    if seeded {
        std::fs::write(&source, "before").unwrap();
    }
    let log = temporary.path().join("hook.jsonl");
    let quoted = log.to_string_lossy().replace('\'', "''");
    let command = format!(
        "$r=[IO.StreamReader]::new([Console]::OpenStandardInput(),[Text.Encoding]::UTF8); \
         $p=$r.ReadToEnd() | ConvertFrom-Json; \
         $exists=[IO.File]::Exists($p.file_path); \
         $content=if($exists){{[IO.File]::ReadAllText($p.file_path)}}else{{$null}}; \
         $entry=@{{payload=$p;exists=$exists;content=$content}} | ConvertTo-Json -Compress -Depth 12; \
         [IO.File]::AppendAllText('{quoted}',$entry+[Environment]::NewLine)"
    );
    let definitions = HookDefinitions::parse(
        &json!({"hooks":{"FileChanged":[{"hooks":[{
            "type":"command","shell":"powershell","timeout":5,"command":command
        }]}]}})
        .to_string(),
    )
    .unwrap();
    let mut hooks = HookRegistry::new();
    assert_eq!(
        definitions.register_into(&mut hooks, Arc::new(SystemHookRunner)),
        1
    );

    let db_path = temporary.path().join("state.sqlite");
    let session_id = SessionId::new();
    let expected_ok = !matches!(case, Case::FailedEdit | Case::PermissionDenied);
    let checkpoint_before_restart;
    {
        let db = Arc::new(Database::open(&db_path).unwrap());
        let events = EventStore::new(&db);
        events
            .create_session(session_id, None, Timestamp::EPOCH)
            .unwrap();
        events
            .append(
                session_id,
                EventPayload::SessionStarted {
                    title: None,
                    mode: Default::default(),
                },
                Timestamp::EPOCH,
            )
            .unwrap();
        RunStore::new(&db)
            .create("h113-file-wire", &session_id.to_string(), None, 1)
            .unwrap();
        let checkpoint = Arc::new(
            CheckpointManager::new(
                db.clone(),
                "h113-file-wire",
                0,
                &workspace,
                temporary.path().join("checkpoint"),
            )
            .unwrap(),
        );
        let mut registry = ToolRegistry::new();
        for tool in file_tools(WorkspaceRoot::new(&workspace)) {
            registry.register(tool).unwrap();
        }
        let permissions = if matches!(case, Case::PermissionDenied) {
            PermissionSet::read_only()
        } else {
            PermissionSet::developer()
        };
        let pipeline = ToolExecutionPipeline::new(&registry, session_id, permissions)
            .with_hooks(Some(&hooks))
            .with_checkpoint(Some(checkpoint.clone()))
            .with_action_persistence(Some(
                ToolActionPersistence::new(db.clone(), "h113-file-wire", "h113-turn").unwrap(),
            ))
            .with_auto_approve(true);
        // Exercise the real read-before-write authority, not a probe tool bypass.
        if matches!(case, Case::Modify | Case::Unchanged | Case::FailedEdit) {
            let read = pipeline
                .execute_registered(
                    ToolInvocation::new("read_file", json!({"path":source})).with_id("h113-read"),
                )
                .await
                .unwrap();
            assert!(read.output.ok, "{case:?}: {:?}", read.output);
        }
        let (name, args) = match case {
            Case::Create | Case::Modify | Case::PermissionDenied => {
                ("write_file", json!({"path":source,"content":"after"}))
            }
            Case::Unchanged => ("write_file", json!({"path":source,"content":"before"})),
            Case::Delete => ("delete_path", json!({"path":source})),
            Case::Move => (
                "move_path",
                json!({"source":source,"destination":destination}),
            ),
            Case::FailedEdit => (
                "edit_file",
                json!({"path":source,"old":"not-present","new":"after"}),
            ),
            Case::ReadOnly => ("read_file", json!({"path":source})),
        };
        let result = pipeline
            .execute_registered(ToolInvocation::new(name, args).with_id("h113-action"))
            .await
            .unwrap();
        assert_eq!(
            result.output.ok, expected_ok,
            "{case:?}: {:?}",
            result.output
        );
        checkpoint.commit().unwrap();
        checkpoint_before_restart = CheckpointStore::new(&db)
            .list_for_run("h113-file-wire")
            .unwrap();
    }
    let expected: Vec<(&std::path::Path, &str, Option<&str>)> = match case {
        Case::Create => vec![(&source, "add", Some("after"))],
        Case::Modify => vec![(&source, "change", Some("after"))],
        Case::Delete => vec![(&source, "unlink", None)],
        Case::Move => vec![
            (&source, "unlink", None),
            (&destination, "add", Some("before")),
        ],
        _ => Vec::new(),
    };
    let observed: Vec<Value> = if log.exists() {
        std::fs::read_to_string(&log)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    } else {
        Vec::new()
    };
    assert_eq!(observed.len(), expected.len(), "{case:?}: {observed:?}");
    for (entry, (path, kind, content)) in observed.iter().zip(expected) {
        assert_eq!(entry["payload"]["hook_event_name"], "FileChanged");
        assert_eq!(entry["payload"]["session_id"], session_id.to_string());
        assert_eq!(
            entry["payload"]["file_path"],
            path.to_string_lossy().as_ref()
        );
        assert_eq!(entry["payload"]["event"], kind);
        assert_eq!(entry["exists"], content.is_some());
        assert_eq!(entry["content"], json!(content));
    }
    match case {
        Case::Create | Case::Modify => {
            assert_eq!(std::fs::read_to_string(&source).unwrap(), "after")
        }
        Case::Delete | Case::Move | Case::PermissionDenied => assert!(!source.exists()),
        _ => assert_eq!(std::fs::read_to_string(&source).unwrap(), "before"),
    }
    if matches!(case, Case::Move) {
        assert_eq!(std::fs::read_to_string(destination).unwrap(), "before");
    }
    let reopened = Database::open(&db_path).unwrap();
    assert_eq!(
        CheckpointStore::new(&reopened)
            .list_for_run("h113-file-wire")
            .unwrap(),
        checkpoint_before_restart
    );
    let action = RunControlStore::new(&reopened)
        .get_action("h113-file-wire", "h113-action")
        .unwrap()
        .unwrap();
    assert_eq!(
        action.state,
        if expected_ok {
            RunActionState::Completed
        } else {
            RunActionState::Failed
        }
    );
    eprintln!(
        "H113 {case:?}: hook_count={}, durable_action={:?}",
        observed.len(),
        action.state
    );
}

#[tokio::test]
async fn h113_production_file_tools_deliver_truthful_command_wire_and_reopen() {
    for case in [
        Case::Create,
        Case::Modify,
        Case::Unchanged,
        Case::Delete,
        Case::Move,
        Case::FailedEdit,
        Case::ReadOnly,
        Case::PermissionDenied,
    ] {
        observe(case).await;
    }
}
