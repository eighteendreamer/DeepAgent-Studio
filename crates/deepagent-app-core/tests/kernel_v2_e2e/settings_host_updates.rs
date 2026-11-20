//! H87: settings written by another host reach the next real kernel/tool run.
use super::*;
use deepagent_app_core::SandboxMode;

struct WriteModel {
    calls: AtomicUsize,
    output: Mutex<Option<serde_json::Value>>,
}

#[async_trait]
impl HttpTransport for WriteModel {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let events = match self.calls.fetch_add(1, Ordering::SeqCst) {
            0 => response_function_call(
                "h87-write",
                "write_file",
                serde_json::json!({"path":"result.txt", "content":"h87 normal write"}),
            ),
            1 => {
                let body: serde_json::Value = serde_json::from_str(&request.body).unwrap();
                let output = body["input"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|item| {
                        item["type"] == "function_call_output" && item["call_id"] == "h87-write"
                    })
                    .expect("paired output of the actual write tool")["output"]
                    .as_str()
                    .unwrap();
                *self.output.lock().unwrap() = Some(serde_json::from_str(output).unwrap());
                response_text("H87 tool result received")
            }
            _ => panic!("unexpected H87 model round"),
        };
        emit_responses_events(sink, &events)
    }
}

#[tokio::test]
async fn h87_next_kernel_run_uses_other_hosts_read_only_restriction_after_refresh() {
    for restrict in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let workspace = directory.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        let path = directory.path().join("settings.sqlite");
        let db = Arc::new(Database::open(&path).unwrap());
        let discovery = Arc::new(MockTransport::with_get_json(
            r#"{"data":[{"id":"deepseek-v4-flash","object":"model","owned_by":"deepseek"}]}"#,
        ));
        let first = Arc::new(SettingsService::new(
            db.clone(),
            discovery.clone(),
            Arc::new(MemorySecretStore::new()),
        ));
        crate::registry_fixture::configure(&first, "h87-synthetic-credential")
            .await
            .unwrap();
        let model = Arc::new(WriteModel {
            calls: AtomicUsize::new(0),
            output: Mutex::new(None),
        });
        let chat = ChatService::new(db, first.clone(), model.clone(), &workspace);
        // Construct ChatService while its SettingsService is warm, then commit
        // from an independent connection before the next run is admitted.
        let second = SettingsService::new(
            Arc::new(Database::open(&path).unwrap()),
            discovery,
            Arc::new(MemorySecretStore::new()),
        );
        if restrict {
            second.set_sandbox_mode(SandboxMode::ReadOnly).unwrap();
        }
        first
            .harness_models()
            .fetch_models("deepseek")
            .await
            .unwrap();
        let run_id = if restrict {
            "h87-restricted"
        } else {
            "h87-control"
        };
        chat.run_in_session(
            "Write result.txt with the requested content.",
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
        let output = model.output.lock().unwrap().clone().unwrap();
        if restrict {
            assert_eq!(output["status"], "error", "{output}");
            assert_eq!(output["error"]["error_type"], "hook_denied", "{output}");
            assert!(
                output["error"]["error"]
                    .as_str()
                    .unwrap()
                    .contains("modify is blocked by the current sandbox mode"),
                "{output}"
            );
            assert!(!workspace.join("result.txt").exists());
        } else {
            assert_eq!(output["status"], "ok", "{output}");
            assert_eq!(
                std::fs::read_to_string(workspace.join("result.txt")).unwrap(),
                "h87 normal write"
            );
        }
        assert!(chat.run_events(run_id, None).unwrap().iter().any(|event| {
            event.event_type == "run_terminal" && event.data["kind"] == "succeeded"
        }));
    }
}
