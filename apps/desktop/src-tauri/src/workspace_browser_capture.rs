use super::core_error;
use deepagent_core::{CoreError, Result};
use tauri::Webview;

#[cfg(windows)]
pub(super) async fn viewport_png(view: &Webview) -> Result<String> {
    use serde_json::Value;
    use std::sync::{Arc, Mutex};
    use webview2_com::{CallDevToolsProtocolMethodCompletedHandler, CoTaskMemPWSTR};
    let (tx, rx) = tokio::sync::oneshot::channel();
    view.with_webview(move |handle| {
        let sender = Arc::new(Mutex::new(Some(tx)));
        let complete = sender.clone();
        let callback = CallDevToolsProtocolMethodCompletedHandler::create(Box::new(
            move |status, response| {
                if let Ok(mut sender) = complete.lock() {
                    if let Some(tx) = sender.take() {
                        let _ = tx.send(status.map(|_| response).map_err(core_error));
                    }
                }
                Ok(())
            },
        ));
        let name = CoTaskMemPWSTR::from("Page.captureScreenshot");
        let params = CoTaskMemPWSTR::from(r#"{"format":"png","captureBeyondViewport":false}"#);
        // Tauri runs this closure on the WebView UI thread. The two strings
        // live through the call and WebView2 holds the COM callback in flight.
        let started = unsafe {
            handle.controller().CoreWebView2().and_then(|core| {
                core.CallDevToolsProtocolMethod(
                    *name.as_ref().as_pcwstr(),
                    *params.as_ref().as_pcwstr(),
                    &callback,
                )
            })
        };
        if let Err(error) = started {
            if let Ok(mut sender) = sender.lock() {
                if let Some(tx) = sender.take() {
                    let _ = tx.send(Err(core_error(error)));
                }
            }
        }
    })
    .map_err(core_error)?;
    let json = rx.await.map_err(core_error)??;
    let value: Value = serde_json::from_str(&json).map_err(core_error)?;
    value
        .get("data")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| CoreError::other("WebView2 returned no screenshot"))
}

#[cfg(target_os = "linux")]
pub(super) async fn viewport_png(view: &Webview) -> Result<String> {
    use base64::{engine::general_purpose::STANDARD, Engine};
    use webkit2gtk::{SnapshotOptions, SnapshotRegion, WebViewExt};

    let (tx, rx) = tokio::sync::oneshot::channel();
    view.with_webview(move |handle| {
        handle.inner().snapshot(
            SnapshotRegion::Visible,
            SnapshotOptions::NONE,
            None::<&gio::Cancellable>,
            move |result| {
                let encoded = result.map_err(core_error).and_then(|surface| {
                    let mut bytes = Vec::new();
                    surface.write_to_png(&mut bytes).map_err(core_error)?;
                    Ok(STANDARD.encode(bytes))
                });
                let _ = tx.send(encoded);
            },
        );
    })
    .map_err(core_error)?;
    rx.await.map_err(core_error)?
}

#[cfg(target_os = "macos")]
pub(super) async fn viewport_png(view: &Webview) -> Result<String> {
    use base64::{engine::general_purpose::STANDARD, Engine};
    use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep, NSImage};
    use objc2_foundation::{NSDictionary, NSError};
    use objc2_web_kit::WKWebView;
    use std::cell::RefCell;

    let (tx, rx) = tokio::sync::oneshot::channel();
    view.with_webview(move |handle| {
        let tx = RefCell::new(Some(tx));
        let done = block2::RcBlock::new(move |image: *mut NSImage, error: *mut NSError| {
            let Some(tx) = tx.borrow_mut().take() else {
                return;
            };
            // Snapshot arguments are borrowed for the duration of this WebKit callback.
            let output = unsafe {
                if let Some(error) = error.as_ref() {
                    Err(core_error(error.localizedDescription()))
                } else {
                    image
                        .as_ref()
                        .and_then(NSImage::TIFFRepresentation)
                        .and_then(|tiff| NSBitmapImageRep::imageRepWithData(&tiff))
                        .and_then(|bitmap| {
                            bitmap.representationUsingType_properties(
                                NSBitmapImageFileType::PNG,
                                &NSDictionary::new(),
                            )
                        })
                        .map(|png| STANDARD.encode(png.to_vec()))
                        .ok_or_else(|| CoreError::other("Could not encode WebKit viewport as PNG"))
                }
            };
            let _ = tx.send(output);
        });
        // Tauri dispatches this closure on the main thread with a valid WKWebView handle.
        unsafe {
            let webview = &*handle.inner().cast::<WKWebView>();
            webview.takeSnapshotWithConfiguration_completionHandler(None, &done);
        }
    })
    .map_err(core_error)?;
    rx.await.map_err(core_error)?
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
pub(super) async fn viewport_png(_view: &Webview) -> Result<String> {
    Err(CoreError::invalid(
        "Native browser screenshots are unavailable on this platform",
    ))
}
