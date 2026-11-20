//! A workspace path must be checked after Windows junction resolution.

use super::*;
use deepagent_app_core::SandboxMode;
use std::process::Command;

const OUTSIDE_CANARY: &str = "AUDIT_JUNCTION_OUTSIDE_6c1e";

struct JunctionGuard(std::path::PathBuf);

impl Drop for JunctionGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir(&self.0);
    }
}

fn create_junction(path: &std::path::Path, target: &std::path::Path) -> JunctionGuard {
    let output = Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(path)
        .arg(target)
        .output()
        .expect("Windows cmd must be available for junction fixture");
    assert!(output.status.success(), "mklink /J failed: {output:?}");
    JunctionGuard(path.to_owned())
}

struct JunctionModel {
    tool: &'static str,
    args: serde_json::Value,
    marker: &'static str,
    calls: AtomicUsize,
    observed: Mutex<Option<bool>>,
    root_replacement: Mutex<Option<RootReplacement>>,
}

struct RootReplacement {
    logical: std::path::PathBuf,
    parked: std::path::PathBuf,
    outside: std::path::PathBuf,
    use_junction: bool,
    junction: Option<JunctionGuard>,
}

impl RootReplacement {
    fn apply(&mut self) {
        std::fs::rename(&self.logical, &self.parked).unwrap();
        if self.use_junction {
            self.junction = Some(create_junction(&self.logical, &self.outside));
        } else {
            std::fs::create_dir(&self.logical).unwrap();
            std::fs::write(self.logical.join("visible.txt"), OUTSIDE_CANARY).unwrap();
        }
    }
}

#[async_trait]
impl HttpTransport for JunctionModel {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let body: serde_json::Value = serde_json::from_str(&request.body).unwrap();
        let output = |call_id: &str| {
            body["input"]
                .as_array()
                .unwrap()
                .iter()
                .find(|item| item["type"] == "function_call_output" && item["call_id"] == call_id)
                .expect("paired tool output must be present")["output"]
                .as_str()
                .unwrap()
                .to_owned()
        };
        let events = match self.calls.fetch_add(1, Ordering::SeqCst) {
            0 => {
                if let Some(replacement) = self.root_replacement.lock().unwrap().as_mut() {
                    replacement.apply();
                }
                response_function_call("audit-junction-read", self.tool, self.args.clone())
            }
            1 => {
                let envelope: serde_json::Value =
                    serde_json::from_str(&output("audit-junction-read")).unwrap();
                *self.observed.lock().unwrap() =
                    Some(result_contains_marker(self.tool, &envelope, self.marker));
                response_text("Offline junction audit completed.")
            }
            _ => panic!("unexpected extra model request in junction audit"),
        };
        emit_responses_events(sink, &events)
    }
}

fn result_contains_marker(tool: &str, envelope: &serde_json::Value, marker: &str) -> bool {
    let result = &envelope["result"];
    match tool {
        "read_file" => result["content"]
            .as_str()
            .is_some_and(|text| text.contains(marker)),
        "list_dir" => result["entries"].as_array().is_some_and(|entries| {
            entries.iter().any(|entry| {
                entry["name"]
                    .as_str()
                    .is_some_and(|text| text.contains(marker))
            })
        }),
        "glob" => result["matches"].as_array().is_some_and(|entries| {
            entries
                .iter()
                .any(|entry| entry.as_str().is_some_and(|text| text.contains(marker)))
        }),
        "grep" => result["hits"].as_array().is_some_and(|entries| {
            entries.iter().any(|entry| {
                entry["text"]
                    .as_str()
                    .is_some_and(|text| text.contains(marker))
            })
        }),
        "write_file" => result["bytes"].as_u64() == Some(marker.len() as u64),
        "delete_path" => result["deleted"] == true,
        "move_path" => result["moved"] == true,
        _ => panic!("unknown junction fixture tool: {tool}"),
    }
}

#[test]
fn directory_marker_observation_ignores_echoed_query_and_error_text() {
    let query_only = serde_json::json!({"result":{"query":OUTSIDE_CANARY,"hits":[]}});
    assert!(!result_contains_marker("grep", &query_only, OUTSIDE_CANARY));
    let actual_hit =
        serde_json::json!({"result":{"query":OUTSIDE_CANARY,"hits":[{"text":OUTSIDE_CANARY}]}});
    assert!(result_contains_marker("grep", &actual_hit, OUTSIDE_CANARY));
    let error_only = serde_json::json!({"error":{"message":OUTSIDE_CANARY}});
    for tool in ["read_file", "list_dir", "glob", "grep"] {
        assert!(!result_contains_marker(tool, &error_only, OUTSIDE_CANARY));
    }
}

