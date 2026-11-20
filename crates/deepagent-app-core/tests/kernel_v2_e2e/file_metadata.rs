//! Existing file access-control policy must survive content-only tool edits.
//! Windows-only fixture; every ACL operation targets one synthetic temp file.

use super::*;
use deepagent_persistence::checkpoint_store::CheckpointStore;
use deepagent_persistence::fs_commit::access::{open_policy_source, WindowsDacl};
use deepagent_persistence::run_store::RunStore;
use serde_json::{json, Value};
use std::path::Path;

const RUN_ID: &str = "audit-file-metadata";
const BEFORE: &str = "synthetic-file-metadata-before\n";
const AFTER: &str = "synthetic-file-metadata-after\n";

fn acl(path: &Path, protect: bool) -> Value {
    // SetAccessRuleProtection(true, false) removes inherited rules. Keep the
    // current owner fully authorized. Never print the SID/SDDL in test output.
    let script = r#"
$ErrorActionPreference = 'Stop'
$policy = [System.IO.File]::GetAccessControl($env:DEEPAGENT_AUDIT_ACL_PATH)
if ($env:DEEPAGENT_AUDIT_ACL_PROTECT -eq 'true') {
    $policy.SetAccessRuleProtection($true, $false)
    $identity = [System.Security.Principal.WindowsIdentity]::GetCurrent().User
    $rule = [System.Security.AccessControl.FileSystemAccessRule]::new(
        $identity, [System.Security.AccessControl.FileSystemRights]::FullControl,
        [System.Security.AccessControl.AccessControlType]::Allow)
    $policy.SetAccessRule($rule)
    [System.IO.File]::SetAccessControl($env:DEEPAGENT_AUDIT_ACL_PATH, $policy)
    $policy = [System.IO.File]::GetAccessControl($env:DEEPAGENT_AUDIT_ACL_PATH)
}
$section = [System.Security.AccessControl.AccessControlSections]::Access
@{
    protected = $policy.AreAccessRulesProtected
    dacl = $policy.GetSecurityDescriptorSddlForm($section)
    rules = @($policy.GetAccessRules($true, $true, [System.Security.Principal.SecurityIdentifier])).Count
} | ConvertTo-Json -Compress
"#;
    let output = std::process::Command::new("powershell.exe")
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            script,
        ])
        .env("DEEPAGENT_AUDIT_ACL_PATH", path)
        .env("DEEPAGENT_AUDIT_ACL_PROTECT", protect.to_string())
        .output()
        .expect("the Windows ACL fixture requires local PowerShell");
    assert!(
        output.status.success(),
        "ACL fixture failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("ACL fixture must return JSON")
}

struct MetadataModel {
    tool: &'static str,
    requests: Mutex<Vec<Value>>,
}

#[async_trait]
impl HttpTransport for MetadataModel {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let body: Value = serde_json::from_str(&request.body)?;
        let step = {
            let mut requests = self.requests.lock().unwrap();
            requests.push(body.clone());
            requests.len()
        };
        let item = match step {
            1 => super::responses_fixture::call(
                "audit-metadata-read",
                "read_file",
                json!({"path":"fixture.txt"}),
            ),
            2 => {
                assert!(body["input"].to_string().contains(BEFORE.trim_end()));
                let args = match self.tool {
                    "write_file" => json!({"path":"fixture.txt","content":AFTER}),
                    "edit_file" => json!({"path":"fixture.txt","old":BEFORE,"new":AFTER}),
                    "multi_edit" => {
                        json!({"path":"fixture.txt","edits":[{"old":BEFORE,"new":AFTER}]})
                    }
                    "read_file" => json!({"path":"fixture.txt","offset":1,"limit":1}),
                    _ => panic!("unsupported fixture tool"),
                };
                super::responses_fixture::call("audit-metadata-action", self.tool, args)
            }
            3 => {
                let outputs = body["input"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|item| {
                        item["type"] == "function_call_output"
                            && item["call_id"] == "audit-metadata-action"
                    })
                    .collect::<Vec<_>>();
                assert_eq!(outputs.len(), 1);
                let result: Value =
                    serde_json::from_str(outputs[0]["output"].as_str().unwrap()).unwrap();
                assert_eq!(result["status"], "ok", "tool error: {}", result["error"]);
                super::responses_fixture::text("audit-metadata-answer", "Metadata audit finished.")
            }
            _ => panic!("unexpected extra metadata audit request"),
        };
        let mut events = super::responses_fixture::completed(
            &format!("audit-metadata-response-{step}"),
            "deepseek-flash",
            item.clone(),
            json!({"input_tokens":10,"output_tokens":4,"total_tokens":14}),
        );
        if item["type"] == "message" {
            events.insert(
                2,
                json!({"type":"response.output_text.delta","item_id":item["id"],
                    "output_index":0,"content_index":0,"delta":item["content"][0]["text"]})
                .to_string(),
            );
        }
        for (sequence, event) in events.iter_mut().enumerate() {
            let mut value: Value = serde_json::from_str(event).unwrap();
            value["sequence_number"] = sequence.into();
            *event = value.to_string();
        }
        emit_responses_events(sink, &events)
    }
}

