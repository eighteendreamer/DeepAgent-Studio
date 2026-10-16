//! Real subprocess diagnostics; follows the existing Node process-fixture boundary.
use deepagent_mcp::{McpClient, McpServerConfig, StdioTransport};
use std::{collections::BTreeMap, sync::Arc, time::Duration};

#[tokio::test]
async fn crashed_server_reports_exit_and_redacted_bounded_stderr() {
    if !std::process::Command::new("node")
        .arg("--version")
        .output()
        .is_ok_and(|out| out.status.success())
    {
        eprintln!("skipping: node not available");
        return;
    }
    let config = McpServerConfig {
        command: Some("node".into()),
        args: vec!["-e".into(), r#"process.stdin.once('data', () => {
          process.stderr.write('x'.repeat(20000) + '\nMODULE_NOT_FOUND password=hidden Bearer hidden-bearer ' + process.env.FIXTURE_API_KEY + '\n', () => process.exit(7));
        });"#.into()],
        env: BTreeMap::from([("FIXTURE_API_KEY".into(), "fixture-raw-credential".into())]),
        transport: None, cwd: None, url: None, headers: BTreeMap::new(),
    };
    let client = McpClient::new(Arc::new(StdioTransport::spawn(&config).unwrap()));
    let error = tokio::time::timeout(
        Duration::from_secs(10),
        client.initialize("diagnostic-fixture"),
    )
    .await
    .unwrap()
    .unwrap_err()
    .to_string();
    assert!(error.contains("MODULE_NOT_FOUND"), "{error}");
    assert!(error.contains('7'), "{error}");
    assert!(
        !error.contains("hidden") && !error.contains("fixture-raw-credential"),
        "{error}"
    );
    assert!(error.len() < 9000);
    assert!(deepagent_mcp::is_transport_closed_error(
        &deepagent_core::CoreError::other(error)
    ));
    client.close().await.unwrap();
}
