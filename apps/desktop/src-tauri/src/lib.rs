//! DeepAgent Studio desktop shell (Tauri v2).
//!
//! Thin command layer over [`deepagent_app_core::AppService`]. Each `#[command]`
//! is a one-liner delegating to the service, which returns the serializable DTOs
//! the React UI consumes. The database lives under the OS app-data dir so the
//! desktop app and CLI can share runtime state if pointed at the same path.

use std::sync::Mutex;

use deepagent_app_core::{AppService, SessionDetailDto, SessionSummaryDto};
use tauri::{Manager, State};

/// Shared application state: the service guarded by a mutex (commands are
/// dispatched from multiple threads).
struct AppState {
    service: Mutex<AppService>,
}

#[tauri::command]
fn list_sessions(state: State<'_, AppState>) -> Result<Vec<SessionSummaryDto>, String> {
    let svc = state.service.lock().map_err(|e| e.to_string())?;
    svc.list_sessions().map_err(|e| e.to_string())
}

#[tauri::command]
fn session_detail(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<SessionDetailDto, String> {
    let svc = state.service.lock().map_err(|e| e.to_string())?;
    svc.session_detail(&session_id).map_err(|e| e.to_string())
}

/// Entry point invoked by `main.rs`.
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            // Resolve a per-user database path under the app-data directory.
            let dir = app
                .path()
                .app_data_dir()
                .unwrap_or_else(|_| std::env::temp_dir());
            std::fs::create_dir_all(&dir).ok();
            let db_path = dir.join("deepagent.db");
            let service = AppService::open(&db_path)
                .map_err(|e| format!("failed to open database: {e}"))?;
            app.manage(AppState {
                service: Mutex::new(service),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![list_sessions, session_detail])
        .run(tauri::generate_context!())
        .expect("error while running DeepAgent Studio");
}
