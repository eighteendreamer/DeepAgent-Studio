//! Native window adapter for the built-in desktop pet. No model or chat executor.
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{Emitter, Manager, State};

use crate::pet_store::{
    clamp_position, Catalog, PetStore, Position, Preferences, PreferencesPatch, Rect, MAX_SIZE,
    MIN_SIZE,
};

const WINDOW_LABEL: &str = "desktop-pet";
const STAGE_TOP: f64 = 88.0;
const STAGE_BOTTOM: f64 = 40.0;
// A 192 px menu, with room for the cursor offset and collision padding.
const MENU_GUTTER_X: f64 = 208.0;
const MENU_GUTTER_Y: f64 = 64.0;

fn pet_frame(size: u32) -> tauri::LogicalSize<f64> {
    let width = f64::from(size);
    tauri::LogicalSize::new(width, width * 9.0 / 16.0 + STAGE_TOP + STAGE_BOTTOM)
}

// Preferences store the original pet frame's origin, without menu gutters.
fn offset_position(position: Position, scale: f64, direction: f64) -> Position {
    Position {
        x: position.x + (MENU_GUTTER_X * scale * direction).round() as i32,
        y: position.y + (MENU_GUTTER_Y * scale * direction).round() as i32,
    }
}

fn visible_menu_bounds(
    origin: Position,
    size: tauri::PhysicalSize<u32>,
    scale: f64,
    area: Rect,
) -> Option<Rect> {
    let wx = f64::from(origin.x);
    let wy = f64::from(origin.y);
    let x = wx.max(area.x);
    let y = wy.max(area.y);
    let width = (wx + f64::from(size.width)).min(area.x + area.width) - x;
    let height = (wy + f64::from(size.height)).min(area.y + area.height) - y;
    (width > 0.0 && height > 0.0).then_some(Rect {
        x: (x - wx) / scale,
        y: (y - wy) / scale,
        width: width / scale,
        height: height / scale,
    })
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PetSnapshot {
    catalog: Catalog,
    resource_root: String,
    preferences: Preferences,
    visible: bool,
    stage_top: f64,
    stage_left: f64,
    size_min: u32,
    size_max: u32,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Interaction {
    regions: Vec<Rect>,
    menu_open: bool,
}

pub struct PetManager {
    store: Mutex<PetStore>,
    interaction: Mutex<Interaction>,
    ready: AtomicBool,
    lifecycle: Mutex<()>,
}

impl PetManager {
    fn preferences(&self) -> Result<Preferences, String> {
        self.store.lock().map_err(|e| e.to_string())?.preferences()
    }

    fn snapshot(&self, app: &tauri::AppHandle) -> Result<PetSnapshot, String> {
        let (preferences, catalog, resource_root) = {
            let mut store = self.store.lock().map_err(|e| e.to_string())?;
            store.install()?;
            (
                store.preferences()?,
                store.catalog.clone(),
                store.resource_root(),
            )
        };
        Ok(PetSnapshot {
            preferences,
            catalog,
            resource_root: resource_root.to_string_lossy().into_owned(),
            stage_top: STAGE_TOP + MENU_GUTTER_Y,
            stage_left: MENU_GUTTER_X,
            size_min: MIN_SIZE,
            size_max: MAX_SIZE,
            visible: app
                .get_webview_window(WINDOW_LABEL)
                .is_some_and(|w| w.is_visible().unwrap_or(false)),
        })
    }

    fn sync_window(&self, app: &tauri::AppHandle, reset: bool) -> Result<(), String> {
        let preferences = self.preferences()?;
        if !preferences.awake() {
            if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
                window.hide().map_err(|e| e.to_string())?;
                emit_to_pet(app, "pet://visibility", false).map_err(|e| e.to_string())?;
            }
            return Ok(());
        }
        self.store.lock().map_err(|e| e.to_string())?.install()?;
        let frame = pet_frame(preferences.size);
        let overlay = tauri::LogicalSize::new(
            frame.width + 2.0 * MENU_GUTTER_X,
            frame.height + 2.0 * MENU_GUTTER_Y,
        );
        let window = match app.get_webview_window(WINDOW_LABEL) {
            Some(window) => window,
            None => {
                self.ready.store(false, Ordering::Release);
                *self.interaction.lock().map_err(|e| e.to_string())? = Interaction::default();
                tauri::WebviewWindowBuilder::new(
                    app,
                    WINDOW_LABEL,
                    tauri::WebviewUrl::App("pet.html".into()),
                )
                .title("DeepAgent Studio · 桌宠")
                .inner_size(overlay.width, overlay.height)
                .transparent(true)
                .background_color(tauri::webview::Color(0, 0, 0, 0))
                .decorations(false)
                .shadow(false)
                .resizable(false)
                .maximizable(false)
                .minimizable(false)
                .skip_taskbar(true)
                .always_on_top(preferences.always_on_top)
                .focused(false)
                .visible(false)
                .build()
                .map_err(|error| format!("创建桌宠窗口失败：{error}"))?
            }
        };
        window
            .set_always_on_top(preferences.always_on_top)
            .map_err(|e| e.to_string())?;
        let (origin, scale) = restore_position(&window, &preferences, frame, reset)?;
        window
            .set_position(tauri::PhysicalPosition::new(origin.x, origin.y))
            .map_err(|e| e.to_string())?;
        window
            .set_size(overlay.to_physical::<u32>(scale))
            .map_err(|e| e.to_string())?;
        if self.ready.load(Ordering::Acquire) {
            window.show().map_err(|e| e.to_string())?;
            emit_to_pet(app, "pet://visibility", true).map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    fn update(
        &self,
        app: &tauri::AppHandle,
        patch: PreferencesPatch,
    ) -> Result<Preferences, String> {
        let _lifecycle = self.lifecycle.lock().map_err(|e| e.to_string())?;
        let next = {
            let mut store = self.store.lock().map_err(|e| e.to_string())?;
            let next = patch.apply(&store.preferences()?);
            if next.awake() {
                store.install()?;
            }
            store.save(next.clone())?;
            next
        };
        self.sync_window(app, false)?;
        app.emit("pet://preferences", &next)
            .map_err(|e| e.to_string())?;
        Ok(next)
    }
}

fn monitor_rect(monitor: &tauri::Monitor) -> Rect {
    let area = monitor.work_area();
    Rect {
        x: f64::from(area.position.x),
        y: f64::from(area.position.y),
        width: f64::from(area.size.width),
        height: f64::from(area.size.height),
    }
}

fn monitor_areas(window: &tauri::WebviewWindow) -> Result<Vec<Rect>, String> {
    window
        .available_monitors()
        .map(|monitors| monitors.iter().map(monitor_rect).collect())
        .map_err(|e| e.to_string())
}

fn restore_position(
    window: &tauri::WebviewWindow,
    preferences: &Preferences,
    frame: tauri::LogicalSize<f64>,
    reset: bool,
) -> Result<(Position, f64), String> {
    let requested = preferences.position.filter(|_| !reset);
    let primary = window.primary_monitor().map_err(|e| e.to_string())?;
    let target = if let Some(position) = requested {
        window
            .monitor_from_point(f64::from(position.x), f64::from(position.y))
            .map_err(|e| e.to_string())?
    } else {
        None
    };
    let monitor = target.as_ref().or(primary.as_ref());
    let scale = monitor
        .map(tauri::Monitor::scale_factor)
        .unwrap_or(window.scale_factor().map_err(|e| e.to_string())?);
    let area = monitor.map(monitor_rect).unwrap_or(Rect {
        x: 0.0,
        y: 0.0,
        width: 1920.0,
        height: 1080.0,
    });
    let size = frame.to_physical::<u32>(scale);
    let position = requested.unwrap_or_else(|| Position {
        x: (area.x + area.width - f64::from(size.width) - 24.0) as i32,
        y: (area.y + area.height - f64::from(size.height) - 24.0) as i32,
    });
    let clamped = clamp_position(
        position,
        f64::from(size.width),
        f64::from(size.height),
        &monitor_areas(window)?,
    );
    Ok((offset_position(clamped, scale, -1.0), scale))
}

fn authorize(window: &tauri::WebviewWindow) -> Result<(), String> {
    if window.label() == WINDOW_LABEL
        || window.label() == "main"
        || window.label().starts_with("session-")
    {
        Ok(())
    } else {
        Err("此窗口不能管理桌宠".into())
    }
}

fn pet_window(window: &tauri::WebviewWindow) -> Result<(), String> {
    if window.label() == WINDOW_LABEL {
        Ok(())
    } else {
        Err("此操作仅允许桌宠窗口调用".into())
    }
}

#[tauri::command]
pub async fn pet_get_state(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
) -> Result<PetSnapshot, String> {
    authorize(&window)?;
    tauri::async_runtime::spawn_blocking(move || app.state::<PetManager>().snapshot(&app))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn pet_get_preferences(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
) -> Result<Preferences, String> {
    authorize(&window)?;
    crate::run_blocking(move || app.state::<PetManager>().preferences()).await
}

#[tauri::command]
pub async fn pet_update_preferences(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    patch: PreferencesPatch,
) -> Result<Preferences, String> {
    authorize(&window)?;
    tauri::async_runtime::spawn_blocking(move || app.state::<PetManager>().update(&app, patch))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn pet_ready(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    manager: State<'_, PetManager>,
) -> Result<(), String> {
    pet_window(&window)?;
    manager.ready.store(true, Ordering::Release);
    if manager.preferences()?.awake() {
        window.show().map_err(|e| e.to_string())?;
        emit_to_pet(&app, "pet://visibility", true).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub fn pet_set_interaction(
    window: tauri::WebviewWindow,
    manager: State<'_, PetManager>,
    interaction: Interaction,
) -> Result<(), String> {
    pet_window(&window)?;
    if interaction.regions.len() > 16
        || interaction.regions.iter().any(|rect| {
            [rect.x, rect.y, rect.width, rect.height]
                .iter()
                .any(|value| !value.is_finite())
                || rect.width < 0.0
                || rect.height < 0.0
                || rect.width > 2000.0
                || rect.height > 2000.0
        })
    {
        return Err("桌宠交互区域无效".into());
    }
    *manager.interaction.lock().map_err(|e| e.to_string())? = interaction;
    Ok(())
}

#[tauri::command]
pub fn pet_start_drag(window: tauri::WebviewWindow) -> Result<(), String> {
    pet_window(&window)?;
    window.start_dragging().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn pet_reset_position(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
) -> Result<(), String> {
    authorize(&window)?;
    crate::run_blocking(move || {
        let manager = app.state::<PetManager>();
        let _lifecycle = manager.lifecycle.lock().map_err(|e| e.to_string())?;
        {
            let mut store = manager.store.lock().map_err(|e| e.to_string())?;
            let mut preferences = store.preferences()?;
            preferences.position = None;
            store.save(preferences)?;
        }
        manager.sync_window(&app, true)
    })
    .await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkSnapshot {
    active_runs: usize,
    pending_approvals: u64,
    running_actions: u64,
}

#[tauri::command]
pub async fn pet_work_snapshot(
    window: tauri::WebviewWindow,
    state: State<'_, crate::AppState>,
) -> Result<WorkSnapshot, String> {
    pet_window(&window)?;
    let chat = state.chat.clone();
    crate::run_blocking(move || {
        let readiness = chat.coordinator_readiness().map_err(|e| e.to_string())?;
        Ok(WorkSnapshot {
            active_runs: readiness.active_runs,
            pending_approvals: readiness.pending_approvals,
            running_actions: readiness.running_actions,
        })
    })
    .await
}

#[tauri::command]
pub fn pet_open_main(app: tauri::AppHandle, window: tauri::WebviewWindow) -> Result<(), String> {
    pet_window(&window)?;
    let main = app.get_webview_window("main").ok_or("主窗口不存在")?;
    main.unminimize().map_err(|e| e.to_string())?;
    main.show().map_err(|e| e.to_string())?;
    main.set_focus().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn pet_render_failed(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    error: String,
) -> Result<(), String> {
    pet_window(&window)?;
    report(
        &app,
        "pet_render_failed",
        &error.chars().take(400).collect::<String>(),
    );
    Ok(())
}

pub fn setup(app: &tauri::AppHandle, root: PathBuf) {
    let development = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/pets/dsh-pet");
    let source = if cfg!(debug_assertions) && development.is_dir() {
        development
    } else {
        app.path()
            .resource_dir()
            .unwrap_or_default()
            .join("resources/pets/dsh-pet")
    };
    app.manage(PetManager {
        store: Mutex::new(PetStore::new(root, source)),
        interaction: Mutex::new(Interaction::default()),
        ready: AtomicBool::new(false),
        lifecycle: Mutex::new(()),
    });
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let startup = app.clone();
        let result = tauri::async_runtime::spawn_blocking(move || {
            let manager = startup.state::<PetManager>();
            let _lifecycle = manager.lifecycle.lock().map_err(|e| e.to_string())?;
            manager.sync_window(&startup, false)
        })
        .await;
        if let Err(error) = result.unwrap_or_else(|error| Err(error.to_string())) {
            report(&app, "pet_startup_failed", &error);
        }
        pointer_loop(app).await;
    });
}

fn report(app: &tauri::AppHandle, event: &str, error: &str) {
    eprintln!("{event}: {error}");
    if let Some(state) = app.try_state::<crate::AppState>() {
        let _ = state.runtime_logs.append(
            deepagent_app_core::NewRuntimeLogEntry::info("pet", event)
                .with_source("desktop-tauri")
                .with_message(error),
        );
    }
}

async fn pointer_loop(app: tauri::AppHandle) {
    let mut ignoring = None;
    let mut last_position = None;
    let mut moved_at = Instant::now();
    let mut checked_at = Instant::now();
    let mut saved_position = None;
    let mut geometry = None;
    let mut menu_bounds = None;
    loop {
        tokio::time::sleep(Duration::from_millis(80)).await;
        let Some(window) = app.get_webview_window(WINDOW_LABEL) else {
            ignoring = None;
            last_position = None;
            saved_position = None;
            geometry = None;
            menu_bounds = None;
            tokio::time::sleep(Duration::from_millis(400)).await;
            continue;
        };
        if !window.is_visible().unwrap_or(false) {
            continue;
        }
        let manager = app.state::<PetManager>();
        let result = (|| -> Result<(), String> {
            let cursor = window.cursor_position().map_err(|e| e.to_string())?;
            let origin = window.inner_position().map_err(|e| e.to_string())?;
            let scale = window.scale_factor().map_err(|e| e.to_string())?;
            let local_x = (cursor.x - f64::from(origin.x)) / scale;
            let local_y = (cursor.y - f64::from(origin.y)) / scale;
            let interaction = manager.interaction.lock().map_err(|e| e.to_string())?;
            let ignore = !interaction.menu_open
                && !interaction
                    .regions
                    .iter()
                    .any(|rect| rect.contains(local_x, local_y));
            drop(interaction);
            if ignoring != Some(ignore) {
                window
                    .set_ignore_cursor_events(ignore)
                    .map_err(|e| e.to_string())?;
                ignoring = Some(ignore);
            }
            let origin = Position {
                x: origin.x,
                y: origin.y,
            };
            let position = offset_position(origin, scale, 1.0);
            let size = window.inner_size().map_err(|e| e.to_string())?;
            let refresh = checked_at.elapsed() >= Duration::from_secs(3);
            let current_geometry = (origin, size, scale);
            if geometry != Some(current_geometry) || refresh {
                let center_x = f64::from(origin.x) + f64::from(size.width) / 2.0;
                let center_y = f64::from(origin.y) + f64::from(size.height) / 2.0;
                let bounds = window
                    .monitor_from_point(center_x, center_y)
                    .map_err(|e| e.to_string())?
                    .and_then(|monitor| {
                        visible_menu_bounds(origin, size, scale, monitor_rect(&monitor))
                    });
                if menu_bounds != bounds {
                    emit_to_pet(&app, "pet://menu-bounds", bounds).map_err(|e| e.to_string())?;
                    menu_bounds = bounds;
                }
                geometry = Some(current_geometry);
            }
            if last_position != Some(position) {
                last_position = Some(position);
                moved_at = Instant::now();
            } else if saved_position != Some(position)
                && moved_at.elapsed() >= Duration::from_millis(600)
            {
                let mut store = manager.store.lock().map_err(|e| e.to_string())?;
                let mut preferences = store.preferences()?;
                if preferences.position != Some(position) {
                    preferences.position = Some(position);
                    store.save(preferences)?;
                }
                saved_position = Some(position);
            }
            if refresh {
                let offset = offset_position(Position { x: 0, y: 0 }, scale, 1.0);
                let clamped = clamp_position(
                    position,
                    f64::from(size.width) - 2.0 * f64::from(offset.x),
                    f64::from(size.height) - 2.0 * f64::from(offset.y),
                    &monitor_areas(&window)?,
                );
                if clamped != position {
                    let next_origin = offset_position(clamped, scale, -1.0);
                    window
                        .set_position(tauri::PhysicalPosition::new(next_origin.x, next_origin.y))
                        .map_err(|e| e.to_string())?;
                }
                checked_at = Instant::now();
            }
            Ok(())
        })();
        if let Err(error) = result {
            report(&app, "pet_window_update_failed", &error);
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    }
}

pub fn on_event(app: &tauri::AppHandle, event: &tauri::RunEvent) {
    if let tauri::RunEvent::WindowEvent {
        label,
        event: tauri::WindowEvent::Destroyed,
        ..
    } = event
    {
        if label == "main" || label.starts_with("session-") {
            let workbench_open = app
                .webview_windows()
                .keys()
                .any(|label| label == "main" || label.starts_with("session-"));
            if !workbench_open {
                if let Some(pet) = app.get_webview_window(WINDOW_LABEL) {
                    if let Err(error) = pet.destroy() {
                        report(app, "pet_shutdown_failed", &error.to_string());
                    }
                }
            }
        }
    }
}

fn emit_to_pet<T: Serialize + Clone>(
    app: &tauri::AppHandle,
    event: &str,
    payload: T,
) -> tauri::Result<()> {
    app.emit_to(
        tauri::EventTarget::webview_window(WINDOW_LABEL),
        event,
        payload,
    )
}

pub fn notify_completion(app: &tauri::AppHandle, status: &str) {
    if app.get_webview_window(WINDOW_LABEL).is_some() && matches!(status, "completed" | "failed") {
        if let Err(error) = emit_to_pet(app, "pet://completed", status) {
            report(
                app,
                "pet_completion_notification_failed",
                &error.to_string(),
            );
        }
    }
}

fn allow_invoke(invoke: &tauri::ipc::Invoke<tauri::Wry>) -> bool {
    invoke.message.webview_ref().label() != WINDOW_LABEL
        || matches!(
            invoke.message.command(),
            "pet_get_state"
                | "pet_update_preferences"
                | "pet_ready"
                | "pet_set_interaction"
                | "pet_start_drag"
                | "pet_reset_position"
                | "pet_work_snapshot"
                | "pet_open_main"
                | "pet_render_failed"
        )
}

/// App commands registered through generate_handler are unrestricted by default;
/// keep the pet webview from invoking the rest of the workbench's command API.
pub fn limit_invoke(
    handler: impl Fn(tauri::ipc::Invoke<tauri::Wry>) -> bool + Send + Sync + 'static,
) -> impl Fn(tauri::ipc::Invoke<tauri::Wry>) -> bool + Send + Sync + 'static {
    move |invoke| {
        if allow_invoke(&invoke) {
            handler(invoke)
        } else {
            invoke
                .resolver
                .reject("This command is unavailable in the desktop pet window");
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlay_padding_preserves_saved_pet_position_at_fractional_scale() {
        let saved = Position { x: -320, y: 734 };
        let origin = offset_position(saved, 1.25, -1.0);
        assert_eq!(origin, Position { x: -580, y: 654 });
        assert_eq!(offset_position(origin, 1.25, 1.0), saved);
    }

    #[test]
    fn menu_bounds_exclude_offscreen_gutters_and_taskbar_at_fractional_scale() {
        let origin = Position { x: -2180, y: 770 };
        let work_area = Rect {
            x: -1920.0,
            y: 0.0,
            width: 1920.0,
            height: 1040.0,
        };
        assert_eq!(
            visible_menu_bounds(origin, tauri::PhysicalSize::new(870, 520), 1.25, work_area),
            Some(Rect {
                x: 208.0,
                y: 0.0,
                width: 488.0,
                height: 216.0
            })
        );
        assert_eq!(
            visible_menu_bounds(
                Position { x: 40, y: 770 },
                tauri::PhysicalSize::new(870, 520),
                1.25,
                work_area
            ),
            None
        );
    }
}