#[derive(Debug)]
struct ReadObservation {
    content_returned: bool,
    tool_completed_ok: Option<bool>,
    tool_blocked: bool,
}

async fn observe_read(
    workspace: &std::path::Path,
    requested_path: String,
    sandbox_mode: SandboxMode,
    run_id: &str,
) -> ReadObservation {
    observe_tool(
        workspace,
        "read_file",
        serde_json::json!({"path": requested_path}),
        OUTSIDE_CANARY,
        sandbox_mode,
        run_id,
    )
    .await
}

async fn observe_tool(
    workspace: &std::path::Path,
    tool: &'static str,
    args: serde_json::Value,
    marker: &'static str,
    sandbox_mode: SandboxMode,
    run_id: &str,
) -> ReadObservation {
    observe_tool_with_root_replacement(workspace, (tool, args, marker), sandbox_mode, run_id, None)
        .await
}

async fn observe_tool_with_root_replacement(
    workspace: &std::path::Path,
    request: (&'static str, serde_json::Value, &'static str),
    sandbox_mode: SandboxMode,
    run_id: &str,
    root_replacement: Option<RootReplacement>,
) -> ReadObservation {
    let (tool, args, marker) = request;
    let db = Arc::new(Database::open_in_memory().unwrap());
    let settings = Arc::new(SettingsService::new(
        db.clone(),
        Arc::new(MockTransport::with_get_json(
            r#"{"data":[{"id":"deepseek-flash","object":"model","owned_by":"deepseek"}]}"#,
        )),
        Arc::new(MemorySecretStore::new()),
    ));
    crate::registry_fixture::configure(&settings, "offline-audit-placeholder")
        .await
        .unwrap();
    settings.set_sandbox_mode(sandbox_mode).unwrap();
    if matches!(tool, "write_file" | "delete_path" | "move_path") {
        // Hold approval constant so these controls measure the filesystem
        // profile applied by the real registry, not absence of a reviewer.
        settings
            .set_approval_policy(deepagent_app_core::ApprovalPolicy::FullAccess)
            .unwrap();
    }
    settings
        .harness_models()
        .select_model("deepseek", "deepseek-flash")
        .unwrap();
    let model = Arc::new(JunctionModel {
        tool,
        args,
        marker,
        calls: AtomicUsize::new(0),
        observed: Mutex::new(None),
        root_replacement: Mutex::new(root_replacement),
    });
    let chat = ChatService::new(db, settings, model.clone(), workspace);
    chat.run_in_session(
        "Run the specified local fixture tool and report its output.",
        None,
        None,
        None,
        Vec::new(),
        None,
        false,
        Some(run_id),
        |_| {},
        |_| {},
    )
    .await
    .unwrap();
    assert_eq!(model.calls.load(Ordering::SeqCst), 2);
    let events = chat.run_events(run_id, None).unwrap();
    let completed: Vec<_> = events
        .iter()
        .filter(|event| {
            event.event_type == "tool_completed" && event.data["call_id"] == "audit-junction-read"
        })
        .collect();
    // This event also represents a pending approval that may later succeed.
    // The observation measures a terminal denial, not a temporary pause.
    let blocked_events: Vec<_> = events
        .iter()
        .filter(|event| event.event_type == "tool_blocked" && event.data["name"] == tool)
        .collect();
    if matches!(tool, "write_file" | "delete_path" | "move_path") {
        for event in &blocked_events {
            eprintln!(
                "mutation gate {tool}: needs_approval={} reason={}",
                event.data["needs_approval"], event.data["reason"]
            );
        }
    }
    let blocked = blocked_events
        .iter()
        .any(|event| event.data["needs_approval"] == false);
    assert!(completed.len() <= 1);
    let content_returned = model.observed.lock().unwrap().unwrap();
    ReadObservation {
        content_returned,
        tool_completed_ok: completed
            .first()
            .map(|event| event.data["ok"].as_bool().unwrap()),
        tool_blocked: blocked,
    }
}

