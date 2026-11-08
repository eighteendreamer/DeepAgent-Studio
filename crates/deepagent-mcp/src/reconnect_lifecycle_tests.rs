//! H-48: one recovery budget per disconnected local connection, not per caller.
use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::Notify;

struct FailingFactory {
    attempts: AtomicUsize,
}

#[async_trait]
impl ReconnectFactory for FailingFactory {
    async fn reconnect(&self) -> Result<Arc<dyn McpTransport>> {
        self.attempts.fetch_add(1, Ordering::SeqCst);
        tokio::task::yield_now().await;
        Err(CoreError::other("connection refused (synthetic fixture)"))
    }
}

fn dead() -> Arc<dyn McpTransport> {
    Arc::new(crate::transport::MockTransport::new().with_failure_after(0, "broken pipe"))
}

fn ping(id: u64) -> JsonRpcRequest {
    JsonRpcRequest::new(id, "ping", None)
}

#[tokio::test]
async fn h48_concurrent_probe_and_requests_share_exhaustion_and_never_reopen_it() {
    let factory = Arc::new(FailingFactory {
        attempts: AtomicUsize::new(0),
    });
    let transport =
        ReconnectingTransport::new(dead(), factory.clone()).with_backoff(vec![Duration::ZERO; 3]);
    let (first, second, third) = (ping(1), ping(2), ping(3));
    let (a, b, c) = tokio::join!(
        transport.send(&first),
        transport.send(&second),
        transport.send(&third),
    );
    assert!(a.is_err() && b.is_err() && c.is_err());
    for id in 4..7 {
        let error = transport.send(&ping(id)).await.unwrap_err();
        assert!(matches!(
            error,
            CoreError::McpUnavailable {
                recovery_attempts: 3,
                ..
            }
        ));
        assert!(!is_transport_closed_error(&error));
        assert!(transport
            .notify("notifications/initialized", None)
            .await
            .is_err());
    }
    assert_eq!(factory.attempts.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn h48_network_factory_keeps_next_request_recovery_policy() {
    struct NetworkFactory(FailingFactory);
    #[async_trait]
    impl ReconnectFactory for NetworkFactory {
        fn parks_on_exhaustion(&self) -> bool {
            false
        }
        async fn reconnect(&self) -> Result<Arc<dyn McpTransport>> {
            self.0.reconnect().await
        }
    }
    let factory = Arc::new(NetworkFactory(FailingFactory {
        attempts: AtomicUsize::new(0),
    }));
    let transport =
        ReconnectingTransport::new(dead(), factory.clone()).with_backoff(vec![Duration::ZERO; 3]);
    for id in 1..3 {
        assert!(transport.send(&ping(id)).await.is_err());
        assert!(transport.connection_status().is_discoverable());
    }
    assert_eq!(factory.0.attempts.load(Ordering::SeqCst), 6);
    transport
        .park_after_liveness_failure("synthetic timeout")
        .await;
    assert!(transport.connection_status().is_discoverable());
    let config = crate::McpServerConfig {
        response_limit_bytes: None,
        transport: Some(TransportType::Http),
        command: None,
        args: vec![],
        env: Default::default(),
        cwd: None,
        url: Some("http://127.0.0.1:1/synthetic-not-contacted".into()),
        headers: Default::default(),
    };
    assert!(!ConfigReconnectFactory::new(config, "fixture").parks_on_exhaustion());
}

#[tokio::test]
async fn h48_ping_timeouts_retire_local_tools_but_unsupported_ping_does_not() {
    struct NoPing {
        pending: bool,
    }
    #[async_trait]
    impl McpTransport for NoPing {
        async fn send(&self, request: &JsonRpcRequest) -> Result<JsonRpcResponse> {
            if request.method == "ping" {
                if self.pending {
                    return std::future::pending().await;
                }
                return Ok(JsonRpcResponse {
                    jsonrpc: "2.0".into(),
                    id: request.id,
                    result: None,
                    error: Some(crate::protocol::JsonRpcError {
                        code: -32601,
                        message: "method not found".into(),
                        data: None,
                    }),
                });
            }
            Ok(JsonRpcResponse {
                jsonrpc: "2.0".into(),
                id: request.id,
                result: Some(
                    serde_json::json!({"tools":[{"name":"echo","inputSchema":{"type":"object"}}]}),
                ),
                error: None,
            })
        }
    }
    for pending in [true, false] {
        let factory = Arc::new(FailingFactory {
            attempts: AtomicUsize::new(0),
        });
        let transport = Arc::new(ReconnectingTransport::new(
            Arc::new(NoPing { pending }),
            factory.clone(),
        ));
        let client = Arc::new(McpClient::new(transport));
        let mut registry = crate::McpRegistry::new();
        registry.register("local", client.clone()).await.unwrap();
        let probe = crate::LivenessProbe::spawn(
            client.clone(),
            "local",
            crate::LivenessConfig {
                interval: Duration::from_millis(1),
                timeout: Duration::from_millis(5),
                failure_limit: 3,
            },
        );
        tokio::time::timeout(Duration::from_secs(2), async {
            while !probe.is_finished() {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(registry.all_tools().is_empty(), pending);
        assert_eq!(client.connection_status().is_discoverable(), !pending);
        assert_eq!(factory.attempts.load(Ordering::SeqCst), 0);
    }
}

struct PendingFactory {
    attempts: AtomicUsize,
    entered: Notify,
}

#[async_trait]
impl ReconnectFactory for PendingFactory {
    async fn reconnect(&self) -> Result<Arc<dyn McpTransport>> {
        self.attempts.fetch_add(1, Ordering::SeqCst);
        self.entered.notify_one();
        std::future::pending().await
    }
}

#[tokio::test]
async fn h48_cancelling_recovery_consumes_started_attempts_without_renewing_budget() {
    let factory = Arc::new(PendingFactory {
        attempts: AtomicUsize::new(0),
        entered: Notify::new(),
    });
    let transport = Arc::new(
        ReconnectingTransport::new(dead(), factory.clone()).with_backoff(vec![Duration::ZERO; 3]),
    );
    assert!(transport
        .send(&JsonRpcRequest::new(1, "tools/call", None))
        .await
        .is_err());
    for id in 2..5 {
        let context = ToolExecutionContext::default();
        let task = tokio::spawn({
            let transport = transport.clone();
            let context = context.clone();
            async move { transport.send_with_context(&ping(id), context).await }
        });
        tokio::time::timeout(Duration::from_secs(2), factory.entered.notified())
            .await
            .unwrap();
        context.cancellation_token().cancel();
        assert!(task
            .await
            .unwrap()
            .unwrap_err()
            .to_string()
            .contains("before dispatch"));
    }
    let final_call =
        tokio::time::timeout(Duration::from_millis(100), transport.send(&ping(5))).await;
    assert!(
        final_call.is_ok(),
        "an exhausted caller must fail without waiting for a new factory"
    );
    assert!(final_call.unwrap().is_err());
    assert_eq!(factory.attempts.load(Ordering::SeqCst), 3);
}

struct LateFactory {
    entered: Notify,
    release: Notify,
    fresh: Arc<ClosingTransport>,
}

#[async_trait]
impl ReconnectFactory for LateFactory {
    async fn reconnect(&self) -> Result<Arc<dyn McpTransport>> {
        self.entered.notify_one();
        self.release.notified().await;
        Ok(self.fresh.clone())
    }
}

#[derive(Default)]
struct ClosingTransport {
    closes: AtomicUsize,
    sends: AtomicUsize,
}

#[async_trait]
impl McpTransport for ClosingTransport {
    async fn send(&self, request: &JsonRpcRequest) -> Result<JsonRpcResponse> {
        self.sends.fetch_add(1, Ordering::SeqCst);
        Ok(JsonRpcResponse {
            jsonrpc: "2.0".into(),
            id: request.id,
            result: Some(serde_json::json!({})),
            error: None,
        })
    }

    async fn close(&self) -> Result<()> {
        self.closes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test]
async fn h48_close_does_not_wait_for_factory_or_publish_its_late_generation() {
    let fresh = Arc::new(ClosingTransport::default());
    let factory = Arc::new(LateFactory {
        entered: Notify::new(),
        release: Notify::new(),
        fresh: fresh.clone(),
    });
    let transport = Arc::new(
        ReconnectingTransport::new(dead(), factory.clone()).with_backoff(vec![Duration::ZERO]),
    );
    let task = tokio::spawn({
        let transport = transport.clone();
        async move { transport.send(&ping(1)).await }
    });
    factory.entered.notified().await;
    let closed = tokio::time::timeout(Duration::from_millis(100), transport.close()).await;
    factory.release.notify_one();
    let result = task.await.unwrap();
    assert!(
        closed.is_ok(),
        "intentional close must not wait behind a handshake"
    );
    closed.unwrap().unwrap();
    assert!(result.is_err());
    // Closing cancels the factory future before it hands out a transport. Its
    // pre-existing test Arc is not an owned generation and must not be closed.
    assert_eq!(fresh.closes.load(Ordering::SeqCst), 0);
    assert_eq!(fresh.sends.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn h48_existing_snapshot_and_adapter_retire_with_the_shared_connection() {
    let initial = Arc::new(
        crate::transport::MockTransport::new()
            .with_result(
                "tools/list",
                serde_json::json!({"tools":[{
                    "name":"echo", "inputSchema":{"type":"object"}
                }]}),
            )
            .with_failure_after(1, "broken pipe"),
    );
    let factory = Arc::new(FailingFactory {
        attempts: AtomicUsize::new(0),
    });
    let transport = Arc::new(
        ReconnectingTransport::new(initial, factory.clone()).with_backoff(vec![Duration::ZERO; 3]),
    );
    let client = Arc::new(McpClient::new(transport));
    let mut registry = crate::McpRegistry::new();
    registry
        .register_with_instructions("local", client.clone(), Some("synthetic".into()))
        .await
        .unwrap();
    let snapshot = Arc::new(registry.clone());
    let prepared = snapshot.discovery_snapshot();
    let old_adapters = crate::adapters_for(snapshot.clone());
    assert!(client.ping().await.is_err());
    assert!(snapshot.all_tools().is_empty());
    assert!(snapshot.find_tool("mcp__local__echo").is_none());
    assert!(snapshot.server_instructions().is_empty());
    assert_eq!(
        prepared.all_tools().len(),
        1,
        "an already prepared run retains its advertised catalog"
    );
    assert!(
        prepared.discovery_snapshot().all_tools().is_empty(),
        "the next request samples the retired identity"
    );
    assert!(crate::adapters_for(Arc::new(registry)).is_empty());
    let output = old_adapters[0].invoke(serde_json::json!({})).await.unwrap();
    assert!(!output.ok);
    assert_eq!(output.value["error_type"], "mcp_unavailable");
    assert_eq!(output.value["outcome"], "not_dispatched");
    assert_eq!(output.value["retryable"], false);
    assert_eq!(factory.attempts.load(Ordering::SeqCst), 3);
}