struct Observation {
    before: Value,
    after: Value,
    restored: Value,
    after_preserved: bool,
    restored_preserved: bool,
}

async fn observe(tool: &'static str, protect: bool) -> Observation {
    let temporary = tempfile::tempdir().unwrap();
    let workspace = temporary.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let path = workspace.join("fixture.txt");
    std::fs::write(&path, BEFORE).unwrap();
    assert!(path.starts_with(temporary.path()));
    let before = acl(&path, protect);
    let native_before = WindowsDacl::capture(&open_policy_source(&path).unwrap()).unwrap();
    if protect {
        assert_eq!(before["protected"], true);
        assert_eq!(before["rules"], 1);
    }
    let db_path = temporary.path().join("state.sqlite");
    let model = Arc::new(MetadataModel {
        tool,
        requests: Mutex::new(Vec::new()),
    });
    let checkpoint_id = {
        let db = Arc::new(Database::open(&db_path).unwrap());
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
        settings
            .harness_models()
            .select_model("deepseek", "deepseek-flash")
            .unwrap();
        let chat = ChatService::new(db.clone(), settings, model.clone(), &workspace);
        chat.run_in_session(
            "Run the supplied controlled file audit and report the tool result.",
            None,
            None,
            None,
            Vec::new(),
            None,
            false,
            Some(RUN_ID),
            |_| {},
            |_| {},
        )
        .await
        .unwrap();
        let events = chat.run_events(RUN_ID, None).unwrap();
        let actions = events
            .iter()
            .filter(|event| {
                event.event_type == "tool_completed"
                    && event.data["call_id"] == "audit-metadata-action"
            })
            .collect::<Vec<_>>();
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].data["ok"], true);
        let terminal = events
            .iter()
            .filter(|event| event.event_type == "run_terminal")
            .collect::<Vec<_>>();
        assert_eq!(terminal.len(), 1);
        assert_eq!(terminal[0].data["kind"], "succeeded");
        let checkpoints = CheckpointStore::new(&db).list_for_run(RUN_ID).unwrap();
        assert_eq!(checkpoints.len(), 1);
        checkpoints[0].id.clone()
    };
    assert_eq!(model.requests.lock().unwrap().len(), 3);
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        if tool == "read_file" { BEFORE } else { AFTER }
    );
    let after = acl(&path, false);
    let after_preserved = native_before
        .verify_publication(&open_policy_source(&path).unwrap(), || {
            open_policy_source(&workspace)
        })
        .unwrap();
    let db = Database::open(&db_path).unwrap();
    let persisted_run = RunStore::new(&db).get(RUN_ID).unwrap().unwrap();
    assert_eq!(persisted_run.terminal_kind.as_deref(), Some("succeeded"));
    deepagent_runtime::CheckpointManager::restore(&db, &checkpoint_id).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), BEFORE);
    let restored = acl(&path, false);
    let restored_preserved = native_before
        .verify_publication(&open_policy_source(&path).unwrap(), || {
            open_policy_source(&workspace)
        })
        .unwrap();
    assert_eq!(RunStore::new(&db).get(RUN_ID).unwrap(), Some(persisted_run));
    assert_eq!(before["protected"], after["protected"]);
    assert_eq!(before["protected"], restored["protected"]);
    assert_eq!(before["rules"], after["rules"]);
    assert_eq!(before["rules"], restored["rules"]);
    eprintln!("file-metadata audit: tool={tool} protected_fixture={protect} before_protected={} after_protected={} before_rules={} after_rules={} policy_equal={} restored_policy_equal={}",
        before["protected"],after["protected"],before["rules"],after["rules"],
        before["dacl"] == after["dacl"], before["dacl"] == restored["dacl"]);
    Observation {
        before,
        after,
        restored,
        after_preserved,
        restored_preserved,
    }
}

#[tokio::test]
async fn file_metadata_read_and_inherited_write_controls() {
    let read = observe("read_file", true).await;
    assert!(read.before == read.after, "read-only control changed DACL");
    assert!(
        read.before == read.restored,
        "read-only restore changed DACL"
    );
    for tool in ["write_file", "edit_file", "multi_edit"] {
        let write = observe(tool, false).await;
        assert!(write.after_preserved, "inherited write changed policy");
        assert!(write.restored_preserved, "inherited restore changed policy");
    }
}

#[tokio::test]
async fn audit_file_content_tools_preserve_protected_dacl() {
    let mut observations = Vec::new();
    for tool in ["write_file", "edit_file", "multi_edit"] {
        observations.push((tool, observe(tool, true).await));
    }
    // Complete every real tool and checkpoint observation before asserting.
    assert!(
        observations
            .iter()
            .all(|(_, value)| value.before == value.after && value.after_preserved),
        "content-only edits replaced the original access-control policy"
    );
    assert!(
        observations
            .iter()
            .all(|(_, value)| value.before == value.restored && value.restored_preserved),
        "checkpoint restore did not recover the original access-control policy"
    );
}