#[tokio::test]
async fn mutation_junction_policy_matches_disk_and_next_model_wire() {
    for inward in [false, true] {
        for (mode, label) in [
            (SandboxMode::WorkspaceWrite, "workspace"),
            (SandboxMode::FullAccess, "full"),
        ] {
            for operation in 0..4 {
                let workspace = tempfile::tempdir().unwrap();
                let outside = tempfile::tempdir().unwrap();
                let real = if inward {
                    let real = workspace.path().join("real");
                    std::fs::create_dir(&real).unwrap();
                    real
                } else {
                    outside.path().to_owned()
                };
                let junction = create_junction(&workspace.path().join("linked"), &real);
                let target = real.join("entry.txt");
                let source = workspace.path().join("source.txt");
                let moved = workspace.path().join("moved.txt");
                if operation == 1 || operation == 2 {
                    std::fs::write(&target, OUTSIDE_CANARY).unwrap();
                }
                if operation == 3 {
                    std::fs::write(&source, OUTSIDE_CANARY).unwrap();
                }
                let (tool, args) = match operation {
                    0 => (
                        "write_file",
                        serde_json::json!({"path":"linked/entry.txt", "content":OUTSIDE_CANARY}),
                    ),
                    1 => (
                        "delete_path",
                        serde_json::json!({"path":"linked/entry.txt"}),
                    ),
                    2 => (
                        "move_path",
                        serde_json::json!({"source":"linked/entry.txt", "destination":"moved.txt"}),
                    ),
                    _ => (
                        "move_path",
                        serde_json::json!({"source":"source.txt", "destination":"linked/entry.txt"}),
                    ),
                };
                let allowed = inward || mode == SandboxMode::FullAccess;
                let observation = observe_tool(
                    workspace.path(),
                    tool,
                    args,
                    OUTSIDE_CANARY,
                    mode,
                    &format!("audit-mutation-junction-{inward}-{label}-{operation}"),
                )
                .await;
                eprintln!("mutation junction inward={inward} mode={label} operation={operation}: {observation:?}");
                // These success fields are read from the actual paired output
                // in the next model request; echoed arguments cannot pass.
                assert_eq!(observation.content_returned, allowed, "{observation:?}");
                if allowed {
                    assert_eq!(observation.tool_completed_ok, Some(true));
                    assert!(!observation.tool_blocked);
                } else {
                    assert!(
                        observation.tool_blocked || observation.tool_completed_ok == Some(false)
                    );
                    assert_ne!(observation.tool_completed_ok, Some(true));
                }
                match operation {
                    0 => assert_eq!(
                        std::fs::read_to_string(&target).ok(),
                        allowed.then(|| OUTSIDE_CANARY.to_owned())
                    ),
                    1 => assert_eq!(
                        std::fs::read_to_string(&target).ok(),
                        (!allowed).then(|| OUTSIDE_CANARY.to_owned())
                    ),
                    2 => {
                        assert_eq!(
                            std::fs::read_to_string(&target).ok(),
                            (!allowed).then(|| OUTSIDE_CANARY.to_owned())
                        );
                        assert_eq!(
                            std::fs::read_to_string(&moved).ok(),
                            allowed.then(|| OUTSIDE_CANARY.to_owned())
                        );
                    }
                    _ => {
                        assert_eq!(
                            std::fs::read_to_string(&source).ok(),
                            (!allowed).then(|| OUTSIDE_CANARY.to_owned())
                        );
                        assert_eq!(
                            std::fs::read_to_string(&target).ok(),
                            allowed.then(|| OUTSIDE_CANARY.to_owned())
                        );
                    }
                }
                drop(junction);
            }
        }
    }
}

#[tokio::test]
async fn direct_outside_read_obeys_workspace_and_full_access_policies() {
    let workspace = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let outside_file = outside.path().join("outside.txt");
    std::fs::write(&outside_file, OUTSIDE_CANARY).unwrap();

    let confined = observe_read(
        workspace.path(),
        outside_file.to_string_lossy().into_owned(),
        SandboxMode::WorkspaceWrite,
        "audit-direct-outside-workspace",
    )
    .await;
    assert!(!confined.content_returned);
    assert_eq!(confined.tool_completed_ok, None);
    assert!(confined.tool_blocked);

    let full = observe_read(
        workspace.path(),
        outside_file.to_string_lossy().into_owned(),
        SandboxMode::FullAccess,
        "audit-direct-outside-full",
    )
    .await;
    assert!(full.content_returned);
    assert_eq!(full.tool_completed_ok, Some(true));
    assert!(!full.tool_blocked);
}

