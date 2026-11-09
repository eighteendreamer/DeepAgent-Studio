use super::*;
use crate::protocol::{JsonRpcRequest, JsonRpcResponse};
use crate::registry::McpRegistry;
use crate::transport::McpTransport;
use async_trait::async_trait;
use deepagent_core::error::{CoreError, Result};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU64, AtomicUsize};
use tokio::sync::{watch, Notify, Semaphore};

struct Peer {
    signal: watch::Sender<u64>,
    tools: Mutex<Value>,
    mode: AtomicUsize,
    lists: AtomicUsize,
    active: AtomicUsize,
    max_active: AtomicUsize,
    entered: Notify,
    release: Semaphore,
    calls: Mutex<Vec<(String, Value)>>,
    disconnect: AtomicBool,
    epoch: AtomicU64,
}

impl Peer {
    fn new(name: &str) -> Arc<Self> {
        Arc::new(Self {
            signal: watch::channel(0).0,
            tools: Mutex::new(definitions(name)),
            mode: AtomicUsize::new(0),
            lists: AtomicUsize::new(0),
            active: AtomicUsize::new(0),
            max_active: AtomicUsize::new(0),
            entered: Notify::new(),
            release: Semaphore::new(0),
            calls: Mutex::new(Vec::new()),
            disconnect: AtomicBool::new(false),
            epoch: AtomicU64::new(0),
        })
    }

    fn change(&self, definitions: Value, count: usize) {
        *self.tools.lock().unwrap() = definitions;
        for _ in 0..count {
            self.signal
                .send_modify(|value| *value = value.wrapping_add(1));
        }
    }
}

fn definitions(name: &str) -> Value {
    json!([{"name":name,"description":"synthetic tool","inputSchema":{"type":"object","additionalProperties":false}}])
}

struct Active<'a>(&'a AtomicUsize);
impl Drop for Active<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

#[async_trait]
impl McpTransport for Peer {
    fn tool_list_changes(&self) -> Option<watch::Receiver<u64>> {
        Some(self.signal.subscribe())
    }
    fn connection_generation(&self) -> u64 {
        self.epoch.load(Ordering::Acquire)
    }

    async fn send(&self, request: &JsonRpcRequest) -> Result<JsonRpcResponse> {
        let result = match request.method.as_str() {
            "tools/list" => {
                self.lists.fetch_add(1, Ordering::AcqRel);
                let active = self.active.fetch_add(1, Ordering::AcqRel) + 1;
                self.max_active.fetch_max(active, Ordering::AcqRel);
                let _active = Active(&self.active);
                let tools = self.tools.lock().unwrap().clone();
                let mode = self.mode.load(Ordering::Acquire);
                self.entered.notify_one();
                match mode {
                    1 => return Err(CoreError::other("synthetic catalog failure")),
                    2 => std::future::pending::<()>().await,
                    3 => {
                        self.release.acquire().await.unwrap().forget();
                    }
                    4 => {
                        self.epoch.fetch_add(1, Ordering::AcqRel);
                    }
                    _ => {}
                }
                if mode == 5
                    && request
                        .params
                        .as_ref()
                        .and_then(|p| p.get("cursor"))
                        .is_none()
                {
                    json!({"tools":tools,"nextCursor":"synthetic-next"})
                } else if mode == 5 {
                    self.epoch.fetch_add(1, Ordering::AcqRel);
                    json!({"tools":definitions("other-peer-tool")})
                } else {
                    json!({"tools":tools})
                }
            }
            "tools/call" => {
                let params = request.params.as_ref().unwrap();
                self.calls.lock().unwrap().push((
                    params["name"].as_str().unwrap().into(),
                    params["arguments"].clone(),
                ));
                json!({"content":[{"type":"text","text":"called"}],"isError":false})
            }
            "ping" if self.disconnect.load(Ordering::Acquire) => {
                return Err(CoreError::other("broken pipe"))
            }
            _ => json!({}),
        };
        Ok(JsonRpcResponse {
            jsonrpc: "2.0".into(),
            id: request.id,
            result: Some(result),
            error: None,
        })
    }
}

