//! H-217: the existing SDK call path preserves both protocol error classes.
use super::*;
use std::time::Duration;

#[tokio::test]
async fn h217_rpc_error_codes_survive_the_reader_and_do_not_close_the_client() {
    let script = r#"
foreach ($code in @(-32601, -32602)) {
    $request = [Console]::In.ReadLine() | ConvertFrom-Json
    [Console]::Out.WriteLine((@{jsonrpc='2.0';id=$request.id;error=@{code=$code;message='synthetic routing rejection'}} | ConvertTo-Json -Compress))
}
$request = [Console]::In.ReadLine() | ConvertFrom-Json
[Console]::Out.WriteLine((@{jsonrpc='2.0';id=$request.id;result=@{tools=@()}} | ConvertTo-Json -Compress))
while ($null -ne [Console]::In.ReadLine()) {}
"#;
    let mut child = tokio::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let client = HarnessClient::from_stdio(
        child.stdin.take().unwrap(),
        child.stdout.take().unwrap(),
        child.stderr.take().unwrap(),
        4,
    );
    *client.initialized.lock().unwrap() = true;
    for expected in [-32601, -32602] {
        let error = tokio::time::timeout(Duration::from_secs(5), client.tool_list())
            .await
            .unwrap()
            .unwrap_err();
        assert!(matches!(error, SdkError::Rpc { code, .. } if code == expected));
        assert!(client.pending.lock().unwrap().is_empty());
    }
    assert!(client.tool_list().await.unwrap().is_success());
    client.close().await;
    assert!(tokio::time::timeout(Duration::from_secs(5), child.wait())
        .await
        .unwrap()
        .unwrap()
        .success());
}
