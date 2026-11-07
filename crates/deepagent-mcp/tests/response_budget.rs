//! H126 real transport probes: limits are applied before EOF, UTF-8 or JSON parsing.
use deepagent_mcp::{protocol::JsonRpcRequest, McpServerConfig, McpTransport, StdioTransport};
use std::{sync::Arc, time::Duration};

const MARKER: &str = "H126_SYNTHETIC_PRIVATE_BODY";
#[tokio::test]
async fn h126_stdio_exact_line_budget_and_legitimate_large_results_remain_usable() {
    for (payload, configured) in [
        (String::from("合成"), Some(1usize)),
        ("x".repeat(2 * 1024 * 1024), None),
        ("x".repeat(2 * 1024 * 1024), Some(3 * 1024 * 1024)),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("respond.cjs");
        std::fs::write(
            &script,
            r#"
const readline=require('node:readline');
readline.createInterface({input:process.stdin}).on('line',line=>{
 const req=JSON.parse(line);process.stdout.write(JSON.stringify({jsonrpc:'2.0',id:req.id,
 result:{payload:req.params.payload}})+'\n');
});
"#,
        )
        .unwrap();
        // The connection's first wire id is 1, irrespective of client-local id.
        let line_bytes = serde_json::to_vec(&serde_json::json!({
            "jsonrpc":"2.0","id":1,"result":{"payload":payload}
        }))
        .unwrap()
        .len()
            + 1;
        let cap = if configured == Some(1) {
            Some(line_bytes)
        } else {
            configured
        };
        let c = config(
            serde_json::json!({"command":"node","args":[script],"response_limit_bytes":cap}),
        );
        let transport = StdioTransport::spawn(&c).unwrap();
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            transport.send(&JsonRpcRequest::new(
                99,
                "tools/call",
                Some(serde_json::json!({"payload":payload})),
            )),
        )
        .await
        .unwrap();
        transport.close().await.unwrap();
        let result = result.unwrap();
        assert_eq!(result.id, 99);
        assert_eq!(result.result.unwrap()["payload"].as_str().unwrap(), payload);
    }
}

