//! Native input ownership and leases, without user data or external services.
use super::*;
use deepagent_core::{
    clock::SystemClock,
    event::{Event, EventPayload},
    id::SessionId,
    message::Message,
};
use deepagent_hooks::{HookData, HookPoint};
use deepagent_session::Session;
use std::sync::Mutex;
use std::time::Duration;

fn fixture() -> (
    Arc<Database>,
    tempfile::TempDir,
    SessionId,
    Arc<HookInputScope>,
) {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let root = tempfile::tempdir().unwrap();
    let session = Session::create_in_project(
        &db,
        &SystemClock,
        Some("H104 synthetic"),
        deepagent_core::SessionMode::Normal,
        root.path().to_str(),
    )
    .unwrap();
    let id = session.id();
    let scope = HookInputScope::new(db.clone(), root.path().to_owned());
    (db, root, id, scope)
}

fn context(id: SessionId) -> HookContext {
    HookContext::new(
        id,
        HookPoint::BeforeToolUse,
        HookData::before_tool("read_file", serde_json::json!({})),
    )
}

fn snapshots(db: &Database, id: SessionId) -> Vec<PathBuf> {
    std::fs::read_dir(db.session_transcript_dir(id))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(".hook-transcript-v1-")
        })
        .collect()
}

#[test]
fn h104_native_views_are_fresh_leased_exports_not_authority_or_cache() {
    let (db, _root, id, scope) = fixture();
    let ctx = context(id);
    let (first, lease) = scope.prepare(&ctx).unwrap();
    assert!(std::fs::metadata(&first.transcript_path)
        .unwrap()
        .permissions()
        .readonly());
    let before = std::fs::read_to_string(&first.transcript_path).unwrap();
    let original = EventStore::new(&db).load_session(id).unwrap();
    assert_eq!(
        serde_json::from_str::<Vec<Event>>(&before).unwrap(),
        original
    );
    let added = Session::recover(&db, &SystemClock, id)
        .unwrap()
        .append(EventPayload::MessageAppended {
            message: Message::user("new durable event"),
        })
        .unwrap();
    let (second, other_lease) = scope.prepare(&ctx).unwrap();
    assert_ne!(first.transcript_path, second.transcript_path);
    assert_eq!(
        std::fs::read_to_string(&first.transcript_path).unwrap(),
        before
    );
    let current: Vec<Event> =
        serde_json::from_str(&std::fs::read_to_string(&second.transcript_path).unwrap()).unwrap();
    // MessageAppended also commits its native response-item projection.
    assert_eq!(&current[..original.len()], original.as_slice());
    assert!(current.contains(&added));
    assert!(current.len() > original.len());
    assert_eq!(current, EventStore::new(&db).load_session(id).unwrap());
    drop(other_lease);
    assert!(!std::path::Path::new(&second.transcript_path).exists());
    assert!(std::path::Path::new(&first.transcript_path).exists());
    drop(lease);
    assert!(snapshots(&db, id).is_empty());
}

#[test]
fn h104_missing_history_or_wrong_workspace_fails_without_fake_paths() {
    let (db, root, id, scope) = fixture();
    assert!(scope
        .prepare(&context(SessionId::nil()))
        .err()
        .unwrap()
        .to_string()
        .contains("missing"));
    let other = tempfile::tempdir().unwrap();
    let wrong = HookInputScope::new(db.clone(), other.path().to_owned());
    assert!(wrong
        .prepare(&context(id))
        .err()
        .unwrap()
        .to_string()
        .contains("different workspace"));
    let absent = HookInputScope::new(db.clone(), root.path().join("does-not-exist"));
    assert!(absent.prepare(&context(id)).is_err());
    assert!(snapshots(&db, id).is_empty());
}

struct Callback {
    paths: Mutex<Vec<String>>,
    entered: tokio::sync::Notify,
    pending: bool,
    fail: bool,
    db: Arc<Database>,
}

