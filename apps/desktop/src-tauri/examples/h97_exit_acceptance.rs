//! Real Wry event loop with only temporary state and an offline model transport.
//! No application setup, WebView, credential vault, real model or user data.
//! On Windows, link this example with Tauri's generated `resource.lib` via
//! `cargo rustc --example h97_exit_acceptance -- -C link-arg=<out_dir>/resource.lib`.
//! tauri-winres normally links the common-controls-v6 manifest into bins only.
#[path = "../src/cron_lifecycle.rs"]
mod cron_lifecycle;

use async_trait::async_trait;
use deepagent_app_core::{
    AppService, ChatService, CronService, MemorySecretStore, SettingsService,
};
use deepagent_core::error::{CoreError, Result};
use deepagent_models::transport::{EventSink, HttpTransport, MockTransport, TransportRequest};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};
use std::time::Duration;
use tauri::Manager;

#[derive(Default)]
struct OfflineHeld {
    calls: AtomicUsize,
    cancelled: AtomicBool,
}
#[async_trait]
impl HttpTransport for OfflineHeld {
    async fn stream(&self, _: TransportRequest, _: &mut dyn EventSink) -> Result<()> {
        Err(CoreError::other(
            "only cancellation-aware offline streaming is accepted",
        ))
    }
    async fn stream_cancelled(
        &self,
        _: TransportRequest,
        _: &mut dyn EventSink,
        cancel: Arc<AtomicBool>,
    ) -> Result<()> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        while !cancel.load(Ordering::Acquire) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        self.cancelled.store(true, Ordering::Release);
        Err(CoreError::other("H97 synthetic request cancelled"))
    }
}

fn main() {
    let close_window = std::env::args().any(|arg| arg == "--close-window");
    let root = tempfile::tempdir().unwrap();
    let app_service = AppService::open(root.path().join("state.sqlite")).unwrap();
    let db = app_service.shared_database();
    let settings = Arc::new(SettingsService::new(
        db.clone(),
        Arc::new(MockTransport::with_get_json(
            r#"{"data":[{"id":"deepseek-v4-flash","object":"model","owned_by":"deepseek"}]}"#,
        )),
        Arc::new(MemorySecretStore::new()),
    ));
    tauri::async_runtime::block_on(settings.initialize("h97-synthetic-credential")).unwrap();
    let model = Arc::new(OfflineHeld::default());
    let chat = Arc::new(ChatService::new(
        db.clone(),
        settings,
        model.clone(),
        root.path(),
    ));
    let cron = Arc::new(CronService::new(root.path(), chat));
    let directory = root.path().join(".deepagent");
    std::fs::create_dir(&directory).unwrap();
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    std::fs::write(directory.join("scheduled-tasks.json"), serde_json::json!({"tasks":[{
        "id":"h97-offline","cron":"* * * * *","prompt":"H97 synthetic desktop exit","createdAt":at-120_000
    }]}).to_string()).unwrap();
    let model_wait = model.clone();
    let mut context = tauri::generate_context!();
    context.config_mut().app.windows.clear();
    let app = tauri::Builder::default()
        .setup(move |app| {
            tauri::WindowBuilder::new(app, "h97-acceptance")
                .visible(false)
                .build()?;
            app.manage(cron_lifecycle::CronLifecycle::start(cron));
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::timeout(Duration::from_secs(15), async {
                    while model_wait.calls.load(Ordering::SeqCst) != 1 {
                        tokio::time::sleep(Duration::from_millis(5)).await;
                    }
                })
                .await
                .expect("actual scheduled model must start before host exit");
                if close_window {
                    handle
                        .get_window("h97-acceptance")
                        .unwrap()
                        .close()
                        .unwrap();
                } else {
                    handle.exit(0);
                }
            });
            Ok(())
        })
        .build(context)
        .unwrap();
    let exit_requests = Arc::new(AtomicUsize::new(0));
    let observed = exit_requests.clone();
    let code = app.run_return(move |app, event| {
        if matches!(event, tauri::RunEvent::ExitRequested { .. }) {
            observed.fetch_add(1, Ordering::SeqCst);
        }
        cron_lifecycle::on_event(app, &event);
    });
    assert_eq!(code, 0);
    assert!(exit_requests.load(Ordering::SeqCst) >= 1);
    assert!(model.cancelled.load(Ordering::Acquire));
    assert_eq!(model.calls.load(Ordering::SeqCst), 1);
    // Reopen the actual temporary database after the Wry host has exited.
    drop(db);
    drop(app_service);
    let reopened = AppService::open(root.path().join("state.sqlite")).unwrap();
    let database = reopened.database();
    let (runs, terminal, cancellation, origins): (i64, i64, i64, i64) = database.with_conn(|conn| {
        conn.query_row("SELECT (SELECT count(*) FROM runs WHERE terminal_kind='cancelled' AND finished_at IS NOT NULL), (SELECT count(*) FROM run_events WHERE event_type='run_terminal'), (SELECT count(*) FROM run_events WHERE event_type='cancel.requested'), (SELECT count(*) FROM run_events WHERE event_type='schedule.accepted')", [], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?)))
            .map_err(|e| CoreError::Persistence(e.to_string()))
    }).unwrap();
    assert_eq!((runs, terminal, cancellation, origins), (1, 1, 1, 1));
    assert!(reopened.recover_unfinished_runs().unwrap().is_empty());
    println!(
        "{}",
        serde_json::json!({"accepted":true,"mode":if close_window {"last-window-close"} else {"programmatic-exit"},"exitRequests":exit_requests.load(Ordering::SeqCst),"calls":1,"cancelled":true,"durableTerminalsAfterReopen":terminal,"recoveryRedispatch":false})
    );
}