#[tokio::test]
async fn audit_read_file_does_not_follow_workspace_junction_outside_root() {
    let workspace = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("outside.txt"), OUTSIDE_CANARY).unwrap();
    let link = workspace.path().join("linked");
    let junction = create_junction(&link, outside.path());
    let observation = observe_read(
        workspace.path(),
        "linked/outside.txt".into(),
        SandboxMode::WorkspaceWrite,
        "audit-junction-boundary",
    )
    .await;
    eprintln!("workspace junction observation: {observation:?}");
    assert!(!observation.content_returned);
    // The shared hook now rejects before invoking the tool, matching the
    // existing direct-outside control (not a failed tool completion).
    assert_eq!(observation.tool_completed_ok, None);
    assert!(observation.tool_blocked);
    let allowed = observe_read(
        workspace.path(),
        "linked/outside.txt".into(),
        SandboxMode::FullAccess,
        "audit-junction-full-access",
    )
    .await;
    assert!(allowed.content_returned);
    assert_eq!(allowed.tool_completed_ok, Some(true));
    assert!(!allowed.tool_blocked);
    drop(junction);
}

#[tokio::test]
async fn internal_junction_and_workspace_root_alias_reads_remain_usable() {
    let workspace = tempfile::tempdir().unwrap();
    let real = workspace.path().join("real");
    std::fs::create_dir(&real).unwrap();
    std::fs::write(real.join("inside.txt"), OUTSIDE_CANARY).unwrap();
    let link = create_junction(&workspace.path().join("linked"), &real);
    let alias_parent = tempfile::tempdir().unwrap();
    let alias_path = alias_parent.path().join("workspace-alias");
    let alias = create_junction(&alias_path, workspace.path());
    for (root, run_id) in [
        (workspace.path(), "audit-inside-junction"),
        (alias_path.as_path(), "audit-root-alias"),
    ] {
        let result = observe_read(
            root,
            "linked/inside.txt".into(),
            SandboxMode::WorkspaceWrite,
            run_id,
        )
        .await;
        assert!(result.content_returned, "{result:?}");
        assert_eq!(result.tool_completed_ok, Some(true));
        assert!(!result.tool_blocked);
    }
    drop((alias, link));
}

#[tokio::test]
async fn directory_tool_results_obey_junction_policy_in_next_model_wire() {
    let workspace = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("outside.txt"), OUTSIDE_CANARY).unwrap();
    let real = workspace.path().join("real");
    std::fs::create_dir(&real).unwrap();
    std::fs::write(real.join("inside.txt"), "INSIDE_DIRECTORY_FIXTURE").unwrap();
    let outward = create_junction(&workspace.path().join("outward"), outside.path());
    let inward = create_junction(&workspace.path().join("inward"), &real);
    let cycle = create_junction(&real.join("back"), workspace.path());
    for (mode, label, allowed) in [
        (SandboxMode::WorkspaceWrite, "workspace", false),
        (SandboxMode::FullAccess, "full", true),
    ] {
        for (tool, args, marker) in [
            (
                "list_dir",
                serde_json::json!({"path":"outward"}),
                "outside.txt",
            ),
            (
                "glob",
                serde_json::json!({"pattern":"**/*.txt"}),
                "outward/outside.txt",
            ),
            (
                "grep",
                serde_json::json!({"query":OUTSIDE_CANARY}),
                OUTSIDE_CANARY,
            ),
        ] {
            let observation = observe_tool(
                workspace.path(),
                tool,
                args,
                marker,
                mode,
                &format!("audit-directory-{tool}-{label}"),
            )
            .await;
            eprintln!("directory next-wire {tool}/{label}: {observation:?}");
            assert_eq!(observation.content_returned, allowed, "{observation:?}");
            let blocked = tool == "list_dir" && !allowed;
            assert_eq!(observation.tool_blocked, blocked, "{observation:?}");
            assert_eq!(
                observation.tool_completed_ok,
                if blocked { None } else { Some(true) }
            );
        }
    }
    // Normal inward aliases must also survive the complete hook/kernel/wire
    // path, not only direct Tool::invoke tests.
    for (tool, args, marker) in [
        (
            "list_dir",
            serde_json::json!({"path":"inward"}),
            "inside.txt",
        ),
        (
            "glob",
            serde_json::json!({"pattern":"inward/*.txt"}),
            "inward/inside.txt",
        ),
        (
            "grep",
            serde_json::json!({"query":"INSIDE_DIRECTORY_FIXTURE"}),
            "INSIDE_DIRECTORY_FIXTURE",
        ),
    ] {
        let observation = observe_tool(
            workspace.path(),
            tool,
            args,
            marker,
            SandboxMode::WorkspaceWrite,
            &format!("audit-directory-{tool}-inward"),
        )
        .await;
        assert!(observation.content_returned, "{observation:?}");
        assert_eq!(observation.tool_completed_ok, Some(true));
        assert!(!observation.tool_blocked);
    }
    drop((cycle, inward, outward));
}

