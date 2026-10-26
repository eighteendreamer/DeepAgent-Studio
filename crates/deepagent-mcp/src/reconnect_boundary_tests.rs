//! H-06: count actual dispatches/effects, independently of reconnect attempts.
use super::*;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex as StdMutex;
use tokio::sync::Notify;

#[derive(Clone, Copy)]
enum Outcome {
    Success,
    LostBeforeEffect,
    LostAfterEffect,
}

struct Server {
    effects: Arc<StdMutex<Vec<u64>>>,
    outcome: Outcome,
}

#[async_trait]
impl McpTransport for Server {
    async fn send(&self, request: &JsonRpcRequest) -> Result<JsonRpcResponse> {
        if !matches!(self.outcome, Outcome::LostBeforeEffect) {
            self.effects.lock().unwrap().push(request.id);
        }
        if !matches!(self.outcome, Outcome::Success) {
            return Err(CoreError::other("broken pipe"));
        }
        Ok(JsonRpcResponse {
            jsonrpc: "2.0".into(),
            id: request.id,
            result: Some(serde_json::json!({"content":[],"isError":false})),
            error: None,
        })
    }
}

struct Factory {
    calls: AtomicUsize,
    generations: StdMutex<VecDeque<Result<Arc<dyn McpTransport>>>>,
}

#[async_trait]
impl ReconnectFactory for Factory {
    async fn reconnect(&self) -> Result<Arc<dyn McpTransport>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        tokio::task::yield_now().await;
        self.generations
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected reconnect")
    }
}

fn server(effects: &Arc<StdMutex<Vec<u64>>>, outcome: Outcome) -> Arc<dyn McpTransport> {
    Arc::new(Server {
        effects: effects.clone(),
        outcome,
    })
}

fn factory(generations: Vec<Result<Arc<dyn McpTransport>>>) -> Arc<Factory> {
    Arc::new(Factory {
        calls: AtomicUsize::new(0),
        generations: StdMutex::new(generations.into()),
    })
}

fn request(id: u64) -> JsonRpcRequest {
    // A name suggesting a read is not proof of idempotency.
    JsonRpcRequest::new(
        id,
        "tools/call",
        Some(serde_json::json!({"name":"read_file"})),
    )
}

fn unknown(error: CoreError, id: u64) {
    assert!(
        !is_transport_closed_error(&error),
        "unknown outcome must not trigger outer replay"
    );
    assert!(
        matches!(error, CoreError::RemoteOutcomeUnknown { request_id, ref method, .. }
        if request_id == id && method == "tools/call")
    );
}

#[tokio::test]
async fn each_entry_point_reports_uncertainty_without_replay_even_before_effect() {
    for outcome in [Outcome::LostBeforeEffect, Outcome::LostAfterEffect] {
        for with_context in [false, true] {
            let effects = Arc::new(StdMutex::new(Vec::new()));
            let factory = factory(vec![Ok(server(&effects, Outcome::Success))]);
            let transport = ReconnectingTransport::new(server(&effects, outcome), factory.clone())
                .with_backoff(vec![Duration::ZERO]);
            let result = if with_context {
                transport
                    .send_with_context(&request(1), ToolExecutionContext::default())
                    .await
            } else {
                transport.send(&request(1)).await
            };
            unknown(result.unwrap_err(), 1);
            // The lower transport provides no proof of dispatch. Even an
            // actual before-effect disconnect is conservatively uncertain.
            let expected = usize::from(matches!(outcome, Outcome::LostAfterEffect));
            assert_eq!(effects.lock().unwrap().len(), expected);
            assert_eq!(factory.calls.load(Ordering::SeqCst), 0);
            transport.send(&request(2)).await.unwrap();
            assert_eq!(effects.lock().unwrap().last(), Some(&2));
            assert_eq!(effects.lock().unwrap().len(), expected + 1);
            assert_eq!(factory.calls.load(Ordering::SeqCst), 1);
        }
    }
}

#[tokio::test]
async fn recovery_failures_do_not_dispatch_waiting_business_request() {
    let effects = Arc::new(StdMutex::new(Vec::new()));
    let factory = factory(vec![
        Err(CoreError::other("connection refused")),
        Err(CoreError::other("connection refused")),
        Ok(server(&effects, Outcome::Success)),
    ]);
    let transport =
        ReconnectingTransport::new(server(&effects, Outcome::LostAfterEffect), factory.clone())
            .with_backoff(vec![Duration::ZERO; 2]);
    unknown(transport.send(&request(1)).await.unwrap_err(), 1);
    let error = transport.send(&request(2)).await.unwrap_err();
    assert!(!matches!(error, CoreError::RemoteOutcomeUnknown { .. }));
    assert_eq!(*effects.lock().unwrap(), vec![1]);
    assert_eq!(factory.calls.load(Ordering::SeqCst), 2);
    transport.send(&request(3)).await.unwrap();
    assert_eq!(*effects.lock().unwrap(), vec![1, 3]);
}