async fn until(mut condition: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("finite synthetic catalog must settle");
}

async fn registry(peer: &Arc<Peer>) -> McpRegistry {
    let mut registry = McpRegistry::new();
    registry
        .register("fixture", Arc::new(McpClient::new(peer.clone())))
        .await
        .unwrap();
    registry
}

#[tokio::test]
async fn h137_storm_coalesces_and_frozen_admission_routes_stay_consistent() {
    let peer = Peer::new("old");
    let registry = registry(&peer).await;
    let frozen = registry.discovery_snapshot();
    let generation = frozen.catalog_generation();
    peer.change(json!([{"name":"new","inputSchema":{"type":"object","required":["value"],"properties":{"value":{"type":"integer"}},"additionalProperties":false}}]), 5000);
    until(|| registry.find_tool("mcp__fixture__new").is_some()).await;
    assert_eq!(
        peer.lists.load(Ordering::Acquire),
        2,
        "burst requires one refresh, not one RPC per notification"
    );
    assert_eq!(frozen.catalog_generation(), generation);
    assert!(frozen.find_tool("mcp__fixture__old").is_some());
    assert!(frozen.find_tool("mcp__fixture__new").is_none());
    assert_ne!(registry.catalog_generation(), generation);
    assert!(registry
        .invoke("mcp__fixture__new", json!({}))
        .await
        .is_err());
    assert!(peer.calls.lock().unwrap().is_empty());
    registry
        .invoke("mcp__fixture__new", json!({"value":7}))
        .await
        .unwrap();
    frozen.invoke("mcp__fixture__old", json!({})).await.unwrap();
    assert_eq!(
        *peer.calls.lock().unwrap(),
        vec![
            ("new".into(), json!({"value":7})),
            ("old".into(), json!({}))
        ]
    );
    let mut tools = deepagent_tools::ToolRegistry::new();
    for tool in crate::adapter::adapters_for(Arc::new(registry.discovery_snapshot())) {
        tools.register(tool).unwrap();
    }
    assert!(
        tools
            .invoke(
                "mcp__fixture__new",
                json!({"value":9}),
                &deepagent_tools::PermissionSet::read_only(),
                false
            )
            .await
            .is_err(),
        "refresh does not bypass Network permission"
    );
    assert_eq!(peer.calls.lock().unwrap().len(), 2);
    registry.close_all().await.unwrap();
}

#[tokio::test]
async fn h137_change_during_inflight_refresh_is_serial_and_not_lost() {
    let peer = Peer::new("old");
    let registry = registry(&peer).await;
    // Consume the initial discovery notification from the fixture barrier.
    peer.entered.notified().await;
    peer.mode.store(3, Ordering::Release);
    peer.change(definitions("middle"), 1);
    tokio::time::timeout(Duration::from_secs(3), peer.entered.notified())
        .await
        .unwrap();
    peer.change(definitions("latest"), 2000);
    peer.mode.store(0, Ordering::Release);
    peer.release.add_permits(1);
    until(|| registry.find_tool("mcp__fixture__latest").is_some()).await;
    assert_eq!(peer.lists.load(Ordering::Acquire), 3);
    assert_eq!(peer.max_active.load(Ordering::Acquire), 1);
    assert!(registry.find_tool("mcp__fixture__middle").is_none());
    registry.close_all().await.unwrap();
}

#[tokio::test]
async fn h137_refresh_error_withdraws_catalog_and_later_change_recovers_without_reconnect() {
    let peer = Peer::new("old");
    let registry = registry(&peer).await;
    let frozen = registry.discovery_snapshot();
    peer.mode.store(1, Ordering::Release);
    peer.change(definitions("new"), 1);
    until(|| registry.server_catalog_error("fixture").is_some()).await;
    assert!(registry.all_tools().is_empty());
    assert!(registry
        .invoke("mcp__fixture__old", json!({}))
        .await
        .is_err());
    assert!(peer.calls.lock().unwrap().is_empty());
    assert!(frozen.find_tool("mcp__fixture__old").is_some());
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert_eq!(
        peer.lists.load(Ordering::Acquire),
        2,
        "failure must not busy-retry"
    );
    peer.mode.store(0, Ordering::Release);
    peer.change(definitions("new"), 1);
    until(|| registry.find_tool("mcp__fixture__new").is_some()).await;
    assert!(registry.server_catalog_error("fixture").is_none());
    registry.close_all().await.unwrap();
}