#[tokio::test]
async fn h126_real_business_overflow_is_unknown_outcome_without_reconnect_or_replay() {
    use deepagent_core::{CoreError, Result};
    use deepagent_mcp::{McpTransport, ReconnectFactory, ReconnectingTransport};
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Factory(Arc<AtomicUsize>);
    #[async_trait::async_trait]
    impl ReconnectFactory for Factory {
        async fn reconnect(&self) -> Result<Arc<dyn McpTransport>> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Err(CoreError::other("unexpected synthetic reconnect"))
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("effect.cjs");
    let counter = dir.path().join("effects");
    std::fs::write(
        &script,
        r#"
const fs=require('node:fs'),rl=require('node:readline').createInterface({input:process.stdin});
rl.on('line',line=>{const r=JSON.parse(line);if(r.method==='tools/call'){
 fs.appendFileSync(process.env.H126_COUNTER,'once\n');
 process.stdout.write(JSON.stringify({jsonrpc:'2.0',id:r.id,result:{data:'x'.repeat(4096)}})+'\n');
}else process.stdout.write(JSON.stringify({jsonrpc:'2.0',id:r.id,result:{}})+'\n');});
"#,
    )
    .unwrap();
    let c = config(serde_json::json!({"command":"node","args":[script],
        "env":{"H126_COUNTER":counter},"response_limit_bytes":256}));
    let factories = Arc::new(AtomicUsize::new(0));
    let raw = Arc::new(StdioTransport::spawn(&c).unwrap());
    let t = ReconnectingTransport::new(raw, Arc::new(Factory(factories.clone())))
        .with_backoff(vec![Duration::ZERO]);
    let result = t.send(&JsonRpcRequest::new(7, "tools/call", None)).await;
    let _ = t.close().await;
    assert!(
        matches!(result,Err(CoreError::RemoteOutcomeUnknown{request_id:7,source,..})
        if matches!(*source,CoreError::McpResponseTooLarge{limit_bytes:256,..}))
    );
    assert_eq!(factories.load(Ordering::SeqCst), 0);
    assert_eq!(std::fs::read_to_string(counter).unwrap(), "once\n");
}
fn config(value: serde_json::Value) -> McpServerConfig {
    serde_json::from_value(value).unwrap()
}
#[test]
fn h126_config_rejects_zero_and_unbounded_override() {
    for limit in [0, 64 * 1024 * 1024 + 1] {
        let c = config(serde_json::json!({"command":"node","response_limit_bytes":limit}));
        assert!(
            c.validate().is_err(),
            "invalid limit {limit} must not silently disable the budget"
        );
    }
}

#[test]
fn h126_wrapped_config_cannot_silently_drop_malformed_budget() {
    for value in [
        serde_json::json!(-1),
        serde_json::json!(1.5),
        serde_json::json!("unlimited"),
    ] {
        let text = serde_json::json!({"mcpServers":{"synthetic":{
            "command":"node","response_limit_bytes":value
        }}})
        .to_string();
        assert!(
            deepagent_mcp::McpConfig::parse(&text).is_err(),
            "a malformed budget cannot fall through into an empty bare-map configuration"
        );
    }
}
#[tokio::test]
async fn h126_stdio_refuses_unterminated_overflow_releases_waiters_and_never_resynchronizes() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("server.cjs");
    let pid_file = dir.path().join("owned-pid");
    std::fs::write(&file, r#"
const readline=require('node:readline');
require('node:fs').writeFileSync(process.env.H126_PIDFILE,String(process.pid));
let held=[];
readline.createInterface({input:process.stdin}).on('line', line=>{
 const req=JSON.parse(line);
 if(req.method==='hold') held.push(req.id);
 else if(req.method==='large') {
  process.stderr.write('H126_SYNTHETIC_PRIVATE_BODY\n');
  process.stdout.write('{"jsonrpc":"2.0","id":'+req.id+',"result":{"payload":"H126_SYNTHETIC_PRIVATE_BODY'+'x'.repeat(2048));
 } else process.stdout.write(JSON.stringify({jsonrpc:'2.0',id:req.id,result:{held:held.length}})+'\n');
});
"#).unwrap();
    let c = config(
        serde_json::json!({"command":"node","args":[file],"response_limit_bytes":256,
        "env":{"H126_PIDFILE":pid_file}}),
    );
    let t = Arc::new(StdioTransport::spawn(&c).unwrap());
    let hold = tokio::spawn({
        let t = t.clone();
        async move { t.send(&JsonRpcRequest::new(1, "hold", None)).await }
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let r = t.send(&JsonRpcRequest::new(2, "ping", None)).await.unwrap();
            if r.result.unwrap()["held"] == 1 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let result = tokio::time::timeout(
        Duration::from_secs(1),
        t.send(&JsonRpcRequest::new(3, "large", None)),
    )
    .await;
    let reaped_before_explicit_close = if matches!(
        &result,
        Ok(Err(deepagent_core::CoreError::McpResponseTooLarge { .. }))
    ) {
        let pid = std::fs::read_to_string(pid_file).unwrap();
        std::process::Command::new("node").args(["-e",
            "try{process.kill(Number(process.argv[1]),0);process.exitCode=1;}catch(e){process.exitCode=e.code==='ESRCH'?0:2;}",
            pid.trim()]).status().unwrap().success()
    } else {
        false
    };
    // Always close the owned real process, even when the old reader waits for LF.
    let _ = t.close().await;
    let held = tokio::time::timeout(Duration::from_secs(2), hold)
        .await
        .unwrap()
        .unwrap();
    let error = result
        .expect("oversize must fail without waiting for LF/EOF")
        .unwrap_err()
        .to_string();
    assert!(error.contains("McpResponseTooLarge"), "{error}");
    assert!(
        !error.contains(MARKER),
        "policy failure cannot append captured stderr/body"
    );
    assert!(held.is_err(), "every pending waiter must be released");
    assert!(
        reaped_before_explicit_close,
        "limit refusal must use the existing real child teardown before returning"
    );
    assert!(
        t.send(&JsonRpcRequest::new(4, "ping", None)).await.is_err(),
        "no resync after untrusted oversized frame"
    );
}

#[cfg(feature = "http")]
mod http {
    #[tokio::test]
    async fn h126_http_default_accepts_large_legal_result_and_next_small_call_survives_refusal() {
        for limit in [None, Some(256)] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let c = config(
                serde_json::json!({"type":"http","url":format!("http://localhost:{}/mcp",listener.local_addr().unwrap().port()),"response_limit_bytes":limit}),
            );
            let t = deepagent_mcp::HttpTransport::connect(&c).unwrap();
            let server = tokio::spawn(async move {
                for (id, size) in [(1, 2 * 1024 * 1024), (2, 16)] {
                    let (mut s, _) = listener.accept().await.unwrap();
                    request(&mut s).await;
                    let body=serde_json::json!({"jsonrpc":"2.0","id":id,"result":{"payload":"x".repeat(size)}}).to_string();
                    let _=s.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await;
                }
            });
            let large = t.send(&JsonRpcRequest::new(1, "tools/call", None)).await;
            if limit.is_some() {
                assert!(matches!(
                    large,
                    Err(deepagent_core::CoreError::McpResponseTooLarge {
                        limit_bytes: 256,
                        ..
                    })
                ));
            } else {
                assert_eq!(
                    large.unwrap().result.unwrap()["payload"]
                        .as_str()
                        .unwrap()
                        .len(),
                    2 * 1024 * 1024
                );
            }
            assert_eq!(
                t.send(&JsonRpcRequest::new(2, "ping", None))
                    .await
                    .unwrap()
                    .result
                    .unwrap()["payload"]
                    .as_str()
                    .unwrap()
                    .len(),
                16
            );
            server.await.unwrap();
        }
    }
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    async fn request(socket: &mut tokio::net::TcpStream) {
        let mut bytes = Vec::new();
        let mut chunk = [0u8; 1024];
        loop {
            let n = socket.read(&mut chunk).await.unwrap();
            assert!(n > 0);
            bytes.extend_from_slice(&chunk[..n]);
            assert!(bytes.len() <= 65536);
            if let Some(p) = bytes.windows(4).position(|x| x == b"\r\n\r\n") {
                let head = String::from_utf8_lossy(&bytes[..p]);
                let n = head
                    .lines()
                    .find_map(|l| {
                        let (k, v) = l.split_once(':')?;
                        k.eq_ignore_ascii_case("content-length")
                            .then(|| v.trim().parse::<usize>().unwrap())
                    })
                    .unwrap_or(0);
                if bytes.len() >= p + 4 + n {
                    return;
                }
            }
        }
    }
    fn connect(port: u16, limit: usize) -> deepagent_mcp::HttpTransport {
        deepagent_mcp::HttpTransport::connect(&config(serde_json::json!({
            "type":"http","url":format!("http://localhost:{port}/mcp"),"response_limit_bytes":limit
        })))
        .unwrap()
    }
    #[tokio::test]
    async fn h126_http_all_body_kinds_stop_before_eof_and_redact_body() {
        for (status, content, declared, notification, chunked) in [
            ("200 OK", "application/json", false, false, false),
            ("200 OK", "text/event-stream", false, false, false),
            ("503 Unavailable", "text/plain", false, false, false),
            ("200 OK", "application/json", true, false, false),
            ("202 Accepted", "application/json", false, true, false),
            ("200 OK", "application/json", false, false, true),
            ("200 OK", "text/event-stream", false, false, true),
        ] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let t = connect(listener.local_addr().unwrap().port(), 256);
            let (release_tx, release_rx) = tokio::sync::oneshot::channel();
            let server = tokio::spawn(async move {
                let (mut s, _) = listener.accept().await.unwrap();
                request(&mut s).await;
                let length = if chunked {
                    "Transfer-Encoding: chunked\r\n"
                } else if declared {
                    "Content-Length: 1000000\r\n"
                } else {
                    ""
                };
                s.write_all(format!("HTTP/1.1 {status}\r\nContent-Type: {content}\r\n{length}Connection: close\r\n\r\n").as_bytes()).await.unwrap();
                let bytes = format!("{MARKER}{}", "x".repeat(2048));
                let bytes = if chunked {
                    format!("{:x}\r\n{bytes}\r\n", bytes.len())
                } else {
                    bytes
                };
                let _ = s.write_all(bytes.as_bytes()).await;
                let _ = release_rx.await; // Never supply EOF until the client has returned.
            });
            let result = tokio::time::timeout(Duration::from_secs(1), async {
                if notification {
                    t.notify("notifications/initialized", None)
                        .await
                        .map(|_| ())
                } else {
                    t.send(&JsonRpcRequest::new(1, "tools/call", None))
                        .await
                        .map(|_| ())
                }
            })
            .await;
            let _ = release_tx.send(());
            server.await.unwrap();
            let error = result
                .expect("budget must not wait for peer EOF")
                .unwrap_err()
                .to_string();
            assert!(
                error.contains("McpResponseTooLarge"),
                "{status} {content}: {error}"
            );
            assert!(!error.contains(MARKER), "{error}");
        }
    }
    #[tokio::test]
    async fn h126_http_exact_budget_accepts_complete_utf8_json_and_errors_do_not_echo_it() {
        for valid in [true, false] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let body = if valid {
                serde_json::json!({"jsonrpc":"2.0","id":1,"result":{"text":"合成"}}).to_string()
            } else {
                format!("{{{MARKER}")
            };
            let t = connect(listener.local_addr().unwrap().port(), body.len());
            let server = tokio::spawn(async move {
                let (mut s, _) = listener.accept().await.unwrap();
                request(&mut s).await;
                s.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
            });
            let result = t.send(&JsonRpcRequest::new(1, "ping", None)).await;
            server.await.unwrap();
            if valid {
                assert_eq!(result.unwrap().result.unwrap()["text"], "合成");
            } else {
                assert!(!result.unwrap_err().to_string().contains(MARKER));
            }
        }
    }
}
