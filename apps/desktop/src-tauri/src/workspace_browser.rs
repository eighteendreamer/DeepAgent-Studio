use deepagent_app_core::{dto::AttachmentIngestDto, AttachmentService};
use deepagent_core::{id::EventId, tool_content::ToolResultPart, CoreError};
use deepagent_tools::{
    permission::{Permission, PermissionSet, RiskLevel},
    Tool, ToolDescriptor, ToolExecutionContext, ToolOutput,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tauri::{Emitter, LogicalPosition, LogicalSize, Manager, State, Url, Webview, WebviewUrl};

#[path = "workspace_browser_capture.rs"]
mod capture;

const TIMEOUT: Duration = Duration::from_secs(8);
const PAGE: &str = include_str!("workspace_browser_page.js");

#[derive(Clone, Default)]
pub(crate) struct WorkspaceBrowser(Arc<Mutex<HashMap<String, Tab>>>);

struct Tab {
    parent: String,
    session: Option<String>,
    granted: bool,
    visible: bool,
    page_loaded: tokio::sync::watch::Sender<Option<Url>>,
}

impl WorkspaceBrowser {
    pub(crate) fn tool(
        &self,
        app: &tauri::AppHandle,
        attachments: &AttachmentService,
        session: &str,
        images: bool,
    ) -> Option<Arc<dyn Tool>> {
        let tabs = self.0.lock().ok()?;
        let (tab_id, _) = tabs.iter().find(|(_, tab)| {
            tab.granted && tab.visible && tab.session.as_deref() == Some(session)
        })?;
        Some(Arc::new(BrowserTool {
            browser: self.clone(),
            app: app.clone(),
            session: session.into(),
            tab_id: tab_id.clone(),
            attachments: attachments.clone(),
            images: images && cfg!(any(windows, target_os = "macos", target_os = "linux")),
        }))
    }

    fn view(&self, caller: &Webview, id: &str) -> Result<Webview, String> {
        let tabs = self.0.lock().map_err(|e| e.to_string())?;
        if !tabs.get(id).is_some_and(|tab| tab.parent == caller.label()) {
            return Err("Browser tab is not owned by this workspace".into());
        }
        caller
            .app_handle()
            .get_webview(id)
            .ok_or_else(|| "Browser tab was closed".into())
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Bounds {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

impl Bounds {
    fn visible(&self) -> bool {
        self.width > 1.0 && self.height > 1.0
    }

    fn geometry(&self) -> Result<(LogicalPosition<f64>, LogicalSize<f64>), String> {
        if [self.x, self.y, self.width, self.height]
            .iter()
            .any(|x| !x.is_finite() || x.abs() > 32768.0)
        {
            return Err("Invalid browser panel bounds".into());
        }
        Ok((
            LogicalPosition::new(self.x, self.y),
            LogicalSize::new(self.width.max(1.0), self.height.max(1.0)),
        ))
    }
}

fn is_page(url: &Url) -> bool {
    matches!(url.scheme(), "http" | "https")
        && url.host_str().is_some_and(|host| {
            !matches!(
                host.trim_end_matches('.'),
                "tauri.localhost" | "asset.localhost" | "ipc.localhost"
            )
        })
        && url.username().is_empty()
        && url.password().is_none()
}

fn page_url(input: &str) -> Result<Url, String> {
    Url::parse(input)
        .ok()
        .filter(is_page)
        .ok_or_else(|| "Use an HTTP or HTTPS address without embedded credentials".into())
}

#[tauri::command]
pub(crate) async fn browser_tab_open(
    caller: Webview,
    browser: State<'_, WorkspaceBrowser>,
    session_id: Option<String>,
    url: String,
    bounds: Bounds,
) -> Result<String, String> {
    if caller.label() != caller.window().label()
        || !(caller.label() == "main" || caller.label().starts_with("session-"))
    {
        return Err("Only a DeepAgent workspace can open a browser tab".into());
    }
    if let Some(session) = session_id.as_deref() {
        session
            .parse::<deepagent_core::id::SessionId>()
            .map_err(|e| e.to_string())?;
    }
    let target = if url.is_empty() {
        Url::parse("about:blank").map_err(|e| e.to_string())?
    } else {
        page_url(&url)?
    };
    let (position, size) = bounds.geometry()?;
    let id = format!("workspace-browser-{}", EventId::new());
    let parent = caller.label().to_string();
    let notify_parent = parent.clone();
    let app_origin = caller
        .app_handle()
        .config()
        .build
        .dev_url
        .as_ref()
        .map(Url::origin);
    if app_origin
        .as_ref()
        .is_some_and(|origin| target.origin() == *origin)
    {
        return Err("Cannot open the DeepAgent application in a browser tab".into());
    }
    let (page_loaded, _) = tokio::sync::watch::channel(None);
    let report_page = page_loaded.clone();
    let panel = Arc::downgrade(&browser.0);
    let builder = tauri::webview::WebviewBuilder::new(&id, WebviewUrl::External(target))
        .on_navigation(move |url| {
            url.as_str() == "about:blank"
                || (is_page(url)
                    && app_origin
                        .as_ref()
                        .is_none_or(|origin| url.origin() != *origin))
        })
        .on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny)
        .on_page_load(move |view, event| {
            if event.event() == tauri::webview::PageLoadEvent::Finished {
                report_page.send_replace(Some(event.url().clone()));
            }
            let visible = panel.upgrade().is_some_and(|tabs| {
                tabs.lock()
                    .is_ok_and(|tabs| tabs.get(view.label()).is_some_and(|tab| tab.visible))
            });
            if visible && is_page(event.url()) {
                let _ = view.show();
            }
            let _ = view.emit_to(
                &notify_parent,
                "workspace-browser://location",
                json!({"id":view.label(), "url":event.url().as_str()}),
            );
        });
    let window = caller.window();
    tauri::async_runtime::spawn_blocking(move || window.add_child(builder, position, size))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    browser.0.lock().map_err(|e| e.to_string())?.insert(
        id.clone(),
        Tab {
            parent,
            session: session_id,
            granted: false,
            visible: bounds.visible(),
            page_loaded,
        },
    );
    Ok(id)
}

#[tauri::command]
pub(crate) fn browser_tab_bounds(
    caller: Webview,
    browser: State<'_, WorkspaceBrowser>,
    id: String,
    bounds: Bounds,
) -> Result<(), String> {
    let view = browser.view(&caller, &id)?;
    let (position, size) = bounds.geometry()?;
    {
        let mut tabs = browser.0.lock().map_err(|e| e.to_string())?;
        tabs.get_mut(&id).ok_or("Browser tab was closed")?.visible = bounds.visible();
    }
    if !bounds.visible() {
        return view.hide().map_err(|e| e.to_string());
    }
    view.set_bounds(tauri::Rect {
        position: position.into(),
        size: size.into(),
    })
    .map_err(|e| e.to_string())?;
    if view.url().is_ok_and(|url| url.as_str() == "about:blank") {
        view.hide().map_err(|e| e.to_string())
    } else {
        view.show().map_err(|e| e.to_string())
    }
}

#[tauri::command]
pub(crate) fn browser_tab_navigate(
    caller: Webview,
    browser: State<'_, WorkspaceBrowser>,
    id: String,
    url: Option<String>,
) -> Result<(), String> {
    let view = browser.view(&caller, &id)?;
    match url {
        Some(url) => view.navigate(page_url(&url)?),
        None => view.reload(),
    }
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) fn browser_tab_close(
    caller: Webview,
    browser: State<'_, WorkspaceBrowser>,
    id: String,
) -> Result<(), String> {
    let view = browser.view(&caller, &id)?;
    browser.0.lock().map_err(|e| e.to_string())?.remove(&id);
    view.close().map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) fn browser_tab_access(
    caller: Webview,
    browser: State<'_, WorkspaceBrowser>,
    id: String,
    enabled: bool,
) -> Result<(), String> {
    browser.view(&caller, &id)?;
    let mut tabs = browser.0.lock().map_err(|e| e.to_string())?;
    let Some(session) = tabs.get(&id).and_then(|tab| tab.session.clone()) else {
        return Err("Open a conversation before allowing browser control".into());
    };
    if enabled {
        for tab in tabs
            .values_mut()
            .filter(|tab| tab.session.as_ref() == Some(&session))
        {
            tab.granted = false;
        }
    }
    tabs.get_mut(&id).ok_or("Browser tab was closed")?.granted = enabled;
    Ok(())
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum Action {
    Navigate { url: String },
    Snapshot {},
    Click { target: String },
    Fill { target: String, text: String },
    Scroll { delta_y: i32 },
    Screenshot {},
}

struct BrowserTool {
    browser: WorkspaceBrowser,
    app: tauri::AppHandle,
    session: String,
    tab_id: String,
    attachments: AttachmentService,
    images: bool,
}

impl BrowserTool {
    fn view(&self) -> deepagent_core::Result<Webview> {
        let tabs = self.browser.0.lock().map_err(core_error)?;
        if !tabs.get(&self.tab_id).is_some_and(|tab| {
            tab.granted && tab.visible && tab.session.as_deref() == Some(&self.session)
        }) {
            return Err(CoreError::invalid(
                "Browser tab was closed or access was revoked",
            ));
        }
        self.app
            .get_webview(&self.tab_id)
            .ok_or_else(|| CoreError::invalid("Browser tab was closed"))
    }
}

#[async_trait::async_trait]
impl Tool for BrowserTool {
    fn descriptor(&self) -> ToolDescriptor {
        let mut actions = vec!["navigate", "snapshot", "click", "fill", "scroll"];
        if self.images {
            actions.push("screenshot");
        }
        ToolDescriptor {
            name:"workspace_browser".into(),
            description:"Control the authorized right sidebar browser tab. Use navigate (url), snapshot (returns text and target references), click (target), fill (target and text), scroll (delta_y), or screenshot if available. Take a new snapshot after navigation or a stale reference. Do not use fill for passwords, OTPs or secrets; let the user enter them directly.".into(),
            parameters:json!({"type":"object","properties":{
                "action":{"type":"string","enum":actions},"url":{"type":"string","maxLength":4096},"target":{"type":"string","maxLength":128},
                "text":{"type":"string","maxLength":2048},"delta_y":{"type":"integer","minimum":-4000,"maximum":4000}},
                "required":["action"],"additionalProperties":false}),
            risk:RiskLevel::High,
            required_permissions:PermissionSet::from_iter_perms([Permission::ReadOnly,Permission::Network]),
        }
    }

    async fn invoke(&self, arguments: Value) -> deepagent_core::Result<ToolOutput> {
        self.invoke_with_context(arguments, ToolExecutionContext::default())
            .await
    }

    async fn invoke_with_context(
        &self,
        arguments: Value,
        context: ToolExecutionContext,
    ) -> deepagent_core::Result<ToolOutput> {
        let token = context.cancellation_token();
        tokio::select! { biased;
            _=token.cancelled()=>Err(CoreError::other("Browser action cancelled")),
            result=tokio::time::timeout(TIMEOUT,self.execute(arguments))=>result.map_err(|_| CoreError::other("Browser action timed out"))?,
        }
    }
}

impl BrowserTool {
    async fn execute(&self, arguments: Value) -> deepagent_core::Result<ToolOutput> {
        let action: Action =
            serde_json::from_value(arguments.clone()).map_err(CoreError::invalid)?;
        let view = self.view()?;
        if let Action::Navigate { url } = &action {
            let target = page_url(url).map_err(CoreError::invalid)?;
            if self
                .app
                .config()
                .build
                .dev_url
                .as_ref()
                .is_some_and(|dev| target.origin() == dev.origin())
            {
                return Err(CoreError::invalid(
                    "Cannot navigate to the DeepAgent application",
                ));
            }
            let mut loaded = self
                .browser
                .0
                .lock()
                .map_err(core_error)?
                .get(&self.tab_id)
                .ok_or_else(|| CoreError::invalid("Browser tab was closed"))?
                .page_loaded
                .subscribe();
            view.navigate(target.clone()).map_err(core_error)?;
            let confirmed = tokio::time::timeout(Duration::from_secs(5), loaded.changed()).await;
            self.view()?;
            let current_url = view.url().map_err(core_error)?;
            let finished =
                matches!(confirmed, Ok(Ok(()))) && loaded.borrow().as_ref() == Some(&current_url);
            return Ok(ToolOutput::success(
                json!({"action":"navigate","requested_url":target,
                "url":current_url,"status":if finished { "loaded" } else { "navigation_started" }}),
            ));
        }
        let url = view.url().map_err(core_error)?;
        if !is_page(&url) {
            return Err(CoreError::invalid(
                "Navigate to an HTTP or HTTPS page first",
            ));
        }
        let code = match action {
            Action::Navigate { .. } => unreachable!(),
            Action::Screenshot {} => return self.screenshot(&view, &url).await,
            Action::Click { target } => json!({"action":"click","target":target}),
            Action::Fill { target, text } => json!({"action":"fill","target":target,"text":text}),
            Action::Scroll { delta_y } => {
                json!({"action":"scroll","delta_y":delta_y.clamp(-4000,4000)})
            }
            Action::Snapshot {} => json!({"action":"snapshot"}),
        };
        let value = eval(&view, format!("({PAGE})({code})")).await?;
        if let Some(error) = value.get("error").and_then(Value::as_str) {
            return Ok(ToolOutput::failure(error));
        }
        Ok(ToolOutput::success(value))
    }

    async fn screenshot(&self, view: &Webview, url: &Url) -> deepagent_core::Result<ToolOutput> {
        if !self.images {
            return Err(CoreError::invalid(
                "Browser screenshots require a supported platform and image-capable model",
            ));
        }
        let data = capture::viewport_png(view).await?;
        if !self
            .view()
            .is_ok_and(|current| current.url().is_ok_and(|current_url| current_url == *url))
        {
            return Err(CoreError::invalid("Browser tab changed during screenshot"));
        }
        let id = format!("browser_{}", EventId::new());
        let dto = self.attachments.ingest(AttachmentIngestDto {
            id: Some(id),
            session_id: Some("workspace-browser".into()),
            kind: "image".into(),
            name: "viewport.png".into(),
            mime: "image/png".into(),
            source: "workspace_browser".into(),
            local_path: None,
            data_url: Some(format!("data:image/png;base64,{data}")),
            text: None,
        })?;
        let result = json!({"action":"screenshot","url":url,"format":"png","scope":"viewport"});
        let mut out = ToolOutput::success(result.clone());
        out.content = vec![
            ToolResultPart::Text {
                text: result.to_string(),
            },
            ToolResultPart::Image {
                attachment: deepagent_core::message::MessageAttachment {
                    id: dto.id,
                    kind: dto.kind,
                    media_type: Some(dto.mime),
                    path: dto.original_path,
                    sha256: dto.sha256,
                },
            },
        ];
        Ok(out)
    }
}

async fn eval(view: &Webview, code: String) -> deepagent_core::Result<Value> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let sender = Mutex::new(Some(tx));
    view.eval_with_callback(code, move |value| {
        if let Ok(mut sender) = sender.lock() {
            if let Some(tx) = sender.take() {
                let _ = tx.send(value);
            }
        }
    })
    .map_err(core_error)?;
    let result = rx.await.map_err(core_error)?;
    serde_json::from_str::<Value>(&result)
        .map_err(core_error)
        .and_then(|value| {
            if value.is_object() {
                Ok(value)
            } else {
                Err(CoreError::other("Browser page returned no result"))
            }
        })
}

fn core_error(error: impl std::fmt::Display) -> CoreError {
    CoreError::other(format!("Workspace browser: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_rejects_application_origins_and_non_http_navigation() {
        for value in [
            "http://tauri.localhost/",
            "https://asset.localhost/a",
            "http://ipc.localhost./",
            "file:///test",
            "https://user:pass@example.test/",
        ] {
            assert!(page_url(value).is_err(), "{value}");
        }
        assert!(page_url("http://127.0.0.1:1234/fixture").is_ok());
        assert!(page_url("https://example.test/a").is_ok());
    }

    #[test]
    fn tool_arguments_are_action_specific() {
        assert!(serde_json::from_value::<Action>(json!({"action":"navigate"})).is_err());
        assert!(
            serde_json::from_value::<Action>(json!({"action":"snapshot", "script":"x"})).is_err()
        );
        assert!(serde_json::from_value::<Action>(
            json!({"action":"fill", "target":"a", "text":"value"})
        )
        .is_ok());
    }
}