#[tokio::test]
async fn hidden_intermediate_junction_targets_obey_policy_in_next_model_wire() {
    let workspace = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let real = workspace.path().join("real");
    std::fs::create_dir(&real).unwrap();
    std::fs::write(real.join("inside.txt"), OUTSIDE_CANARY).unwrap();
    let back = create_junction(&outside.path().join("back"), &real);
    let chain = create_junction(
        &workspace.path().join("chain"),
        &outside.path().join("back"),
    );
    let sensitive = create_junction(&workspace.path().join("secret-store"), &real);
    let alias = create_junction(
        &workspace.path().join("alias"),
        &workspace.path().join("secret-store"),
    );
    let bridge = create_junction(&workspace.path().join("bridge"), &real);
    let inward = create_junction(
        &workspace.path().join("inward-chain"),
        &workspace.path().join("bridge"),
    );
    for (path, mode, run_id, allowed) in [
        (
            "chain/inside.txt",
            SandboxMode::WorkspaceWrite,
            "audit-hidden-outward",
            false,
        ),
        (
            "chain/inside.txt",
            SandboxMode::FullAccess,
            "audit-hidden-full",
            true,
        ),
        (
            "alias/inside.txt",
            SandboxMode::FullAccess,
            "audit-hidden-sensitive",
            false,
        ),
        (
            "inward-chain/inside.txt",
            SandboxMode::WorkspaceWrite,
            "audit-hidden-inward",
            true,
        ),
    ] {
        let result = observe_read(workspace.path(), path.into(), mode, run_id).await;
        eprintln!("{run_id}: {result:?}");
        assert_eq!(result.content_returned, allowed, "{run_id}: {result:?}");
        assert_eq!(result.tool_blocked, !allowed, "{run_id}: {result:?}");
        assert_eq!(
            result.tool_completed_ok,
            allowed.then_some(true),
            "{run_id}: {result:?}"
        );
    }
    drop((inward, bridge, alias, sensitive, chain, back));
}

#[tokio::test]
async fn root_replacement_after_registry_assembly_is_rejected_in_next_model_wire() {
    for use_junction in [false, true] {
        let home = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let logical = home.path().join("workspace");
        let parked = home.path().join("parked");
        std::fs::create_dir(&logical).unwrap();
        std::fs::write(logical.join("visible.txt"), "ORIGINAL_ROOT_FIXTURE").unwrap();
        std::fs::write(outside.path().join("visible.txt"), OUTSIDE_CANARY).unwrap();
        let run_id = if use_junction {
            "audit-root-junction-swap"
        } else {
            "audit-root-ordinary-swap"
        };
        let result = observe_tool_with_root_replacement(
            &logical,
            (
                "read_file",
                serde_json::json!({"path":"visible.txt"}),
                OUTSIDE_CANARY,
            ),
            SandboxMode::WorkspaceWrite,
            run_id,
            Some(RootReplacement {
                logical: logical.clone(),
                parked: parked.clone(),
                outside: outside.path().to_path_buf(),
                use_junction,
                junction: None,
            }),
        )
        .await;
        eprintln!("{run_id}: {result:?}");
        assert!(!result.content_returned, "{result:?}");
        assert!(result.tool_blocked, "{result:?}");
        assert_eq!(result.tool_completed_ok, None, "{result:?}");
        assert_eq!(
            std::fs::read_to_string(parked.join("visible.txt")).unwrap(),
            "ORIGINAL_ROOT_FIXTURE"
        );
        assert_eq!(
            std::fs::read_to_string(outside.path().join("visible.txt")).unwrap(),
            OUTSIDE_CANARY
        );
    }
}
