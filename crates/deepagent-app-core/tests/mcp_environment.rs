//! H-135: real service probe, preparation and adapter, with synthetic ambient data.
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use deepagent_app_core::{McpServerDto, McpService};
use deepagent_persistence::Database;
use serde_json::{json, Value};

const SECRET: &str = "DEEPAGENT_MCP_AUDIT_SECRET";
const PLAIN: &str = "DEEPAGENT_MCP_AUDIT_PLAIN";
const CHILD: &str = "DEEPAGENT_H135_SERVICE_CHILD";
const SERVER: &str = include_str!("../../deepagent-mcp/tests/fixtures/environment-server.cjs");

#[tokio::test]
async fn stdio_environment_policy_is_shared_by_probe_preparation_and_adapter() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    std::fs::create_dir(&home).unwrap();
    let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "stdio_environment_service_child",
            "--ignored",
            "--test-threads=1",
        ])
        .env_clear()
        .kill_on_drop(true);
    for name in [
        "PATH",
        "SystemRoot",
        "WINDIR",
        "SystemDrive",
        "PATHEXT",
        "COMSPEC",
    ] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    let nonce = root.path().file_name().unwrap().to_str().unwrap();
    command
        .env(CHILD, nonce)
        .env(SECRET, format!("ambient-{nonce}"))
        .env(PLAIN, "ambient-plain")
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .env("TEMP", root.path())
        .env("TMP", root.path())
        .env("TMPDIR", root.path())
        .env("LANG", "C.UTF-8");
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    let output = tokio::time::timeout(Duration::from_secs(20), command.output())
        .await
        .expect("isolated service host deadline")
        .unwrap();
    // A regression must not turn a child environment dump into diagnostics.
    assert!(
        output.status.success(),
        "isolated MCP service environment host failed"
    );
}

#[tokio::test]
#[ignore = "helper: only launched by the isolated H-135 service test"]
async fn stdio_environment_service_child() {
    let nonce = std::env::var(CHILD).expect("run the outer isolated service test");
    let root = tempfile::tempdir().unwrap();
    let script = root.path().join("environment-server.cjs");
    std::fs::write(&script, SERVER).unwrap();
    for mode in ["ambient", "explicit"] {
        let report = root.path().join(format!("{mode}.json"));
        let db = Arc::new(Database::open(root.path().join(format!("{mode}.sqlite"))).unwrap());
        let service = Arc::new(McpService::new(db));
        let env = if mode == "explicit" {
            BTreeMap::from([
                (SECRET, format!("${{{SECRET}}}")),
                (PLAIN, "explicit-plain".into()),
            ])
        } else {
            BTreeMap::new()
        };
        let dto: McpServerDto = serde_json::from_value(json!({
            "name":"env", "transport":"stdio", "enabled":true, "command":"node",
            "args":[script, nonce, mode, report], "env":env, "cwd":root.path(),
        }))
        .unwrap();
        let status = service.test_server(dto.clone()).await.unwrap();
        assert_eq!(status.status, "connected");
        assert_eq!(status.tools.len(), 1);
        let probe: Value = serde_json::from_slice(&std::fs::read(&report).unwrap()).unwrap();
        assert_eq!(probe["ambient_secret_present"], mode == "explicit");
        assert_eq!(probe["ambient_secret_matches"], mode == "explicit");
        assert_eq!(probe["plain_present"], mode == "explicit");
        assert_eq!(probe["runtime_base_present"], true);
        service.upsert(dto).unwrap();
        service.start_preparation();
        tokio::time::timeout(Duration::from_secs(5), async {
            while service.ready_registry().server_count() != 1 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("formal service preparation must publish the server");
        let registry = service.ready_registry();
        let adapters = deepagent_mcp::adapters_for(registry.clone());
        assert_eq!(adapters.len(), 1);
        let result = tokio::time::timeout(Duration::from_secs(5), adapters[0].invoke(json!({})))
            .await
            .unwrap()
            .unwrap();
        registry.close_all().await.unwrap();
        assert!(result.ok);
        assert_eq!(result.value["structuredContent"], probe);
    }
}