#[tokio::test]
async fn h137_timeout_and_retirement_drop_discovery_without_stale_publication() {
    for retire in [false, true] {
        let peer = Peer::new("old");
        let client = Arc::new(McpClient::new(peer.clone()));
        let initial = validated_tools("fixture", client.list_tools().await.unwrap()).unwrap();
        let catalog = ToolCatalog::start(
            "fixture".into(),
            client,
            initial,
            Duration::ZERO,
            Duration::from_millis(40),
        );
        peer.entered.notified().await;
        peer.mode.store(2, Ordering::Release);
        peer.change(definitions("new"), 1);
        tokio::time::timeout(Duration::from_secs(3), peer.entered.notified())
            .await
            .unwrap();
        if retire {
            catalog.stop();
            until(|| peer.active.load(Ordering::Acquire) == 0).await;
            assert_eq!(catalog.snapshot().tools[0].local_name, "old");
        } else {
            until(|| catalog.snapshot().error.is_some()).await;
            assert!(catalog.snapshot().tools.is_empty());
            assert_eq!(peer.active.load(Ordering::Acquire), 0);
        }
        catalog.stop();
        peer.change(definitions("late"), 100);
        tokio::time::sleep(Duration::from_millis(80)).await;
        assert_eq!(peer.lists.load(Ordering::Acquire), 2);
        assert!(!catalog
            .snapshot()
            .tools
            .iter()
            .any(|tool| tool.local_name == "late"));
    }
}

#[tokio::test]
async fn h137_discovery_rejects_cross_connection_results() {
    for mode in [4, 5] {
        let peer = Peer::new("tool");
        peer.mode.store(mode, Ordering::Release);
        let client = McpClient::new(peer.clone());
        let error = client.list_tools().await.unwrap_err();
        assert!(error.to_string().contains("physical connection generation"));
        assert_eq!(
            peer.lists.load(Ordering::Acquire),
            if mode == 5 { 2 } else { 1 }
        );
    }
}

#[tokio::test]
async fn h137_reconnect_relay_follows_current_peer_without_replaying_business_calls() {
    struct Factory(Arc<Peer>);
    #[async_trait]
    impl crate::ReconnectFactory for Factory {
        async fn reconnect(&self) -> Result<Arc<dyn McpTransport>> {
            Ok(self.0.clone())
        }
    }
    let old = Peer::new("old");
    let fresh = Peer::new("fresh");
    let transport = Arc::new(
        crate::ReconnectingTransport::new(old.clone(), Arc::new(Factory(fresh.clone())))
            .with_backoff(vec![Duration::ZERO]),
    );
    let client = Arc::new(McpClient::new(transport));
    let mut registry = McpRegistry::new();
    registry.register("fixture", client.clone()).await.unwrap();
    old.disconnect.store(true, Ordering::Release);
    client.ping().await.unwrap();
    until(|| registry.find_tool("mcp__fixture__fresh").is_some()).await;
    old.change(definitions("obsolete"), 100);
    fresh.change(definitions("latest"), 1);
    until(|| registry.find_tool("mcp__fixture__latest").is_some()).await;
    registry
        .invoke("mcp__fixture__latest", json!({}))
        .await
        .unwrap();
    assert!(old.calls.lock().unwrap().is_empty());
    assert_eq!(fresh.calls.lock().unwrap().len(), 1);
    assert_eq!(fresh.max_active.load(Ordering::Acquire), 1);
    registry.close_all().await.unwrap();
}
