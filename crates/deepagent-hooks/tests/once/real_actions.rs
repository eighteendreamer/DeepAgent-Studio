use super::*;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn registry_for(definitions: &HookDefinitions, hosted: bool) -> HookRegistry {
    let mut registry = HookRegistry::new();
    let count = if hosted {
        definitions.register_into_with_host(
            &mut registry,
            Arc::new(SystemHookRunner),
            Arc::new(Host {
                calls: AtomicUsize::new(0),
                outcome: None,
            }),
        )
    } else {
        definitions.register_into(&mut registry, Arc::new(SystemHookRunner))
    };
    assert_eq!(count, 1);
    registry
}

#[tokio::test]
async fn h240_real_command_in_both_registration_paths_runs_only_on_first_matching_dispatch() {
    for hosted in [false, true] {
        for filter in ["matcher", "condition", "both"] {
            let temporary = tempfile::tempdir().unwrap();
            let file = temporary.path().join("executions");
            let escaped = file.display().to_string().replace('\'', "''");
            let (shell, command) = if cfg!(windows) {
                (
                    "powershell",
                    format!("[IO.File]::AppendAllText('{escaped}', 'x')"),
                )
            } else {
                (
                    "sh",
                    format!(
                        "printf x >> '{}'",
                        file.display().to_string().replace('\'', "'\\''")
                    ),
                )
            };
            let definitions = definitions(
                json!({"type":"command","shell":shell,"command":command,"timeout":5}),
                filter,
            );
            let registry = registry_for(&definitions, hosted);
            registry
                .dispatch(&context("read_file", "primer"))
                .await
                .unwrap();
            assert!(!file.exists());
            for _ in 0..2 {
                assert_eq!(
                    registry
                        .dispatch(&context("write_file", "allowed.txt"))
                        .await
                        .unwrap(),
                    HookOutcome::Continue
                );
            }
            assert_eq!(
                std::fs::read_to_string(&file).unwrap(),
                "x",
                "{hosted}/{filter}"
            );
        }
    }
}

async fn serve_one(socket: &mut tokio::net::TcpStream) {
    let mut bytes = Vec::new();
    let (header_end, length) = loop {
        let mut buffer = [0; 4096];
        let read = socket.read(&mut buffer).await.unwrap();
        assert!(read > 0 && bytes.len() < 16384);
        bytes.extend_from_slice(&buffer[..read]);
        if let Some(index) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
            let headers = std::str::from_utf8(&bytes[..index]).unwrap();
            let length: usize = headers
                .lines()
                .find_map(|line| {
                    let (key, value) = line.split_once(':')?;
                    key.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse().unwrap())
                })
                .unwrap();
            break (index + 4, length);
        }
    };
    while bytes.len() < header_end + length {
        let mut buffer = [0; 4096];
        let read = socket.read(&mut buffer).await.unwrap();
        assert!(read > 0 && bytes.len() < 16384);
        bytes.extend_from_slice(&buffer[..read]);
    }
    let payload: Value = serde_json::from_slice(&bytes[header_end..header_end + length]).unwrap();
    assert_eq!(payload["tool_name"], "write_file");
    assert_eq!(payload["tool_input"]["path"], "allowed.txt");
    socket
        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}")
        .await
        .unwrap();
}

#[tokio::test]
async fn h240_real_loopback_http_uses_same_matching_before_once_in_both_registration_paths() {
    for hosted in [false, true] {
        for filter in ["matcher", "condition", "both"] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let calls = Arc::new(AtomicUsize::new(0));
            let seen = calls.clone();
            let server = tokio::spawn(async move {
                loop {
                    let (mut socket, _) = listener.accept().await.unwrap();
                    serve_one(&mut socket).await;
                    seen.fetch_add(1, Ordering::SeqCst);
                }
            });
            let definitions = definitions(
                json!({"type":"http","url":format!("http://{address}"),"timeout":1}),
                filter,
            );
            let registry = registry_for(&definitions, hosted);
            let result = tokio::time::timeout(Duration::from_secs(3), async {
                registry
                    .dispatch(&context("read_file", "primer"))
                    .await
                    .unwrap();
                assert_eq!(calls.load(Ordering::SeqCst), 0);
                for _ in 0..2 {
                    assert_eq!(
                        registry
                            .dispatch(&context("write_file", "allowed.txt"))
                            .await
                            .unwrap(),
                        HookOutcome::Continue
                    );
                }
            })
            .await;
            server.abort();
            let _ = server.await;
            result.unwrap();
            assert_eq!(calls.load(Ordering::SeqCst), 1, "{hosted}/{filter}");
        }
    }
}