#[async_trait]
impl Hook for Callback {
    fn name(&self) -> &str {
        "h104_callback"
    }

    async fn run(&self, ctx: &HookContext) -> Result<HookOutcome> {
        let base = ctx.base_input.as_ref().unwrap();
        let events: Vec<Event> =
            serde_json::from_str(&std::fs::read_to_string(&base.transcript_path).unwrap()).unwrap();
        // Proves no session/database mutex is held across callback execution.
        assert_eq!(
            events,
            EventStore::new(&self.db)
                .load_session(ctx.session_id)
                .unwrap()
        );
        self.paths
            .lock()
            .unwrap()
            .push(base.transcript_path.clone());
        self.entered.notify_one();
        if self.pending {
            std::future::pending::<()>().await;
        }
        if self.fail {
            return Err(CoreError::other("synthetic callback failure"));
        }
        Ok(HookOutcome::Continue)
    }
}

fn callback(db: Arc<Database>, pending: bool, fail: bool) -> Arc<Callback> {
    Arc::new(Callback {
        db,
        pending,
        fail,
        paths: Mutex::new(Vec::new()),
        entered: tokio::sync::Notify::new(),
    })
}

#[tokio::test]
async fn h104_leases_clean_on_error_cancel_deadline_and_future_drop() {
    let (db, _root, id, scope) = fixture();
    let ctx = context(id);
    let failing = callback(db.clone(), false, true);
    assert!(scope.wrap(failing.clone()).run(&ctx).await.is_err());
    assert_eq!(failing.paths.lock().unwrap().len(), 1);
    assert!(snapshots(&db, id).is_empty());
    for deadline in [false, true] {
        let inner = callback(db.clone(), true, false);
        let control = ToolExecutionContext::default().with_timeout(Duration::from_millis(100));
        let mut registry = deepagent_hooks::HookRegistry::new();
        registry.register(ctx.point, scope.wrap(inner.clone()));
        let cancel = async {
            inner.entered.notified().await;
            if !deadline {
                control.cancellation_token().cancel();
            }
        };
        let (result, ()) = tokio::time::timeout(Duration::from_secs(3), async {
            let operation = async {
                let dispatch = registry.dispatch_with_context(&ctx, control.clone());
                if deadline {
                    // The caller owns its total deadline; the context's
                    // timeout value alone is not an active timer.
                    tokio::time::timeout(control.timeout(), dispatch)
                        .await
                        .map_err(|_| CoreError::other("synthetic caller deadline"))?
                } else {
                    dispatch.await
                }
            };
            tokio::join!(operation, cancel)
        })
        .await
        .unwrap();
        assert!(result.is_err(), "deadline={deadline}");
        assert_eq!(inner.paths.lock().unwrap().len(), 1);
        assert!(snapshots(&db, id).is_empty());
    }
    let inner = callback(db.clone(), true, false);
    let hook = scope.wrap(inner.clone());
    let mut future = Box::pin(hook.run(&ctx));
    tokio::select! {
        _ = &mut future => panic!("callback must stay pending"),
        _ = inner.entered.notified() => {}
    }
    assert_eq!(snapshots(&db, id).len(), 1);
    drop(future);
    assert!(snapshots(&db, id).is_empty());
}

#[tokio::test]
async fn h104_cancelled_input_has_zero_side_effects_and_scope_identity_is_preserved() {
    let (db, _root, id, scope) = fixture();
    let inner = callback(db.clone(), false, false);
    let control = ToolExecutionContext::default();
    control.cancellation_token().cancel();
    let first = scope.wrap(inner.clone());
    let second = scope.wrap(inner.clone());
    assert_eq!(first.dedup_key(), second.dedup_key());
    assert!(first.run_with_context(&context(id), control).await.is_err());
    assert!(inner.paths.lock().unwrap().is_empty());
    assert!(snapshots(&db, id).is_empty());
    let other = HookInputScope::new(db, scope.root.clone()).wrap(inner);
    assert_ne!(first.dedup_key(), other.dedup_key());
}
