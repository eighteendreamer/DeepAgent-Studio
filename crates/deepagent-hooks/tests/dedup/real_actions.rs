use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn registry_for(defs: &HookDefinitions, hosted: bool) -> HookRegistry {
    if hosted {
        register(defs, Arc::new(Host::default()))
    } else {
        let mut registry = HookRegistry::new();
        defs.register_into(&mut registry, Arc::new(SystemHookRunner));
        registry
    }
}

#[tokio::test]
async fn h239_real_command_auto_shell_timeout_and_wildcards_keep_equivalent_duplicates_once() {
    for hosted in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let output = temp.path().join("default-effects");
        let escaped = output.display().to_string().replace('\'', "''");
        let command = if cfg!(windows) {
            format!("[IO.File]::AppendAllText('{escaped}', 'x')")
        } else {
            format!(
                "printf x >> '{}'",
                output.display().to_string().replace('\'', "'\\''")
            )
        };
        let mut first = group(json!({"type":"command","command":command}), None, false);
        first["matcher"] = Value::Null;
        let mut second = first.clone();
        second["matcher"] = json!(" .* ");
        second["if"] = json!(" * ");
        second["hooks"][0]["timeout"] = json!(60);
        second["hooks"][0]["shell"] =
            json!(deepagent_hooks::HookCommandShell::Auto.resolved_label());
        registry_for(&definitions(vec![first, second]), hosted)
            .dispatch(&context("read_file"))
            .await
            .unwrap();
        assert_eq!(std::fs::read_to_string(output).unwrap(), "x", "{hosted}");
    }
}

#[tokio::test]
async fn h239_real_command_environment_and_conditions_survive_both_registration_paths() {
    for hosted in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let output = temp.path().join("effects");
        let (shell, command) = if cfg!(windows) {
            (
                "powershell",
                format!(
                    "[IO.File]::AppendAllText(('{}' + $env:SYNTHETIC_POLICY), 'x')",
                    output.display().to_string().replace('\'', "''")
                ),
            )
        } else {
            (
                "sh",
                format!(
                    "printf x >> '{}'\"$SYNTHETIC_POLICY\"",
                    output.display().to_string().replace('\'', "'\\''")
                ),
            )
        };
        let a = json!({"type":"command","shell":shell,"command":command,"timeout":5,"env":{"SYNTHETIC_POLICY":"A"}});
        let mut b = a.clone();
        b["env"]["SYNTHETIC_POLICY"] = json!("B");
        let defs = definitions(vec![
            group(a.clone(), Some("write_file"), false),
            group(a, Some("read_file"), false),
            group(b.clone(), Some("read_file"), false),
            group(b, Some("read_file"), false),
        ]);
        registry_for(&defs, hosted)
            .dispatch(&context("read_file"))
            .await
            .unwrap();
        for policy in ['A', 'B'] {
            assert_eq!(
                std::fs::read_to_string(format!("{}{policy}", output.display())).unwrap(),
                "x",
                "{hosted}/{policy}"
            );
        }
    }
}

async fn request(socket: &mut tokio::net::TcpStream) -> String {
    let mut bytes = Vec::new();
    let (end, length) = loop {
        let mut buffer = [0; 4096];
        let count = socket.read(&mut buffer).await.unwrap();
        assert!(count > 0 && bytes.len() < 16384);
        bytes.extend_from_slice(&buffer[..count]);
        if let Some(index) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
            let header = std::str::from_utf8(&bytes[..index]).unwrap();
            let length: usize = header
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse().unwrap())
                })
                .unwrap();
            break (index + 4, length);
        }
    };
    while bytes.len() < end + length {
        let mut buffer = [0; 4096];
        let count = socket.read(&mut buffer).await.unwrap();
        assert!(count > 0 && bytes.len() < 16384);
        bytes.extend_from_slice(&buffer[..count]);
    }
    let payload: Value = serde_json::from_slice(&bytes[end..end + length]).unwrap();
    assert_eq!(payload["tool_name"], "read_file");
    let headers = String::from_utf8(bytes[..end].to_vec())
        .unwrap()
        .to_ascii_lowercase();
    socket
        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}")
        .await
        .unwrap();
    headers
}

#[tokio::test]
async fn h239_real_http_conditions_headers_method_and_defaults_define_dispatch_identity() {
    for hosted in [false, true] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let captured = seen.clone();
        let server = tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let headers = request(&mut socket).await;
                captured.lock().unwrap().push(headers);
            }
        });
        let a = json!({"type":"http","url":format!("http://{address}/synthetic"),"headers":{"X-Synthetic":"A"},"timeout":2});
        let mut b = a.clone();
        b["headers"]["X-Synthetic"] = json!("B");
        let mut c = b.clone();
        c["method"] = json!("PUT");
        let mut explicit = b.clone();
        explicit["method"] = json!("POST");
        let defs = definitions(vec![
            group(a.clone(), Some("write_file"), false),
            group(a, Some("read_file"), false),
            group(b, Some("read_file"), false),
            group(explicit, Some("read_file"), false),
            group(c, Some("read_file"), false),
        ]);
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            registry_for(&defs, hosted).dispatch(&context("read_file")),
        )
        .await;
        server.abort();
        let _ = server.await;
        result.unwrap().unwrap();
        let actual = seen.lock().unwrap();
        assert_eq!(actual.len(), 3, "{hosted}");
        assert_eq!(
            actual
                .iter()
                .filter(|head| head.starts_with("post "))
                .count(),
            2
        );
        assert_eq!(
            actual
                .iter()
                .filter(|head| head.starts_with("put "))
                .count(),
            1
        );
        assert_eq!(
            actual
                .iter()
                .filter(|head| head.contains("x-synthetic: a"))
                .count(),
            1
        );
        assert_eq!(
            actual
                .iter()
                .filter(|head| head.contains("x-synthetic: b"))
                .count(),
            2
        );
    }
}