#[tokio::test]
async fn repeated_post_effect_disconnects_never_replay_any_generation() {
    let effects = Arc::new(StdMutex::new(Vec::new()));
    let factory = factory(vec![
        Ok(server(&effects, Outcome::LostAfterEffect)),
        Ok(server(&effects, Outcome::LostAfterEffect)),
        Ok(server(&effects, Outcome::Success)),
    ]);
    let transport =
        ReconnectingTransport::new(server(&effects, Outcome::LostAfterEffect), factory.clone())
            .with_backoff(vec![Duration::ZERO; 3]);
    for id in 1..=3 {
        unknown(transport.send(&request(id)).await.unwrap_err(), id);
    }
    transport.send(&request(4)).await.unwrap();
    assert_eq!(*effects.lock().unwrap(), vec![1, 2, 3, 4]);
    assert_eq!(factory.calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn concurrent_new_requests_share_one_recovery() {
    let effects = Arc::new(StdMutex::new(Vec::new()));
    let factory = factory(vec![Ok(server(&effects, Outcome::Success))]);
    let transport =
        ReconnectingTransport::new(server(&effects, Outcome::LostAfterEffect), factory.clone())
            .with_backoff(vec![Duration::ZERO]);
    unknown(transport.send(&request(1)).await.unwrap_err(), 1);
    let second = request(2);
    let third = request(3);
    let (a, b) = tokio::join!(transport.send(&second), transport.send(&third));
    a.unwrap();
    b.unwrap();
    let mut actual = effects.lock().unwrap().clone();
    actual.sort();
    assert_eq!(actual, vec![1, 2, 3]);
    assert_eq!(factory.calls.load(Ordering::SeqCst), 1);
}

struct LateFailure {
    entered: Notify,
    release: Notify,
}

#[async_trait]
impl McpTransport for LateFailure {
    async fn send(&self, req: &JsonRpcRequest) -> Result<JsonRpcResponse> {
        if req.id == 1 {
            self.entered.notify_one();
            self.release.notified().await;
        }
        Err(CoreError::other("connection lost"))
    }
}

#[tokio::test]
async fn delayed_old_failure_does_not_retire_recovered_generation() {
    let old = Arc::new(LateFailure {
        entered: Notify::new(),
        release: Notify::new(),
    });
    let effects = Arc::new(StdMutex::new(Vec::new()));
    let factory = factory(vec![Ok(server(&effects, Outcome::Success))]);
    let transport = Arc::new(
        ReconnectingTransport::new(old.clone(), factory.clone()).with_backoff(vec![Duration::ZERO]),
    );
    let pending = tokio::spawn({
        let t = transport.clone();
        async move { t.send(&request(1)).await }
    });
    old.entered.notified().await;
    unknown(transport.send(&request(2)).await.unwrap_err(), 2);
    transport.send(&request(3)).await.unwrap();
    old.release.notify_one();
    unknown(pending.await.unwrap().unwrap_err(), 1);
    transport.send(&request(4)).await.unwrap();
    assert_eq!(*effects.lock().unwrap(), vec![3, 4]);
    assert_eq!(factory.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn cancellation_before_dispatch_and_while_awaiting_reply_never_replays() {
    let old = Arc::new(LateFailure {
        entered: Notify::new(),
        release: Notify::new(),
    });
    let factory = factory(vec![]);
    let transport = Arc::new(ReconnectingTransport::new(old.clone(), factory.clone()));
    let context = ToolExecutionContext::default();
    context.cancellation_token().cancel();
    assert!(transport
        .send_with_context(&request(1), context)
        .await
        .unwrap_err()
        .to_string()
        .contains("before dispatch"));
    let context = ToolExecutionContext::default();
    let pending = tokio::spawn({
        let t = transport.clone();
        let c = context.clone();
        async move { t.send_with_context(&request(1), c).await }
    });
    old.entered.notified().await;
    context.cancellation_token().cancel();
    unknown(pending.await.unwrap().unwrap_err(), 1);
    assert_eq!(factory.calls.load(Ordering::SeqCst), 0);
}

struct PendingRecovery {
    entered: Notify,
}

#[async_trait]
impl ReconnectFactory for PendingRecovery {
    async fn reconnect(&self) -> Result<Arc<dyn McpTransport>> {
        self.entered.notify_one();
        std::future::pending().await
    }
}

#[tokio::test]
async fn cancelling_during_recovery_does_not_dispatch_or_clear_disconnected_state() {
    let effects = Arc::new(StdMutex::new(Vec::new()));
    let factory = Arc::new(PendingRecovery {
        entered: Notify::new(),
    });
    let transport = Arc::new(
        ReconnectingTransport::new(server(&effects, Outcome::LostAfterEffect), factory.clone())
            .with_backoff(vec![Duration::ZERO]),
    );
    unknown(transport.send(&request(1)).await.unwrap_err(), 1);
    let context = ToolExecutionContext::default();
    let pending = tokio::spawn({
        let t = transport.clone();
        let c = context.clone();
        async move { t.send_with_context(&request(2), c).await }
    });
    factory.entered.notified().await;
    context.cancellation_token().cancel();
    assert!(pending
        .await
        .unwrap()
        .unwrap_err()
        .to_string()
        .contains("before dispatch"));
    assert!(transport.inner.lock().await.disconnected);
    assert_eq!(*effects.lock().unwrap(), vec![1]);
    transport.close().await.unwrap();
    assert!(transport
        .send(&request(3))
        .await
        .unwrap_err()
        .to_string()
        .contains("closed"));
    assert_eq!(*effects.lock().unwrap(), vec![1]);
}
